import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import {recoverAudioOutput} from './audio-output-helper.js';
const baseline=process.env.NIR_SAVE_READ_BASELINE==='1';
const errors=new WeakMap();
test.beforeEach(async({page})=>{const list=[];errors.set(page,list);page.on('pageerror',e=>list.push(e.message));});
test.afterEach(async({page},info)=>{
 const final=await page.evaluate(()=>globalThis.saveReadSnapshot?.()).catch(()=>null);
 await fs.writeFile(info.outputPath('terminal.json'),JSON.stringify({status:info.status,errors:errors.get(page),final},null,2)+'\n');
});
async function boot(page,worker) {
 await page.addInitScript(()=>{
  const transaction=IDBDatabase.prototype.transaction,abort=IDBTransaction.prototype.abort,rows=new WeakMap();
  window.saveReadRows=[];window.saveReadAudio=[];
  IDBDatabase.prototype.transaction=function(stores,mode,...args){
   const tx=transaction.call(this,stores,mode,...args);
   if((typeof stores==='string'?stores:stores[0])==='saves'&&mode==='readonly'){
    const row={id:saveReadRows.length+1,complete:false,aborted:false,abortCalls:0};saveReadRows.push(row);rows.set(tx,row);
    tx.addEventListener('complete',()=>row.complete=true);tx.addEventListener('abort',()=>row.aborted=true);
   }return tx;
  };
  IDBTransaction.prototype.abort=function(...args){const row=rows.get(this);if(row)row.abortCalls++;return abort.apply(this,args);};
  const create=AudioContext.prototype.createBufferSource;
  AudioContext.prototype.createBufferSource=function(...args){const source=create.apply(this,args),row={source,context:this,id:saveReadAudio.length+1,stops:0};saveReadAudio.push(row);const stop=source.stop;source.stop=function(...args){row.stops++;return stop.apply(this,args);};return source;};
  window.saveReadSnapshot=()=>{const s=__nir.state();return {state:s,story:{session:s.session,interaction:s.interaction,position:s.position,tick:s.tick_us,history:s.history_count,variables:s.variables},loops:saveReadAudio.filter(r=>r.source.loop).map(r=>({id:r.id,stops:r.stops,clock:r.context.currentTime})),reads:structuredClone(saveReadRows)};};
  window.saveReadConnect=async()=>{
   const ch=await(await fetch('/channels/stable.json')).json(),release=await(await fetch(`/releases/${ch.release}.json`)).json();
   window.saveReadHost=await import('/'+release.objects[release.engine.host].path);window.saveReadDb=await saveReadHost.openSaveDatabase();
   window.saveReadKey=saveReadHost.saveKey(release.game_id,release.profile,ch.release,1);window.saveReadIdentity={game:release.game_id,profile:release.profile,release:ch.release};
  };
  window.lockSaves=()=>new Promise(ok=>{
   const tx=transaction.call(saveReadDb,'saves','readwrite'),store=tx.objectStore('saves'),readers=[];let keep=true;
   const done=new Promise(resolve=>{tx.oncomplete=tx.onabort=resolve;});
   window.saveReadLock={read:()=>new Promise(resolve=>readers.push(resolve)),release:async()=>{keep=false;await done;}};
   const next=()=>{const r=store.get(saveReadKey);r.onsuccess=()=>{ok();for(const resolve of readers.splice(0))resolve(r.result);if(keep)next();};};next();
  });
 });
 await page.goto(`http://127.0.0.1:4259/?test=1&worker=${worker}&backend=webgl2`);
 await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
 await page.keyboard.press('Enter');await recoverAudioOutput(page);
 await page.waitForFunction(()=>__nir.state().dialogue?.id==='intro'&&__nir.state().dialogue.ready&&!__nir.state().paused&&!__nir.state().loading);
 expect(await page.evaluate(()=>__nir.state().execution.runtime)).toBe(worker==='required'?'worker':'main');
 await page.evaluate(()=>__nir.action({type:'save',slot:1}));await page.waitForFunction(()=>__nir.state().status==='Saved');
 await page.evaluate(async()=>{await saveReadConnect();window.saveReadGood=await saveReadHost.readSaveRecord(saveReadDb,saveReadKey);});
 await page.evaluate(()=>__nir.action({type:'close'}));await page.waitForFunction(()=>__nir.state().screen==='Story');
 await page.evaluate(()=>__nir.action({type:'menu'}));await page.waitForFunction(()=>__nir.state().screen==='Menu');
}
const snapshot=page=>page.evaluate(()=>saveReadSnapshot());
const loops=s=>s.loops.map(({id,stops})=>({id,stops}));
async function unchanged(page,before){const after=await snapshot(page);expect(after.story).toEqual(before.story);expect(loops(after)).toEqual(loops(before));expect(after.loops[0].clock).toBeGreaterThan(before.loops[0].clock);expect(after.state.error).toBeNull();return after;}
async function lock(page){await page.evaluate(async()=>{await lockSaves();saveReadRows.length=0;});}
async function record(page,info,name,data){await fs.writeFile(info.outputPath(name+'.json'),JSON.stringify({...data,final:await snapshot(page),scope:'Real desktop Chromium readonly transactions blocked by native readwrite get-only store lock, exact SDK and MP3 fixture. Native source/clock evidence, no physical sound/disk/Android/latency claim.'},null,2)+'\n');}
for(const worker of ['main','required']) {
 if(baseline){
  test(`baseline actual save store lock has no read deadline; ${worker}`,async({page},info)=>{
   await boot(page,worker);await lock(page);const before=await snapshot(page);
   await page.evaluate(()=>{window.saveReadProbe={done:false};void saveReadHost.readSaveRecord(saveReadDb,saveReadKey).then(value=>saveReadProbe={done:true,value},error=>saveReadProbe={done:true,error:String(error)});});
   await page.waitForFunction(()=>saveReadRows.length===1);await page.waitForTimeout(6000);
   const held=await unchanged(page,before);expect(await page.evaluate(()=>saveReadProbe.done)).toBe(false);expect(held.reads[0].abortCalls).toBe(0);
   const old=await page.evaluate(()=>saveReadLock.read());expect(old).toEqual(await page.evaluate(()=>saveReadGood));
   await page.evaluate(()=>saveReadLock.release());await page.waitForFunction(()=>saveReadProbe.done);
   expect(await page.evaluate(()=>saveReadProbe.value)).toEqual(old);await record(page,info,'baseline-save-read',{worker,before,held,old});
  });continue;
 }
 test(`blocked load fails without replacing current reading, then explicit load recovers; ${worker}`,async({page},info)=>{
  await boot(page,worker);await page.evaluate(()=>__nir.action({type:'saves'}));await page.waitForFunction(()=>[...document.querySelectorAll('#actions button')].some(n=>{const a=JSON.parse(n.dataset.action);return a.type==='save'&&a.slot===1&&!n.disabled;}));
  await lock(page);const before=await snapshot(page);await page.evaluate(()=>__nir.action({type:'load',slot:1}));
  await page.waitForFunction(()=>__nir.state().diagnostic?.message.includes('E_STORAGE_TIMEOUT'));const failed=await unchanged(page,before);
  expect(failed.state.status).toContain('Storage operation failed');
  expect(failed.state.screen).toBe('Saves');expect(failed.reads[0].aborted).toBe(true);expect(failed.reads[0].abortCalls).toBe(1);
  const old=await page.evaluate(()=>saveReadLock.read());expect(old).toEqual(await page.evaluate(()=>saveReadGood));
  await page.evaluate(()=>saveReadLock.release());await page.evaluate(()=>__nir.action({type:'load',slot:1}));
  await page.waitForFunction(s=>__nir.state().session>s&&!__nir.state().loading&&__nir.state().screen==='Story',before.story.session);
  expect(await page.evaluate(()=>__nir.state().paused)).toBe(true);await record(page,info,'blocked-load',{worker,before,failed,old});
 });
 test(`blocked slot list terminates all three checks and can be reopened; ${worker}`,async({page},info)=>{
  await boot(page,worker);await lock(page);const before=await snapshot(page);await page.evaluate(()=>__nir.action({type:'saves'}));
  await page.waitForFunction(()=>saveReadRows.length===3&&saveReadRows.every(r=>r.aborted),null,{timeout:20000});
  await page.waitForFunction(()=>{const message=__nir.state().diagnostic?.message||'';return [0,1,2].every(slot=>message.includes(`slot ${slot}:`));});
  const failed=await unchanged(page,before);expect(failed.reads.every(r=>r.abortCalls===1)).toBe(true);
  const old=await page.evaluate(()=>saveReadLock.read());expect(old).toEqual(await page.evaluate(()=>saveReadGood));
  await page.evaluate(()=>saveReadLock.release());await page.evaluate(()=>__nir.action({type:'close'}));await page.waitForFunction(()=>__nir.state().screen==='Story');
  await page.evaluate(()=>__nir.action({type:'saves'}));await page.waitForFunction(()=>[...document.querySelectorAll('#actions button')].some(n=>{const a=JSON.parse(n.dataset.action);return a.type==='save'&&a.slot===1&&!n.disabled;}));
  expect(await page.evaluate(()=>[...document.querySelectorAll('#actions button')].filter(n=>JSON.parse(n.dataset.action).type==='save').every(n=>!n.disabled))).toBe(true);await record(page,info,'blocked-slot-list',{worker,before,failed,old});
 });
 test(`blocked archive cursor reports failure and trusted refresh recovers; ${worker}`,async({page},info)=>{
  await boot(page,worker);await lock(page);const before=await snapshot(page);await page.locator('#nir-history-button').click();
  await expect(page.locator('#nir-history-panel [role=status]')).toContainText('E_STORAGE_TIMEOUT',{timeout:10000});
  await expect(page.locator('#nir-history-refresh')).toBeEnabled();const failed=await unchanged(page,before);expect(failed.reads[0].aborted).toBe(true);
  const old=await page.evaluate(()=>saveReadLock.read());expect(old).toEqual(await page.evaluate(()=>saveReadGood));await page.evaluate(()=>saveReadLock.release());
  await page.locator('#nir-history-refresh').click();await expect(page.locator('#nir-history-panel .nir-history-status')).toHaveText('Available');
  await unchanged(page,before);await record(page,info,'blocked-archive',{worker,before,failed,old});
 });
 test(`blocked archive export reports failure and retries current stored record; ${worker}`,async({page},info)=>{
  await boot(page,worker);await page.locator('#nir-history-button').click();await expect(page.locator('#nir-history-panel .nir-history-status')).toHaveText('Available');
  await lock(page);const before=await snapshot(page);let downloads=0;page.on('download',()=>downloads++);
  const button=page.locator('#nir-history-panel').getByRole('button',{name:'Export',exact:true});await button.click();
  await expect(page.locator('#nir-history-panel [role=status]')).toContainText('Check timed out; refresh to retry.',{timeout:10000});
  const failed=await unchanged(page,before);expect(failed.reads[0].aborted).toBe(true);expect(downloads).toBe(0);
  await page.evaluate(()=>saveReadLock.release());const pending=page.waitForEvent('download');await button.click();const download=await pending;
  const stream=await download.createReadStream(),chunks=[];for await(const chunk of stream)chunks.push(chunk);const envelope=JSON.parse(Buffer.concat(chunks).toString());
  expect(envelope.revision).toBe(1);expect(envelope.snapshot.scene.some(n=>Object.is(n.x,-0))).toBe(true);
  expect(await page.evaluate(()=>saveReadHost.readSaveRecord(saveReadDb,saveReadKey))).toEqual(await page.evaluate(()=>saveReadGood));
  await unchanged(page,before);await record(page,info,'blocked-export',{worker,before,failed,exportedRevision:envelope.revision});
 });
 test(`closing archive aborts pending cursor and stale failure cannot affect reopened panel; ${worker}`,async({page},info)=>{
  await boot(page,worker);await lock(page);const before=await snapshot(page);await page.locator('#nir-history-button').click();await page.waitForFunction(()=>saveReadRows.length===1);
  await page.locator('#nir-history-panel').getByRole('button',{name:'关闭 / Close',exact:true}).click();await page.waitForFunction(()=>saveReadRows[0].aborted);
  const cancelled=await unchanged(page,before);expect(cancelled.reads[0].abortCalls).toBe(1);
  await page.evaluate(()=>saveReadLock.release());await page.locator('#nir-history-button').click();await expect(page.locator('#nir-history-panel .nir-history-status')).toHaveText('Available');
  await expect(page.locator('#nir-history-panel [role=status]')).toHaveText('1 saved slot');await unchanged(page,before);
  await record(page,info,'closed-archive',{worker,before,cancelled});
 });
}
