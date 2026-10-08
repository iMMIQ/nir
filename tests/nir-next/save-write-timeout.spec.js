import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import {recoverAudioOutput} from './audio-output-helper.js';
const baseline=process.env.NIR_SAVE_WRITE_BASELINE==='1';
const errors=new WeakMap();
test.beforeEach(async({page})=>{const list=[];errors.set(page,list);page.on('pageerror',e=>list.push(e.message));});
test.afterEach(async({page},info)=>{const final=await page.evaluate(()=>globalThis.saveWriteSnapshot?.()).catch(()=>null);await fs.writeFile(info.outputPath('terminal.json'),JSON.stringify({status:info.status,errors:errors.get(page),final},null,2)+'\n');});
async function boot(page,worker) {
 await page.addInitScript(()=>{
  const transaction=IDBDatabase.prototype.transaction,abort=IDBTransaction.prototype.abort,put=IDBObjectStore.prototype.put;
  const complete=Object.getOwnPropertyDescriptor(IDBTransaction.prototype,'oncomplete'),aborted=Object.getOwnPropertyDescriptor(IDBTransaction.prototype,'onabort'),rows=new WeakMap();
  window.saveWriteRows=[];window.saveWriteAudio=[];window.armSaveWrite=null;
  IDBDatabase.prototype.transaction=function(stores,mode,...args){
   const saves=(typeof stores==='string'?stores:stores[0])==='saves'&&mode==='readwrite';
   const armed=saves?armSaveWrite:null;if(saves)armSaveWrite=null;
   if(armed?.kind==='block'){
    const tx=transaction.call(this,'saves','readwrite'),store=tx.objectStore('saves'),readers=[];let keep=true;
    const done=new Promise(resolve=>{tx.oncomplete=tx.onabort=resolve;});
    window.saveWriteLock={ready:false,read:()=>new Promise(resolve=>readers.push(resolve)),release:async()=>{keep=false;await done;}};
    const next=()=>{const r=store.get(saveWriteKey);r.onsuccess=()=>{saveWriteLock.ready=true;for(const resolve of readers.splice(0))resolve(r.result);if(keep)next();};};next();
   }
   const tx=transaction.call(this,stores,mode,...args);if(!saves)return tx;
   const row={id:saveWriteRows.length+1,kind:armed?.kind||'normal',slot:armed?.slot??null,values:[],complete:false,aborted:false,abortCalls:0,abortThrows:0,delivered:false};saveWriteRows.push(row);rows.set(tx,row);
   tx.addEventListener('complete',()=>row.complete=true);tx.addEventListener('abort',()=>row.aborted=true);
   for(const [name,descriptor,hold] of [['oncomplete',complete,row.kind==='complete'],['onabort',aborted,row.kind==='abort']]){
    let handler=null;Object.defineProperty(tx,name,{configurable:true,get:()=>handler,set:value=>{
     handler=value;descriptor.set.call(tx,value?event=>{if(hold)row.deliver=()=>{row.delivered=true;row.deliver=null;value.call(tx,event);};else {row.delivered=true;value.call(tx,event);}}:null);
    }});
   }return tx;
  };
  IDBObjectStore.prototype.put=function(value,...args){const result=put.call(this,value,...args),row=rows.get(this.transaction),tx=this.transaction;
   if(row){row.slot=value.slot;row.values.push(structuredClone(value));result.addEventListener('success',()=>{if(row.kind==='complete')tx.commit();if(row.kind==='abort')abort.call(tx);});}return result;
  };
  IDBTransaction.prototype.abort=function(...args){const row=rows.get(this);if(row)row.abortCalls++;try{return abort.apply(this,args);}catch(e){if(row)row.abortThrows++;throw e;}};
  const create=AudioContext.prototype.createBufferSource;
  AudioContext.prototype.createBufferSource=function(...args){const source=create.apply(this,args),row={source,context:this,id:saveWriteAudio.length+1,stops:0};saveWriteAudio.push(row);const stop=source.stop;source.stop=function(...args){row.stops++;return stop.apply(this,args);};return source;};
  window.saveWriteSnapshot=()=>{const s=__nir.state();return {state:s,story:{session:s.session,interaction:s.interaction,position:s.position,tick:s.tick_us,history:s.history_count,variables:s.variables},loops:saveWriteAudio.filter(r=>r.source.loop).map(r=>({id:r.id,stops:r.stops,clock:r.context.currentTime})),writes:saveWriteRows.map(({deliver,...row})=>row),buttons:[...document.querySelectorAll('#actions button')].map(b=>({action:JSON.parse(b.dataset.action),disabled:b.disabled}))};};
  window.saveWriteConnect=async()=>{const ch=await(await fetch('/channels/stable.json')).json(),rel=await(await fetch(`/releases/${ch.release}.json`)).json();window.saveWriteHost=await import('/'+rel.objects[rel.engine.host].path);window.saveWriteDb=await saveWriteHost.openSaveDatabase();window.saveWriteKey=saveWriteHost.saveKey(rel.game_id,rel.profile,ch.release,1);window.saveWriteSlot=slot=>[...saveWriteKey.slice(0,3),slot];};
 });
 await page.goto(`http://127.0.0.1:4259/?test=1&worker=${worker}&backend=webgl2`);await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
 await page.keyboard.press('Enter');await recoverAudioOutput(page);await page.waitForFunction(()=>__nir.state().dialogue?.id==='intro'&&__nir.state().dialogue.ready&&!__nir.state().paused&&!__nir.state().loading);
 expect(await page.evaluate(()=>__nir.state().execution.runtime)).toBe(worker==='required'?'worker':'main');
 await page.evaluate(()=>__nir.action({type:'save',slot:1}));await page.waitForFunction(()=>__nir.state().status==='Saved');
 await page.evaluate(async()=>{await saveWriteConnect();window.saveWriteGood=await saveWriteHost.readSaveRecord(saveWriteDb,saveWriteKey);});
 await page.evaluate(()=>__nir.action({type:'saves'}));await page.waitForFunction(()=>[...document.querySelectorAll('#actions button')].some(b=>{const a=JSON.parse(b.dataset.action);return a.type==='save'&&a.slot===1&&!b.disabled;}));await page.evaluate(()=>saveWriteRows.length=0);
}
const snapshot=page=>page.evaluate(()=>saveWriteSnapshot());
const loops=s=>s.loops.map(({id,stops})=>({id,stops}));
async function unchanged(page,before){const after=await snapshot(page);expect(after.story).toEqual(before.story);expect(loops(after)).toEqual(loops(before));expect(after.loops[0].clock).toBeGreaterThan(before.loops[0].clock);expect(after.state.error).toBeNull();return after;}
async function arm(page,kind,slot=1){await page.evaluate(({kind,slot})=>{armSaveWrite={kind,slot};__nir.action({type:'save',slot});},{kind,slot});await page.waitForFunction(slot=>saveWriteRows.some(r=>r.slot===slot),slot);}
async function pending(page){await page.waitForFunction(()=>__nir.state().diagnostic?.code==='E_STORAGE_UNCERTAIN');expect(await page.evaluate(()=>__nir.state().status)).toContain('Still confirming');}
async function record(page,info,name,data){await fs.writeFile(info.outputPath(name+'.json'),JSON.stringify({...data,final:await snapshot(page),scope:'Real native IndexedDB writer blocked after preflight, or committed/aborted natively with controlled terminal callback delivery. No disk/fsync delay or physical audio/Android/latency claim.'},null,2)+'\n');}
for(const worker of ['main','required']) {
 if(baseline){test(`baseline native write lock after inspection has no commit deadline; ${worker}`,async({page},info)=>{
  await boot(page,worker);const before=await snapshot(page);await arm(page,'block');await page.waitForFunction(()=>saveWriteLock.ready);await page.waitForTimeout(6000);
  const held=await unchanged(page,before);expect(held.writes).toHaveLength(1);expect(held.writes[0].abortCalls).toBe(0);expect(held.writes[0].values).toEqual([]);expect(held.state.status).toBe('Saving…');
  const old=await page.evaluate(()=>saveWriteLock.read());expect(old).toEqual(await page.evaluate(()=>saveWriteGood));await page.evaluate(()=>saveWriteLock.release());await page.waitForFunction(()=>__nir.state().status==='Saved');
  expect(await page.evaluate(async()=>(await saveWriteHost.readSaveRecord(saveWriteDb,saveWriteKey)).envelope.revision)).toBe(2);await record(page,info,'baseline-save-write',{worker,before,held,old});
 });continue;}
 test(`native write lock after inspection aborts without put and explicit save recovers; ${worker}`,async({page},info)=>{
  await boot(page,worker);const before=await snapshot(page);await arm(page,'block');await page.waitForFunction(()=>__nir.state().diagnostic?.message.includes('E_STORAGE_TIMEOUT: save write'));
  const failed=await unchanged(page,before);expect(failed.writes[0].aborted).toBe(true);expect(failed.writes[0].abortCalls).toBe(1);expect(failed.writes[0].values).toEqual([]);
  const old=await page.evaluate(()=>saveWriteLock.read());expect(old).toEqual(await page.evaluate(()=>saveWriteGood));await page.evaluate(()=>saveWriteLock.release());await page.evaluate(()=>__nir.action({type:'save',slot:1}));await page.waitForFunction(()=>__nir.state().status==='Saved');
  expect(await page.evaluate(async()=>(await saveWriteHost.readSaveRecord(saveWriteDb,saveWriteKey)).envelope.revision)).toBe(2);await unchanged(page,before);await record(page,info,'blocked-save-write',{worker,before,failed,old});
 });
 test(`uncertain native commit keeps slot busy, rejects repeats and accepts original confirmation; ${worker}`,async({page},info)=>{
  await boot(page,worker);const before=await snapshot(page);await arm(page,'complete');await pending(page);
  const held=await unchanged(page,before);expect(held.writes[0].complete).toBe(true);expect(held.writes[0].abortThrows).toBe(1);expect(held.writes[0].delivered).toBe(false);
  expect(held.buttons.find(b=>b.action.type==='save'&&b.action.slot===1).disabled).toBe(true);
  await page.evaluate(()=>{for(let i=0;i<20;i++)__nir.action({type:'save',slot:1});});await page.waitForTimeout(100);expect((await snapshot(page)).writes).toHaveLength(1);
  expect(await page.evaluate(async()=>(await saveWriteHost.readSaveRecord(saveWriteDb,saveWriteKey)).envelope.revision)).toBe(2);
  await page.evaluate(()=>saveWriteRows[0].deliver());await page.waitForFunction(()=>__nir.state().status==='Saved'&&__nir.state().diagnostic===null);await unchanged(page,before);
  await page.waitForFunction(()=>[...document.querySelectorAll('#actions button')].some(b=>{const a=JSON.parse(b.dataset.action);return a.type==='save'&&a.slot===1&&!b.disabled;}));
  await record(page,info,'uncertain-save-commit',{worker,before,held});
 });
 test(`uncertain native abort retains old save, then explicit retry uses its original revision; ${worker}`,async({page},info)=>{
  await boot(page,worker);const before=await snapshot(page);await arm(page,'abort');await pending(page);const held=await unchanged(page,before);
  expect(held.writes[0].aborted).toBe(true);expect(held.writes[0].abortThrows).toBe(1);expect(held.writes[0].delivered).toBe(false);
  const old=await page.evaluate(()=>saveWriteHost.readSaveRecord(saveWriteDb,saveWriteKey));expect(old).toEqual(await page.evaluate(()=>saveWriteGood));
  await page.evaluate(()=>saveWriteRows[0].deliver());await page.waitForFunction(()=>__nir.state().status.includes('Storage operation failed'));
  await page.evaluate(()=>__nir.action({type:'save',slot:1}));await page.waitForFunction(()=>__nir.state().status==='Saved');
  expect(await page.evaluate(async()=>(await saveWriteHost.readSaveRecord(saveWriteDb,saveWriteKey)).envelope.revision)).toBe(2);expect((await snapshot(page)).writes).toHaveLength(2);await unchanged(page,before);
  await record(page,info,'uncertain-save-abort',{worker,before,held,old});
 });
 test(`multiple unknown slots stay visible through other completion and settle independently; ${worker}`,async({page},info)=>{
  await boot(page,worker);const before=await snapshot(page);await arm(page,'complete');await pending(page);const first=await snapshot(page);
  await arm(page,'complete',0);await page.waitForFunction(()=>saveWriteRows.length===2&&saveWriteRows.every(r=>r.abortThrows===1));
  const second=await snapshot(page);const remainingJob=second.state.diagnostic.details.request;
  await page.evaluate(()=>__nir.action({type:'save',slot:2}));await page.waitForFunction(()=>saveWriteRows.some(r=>r.slot===2&&r.complete));
  await pending(page);await page.evaluate(()=>saveWriteRows.find(r=>r.slot===1).deliver());await page.waitForFunction(job=>__nir.state().diagnostic?.details.request===job,remainingJob);
  const partial=await unchanged(page,before);expect(partial.state.status).toContain('Still confirming');expect(partial.buttons.find(b=>b.action.type==='save'&&b.action.slot===0).disabled).toBe(true);
  await page.evaluate(()=>saveWriteRows.find(r=>r.slot===0).deliver());await page.waitForFunction(()=>__nir.state().status==='Saved'&&__nir.state().diagnostic===null);await unchanged(page,before);
  expect((await snapshot(page)).writes).toHaveLength(3);await record(page,info,'multiple-save-confirmations',{worker,before,first,second,partial});
 });
 test(`reading continues while save is unconfirmed and its late acknowledgement cannot restore old scene; ${worker}`,async({page},info)=>{
  await boot(page,worker);await arm(page,'complete');await pending(page);
  await page.evaluate(()=>__nir.action({type:'close'}));await page.waitForFunction(()=>__nir.state().screen==='Story'&&!__nir.state().loading);
  await page.keyboard.press('Enter');await page.waitForFunction(()=>__nir.state().dialogue?.id==='arrival'&&!__nir.state().loading);
  const continued=await snapshot(page);expect(continued.writes).toHaveLength(1);expect(continued.writes[0].delivered).toBe(false);
  await page.evaluate(()=>saveWriteRows[0].deliver());await page.waitForFunction(()=>__nir.state().status==='Saved'&&__nir.state().diagnostic===null);
  const after=await snapshot(page),plot=({tick,...rest})=>rest;
  expect(after.state.screen).toBe('Story');expect(after.state.dialogue.id).toBe('arrival');expect(plot(after.story)).toEqual(plot(continued.story));expect(loops(after)).toEqual(loops(continued));
  await record(page,info,'reading-during-save-confirmation',{worker,continued,after});
 });
 test(`save confirmation retains origin across title navigation and does not reopen story; ${worker}`,async({page},info)=>{
  await boot(page,worker);await arm(page,'complete');await page.evaluate(()=>__nir.action({type:'title'}));await page.waitForFunction(()=>__nir.state().screen==='Title'&&!__nir.state().loading);
  await pending(page);const held=await snapshot(page);expect(held.state.diagnostic.details.session).toBeLessThan(held.state.session);
  await page.evaluate(()=>saveWriteRows[0].deliver());await page.waitForFunction(()=>__nir.state().status==='Saved'&&__nir.state().diagnostic===null);
  const after=await snapshot(page);expect(after.state.screen).toBe('Title');expect(after.story).toEqual(held.story);expect(loops(after)).toEqual(loops(held));expect(after.writes).toHaveLength(1);
  await record(page,info,'save-confirmation-title',{worker,held,after});
 });
}
