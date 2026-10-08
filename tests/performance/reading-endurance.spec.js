import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import path from 'node:path';
import {sampleProcessMemory,trend} from './process-memory.js';
import {positiveInteger} from './scenarios.js';
import {recoverAudioOutput} from '../nir-next/audio-output-helper.js';
import {buildReadingEnduranceFixture,closeScaleFixture} from './fixtures.js';

const seconds=positiveInteger(process.env.NIR_READING_ENDURANCE_SECONDS,120,'NIR_READING_ENDURANCE_SECONDS');
const paceMs=positiveInteger(process.env.NIR_READING_ENDURANCE_PACE_MS,3000,'NIR_READING_ENDURANCE_PACE_MS',1500);
const worker=process.env.NIR_READING_ENDURANCE_WORKER||'required';
if(!['required','main'].includes(worker))throw new Error('NIR_READING_ENDURANCE_WORKER must be required or main');
const reportPath=process.env.NIR_READING_ENDURANCE_REPORT||`reports/experience-reading-endurance-${worker}-${seconds}s.json`;

// Instrument successful native starts with numeric records only. Retain no
// AudioBufferSourceNode/AudioBuffer references, and delete records on stop/end.
async function installAudioAudit(page){
  await page.addInitScript(()=>{
    const contexts=new WeakMap(),Native=AudioContext;
    globalThis.endurancePhase='reading';
    globalThis.enduranceAudio={contexts:0,next:0,starts:0,stops:0,ends:0,loopStarts:0,loopStops:0,active:new Map(),readingStopSamples:[],maxReadingStopRemainderSeconds:0};
    globalThis.AudioContext=class extends Native {
      constructor(...args){super(...args);contexts.set(this,enduranceAudio.contexts++);}
    };
    const create=Native.prototype.createBufferSource;
    Native.prototype.createBufferSource=function(...args){
      const source=create.apply(this,args),start=source.start,stop=source.stop,route=contexts.get(this);let id=null;
      source.start=function(...args){
        const result=start.apply(this,args);id=++enduranceAudio.next;
        const record={id,route,loop:this.loop,frames:this.buffer.length,rate:this.buffer.sampleRate,
          duration:this.buffer.duration,offset:args[1]||0,startedMs:performance.now(),startedClock:this.context.currentTime};
        enduranceAudio.starts++;if(record.loop)enduranceAudio.loopStarts++;
        enduranceAudio.active.set(id,record);return result;
      };
      source.stop=function(...args){
        if(id!==null&&enduranceAudio.active.has(id)){
          const record=enduranceAudio.active.get(id);
          if(record.route===2&&endurancePhase==='reading'){
            const remaining=Math.max(0,record.duration-record.offset-(this.context.currentTime-record.startedClock));
            enduranceAudio.maxReadingStopRemainderSeconds=Math.max(enduranceAudio.maxReadingStopRemainderSeconds,remaining);
            enduranceAudio.readingStopSamples.push({id,remainingSeconds:remaining,clock:this.context.currentTime});
            if(enduranceAudio.readingStopSamples.length>16)enduranceAudio.readingStopSamples.shift();
          }
          enduranceAudio.stops++;if(this.loop)enduranceAudio.loopStops++;
          enduranceAudio.active.delete(id);
        }
        return stop.apply(this,args);
      };
      source.addEventListener('ended',()=>{if(id!==null&&enduranceAudio.active.delete(id))enduranceAudio.ends++;},{once:true});
      return source;
    };
  });
}
const snapshot=page=>page.evaluate(()=>{
  const s=__nir.state(),d=__nir.diagnostics(),a=enduranceAudio;
  const {turns=[],...performanceSummary}=d.performance;
  const durations=turns.map(t=>t.total_us);
  performanceSummary.recent_turns={samples:durations.length,
    min_total_us:durations.length?Math.min(...durations):null,
    max_total_us:durations.length?Math.max(...durations):null,
    mean_total_us:durations.length?durations.reduce((sum,v)=>sum+v,0)/durations.length:null};
  return {state:s,audio:{contexts:a.contexts,starts:a.starts,stops:a.stops,ends:a.ends,
    loopStarts:a.loopStarts,loopStops:a.loopStops,active:[...a.active.values()],
    maxReadingStopRemainderSeconds:a.maxReadingStopRemainderSeconds,readingStopSamples:[...a.readingStopSamples]},
    domains:d.host_work.audio_domains,media:d.resource_memory.media,renderer:d.resource_memory.renderer,
    host:d.host_work,staging:d.content_staging,performance:performanceSummary,trace:{dropped:d.dropped,events:d.events?.length},
    metrics:{...__nir.metrics}};
});
const readingIdentity=s=>({line:s.variables.line_count.value,interaction:s.interaction,history:s.history_count,
  variables:s.variables,position:s.position,tick:s.tick_us});
