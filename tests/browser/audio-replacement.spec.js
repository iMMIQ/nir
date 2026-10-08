import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import {buildAudioReplacementFixture} from './audio-replacement.fixture.js';
import {closeScaleFixture} from '../performance/fixtures.js';
import {recoverAudioOutput} from '../nir-next/audio-output-helper.js';
import {readPng} from './pixels.js';

let fixture;
test.beforeAll(async()=>{fixture=await buildAudioReplacementFixture();});
test.afterAll(async()=>{if(fixture)await closeScaleFixture(fixture);});

async function audit(page) {
  // Observe native starts/stops, keeping only numeric records. This is device
  // API evidence; Playwright's default mute flag prevents a hardware claim.
  await page.addInitScript(()=>{
    const Native=AudioContext,contexts=new WeakMap();let nextContext=0;
    globalThis.replacementAudit={next:0,starts:[],stops:[],decodes:[],active:new Map()};
    globalThis.AudioContext=class extends Native {constructor(...args){super(...args);contexts.set(this,nextContext++);}};
    const decode=Native.prototype.decodeAudioData;
    Native.prototype.decodeAudioData=async function(...args){
      const buffer=await decode.apply(this,args);
      replacementAudit.decodes.push({duration:buffer.duration,frames:buffer.length,rate:buffer.sampleRate});return buffer;
    };
    const create=Native.prototype.createBufferSource;
    Native.prototype.createBufferSource=function(...args){
      const source=create.apply(this,args),start=source.start,stop=source.stop;let id;
      source.start=function(...args){
        const result=start.apply(this,args);id=++replacementAudit.next;
        const row={id,route:contexts.get(this.context),loop:this.loop,duration:this.buffer.duration,
          offset:args[1]||0,startedClock:this.context.currentTime};
        replacementAudit.starts.push(row);replacementAudit.active.set(id,row);return result;
      };
      source.stop=function(...args){
        if(id!==undefined){replacementAudit.stops.push({id,clock:this.context.currentTime});replacementAudit.active.delete(id);}
        return stop.apply(this,args);
      };
      source.addEventListener('ended',()=>replacementAudit.active.delete(id),{once:true});return source;
    };
  });
}
const sample=page=>page.evaluate(()=>({state:__nir.state(),audio:{starts:replacementAudit.starts,stops:replacementAudit.stops,decodes:replacementAudit.decodes,active:[...replacementAudit.active.values()]},
  bgm:__nir.diagnostics().host_work.audio_domains.story.buses.bgm,media:__nir.diagnostics().resource_memory.media}));
const identity=s=>({session:s.session,interaction:s.interaction,history:s.history_count,variables:s.variables,position:s.position});
async function pixel(page){const image=readPng(await page.screenshot());const i=(Math.floor(image.height/2)*image.width+8)*image.channels;return [...image.pixels.subarray(i,i+3)];}
async function ready(page,history){
  await page.waitForFunction(history=>__nir.state().screen==='Story'&&!__nir.state().loading&&__nir.state().dialogue?.ready&&__nir.state().history_count===history,history);
  await recoverAudioOutput(page);
}
async function retry(page){
  const button=page.getByRole('button',{name:'Retry',exact:true});
  await expect(button).toBeEnabled();
  const rect=JSON.parse(await button.getAttribute('data-rect'));
  expect(rect[3]).toBeGreaterThanOrEqual(44);
  await page.mouse.click(rect[0]+rect[2]/2,rect[1]+rect[3]/2);
}

