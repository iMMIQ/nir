import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import {distribution,installAdapterProbe,assertHardwareAdapter} from './metrics.js';
import {networkProfiles,applyNetwork,positiveInteger} from './scenarios.js';
const samples=positiveInteger(process.env.NIR_PERF_SAMPLES,20,'NIR_PERF_SAMPLES',2);
const cycles=positiveInteger(process.env.NIR_PERF_CYCLES,30,'NIR_PERF_CYCLES');
const hardware=process.env.NIR_PERF_MODE==='hardware';
async function ready(page){await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);}
async function firstLine(page){await page.keyboard.press('Space');await page.waitForFunction(()=>window.__nir.state().dialogue&&!window.__nir.state().loading);}
async function measure(page,cache){
  await ready(page);
  if(hardware)assertHardwareAdapter(...await page.evaluate(()=>[window.__nir.state().adapter,window.__nirActualAdapters]));
  await page.evaluate(()=>{
    window.__activeIntervals=[];window.__measureActive=true;let last=null;
    function sample(now){if(!window.__measureActive)return;const active=window.__nir.needsClock();if(active&&last!==null&&window.__activeIntervals.length<512)window.__activeIntervals.push(now-last);last=active?now:null;requestAnimationFrame(sample);}
    requestAnimationFrame(sample);
  });
  await firstLine(page);await page.waitForTimeout(1200);
  return page.evaluate(async cache=>{
    window.__measureActive=false;
    const api=window.__nir,frames=api.state().frames;
    await api.action({type:'menu'});
    await new Promise((resolve,reject)=>{const deadline=performance.now()+10000;function poll(){const state=api.state();if(state.screen==='Menu'&&state.frames>frames)return resolve();if(performance.now()>deadline)return reject(Error('no related submit'));requestAnimationFrame(poll);}poll();});
    const report=api.diagnostics(),rows=report.events;
    const input=rows.findLast(e=>e.stage==='input_received');
    const submit=input&&rows.find(e=>e.stage==='render_submitted'&&e.frames===api.state().frames&&BigInt(e.at_us)>=BigInt(input.at_us));
    const intervals=rows.filter(e=>e.stage==='render_submitted').map(e=>Number(e.at_us)/1000);
    const resources=performance.getEntriesByType('resource').filter(e=>e.name.includes('/objects/'));
    if(!input||!submit)throw Error('incomplete prepared-input trace');
    return {cache,adapterProbe:window.__nirActualAdapters.at(-1),actualAdapters:window.__nirActualAdapters,release:report.release,engine:report.engine,adapter:api.state().adapter,navigationToFirstLineMs:api.metrics.navigationToFirstLineMs,
      preparedInput:'open_menu_on_prepared_scene',
      preparedInputToSubmitMs:(Number(submit.at_us)-Number(input.at_us))/1000,
      activeAnimationIntervalsMs:window.__activeIntervals,
      submittedIntervalsMs:intervals.slice(1).map((t,i)=>t-intervals[i]),
      wasmMemoryBytes:api.state().wasm_memory_bytes,estimatedResidentBytes:api.state().resident_bytes,
      startupEvents:rows.filter(e=>['bootstrap_started','channel_ready','release_verified','object_downloaded','object_verified','wasm_verified','wasm_initialized','preferences_loaded','engine_created','first_line_submitted'].includes(e.stage)),
      resources:resources.map(e=>({name:new URL(e.name).pathname,startTimeMs:e.startTime,durationMs:e.duration,
        fetchStartMs:e.fetchStart,domainLookupStartMs:e.domainLookupStart,domainLookupEndMs:e.domainLookupEnd,
        connectStartMs:e.connectStart,connectEndMs:e.connectEnd,requestStartMs:e.requestStart,
        responseStartMs:e.responseStart,responseEndMs:e.responseEnd,transferSize:e.transferSize,
        encodedBodySize:e.encodedBodySize,decodedBodySize:e.decodedBodySize})),
      objectRequests:resources.length,zeroTransferObjects:resources.filter(e=>e.transferSize===0).length};
  },cache);
}
for(const profile of networkProfiles)test(`navigation baseline with isolated browser caches and warm reloads (${profile.id})`,async({browser})=>{
  expect(samples).toBeGreaterThanOrEqual(2);const runs=[];
  for(let i=0;i<samples;i++){
    const context=await browser.newContext({viewport:{width:1280,height:800},locale:'zh-CN'});
    try{
      await installAdapterProbe(context);
      const page=await context.newPage();await applyNetwork(context,page,profile);await page.goto('http://127.0.0.1:4173/?test=1');
      runs.push(await measure(page,'isolated-context'));
      await page.reload();runs.push(await measure(page,'warm-reload'));
    }finally{await context.close();}
  }
  const summary=Object.fromEntries(['isolated-context','warm-reload'].map(cache=>{
    const r=runs.filter(r=>r.cache===cache),active=r.flatMap(r=>r.activeAnimationIntervalsMs);return [cache,{activeAnimationIntervalsMs:distribution(active),activeOver33msRatio:active.filter(ms=>ms>33.3).length/active.length,navigationToFirstLineMs:distribution(r.map(r=>r.navigationToFirstLineMs)),preparedInputToSubmitMs:distribution(r.map(r=>r.preparedInputToSubmitMs))}];
  }));
  expect(runs.every(r=>r.release===runs[0].release)).toBe(true);
  await fs.mkdir('reports',{recursive:true});
  const filename=profile.id==='loopback'?'performance-baseline.json':`performance-baseline-${profile.id}.json`;
  const referenceTargets={navigationToFirstLineP95Ms:4000,preparedInputToSubmitP95Ms:50,enforced:false};
  const targetComparison=Object.fromEntries(Object.entries(summary).map(([cache,row])=>[cache,{firstLineWithinTarget:row.navigationToFirstLineMs.p95<=4000,preparedInputWithinTarget:row.preparedInputToSubmitMs.p95<=50}]));
  await fs.writeFile(`reports/${filename}`,JSON.stringify({format:1,browser:browser.version(),mode:hardware?'hardware':'ci',network:profile,smoke:samples<20,instrumentation:{runtimeDiagnostics:true,playwrightTrace:test.info().project.use.trace},environment:`${profile.id} HTTP; new contexts isolate HTTP caches, shared browser/GPU process is not cold`,gpuTime:'unmeasured',physicalMemory:'unmeasured',referenceTargets,targetComparison,summary,runs},null,2));
});
test('repeated save restore and device recovery resource trend',async({page,context,browser})=>{
  await installAdapterProbe(context);
  await page.goto('/?test=1');await ready(page);await firstLine(page);
  if(hardware)assertHardwareAdapter(...await page.evaluate(()=>[window.__nir.state().adapter,window.__nirActualAdapters]));
  const action=a=>page.evaluate(a=>window.__nir.action(a),a);
  await action({type:'saves'});await action({type:'save',slot:0});
  await page.waitForFunction(()=>/已保存|Saved/.test(window.__nir.state().status));
  const rows=[];
  for(let cycle=0;cycle<cycles;cycle++){
    await action({type:'load',slot:0});
    await page.waitForFunction(()=>window.__nir.state().screen==='Story'&&window.__nir.state().paused&&!window.__nir.state().loading);
    if(cycle%10===9){
      const device=await page.evaluate(()=>{const d=window.__nir.state().device;window.__nir.loseDevice();return d;});
      await page.waitForFunction(d=>window.__nir.state().device>d&&window.__nir.state().ready&&!window.__nir.state().loading,device);
      if(hardware)assertHardwareAdapter(...await page.evaluate(()=>[window.__nir.state().adapter,window.__nirActualAdapters]));
    }
    await action({type:'title'});await ready(page);
    await page.waitForFunction(()=>window.__nir.metrics.activeRequests===0);
    rows.push(await page.evaluate(cycle=>({cycle,atMs:performance.now(),wasmMemoryBytes:window.__nir.state().wasm_memory_bytes,estimatedResidentBytes:window.__nir.state().resident_bytes,activeRequests:window.__nir.metrics.activeRequests,device:window.__nir.state().device}),cycle));
    await action({type:'saves'});
  }
  await fs.writeFile('reports/performance-recovery.json',JSON.stringify({format:1,browser:browser.version(),cycles,physicalMemory:'unmeasured',rows},null,2));
  expect(rows.every(r=>r.activeRequests===0)).toBe(true);
});
