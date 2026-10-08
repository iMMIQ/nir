import {test,expect} from '@playwright/test';
import {recoverAudioOutput} from '../nir-next/audio-output-helper.js';
import {expectPainted,readPng} from './pixels.js';

async function waitForPaintedScene(page){
 await expect.poll(async()=>{
  try{expectPainted(await page.locator('#stage').screenshot());return true;}
  catch(error){
   if(!String(error.message).includes('canvas luminance range')&&!String(error.message).includes('canvas color diversity'))throw error;
   return false;
  }
 },{timeout:15000,message:'Scene must be visible in actual canvas pixels'}).toBe(true);
}
async function scenePixels(page){
 const image=readPng(await page.locator('#stage').screenshot()),pixels=[];
 // The rain fixture's static scene occupies this region. Exclude the top
 // toolbar and lower dialogue window so text/UI cannot mask a lost image.
 for(const y of [.1,.2,.3,.45,.6])for(const x of [.05,.2,.35,.6,.75,.9]){
  const i=(Math.floor(y*image.height)*image.width+Math.floor(x*image.width))*image.channels;
  pixels.push([...image.pixels.subarray(i,i+3)]);
 }
 expect(new Set(pixels.map(p=>p.join(','))).size,'rain scene must have varied background pixels').toBeGreaterThan(8);
 return pixels;
}

async function boot(page,options='worker=required&backend=webgl2'){
 await page.goto('/?test=1&'+options);
 await page.waitForFunction(()=>(globalThis.__nir?.state().ready&&!__nir.state().loading)||document.querySelector('#reload')?.hidden===false);
 expect(await page.locator('#shell-message').textContent()).not.toMatch(/E_[A-Z_]+/);
}
async function start(page){await page.keyboard.press('Space');await page.waitForFunction(()=>!!__nir.state().dialogue&&!__nir.state().loading);}
const act=(page,action)=>page.evaluate(a=>__nir.action(a),action);

test('Runtime owns WASM and clock; Asset Worker verifies and decodes; output stays bounded during a main stall',async({page})=>{
 await page.addInitScript(()=>{window.mainWasmInstantiations=0;for(const name of ['instantiate','instantiateStreaming']){const original=WebAssembly[name];WebAssembly[name]=function(...args){mainWasmInstantiations++;return original.apply(this,args);};}});
 await boot(page);await start(page);
 expect(await page.evaluate(()=>__nir.state().execution)).toMatchObject({runtime:'worker',asset:'worker',protocol:1});
 expect(await page.evaluate(()=>mainWasmInstantiations)).toBe(0);
 const workers=page.workers();expect(workers.length).toBe(2);
 const roles=await Promise.all(workers.map(async worker=>({worker,role:await worker.evaluate(()=>self.__nirWorker?.role)})));
 const runtime=roles.find(w=>w.role==='runtime').worker,asset=roles.find(w=>w.role==='asset').worker;
 expect(await runtime.evaluate(()=>typeof document)).toBe('undefined');
 const before=await runtime.evaluate(()=>__nirWorker.state().tick_us);
 await page.evaluate(()=>{const end=performance.now()+900;while(performance.now()<end){};});
 const after=await runtime.evaluate(()=>__nirWorker.state().tick_us);
 expect(Number(after)-Number(before)).toBeGreaterThan(650000);
 const queue=await runtime.evaluate(()=>__nirWorker.queues());expect(queue.unacknowledged).toBeLessThanOrEqual(1);expect(queue.outbound).toBeLessThanOrEqual(248);
 const assets=await asset.evaluate(()=>__nirWorker.stats());expect(assets.fetches).toBeGreaterThan(0);expect(assets.decodes).toBeGreaterThan(0);
 const timing=await page.evaluate(()=>__nir.diagnostics().worker_resource_timing);
 expect(timing.incomplete).toBe(false);expect(timing.rows.length).toBeGreaterThan(0);
 for(const row of timing.rows){expect(row.object).toMatch(/^[a-f0-9]{64}$/);expect(row.responseEnd).toBeGreaterThanOrEqual(row.startTime);expect(row.decodedBodySize).toBeGreaterThan(0);expect(row.name).toBeUndefined();}
 await act(page,{type:'menu'});await page.waitForFunction(()=>__nir.state().screen==='Menu');
 const paused=await runtime.evaluate(()=>__nirWorker.state().tick_us);await page.waitForTimeout(150);
 expect(await runtime.evaluate(()=>__nirWorker.state().tick_us)).toBe(paused);
 await act(page,{type:'save',slot:0});await page.waitForFunction(()=>/Saved|已保存/.test(__nir.state().status));
 await act(page,{type:'load',slot:0});await page.waitForFunction(()=>!__nir.state().loading&&__nir.state().paused);
 expect(await page.evaluate(()=>__nir.state().error)).toBeNull();
 await act(page,{type:'close'});await act(page,{type:'continue'});
 await page.setViewportSize({width:844,height:390});await page.waitForTimeout(150);
 expect(await page.evaluate(()=>__nir.state().execution.runtime)).toBe('worker');
 await act(page,{type:'title'});await page.waitForFunction(()=>__nir.state().screen==='Title'&&!__nir.state().loading);
 await page.waitForFunction(()=>__nir.metrics.activeRequests===0);
});