async function waitScreen(page,screen){
  await page.waitForFunction(screen=>__nir.state().screen===screen&&!__nir.state().loading&&!__nir.state().locale_pending,screen);
}
async function activate(page,type,fields={}){
  const rect=await page.evaluate(({type,fields})=>{
    const b=[...document.querySelectorAll('#actions button')].find(b=>{
      const a=JSON.parse(b.dataset.action);return a.type===type&&Object.entries(fields).every(([k,v])=>a[k]===v);
    });
    if(!b||b.disabled)throw new Error(`No enabled control: ${JSON.stringify({type,fields})}`);
    return JSON.parse(b.dataset.rect);
  },{type,fields});
  const started=performance.now();await page.mouse.click(rect[0]+rect[2]/2,rect[1]+rect[3]/2);
  return performance.now()-started;
}
async function readyLine(page,line){
  await page.waitForFunction(line=>{
    const s=__nir.state();return s.screen==='Story'&&!s.loading&&s.variables.line_count.value===line&&s.dialogue?.ready&&!s.choice;
  },line);
  await recoverAudioOutput(page);
  expect(await page.evaluate(()=>[...enduranceAudio.active.values()].some(v=>v.route===2))).toBe(true);
  const s=await page.evaluate(()=>__nir.state());expect(s.error).toBeNull();expect(s.outcome).toBeNull();
  expect(s.variables.line_count.value).toBe(line);
  expect(s.history_count).toBe(Math.min(1000,line+Math.floor((line-1)/6)));
  return s;
}
async function loseRendererContext(page){
  // wgpu Device.destroy() alone does not deliver WebGL context loss on the
  // main thread. Exercise the real browser event for both ownership modes.
  if(worker==='required'){
    const owners=[];
    for(const w of page.workers())if(await w.evaluate(()=>self.__nirWorker?.role)==='runtime')owners.push(w);
    expect(owners).toHaveLength(1);
    await owners[0].evaluate(()=>__nirWorker.loseContext());
  }else{
    await page.evaluate(()=>{
      const gl=document.querySelector('#stage').getContext('webgl2');
      const extension=gl?.getExtension('WEBGL_lose_context');
      if(!extension)throw new Error('WebGL context loss unavailable');
      extension.loseContext();
    });
  }
}
async function storedSave(page){
  return page.evaluate(async()=>{
    const db=await new Promise((ok,no)=>{const r=indexedDB.open('nir-player-isolated-v1');r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});
    try {
      const rows=await new Promise((ok,no)=>{const tx=db.transaction('saves'),r=tx.objectStore('saves').getAll();tx.oncomplete=()=>ok(r.result);tx.onerror=()=>no(tx.error);});
      return rows.find(r=>r.envelope?.slot===0)?.envelope||null;
    }finally{db.close();}
  });
}

