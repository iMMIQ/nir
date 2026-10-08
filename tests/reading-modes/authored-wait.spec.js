import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import {recoverAudioOutput} from '../nir-next/audio-output-helper.js';

const origin='http://127.0.0.1:4275';
const traces=new WeakMap();
const snapshot=page=>page.evaluate(()=>({state:__nir.state(),audio:authoredAudio.filter(r=>r.started!==null).map(r=>({
  id:r.id,loop:r.source.loop,stops:r.stops,ended:r.ended,started:r.started,endedAt:r.endedAt,
  clock:r.context.currentTime,contextState:r.context.state,duration:r.duration,startedOrder:r.startedOrder,endedOrder:r.endedOrder,
})),domains:__nir.diagnostics().host_work.audio_domains}));
const identity=s=>({session:s.state.session,interaction:s.state.interaction,position:s.state.position,
  history:s.state.history_count,tick:s.state.tick_us});
const loops=s=>s.audio.filter(r=>r.loop).map(r=>({id:r.id,stops:r.stops}));
async function checkpoint(page,info,name){
  const row=await snapshot(page);traces.get(page).stages[name]=row;
  await fs.writeFile(info.outputPath('stages.json'),JSON.stringify(traces.get(page),null,2)+'\n');return row;
}
async function activate(page,type){
  await page.waitForFunction(type=>[...document.querySelectorAll('#actions button')].some(b=>!b.disabled&&JSON.parse(b.dataset.action).type===type),type);
  const rect=await page.evaluate(type=>JSON.parse([...document.querySelectorAll('#actions button')].find(b=>!b.disabled&&JSON.parse(b.dataset.action).type===type).dataset.rect),type);
  await page.mouse.click(rect[0]+rect[2]/2,rect[1]+rect[3]/2);
}
async function recoverReadingOutput(page){
  // An active reading mode can advance after the native Sfx ends while the
  // trusted recovery click is in flight. Check source ordering below instead
  // of requiring an unchanged interaction across that legitimate transition.
  await page.waitForFunction(()=>!__nir.state().paused||document.querySelector('#nir-audio-resume')?.hidden===false);
  const prompt=page.locator('#nir-audio-resume');
  if(await prompt.isVisible()){
    await page.waitForFunction(()=>{const b=document.querySelector('#nir-audio-resume');return b.hidden||!b.disabled;});
    if(await prompt.isVisible()){
      try{await prompt.click({timeout:1000});}
      catch(error){if(error.name!=='TimeoutError'||!await page.evaluate(()=>!__nir.state().paused&&document.querySelector('#nir-audio-resume').hidden))throw error;}
    }
  }
  await page.waitForFunction(()=>!__nir.state().paused&&document.querySelector('#nir-audio-resume').hidden);
}
async function boot(page,worker){
  await page.addInitScript(()=>{
    globalThis.authoredAudio=[];globalThis.authoredAudioOrder=0;
    const create=AudioContext.prototype.createBufferSource;
    AudioContext.prototype.createBufferSource=function(...args){
      const source=create.apply(this,args),context=this,row={id:authoredAudio.length+1,source,context,
        started:null,duration:null,stops:0,ended:false,endedAt:null};authoredAudio.push(row);
      const start=source.start,stop=source.stop;
      source.start=function(...args){const result=start.apply(this,args);row.started=context.currentTime;row.duration=this.buffer.duration;row.startedOrder=++authoredAudioOrder;return result;};
      source.stop=function(...args){row.stops++;return stop.apply(this,args);};
      source.addEventListener('ended',()=>{row.ended=true;row.endedAt=context.currentTime;row.endedOrder=++authoredAudioOrder;});return source;
    };
  });
  const fixture=await(await page.request.get(`${origin}/__reading_fixture/delay`)).json();
  await page.goto(`${origin}/?test=1&worker=${worker}&backend=webgl2`);
  await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
  await page.evaluate(async()=>{
    const channel=await(await fetch('/channels/stable.json')).json(),release=await(await fetch(`/releases/${channel.release}.json`)).json();
    const request=indexedDB.open('nir-player-isolated-v1',1),db=await new Promise(ok=>{request.onsuccess=()=>ok(request.result);});
    try{await new Promise((ok,no)=>{const tx=db.transaction('profile','readwrite');tx.objectStore('profile').put(['read:letter:1'],[release.game_id,release.profile]);tx.oncomplete=ok;tx.onabort=()=>no(tx.error);});}finally{db.close();}
  });
  await page.reload();await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
  expect(await page.evaluate(()=>__nir.state().execution.runtime)).toBe(worker==='required'?'worker':'main');
  return fixture;
}
test.beforeEach(async({page})=>{
  const row={errors:[],stages:{},scope:'Current SDK neutral compiled MP3 dialogue Gate and native Sfx completion; trusted reading inputs, resource barriers and native source clocks. No physical-output or Android proof.'};
  traces.set(page,row);page.on('pageerror',e=>row.errors.push(e.message));
});
test.afterEach(async({page},info)=>{
  const final=await snapshot(page).catch(()=>null);
  await fs.writeFile(info.outputPath('terminal.json'),JSON.stringify({...traces.get(page),status:info.status,final},null,2)+'\n');
});