test('startup failure rebuilds the canvas and runs the same engine on main',async({page})=>{
 await page.addInitScript(()=>{const Worker=window.Worker;window.Worker=class extends Worker{constructor(){throw new Error('worker startup test failure');}};});
 await boot(page,'backend=webgl2');await start(page);
 const s=await page.evaluate(()=>__nir.state());expect(s.execution.runtime).toBe('main');expect(s.execution.fallback_reason).toContain('worker startup test failure');expect(s.error).toBeNull();
});

test('Worker GPU recovery keeps the authoritative session and resumes its existing reading policy',async({page})=>{
 await boot(page);await start(page);
 await recoverAudioOutput(page);
 await waitForPaintedScene(page);
 const pixels=await scenePixels(page);
 const before=await page.evaluate(()=>__nir.state());
 for(const worker of page.workers())if(await worker.evaluate(()=>self.__nirWorker?.role)==='runtime')await worker.evaluate(()=>__nirWorker.loseContext());
 await page.waitForFunction(device=>__nir.state().device>device&&__nir.state().ready&&!__nir.state().loading,before.device,{timeout:15000});
 await recoverAudioOutput(page);
 await waitForPaintedScene(page);
 expect(await scenePixels(page)).toEqual(pixels);
 const after=await page.evaluate(()=>__nir.state());expect(after.session).toBe(before.session);expect(after.variables).toEqual(before.variables);expect(after.error).toBeNull();expect(after.execution.runtime).toBe('worker');
 expect(after.paused).toBe(false);expect(after.position).toBe(before.position);expect(after.interaction).toBe(before.interaction);expect(after.history_count).toBe(before.history_count);
});

test('WebGPU renders in Runtime Worker and recovers without replacing its session',async({page})=>{
 await boot(page,'worker=required&backend=webgpu');await start(page);
 await recoverAudioOutput(page);
 await waitForPaintedScene(page);
 const pixels=await scenePixels(page);
 const before=await page.evaluate(()=>__nir.state());expect(before.backend).toBe('webgpu');expect(before.execution.runtime).toBe('worker');
 expect((await page.evaluate(()=>__nir.actualAdapters())).some(row=>row.adapterAvailable)).toBe(true);
 await page.evaluate(()=>__nir.loseDevice());
 await page.waitForFunction(device=>__nir.state().device>device&&__nir.state().ready&&!__nir.state().loading,before.device);
 await recoverAudioOutput(page);
 await waitForPaintedScene(page);
 expect(await scenePixels(page)).toEqual(pixels);
 const after=await page.evaluate(()=>__nir.state());expect(after.session).toBe(before.session);expect(after.position).toBe(before.position);expect(after.paused).toBe(false);expect(after.error).toBeNull();expect(after.interaction).toBe(before.interaction);expect(after.history_count).toBe(before.history_count);
});