for(const worker of ['required','main'])test(`delayed and failed MP3 replacement commits once, ordinary pages continue, explicit replay restarts, ${worker}`,async({page},testInfo)=>{
  await audit(page);
  const gates=[];let requests=0;
  const pattern=`**/${fixture.replacementPath}`;
  await page.context().route(pattern,async route=>{
    const attempt=++requests;
    await new Promise(resolve=>gates.push(resolve));
    await route.fulfill(attempt===1?{status:503,contentType:'text/plain',body:'temporary MP3 failure'}:
      {status:200,contentType:'audio/mpeg',body:fixture.replacementBytes}).catch(()=>{});
  });
  const evidence={worker,verification:fixture.verification,limitations:['native API sources and clocks, not hardware sound','headless default mute audio','SwiftShader; mobile viewport is not Android hardware']};
  try {
    await page.goto(`${fixture.origin}/?test=1&worker=${worker}&backend=webgl2`);
    await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    await page.keyboard.press('Enter');await ready(page,1);
    const before=await sample(page);evidence.before=before;
    expect(before.audio.active).toHaveLength(1);expect(before.audio.active[0].route).toBe(0);
    expect(before.audio.active[0].loop).toBe(true);expect(before.audio.active[0].duration).toBeCloseTo(8,3);
    expect(await pixel(page)).toEqual([255,0,0]);expect(requests).toBe(0);
    await page.keyboard.press('Enter');
    await page.waitForFunction(()=>__nir.state().loading);
    await expect.poll(()=>requests).toBe(1);
    const waiting=await sample(page);await page.waitForTimeout(600);const delayed=await sample(page);
    expect(delayed.state.loading).toBe(true);expect(delayed.state.tick_us).toBe(waiting.state.tick_us);
    expect(identity(delayed.state)).toEqual(identity(waiting.state));expect(delayed.state.history_count).toBe(1);
    expect(delayed.audio.active).toEqual(before.audio.active);expect(delayed.audio.stops).toHaveLength(0);
    expect(delayed.bgm.clock_seconds-waiting.bgm.clock_seconds).toBeGreaterThan(.4);
    expect(await pixel(page)).toEqual([255,0,0]);evidence.waiting=waiting;evidence.delayed=delayed;
    gates.shift()();await page.waitForFunction(()=>__nir.state().error!==null);
    const failed=await sample(page);evidence.failed=failed;
    expect(failed.audio.active).toEqual(before.audio.active);expect(failed.audio.stops).toHaveLength(0);
    expect(failed.state.history_count).toBe(1);expect(await pixel(page)).toEqual([255,0,0]);
    await page.waitForTimeout(400);const failedLater=await sample(page);
    expect(failedLater.state.tick_us).toBe(failed.state.tick_us);
    expect(failedLater.bgm.clock_seconds-failed.bgm.clock_seconds).toBeGreaterThan(.25);evidence.failedLater=failedLater;
    await retry(page);await expect.poll(()=>requests).toBe(2);
    const retrying=await sample(page);await page.waitForTimeout(400);const retryDelayed=await sample(page);
    expect(retryDelayed.audio.active).toEqual(before.audio.active);expect(retryDelayed.audio.stops).toHaveLength(0);
    expect(retryDelayed.state.history_count).toBe(1);expect(await pixel(page)).toEqual([255,0,0]);
    expect(retryDelayed.state.tick_us).toBe(retrying.state.tick_us);
    expect(retryDelayed.bgm.clock_seconds-retrying.bgm.clock_seconds).toBeGreaterThan(.25);evidence.retryDelayed=retryDelayed;
    gates.shift()();await ready(page,2);
    const committed=await sample(page);evidence.committed=committed;
    expect(committed.audio.starts).toHaveLength(2);expect(committed.audio.stops.map(s=>s.id)).toEqual([before.audio.active[0].id]);
    expect(committed.audio.active).toHaveLength(1);expect(committed.audio.active[0].offset).toBe(0);
    expect(committed.audio.active[0].duration).toBeCloseTo(.8,3);expect(committed.state.session).toBe(before.state.session);
    expect(committed.state.error).toBeNull();expect(await pixel(page)).toEqual([0,0,255]);
    await page.keyboard.press('Enter');await ready(page,3);const ordinary=await sample(page);evidence.ordinary=ordinary;
    expect(ordinary.audio.active).toEqual(committed.audio.active);expect(ordinary.audio.starts).toEqual(committed.audio.starts);
    expect(ordinary.audio.stops).toEqual(committed.audio.stops);
    await page.keyboard.press('Enter');await ready(page,4);const replay=await sample(page);evidence.replay=replay;
    expect(replay.audio.starts).toHaveLength(3);expect(replay.audio.active).toHaveLength(1);
    expect(replay.audio.active[0].offset).toBe(0);expect(replay.audio.active[0].id).not.toBe(committed.audio.active[0].id);
    expect(replay.audio.stops.map(s=>s.id)).toEqual([before.audio.active[0].id,committed.audio.active[0].id]);expect(requests).toBe(2);
    await page.evaluate(()=>__nir.action({type:'title'}));
    await page.waitForFunction(()=>__nir.state().screen==='Title'&&!__nir.state().loading&&__nir.metrics.activeRequests===0);
    await expect.poll(()=>page.evaluate(()=>replacementAudit.active.size)).toBe(0);
    await expect.poll(()=>page.evaluate(()=>__nir.diagnostics().resource_memory.media.decoded_audio_bytes)).toBe(0);
    evidence.final=await sample(page);evidence.requests=requests;evidence.status='passed';
  }catch(error){evidence.status='failed';evidence.error=String(error);if(!page.isClosed())evidence.failure=await sample(page).catch(()=>null);throw error;}
  finally {
    for(const release of gates)release();
    if(!page.isClosed())await page.context().unroute(pattern);
    await fs.writeFile(testInfo.outputPath('audio-replacement.json'),JSON.stringify(evidence,null,2)+'\n');
  }
});