for(const worker of ['required','main'])for(const mode of ['auto','held','latched'])
for(const disruption of ['delay','http','menu','stall'])test(`${mode} respects an authored Gate and Sfx await through ${disruption}; ${worker}`,async({page},info)=>{
  Object.assign(traces.get(page),{worker,mode,disruption});
  const fixture=await boot(page,worker),path=fixture.voiceObjects[1];
  let attempts=0,releaseFirst,releaseRetry;
  const first=new Promise(ok=>releaseFirst=ok),retry=new Promise(ok=>releaseRetry=ok);
  if(disruption==='delay'||disruption==='http')await page.context().route(`**/${path}`,async route=>{
    if(++attempts===1){await first;if(disruption==='http')return route.fulfill({status:503,body:'temporary Sfx failure'}).catch(()=>{});}
    else await retry;
    await route.continue().catch(()=>{});
  });
  try{
    await activate(page,'new_game');
    await page.waitForFunction(()=>__nir.state().dialogue?.id==='letter'&&!__nir.state().loading&&authoredAudio.some(r=>r.started!==null&&!r.source.loop));
    // Release the output barrier after the actual voice has been scheduled.
    // An earlier API-ready snapshot can precede the new bus resume request.
    await recoverAudioOutput(page);
    await page.waitForFunction(()=>!__nir.state().paused&&Object.values(__nir.diagnostics().host_work.audio_domains.story.buses).every(bus=>bus.state==='running'&&!bus.resume_pending));
    const before=await checkpoint(page,info,'before');
    if(mode==='auto'){
      await page.evaluate(()=>__nir.action({type:'auto_wait_voice',enabled:false}));
      await page.waitForFunction(()=>!__nir.state().preferences.auto_wait_voice);await activate(page,'toggle_auto');
      // A trusted canvas click returns before its Worker pointer query commits.
      // This scenario requires Auto enabled before revealing the authored Gate.
      await page.waitForFunction(()=>__nir.state().auto);
      await page.keyboard.press('Space');
    }else if(mode==='held')await page.keyboard.down('ControlLeft');
    else await activate(page,'toggle_skip');
    await page.waitForFunction(()=>__nir.state().dialogue?.gate);
    const gate=await checkpoint(page,info,'gate');expect(gate.state.interaction).toBe(before.state.interaction);
    expect(gate.state.auto).toBe(mode==='auto');
    // The host may release held input as soon as the Gate enters loading.
    if(mode==='latched')expect(gate.state.skip).toBe(true);
    expect(gate.state.history_count).toBe(before.state.history_count);expect(loops(gate)).toEqual(loops(before));
    if(disruption==='delay'||disruption==='http'){
      await page.waitForFunction(()=>__nir.state().loading);await expect.poll(()=>attempts).toBe(1);
      if(mode==='held')await page.keyboard.up('ControlLeft');
      const held=await checkpoint(page,info,'held');
      for(let i=0;i<5;i++)await page.keyboard.press('Space');
      await page.evaluate(()=>Promise.all(Array.from({length:30},()=>__nir.action({type:'advance'}))));
      await page.waitForTimeout(700);const waiting=await checkpoint(page,info,'waiting');
      expect(identity(waiting)).toEqual(identity(held));expect(waiting.state.dialogue.gate).toBe(true);
      expect(waiting.domains.story.buses.bgm.clock_seconds-held.domains.story.buses.bgm.clock_seconds).toBeGreaterThan(.5);
      expect(loops(waiting)).toEqual(loops(before));releaseFirst();
      if(disruption==='http'){
        await page.waitForFunction(()=>__nir.state().error!==null);const failed=await checkpoint(page,info,'failed');
        await page.waitForTimeout(250);expect(identity(await snapshot(page))).toEqual(identity(failed));
        await activate(page,'retry');await expect.poll(()=>attempts).toBe(2);
        await page.waitForFunction(()=>__nir.state().loading&&__nir.state().retrying);
        const retrying=await checkpoint(page,info,'retrying');await page.waitForTimeout(250);
        expect(identity(await snapshot(page))).toEqual(identity(retrying));releaseRetry();
      }
    }
    await page.waitForFunction(()=>!__nir.state().loading&&authoredAudio.some(r=>r.started!==null&&!r.source.loop&&Math.abs(r.duration-2)<.01&&!r.ended));
    await recoverAudioOutput(page);const playing=await checkpoint(page,info,'playing'),sfx=playing.audio.find(r=>!r.loop&&Math.abs(r.duration-2)<.01);
    expect(playing.state.dialogue.id).toBe('letter');expect(playing.state.dialogue.gate).toBe(true);
    expect(playing.state.error).toBeNull();expect(playing.state.story_clock.paused_advance_us).toBe(0);
    expect(sfx.stops).toBe(0);expect(loops(playing)).toEqual(loops(before));
    if(disruption==='menu'){
      await activate(page,'menu');await page.waitForFunction(()=>__nir.state().screen==='Menu');
      const menu=await checkpoint(page,info,'menu');await page.waitForTimeout(400);const paused=await checkpoint(page,info,'menuHeld');
      expect(identity(paused)).toEqual(identity(menu));
      for(const bus of ['voice','sfx'])expect(Math.abs(paused.domains.story.buses[bus].clock_seconds-menu.domains.story.buses[bus].clock_seconds)).toBeLessThan(.02);
      expect(paused.domains.story.buses.bgm.clock_seconds-menu.domains.story.buses.bgm.clock_seconds).toBeGreaterThan(.3);
      expect(paused.audio.find(r=>r.id===sfx.id).ended).toBe(false);
      if(mode==='held')await page.keyboard.up('ControlLeft');
      await activate(page,'close');
      if(mode==='held')await recoverAudioOutput(page);else await recoverReadingOutput(page);
    }else if(disruption==='stall'){
      if(mode==='held')await page.keyboard.up('ControlLeft');
      traces.get(page).stall=await page.evaluate(()=>{const start=performance.now(),until=start+2500;while(performance.now()<until){/* Main-thread work; native audio graph keeps running. */}return {elapsedMs:performance.now()-start};});
      expect(traces.get(page).stall.elapsedMs).toBeGreaterThanOrEqual(2500);
    }else {
      for(let i=0;i<5;i++)await page.keyboard.press('Space');
      const early=await checkpoint(page,info,'early');expect(early.state.dialogue.gate).toBe(true);
      expect(early.state.history_count).toBe(before.state.history_count);expect(early.audio.find(r=>r.id===sfx.id).stops).toBe(0);
    }
    await page.waitForFunction(id=>authoredAudio.find(r=>r.id===id)?.ended,sfx.id);
    if(mode==='held'){
      await page.waitForFunction(()=>__nir.state().dialogue?.id==='letter'&&!__nir.state().dialogue.gate&&!__nir.state().loading);
      const released=await checkpoint(page,info,'released');expect(released.state.skip).toBe(false);
      expect(released.state.history_count).toBe(before.state.history_count);
      await page.waitForTimeout(350);expect((await snapshot(page)).state.interaction).toBe(before.state.interaction);
      if(!await page.evaluate(()=>__nir.state().dialogue.ready))await page.keyboard.press('Space');
      await page.waitForFunction(()=>__nir.state().dialogue?.ready);await page.keyboard.press('Space');
    }
    await page.waitForFunction(()=>__nir.state().dialogue?.id==='after_gate'&&!__nir.state().loading&&!__nir.state().paused);
    if(mode==='auto'){await activate(page,'toggle_auto');await page.waitForFunction(()=>!__nir.state().auto);}
    else await page.waitForFunction(()=>!__nir.state().skip);
    const after=await checkpoint(page,info,'after'),finished=after.audio.find(r=>r.id===sfx.id);
    expect(finished.ended).toBe(true);expect(finished.stops).toBe(0);
    expect(finished.clock-finished.started).toBeGreaterThanOrEqual(sfx.duration-.02);
    expect(after.state.history_count).toBe(before.state.history_count+1);expect(after.state.skip).toBe(false);
    const nextVoice=after.audio.filter(r=>!r.loop&&Math.abs(r.duration-3)<.01);expect(nextVoice).toHaveLength(1);
    expect(nextVoice[0].startedOrder).toBeGreaterThan(finished.endedOrder);
    expect(loops(after)).toEqual(loops(before));expect(after.state.error).toBeNull();expect(traces.get(page).errors).toEqual([]);
    traces.get(page).attempts=attempts;
    await page.waitForTimeout(350);const settled=await checkpoint(page,info,'settled');
    expect(settled.state.interaction).toBe(after.state.interaction);expect(settled.state.history_count).toBe(after.state.history_count);
  }finally{
    releaseFirst();releaseRetry();await page.context().unroute(`**/${path}`).catch(error=>{if(!page.isClosed())throw error;});
    if(!page.isClosed())await page.keyboard.up('ControlLeft').catch(error=>{if(!page.isClosed())throw error;});
  }
});