test('runtime failure terminates both owners and exposes a reload',async({page})=>{
 await boot(page);await start(page);
 // Either worker failure is terminal; test the actual Runtime owner.
 for(const worker of page.workers())if(await worker.evaluate(()=>__nirWorker.role)==='runtime')await worker.evaluate(()=>{setTimeout(()=>{throw new Error('injected runtime failure');},0);});
 await expect(page.locator('#reload')).toBeVisible();
 await expect(page.locator('#shell-message')).toContainText('E_WORKER_CRASH');
 await expect.poll(()=>page.workers().length).toBe(0);
 const d=await page.evaluate(()=>__nir.diagnostics());expect(d.execution.runtime_pending).toBe(0);expect(d.execution.asset_pending).toBe(0);expect(d.host_work.request_slots).toBe(0);
});

test('page disposal closes workers and all outstanding reservations',async({page})=>{
 await boot(page);await start(page);
 await page.evaluate(()=>window.dispatchEvent(new PageTransitionEvent('pagehide',{persisted:false})));
 await expect.poll(()=>page.workers().length).toBe(0);
 const d=await page.evaluate(()=>__nir.diagnostics());expect(d.execution.runtime_pending).toBe(0);expect(d.execution.asset_pending).toBe(0);expect(d.host_work.request_slots).toBe(0);
});

test('idle loading time is discarded before the Runtime clock resumes',async({page,request})=>{
 const channel=await (await request.get('channels/stable.json')).json();
 const release=await (await request.get(`releases/${channel.release}.json`)).json();
 const index=await (await request.get(release.objects[release.program].path)).json();
 const image=release.objects[index.program.assets['actor.aki'].object].path;
 let unblock;const gate=new Promise(r=>unblock=r);
 await page.context().route(`**/${image}`,async route=>{await gate;await route.continue().catch(()=>{});});
 try{
  await boot(page);await start(page);await act(page,{type:'advance'});await act(page,{type:'advance'});
  await page.waitForFunction(()=>__nir.state().loading&&__nir.state().paused);
  const before=await page.evaluate(()=>__nir.state().tick_us);
  await page.waitForTimeout(900);expect(await page.evaluate(()=>__nir.state().tick_us)).toBe(before);
  unblock();await page.waitForFunction(()=>!__nir.state().loading&&!__nir.state().paused);
  expect(Number(await page.evaluate(()=>__nir.state().tick_us))-Number(before)).toBeLessThan(400000);
 }finally{unblock();if(!page.isClosed())await page.context().unroute(`**/${image}`);}
});

test('production updates contain compact host state and no VM variables',async({page})=>{
 await page.addInitScript(()=>{
  const Original=window.Worker;window.workerStateSamples=[];
  window.Worker=class extends Original{
   constructor(...args){super(...args);this.addEventListener('message',event=>{
    if(event.data.kind!=='update'||!event.data.value.host)return;
    const s=JSON.parse(event.data.value.state);
    if(workerStateSamples.length<64)workerStateSamples.push({session:s.session,full:'variables' in s||'dialogue' in s});
   });}
  };
 });
 await page.goto('/?worker=required&backend=webgl2');
 await expect(page.locator('#shell')).toBeHidden();
 await page.keyboard.press('Space');
 await expect(page.locator('#announcement')).toContainText('末班电车');
 expect(await page.evaluate(()=>window.__nir)).toBeUndefined();
 const samples=await page.evaluate(()=>workerStateSamples);
 expect(samples.length).toBeGreaterThan(0);expect(samples.every(s=>Number.isInteger(s.session)&&!s.full)).toBe(true);
});
