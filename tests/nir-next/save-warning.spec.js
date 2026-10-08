import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import {recoverAudioOutput} from './audio-output-helper.js';

const baseline=process.env.NIR_SAVE_WARNING_BASELINE==='1';
const errors=new WeakMap();
test.beforeEach(async({page})=>{const messages=[];errors.set(page,messages);page.on('pageerror',error=>messages.push(error.message));});
test.afterEach(async({page},info)=>{
  await fs.writeFile(info.outputPath('terminal.json'),JSON.stringify({status:info.status,errors:errors.get(page),
    final:await page.evaluate(()=>globalThis.warningSnapshot?.()).catch(()=>null)},null,2)+'\n');
});
async function boot(page,worker) {
  await page.addInitScript(()=>{
    const create=AudioContext.prototype.createBufferSource,put=IDBObjectStore.prototype.put;
    globalThis.warningAudio=[];globalThis.warningFail={};globalThis.warningWrites=[];
    IDBObjectStore.prototype.put=function(value,...args) {
      const key=this.name==='saves'?`save:${value.slot}`:this.name,reason=warningFail[key];
      const row={store:this.name,slot:value.slot??null,revision:value.envelope?.revision??null,
        rejected:!!reason,completed:false,aborted:false};warningWrites.push(row);
      this.transaction.addEventListener('complete',()=>row.completed=true,{once:true});
      this.transaction.addEventListener('abort',()=>row.aborted=true,{once:true});
      if(reason){delete warningFail[key];throw new DOMException('controlled write failure',reason);}
      return put.call(this,value,...args);
    };
    AudioContext.prototype.createBufferSource=function(...args) {
      const source=create.apply(this,args),row={source,context:this,id:warningAudio.length+1,stops:0};warningAudio.push(row);
      const stop=source.stop;source.stop=function(...args){row.stops++;return stop.apply(this,args);};return source;
    };
    globalThis.warningSnapshot=()=>{
      const s=__nir.state();return {state:s,story:{session:s.session,interaction:s.interaction,position:s.position,tick:s.tick_us,history:s.history_count,variables:s.variables},
        loops:warningAudio.filter(r=>r.source.loop).map(r=>({id:r.id,stops:r.stops,clock:r.context.currentTime})),writes:warningWrites,
        buttons:[...document.querySelectorAll('#actions button')].map(b=>({action:JSON.parse(b.dataset.action),disabled:b.disabled}))};
    };
  });
  await page.goto(`http://127.0.0.1:4259/?test=1&worker=${worker}&backend=webgl2`);
  await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
  await page.keyboard.press('Enter');await recoverAudioOutput(page);
  await page.waitForFunction(()=>__nir.state().dialogue?.ready&&!__nir.state().loading&&!__nir.state().paused);
  expect(await page.evaluate(()=>__nir.state().execution.runtime)).toBe(worker==='required'?'worker':'main');
  await page.evaluate(()=>__nir.action({type:'saves'}));
  await save(page,1);await page.waitForFunction(()=>__nir.state().status==='Saved');
  await page.evaluate(async()=>{
    const ch=await(await fetch('/channels/stable.json')).json(),rel=await(await fetch(`/releases/${ch.release}.json`)).json();
    globalThis.warningHost=await import('/'+rel.objects[rel.engine.host].path);globalThis.warningDb=await warningHost.openSaveDatabase();
    globalThis.warningKey=slot=>warningHost.saveKey(rel.game_id,rel.profile,ch.release,slot);
    globalThis.warningGood=await warningHost.readSaveRecord(warningDb,warningKey(1));
  });
  return page.evaluate(()=>warningSnapshot());
}
async function save(page,slot) {
  await page.waitForFunction(slot=>[...document.querySelectorAll('#actions button')].some(b=>{const a=JSON.parse(b.dataset.action);return a.type==='save'&&a.slot===slot&&!b.disabled;}),slot);
  const rect=await page.evaluate(slot=>JSON.parse([...document.querySelectorAll('#actions button')].find(b=>{const a=JSON.parse(b.dataset.action);return a.type==='save'&&a.slot===slot;}).dataset.rect),slot);
  expect(rect[2]).toBeGreaterThan(0);expect(rect[3]).toBeGreaterThan(0);
  await page.mouse.click(rect[0]+rect[2]/2,rect[1]+rect[3]/2);
}
const snapshot=page=>page.evaluate(()=>warningSnapshot());
const loops=s=>s.loops.map(({id,stops})=>({id,stops}));
async function unchanged(page,before) {
  const after=await snapshot(page);expect(after.story).toEqual(before.story);expect(loops(after)).toEqual(loops(before));expect(after.state.error).toBeNull();return after;
}
async function failSave(page,slot,reason) {
  await page.evaluate(({slot,reason})=>{warningFail[`save:${slot}`]=reason;},{slot,reason});
  await save(page,slot);await page.waitForFunction(slot=>!warningFail[`save:${slot}`]&&__nir.state().diagnostic?.details.operation==='save'&&__nir.state().status.includes('Storage operation failed'),slot);
  return snapshot(page);
}
async function savedRevision(page,slot,revision) {
  await expect.poll(()=>page.evaluate(async slot=>(await warningHost.readSaveRecord(warningDb,warningKey(slot)))?.envelope.revision,slot)).toBe(revision);
  await page.waitForFunction(()=>!__nir.state().status.includes('Saving'));
}
async function record(page,info,name,data) {
  expect(errors.get(page)).toEqual([]);
  await fs.writeFile(info.outputPath(name+'.json'),JSON.stringify({...data,baseline,final:await snapshot(page),
    scope:'Native IndexedDB abort/commit and another same-origin page write, trusted Save controls. Injected quota exception is not physical quota; no device/audio-output/Android claim.'},null,2)+'\n');
}
for(const worker of ['main','required']) {
  test(`save warning quota clears only on its confirmed retry; ${worker}`,async({page},info)=>{
    const before=await boot(page,worker),failed=await failSave(page,1,'QuotaExceededError');
    expect(failed.state.diagnostic.code).toBe('E_STORAGE_QUOTA');await unchanged(page,before);
    expect(await page.evaluate(()=>warningHost.readSaveRecord(warningDb,warningKey(1)))).toEqual(await page.evaluate(()=>warningGood));
    await save(page,1);await savedRevision(page,1,2);
    await page.waitForFunction(()=>__nir.state().status==='Saved');
    expect((await snapshot(page)).state.diagnostic?.code??null).toBe(baseline?'E_STORAGE_QUOTA':null);
    await unchanged(page,before);await record(page,info,'quota-retry',{worker,before,failed});
  });
  test(`save warning conflict preserves concurrent record, refreshes revision and clears after retry; ${worker}`,async({page},info)=>{
    const before=await boot(page,worker),key=await page.evaluate(()=>warningKey(1)),other=await page.context().newPage();
    try {
      await other.goto('http://127.0.0.1:4259/channels/stable.json');
      await other.evaluate(async key=>{
        const r=indexedDB.open('nir-player-isolated-v1',1),db=await new Promise((ok,no)=>{r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});
        try {await new Promise((ok,no)=>{
          const tx=db.transaction('saves','readwrite'),store=tx.objectStore('saves'),get=store.get(key);
          get.onsuccess=()=>{const value=get.result;value.envelope.revision=2;store.put(value,key);};tx.oncomplete=ok;tx.onabort=()=>no(tx.error);
        });}finally{db.close();}
      },key);
    }finally{await other.close();}
    const concurrent=await page.evaluate(async()=>{
      const value=await warningHost.readSaveRecord(warningDb,warningKey(1));
      const checked=await __nir.inspectSave(warningHost.stringifyPlayerData(value.envelope),warningKey(1));return {value,checked};
    });expect(concurrent.checked).toBe(2);
    await save(page,1);await page.waitForFunction(()=>__nir.state().diagnostic?.code==='E_SAVE_CONFLICT');
    const failed=await unchanged(page,before);expect(await page.evaluate(()=>warningHost.readSaveRecord(warningDb,warningKey(1)))).toEqual(concurrent.value);
    const completed=await page.evaluate(()=>__nir.metrics.completedRequests);
    await page.evaluate(()=>{__nir.action({type:'close'});__nir.action({type:'saves'});});
    await page.waitForFunction(n=>__nir.metrics.completedRequests>n&&__nir.state().screen==='Saves',completed);
    await save(page,1);await savedRevision(page,1,3);await page.waitForFunction(()=>__nir.state().status==='Saved');
    expect((await snapshot(page)).state.diagnostic?.code??null).toBe(baseline?'E_SAVE_CONFLICT':null);
    await unchanged(page,before);await record(page,info,'conflict-retry',{worker,before,concurrent,failed});
  });
  test(`save warning on one slot survives another successful save; ${worker}`,async({page},info)=>{
    const before=await boot(page,worker),failed=await failSave(page,0,'UnknownError');
    const request=failed.state.diagnostic.details.request;expect(failed.state.diagnostic.code).toBe('E_STORAGE');
    await save(page,2);await savedRevision(page,2,1);const other=await unchanged(page,before);
    expect(other.state.diagnostic?.code??null).toBe(baseline?null:'E_STORAGE');
    if(!baseline)expect(other.state.diagnostic.details.request).toBe(request);
    expect(await page.evaluate(()=>warningHost.readSaveRecord(warningDb,warningKey(0)))).toBeUndefined();
    await save(page,0);await savedRevision(page,0,1);await page.waitForFunction(()=>__nir.state().status==='Saved');
    expect((await snapshot(page)).state.diagnostic).toBeNull();await record(page,info,'other-slot-success',{worker,before,failed,other});
  });
  test(`save warnings retain two independently failed slots until each is retried; ${worker}`,async({page},info)=>{
    const before=await boot(page,worker),first=await failSave(page,0,'QuotaExceededError'),second=await failSave(page,1,'QuotaExceededError');
    await save(page,2);await savedRevision(page,2,1);const other=await unchanged(page,before);
    expect(other.state.diagnostic.details.request).toBe((baseline?second:first).state.diagnostic.details.request);
    await save(page,1);await savedRevision(page,1,2);const partial=await unchanged(page,before);
    expect(partial.state.diagnostic.details.request).toBe((baseline?second:first).state.diagnostic.details.request);
    await save(page,0);await savedRevision(page,0,1);
    expect((await snapshot(page)).state.diagnostic?.code??null).toBe(baseline?'E_STORAGE_QUOTA':null);
    await record(page,info,'independent-failed-slots',{worker,before,first,second,other,partial});
  });
  test(`successful save retry preserves a failed preference write until explicit recovery; ${worker}`,async({page},info)=>{
    const before=await boot(page,worker),failed=await failSave(page,1,'QuotaExceededError');
    await page.evaluate(()=>{warningFail.preferences='QuotaExceededError';__nir.action({type:'volume',bus:'voice',delta:-.1});});
    await page.waitForFunction(()=>__nir.state().diagnostic?.location==='preferences');
    await save(page,1);await savedRevision(page,1,2);const partial=await unchanged(page,before);
    expect(partial.state.diagnostic.location).toBe('preferences');expect(partial.state.diagnostic.details.operation).toBe('persist');
    await page.evaluate(()=>__nir.action({type:'menu'}));await expect(page.locator('#nir-storage-retry')).toBeVisible();
    await page.locator('#nir-storage-retry').click();await page.waitForFunction(()=>__nir.state().diagnostic===null);
    expect(loops(await snapshot(page))).toEqual(loops(before));await record(page,info,'save-and-preference-recovery',{worker,before,failed,partial});
  });
}