test(`continuous Story reading with menus history and real storage, ${worker}`,async({browser,context,page},info)=>{
  test.setTimeout((seconds+240)*1000);
  const fixture=await buildReadingEnduranceFixture();
  const report={format:1,status:'incomplete',requestedSeconds:seconds,paceMs,worker,
    instrumentation:{nativeAudioSourceCounters:true,sourceReferencesRetained:false,runtimeProfiling:true,playwrightTrace:info.project.use.trace,
      deviceLoss:'WEBGL_lose_context browser event',headless:info.project.use.headless,playwrightDefaultMuteAudio:true,
      performanceSamples:'cumulative stages and recent-turn duration summary; full rolling trace not duplicated per line'},
    limits:['Neutral compiler-built route, not the supplied commercial game.','Native AudioContext, with Playwright default --mute-audio; API clocks and sources do not prove acoustic or unmuted device output.','RSS sum double-counts shared pages; PSS covers reported browser processes. GPU physical allocation and decoder workspace unmeasured.'],
    fixtureVerification:fixture.verification,
    operations:{menu:0,history:0,historyReplay:0,saveLoad:0,choice:0,deviceRecovery:0},rows:[],actions:[]};
  const browserSession=await browser.newBrowserCDPSession(),pageSession=await context.newCDPSession(page);
  report.browser=await browserSession.send('Browser.getVersion');
  await pageSession.send('Performance.enable');
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await installAudioAudit(page);await fs.mkdir(path.dirname(reportPath),{recursive:true});
  const samplesFile=reportPath+'.jsonl';await fs.writeFile(samplesFile,'');report.samplesFile=samplesFile;
  let checkpointAt=0,started=performance.now();
  const checkpoint=async(force=false)=>{
    if(!force&&performance.now()-checkpointAt<60000)return;
    report.elapsedSeconds=(performance.now()-started)/1000;
    await fs.writeFile(reportPath+'.tmp',JSON.stringify(report,null,2));await fs.rename(reportPath+'.tmp',reportPath);
    checkpointAt=performance.now();
  };
  try {
    await page.goto(`${fixture.origin}/?test=1&worker=${worker}&backend=webgl2`);
    await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    report.release=await page.evaluate(()=>__nir.diagnostics().release);
    report.engine=await page.evaluate(()=>__nir.diagnostics().engine);
    report.execution=await page.evaluate(()=>__nir.diagnostics().execution);
    report.adapter=await page.evaluate(()=>__nir.state().adapter);
    await activate(page,'new_game');await readyLine(page,1);
    started=performance.now();await checkpoint(true);
    let line=1;
    do {
      const initial=await snapshot(page),loop=initial.audio.active.find(v=>v.loop);
      expect(initial.audio.contexts).toBe(4);expect(loop).toBeTruthy();
      expect(initial.audio.loopStarts).toBe(1+report.operations.saveLoad);
      expect(initial.audio.loopStops).toBe(report.operations.saveLoad);
      if(line%4===0){
        await page.evaluate(()=>{endurancePhase='menu';});
        await activate(page,'menu');await waitScreen(page,'Menu');
        const before=await snapshot(page);await page.waitForTimeout(150);const after=await snapshot(page);
        expect(readingIdentity(after.state)).toEqual(readingIdentity(before.state));
        expect(after.audio.active.find(v=>v.loop)?.id).toBe(loop.id);
        expect(after.domains.story.buses.bgm.clock_seconds).toBeGreaterThan(before.domains.story.buses.bgm.clock_seconds);
        expect(after.domains.story.buses.voice.paused).toBe(true);
        await activate(page,'close');await waitScreen(page,'Story');await recoverAudioOutput(page);
        expect((await snapshot(page)).audio.active.find(v=>v.loop)?.id).toBe(loop.id);
        report.operations.menu++;
      }
      if(line%3===0){
        await page.evaluate(()=>{endurancePhase='history';});
        await activate(page,'history');await waitScreen(page,'History');
        const before=await snapshot(page);
        await activate(page,'history_voice');
        await page.waitForFunction(()=>__nir.state().history_voice&&!__nir.state().history_voice.preparing&&[...enduranceAudio.active.values()].some(v=>v.route===1));
        await page.waitForTimeout(150);const playing=await snapshot(page);
        expect(readingIdentity(playing.state)).toEqual(readingIdentity(before.state));
        expect(playing.audio.active.find(v=>v.loop)?.id).toBe(loop.id);
        expect(playing.domains.story.buses.bgm.clock_seconds).toBeGreaterThan(before.domains.story.buses.bgm.clock_seconds);
        expect(playing.domains.foreground_ui.buses.voice.state).toBe('running');
        await activate(page,'close');await waitScreen(page,'Story');await recoverAudioOutput(page);
        const closed=await snapshot(page);expect(closed.state.history_voice).toBeNull();
        expect(closed.audio.active.filter(v=>v.route===1)).toHaveLength(0);
        expect(closed.state.variables).toEqual(before.state.variables);
        expect(closed.state.interaction).toBe(before.state.interaction);
        expect(closed.audio.active.find(v=>v.loop)?.id).toBe(loop.id);
        report.operations.history++;report.operations.historyReplay++;
      }
      if(line%5===0){
        await page.evaluate(()=>{endurancePhase='save-load';});
        await activate(page,'menu');await waitScreen(page,'Menu');
        await activate(page,'saves');await waitScreen(page,'Saves');
        const saved=await snapshot(page),previous=await storedSave(page);
        await activate(page,'save',{slot:0});let confirmed=false;
        await expect.poll(async()=>{
          const confirmation=await page.evaluate(()=>[...document.querySelectorAll('#actions button')].some(b=>JSON.parse(b.dataset.action).type==='confirm_save'));
          if(confirmation&&!confirmed){confirmed=true;await activate(page,'confirm_save');}
          return (await storedSave(page))?.revision||0;
        }).toBe((previous?.revision||0)+1);
        const envelope=await storedSave(page);
        expect(envelope.snapshot.variables).toEqual(saved.state.variables);
        expect(envelope.snapshot.history).toHaveLength(saved.state.history_count);
        const music=Object.values(envelope.snapshot.tasks).find(t=>t.effect.type==='audio'&&t.effect.looped);
        expect(music.audio_position_us).not.toBeNull();
        await activate(page,'load',{slot:0});await waitScreen(page,'Story');
        await page.waitForFunction(session=>__nir.state().session!==session&&!__nir.state().loading,saved.state.session);
        const restored=await snapshot(page);
        expect(restored.state.variables).toEqual(saved.state.variables);expect(restored.state.position).toEqual(saved.state.position);
        expect(restored.state.history_count).toBe(saved.state.history_count);
        expect(restored.state.paused).toBe(true);
        const restoredLoop=restored.audio.active.find(v=>v.loop);expect(restoredLoop.id).not.toBe(loop.id);
        expect(restoredLoop.offset).toBeCloseTo(Number(music.audio_position_us)/1e6%restoredLoop.duration,6);
        await activate(page,'continue');await recoverAudioOutput(page);
        const continued=await snapshot(page);expect(continued.state.interaction).toBe(restored.state.interaction);
        expect(continued.audio.active.find(v=>v.loop)?.id).toBe(restoredLoop.id);
        report.operations.saveLoad++;
        if(report.operations.saveLoad%3===0){
          const before=await snapshot(page);
          await loseRendererContext(page);
          await page.waitForFunction(device=>__nir.state().device>device&&__nir.state().ready&&!__nir.state().loading,before.state.device,{timeout:15000});
          await recoverAudioOutput(page);
          const after=await snapshot(page);
          expect(after.state.variables).toEqual(before.state.variables);expect(after.state.interaction).toBe(before.state.interaction);
          expect(after.audio.active.find(v=>v.loop)?.id).toBe(restoredLoop.id);report.operations.deviceRecovery++;
        }
      }
      await page.evaluate(()=>{endurancePhase='reading';});
      await page.waitForTimeout(paceMs);
      const beforeAdvance=await snapshot(page);
      expect(beforeAdvance.state.paused).toBe(false);expect(beforeAdvance.state.error).toBeNull();
      await page.evaluate(()=>{endurancePhase='advance';});
      const dispatchStart=performance.now(),dispatchMs=await activate(page,'advance');
      if(line%6===0){
        await page.waitForFunction(()=>__nir.state().choice&&!__nir.state().loading);
        const choice=report.operations.choice%2?'stay':'walk';
        await activate(page,'choose',{option:choice});report.operations.choice++;
      }
      await readyLine(page,++line);
      const sample=await snapshot(page);expect(errors).toEqual([]);
      expect(sample.audio.active.length).toBeLessThanOrEqual(3);
      expect(sample.state.content_residency.resident_bytes).toBeLessThanOrEqual(sample.state.content_residency.budget_bytes);
      const memory=await sampleProcessMemory(browserSession,pageSession);
      report.actions.push({line,dispatchMs,actionToReadyMs:performance.now()-dispatchStart});
      const row={line,elapsedMs:performance.now()-started,identity:readingIdentity(sample.state),session:sample.state.session,device:sample.state.device,
        audio:sample.audio,domains:sample.domains,media:sample.media,renderer:sample.renderer,
        estimatedMediaBytes:sample.state.resident_bytes,wasmCapacityBytes:sample.state.wasm_memory_bytes,
        staging:sample.staging,performance:sample.performance,metrics:sample.metrics,...memory};
      report.rows.push(row);await fs.appendFile(samplesFile,JSON.stringify(row)+'\n');await checkpoint();
    }while(performance.now()-started<seconds*1000);
    report.finalReading=await snapshot(page);report.lines=line;
    await page.screenshot({path:info.outputPath('final-reading.png')});
    await activate(page,'menu');await waitScreen(page,'Menu');await activate(page,'title');await waitScreen(page,'Title');
    await page.waitForFunction(()=>{
      const h=__nir.diagnostics().host_work;return !h.media_jobs&&!h.content_jobs&&!h.shared_fetches&&!h.decode_pool_active&&!h.decode_pool_waiting&&!h.pending_owner_callbacks&&__nir.metrics.activeRequests===0;
    });
    report.finalTitle=await snapshot(page);expect(report.finalTitle.audio.active).toHaveLength(0);
    expect(report.finalTitle.media.decoded_audio_bytes).toBe(0);
    expect(report.finalTitle.audio.loopStarts).toBe(1+report.operations.saveLoad);
    expect(report.finalTitle.audio.loopStops).toBe(1+report.operations.saveLoad);
    report.trends=Object.fromEntries(['wasmCapacityBytes','rssSumBytes','pssSumBytes','jsHeapUsedBytes'].map(field=>[field,trend(report.rows,field)]));
    report.memoryCoverage=Object.fromEntries(['rssSumBytes','pssSumBytes','jsHeapUsedBytes'].map(field=>[field,report.rows.filter(r=>Number.isFinite(r[field])).length/report.rows.length]));
    report.status='passed';
  }catch(error){
    report.status='failed';report.error=String(error);report.failure=await snapshot(page).catch(()=>null);throw error;
  }finally{
    try {await checkpoint(true);}finally{
      // Playwright may already have closed the browser after a timeout. Its
      // detached CDP sessions must not mask the failure or leak our server.
      await Promise.allSettled([pageSession.detach(),browserSession.detach()]);
      await closeScaleFixture(fixture);
    }
  }
});