for(const worker of ['required','main'])test(`new MP3 is decoded while delayed or failed scene keeps old music, ${worker}`,async({page},testInfo)=>{
  await audit(page);
  const gates=[];let requests=0;
  const pattern=`**/${fixture.imagePath}`;
  await page.context().route(pattern,async route=>{
    const attempt=++requests;await new Promise(resolve=>gates.push(resolve));
    await route.fulfill(attempt===1?{status:503,contentType:'text/plain',body:'temporary scene failure'}:
      {status:200,contentType:fixture.imageMediaType,body:fixture.imageBytes}).catch(()=>{});
  });
  const evidence={worker,verification:fixture.verification,limits:'Native source/clock and actual pixels; no hardware sound or Android device evidence.'};
  try {
    await page.goto(`${fixture.origin}/?test=1&worker=${worker}&backend=webgl2`);
    await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    await page.keyboard.press('Enter');await ready(page,1);
    const before=await sample(page);evidence.before=before;expect(requests).toBe(0);
    await page.keyboard.press('Enter');await expect.poll(()=>requests).toBe(1);
    await page.waitForFunction(()=>__nir.state().loading&&replacementAudit.decodes.some(d=>Math.abs(d.duration-.8)<.001));
    const waiting=await sample(page);await page.waitForTimeout(600);const delayed=await sample(page);evidence.delayed=delayed;
    expect(delayed.audio.active).toEqual(before.audio.active);expect(delayed.audio.starts).toHaveLength(1);expect(delayed.audio.stops).toHaveLength(0);
    expect(delayed.state.tick_us).toBe(waiting.state.tick_us);expect(delayed.state.history_count).toBe(1);
    expect(delayed.bgm.clock_seconds-waiting.bgm.clock_seconds).toBeGreaterThan(.4);expect(await pixel(page)).toEqual([255,0,0]);
    gates.shift()();await page.waitForFunction(()=>__nir.state().error!==null);const failed=await sample(page);evidence.failed=failed;
    expect(failed.audio.active).toEqual(before.audio.active);expect(failed.audio.stops).toHaveLength(0);expect(failed.state.history_count).toBe(1);
    expect(await pixel(page)).toEqual([255,0,0]);
    await retry(page);await expect.poll(()=>requests).toBe(2);
    const retrying=await sample(page);await page.waitForTimeout(400);const retryDelayed=await sample(page);evidence.retryDelayed=retryDelayed;
    expect(retryDelayed.audio.active).toEqual(before.audio.active);expect(retryDelayed.audio.stops).toHaveLength(0);
    expect(retryDelayed.state.tick_us).toBe(retrying.state.tick_us);expect(retryDelayed.state.history_count).toBe(1);
    expect(retryDelayed.bgm.clock_seconds-retrying.bgm.clock_seconds).toBeGreaterThan(.25);expect(await pixel(page)).toEqual([255,0,0]);
    gates.shift()();await ready(page,2);const committed=await sample(page);evidence.committed=committed;
    expect(committed.audio.starts).toHaveLength(2);expect(committed.audio.stops.map(s=>s.id)).toEqual([before.audio.active[0].id]);
    expect(committed.audio.active).toHaveLength(1);expect(committed.audio.active[0].offset).toBe(0);expect(committed.audio.active[0].duration).toBeCloseTo(.8,3);
    expect(committed.state.session).toBe(before.state.session);expect(committed.state.error).toBeNull();expect(await pixel(page)).toEqual([0,0,255]);
    await page.evaluate(()=>__nir.action({type:'title'}));
    await page.waitForFunction(()=>__nir.state().screen==='Title'&&!__nir.state().loading&&__nir.metrics.activeRequests===0);
    await expect.poll(()=>page.evaluate(()=>replacementAudit.active.size)).toBe(0);
    await expect.poll(()=>page.evaluate(()=>__nir.diagnostics().resource_memory.media.decoded_audio_bytes)).toBe(0);
    evidence.final=await sample(page);evidence.requests=requests;evidence.status='passed';
  }catch(error){evidence.status='failed';evidence.error=String(error);if(!page.isClosed())evidence.failure=await sample(page).catch(()=>null);throw error;}
  finally {
    for(const release of gates)release();if(!page.isClosed())await page.context().unroute(pattern);
    await fs.writeFile(testInfo.outputPath('scene-audio-replacement.json'),JSON.stringify(evidence,null,2)+'\n');
  }
});
