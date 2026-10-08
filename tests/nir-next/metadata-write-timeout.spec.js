import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import {recoverAudioOutput} from './audio-output-helper.js';

const baseline=process.env.NIR_WRITE_BASELINE==='1';
const pageErrors=new WeakMap();
test.beforeEach(async({page})=>{const errors=[];pageErrors.set(page,errors);page.on('pageerror',e=>errors.push(e.message));});
test.afterEach(async({page},info)=>{
  const final=await page.evaluate(()=>globalThis.__nir&&globalThis.storageSnapshot?storageSnapshot():null).catch(()=>null);
  await fs.writeFile(info.outputPath('terminal.json'),JSON.stringify({status:info.status,errors:pageErrors.get(page),final},null,2)+'\n');
});
async function boot(page,worker) {
  await page.addInitScript(()=>{
    const transaction=IDBDatabase.prototype.transaction,put=IDBObjectStore.prototype.put,abort=IDBTransaction.prototype.abort;
    const complete=Object.getOwnPropertyDescriptor(IDBTransaction.prototype,'oncomplete'),rows=new WeakMap();
    window.writeRows=[];window.holdWrite={};window.abortWrite={};window.storageAudio=[];
    IDBDatabase.prototype.transaction=function(stores,mode,...args){
      const tx=transaction.call(this,stores,mode,...args),kind=typeof stores==='string'?stores:stores[0];
      if(mode!=='readwrite'||!['preferences','profile'].includes(kind))return tx;
      const row={id:writeRows.length+1,kind,hold:!!holdWrite[kind],values:[],nativeCompleted:false,aborted:false,abortCalls:0,abortThrows:0,delivered:false};
      holdWrite[kind]=false;writeRows.push(row);rows.set(tx,row);
      tx.addEventListener('complete',()=>{row.nativeCompleted=true;});tx.addEventListener('abort',()=>{row.aborted=true;});
      let handler=null;
      Object.defineProperty(tx,'oncomplete',{configurable:true,get:()=>handler,set:value=>{
        handler=value;complete.set.call(tx,value?event=>{
          if(row.hold)row.deliver=()=>{row.delivered=true;row.deliver=null;value.call(tx,event);};
          else {row.delivered=true;value.call(tx,event);}
        }:null);
      }});
      return tx;
    };
    IDBObjectStore.prototype.put=function(value,...args){
      const result=put.call(this,value,...args),row=rows.get(this.transaction),tx=this.transaction;
      if(row){row.values.push(structuredClone(value));result.addEventListener('success',()=>{if(row.hold)tx.commit();});
        if(abortWrite[row.kind]){abortWrite[row.kind]--;queueMicrotask(()=>tx.abort());}}
      return result;
    };
    IDBTransaction.prototype.abort=function(...args){
      const row=rows.get(this);if(row)row.abortCalls++;
      try{return abort.apply(this,args);}catch(e){if(row)row.abortThrows++;throw e;}
    };
    const create=AudioContext.prototype.createBufferSource;
    AudioContext.prototype.createBufferSource=function(...args){
      const source=create.apply(this,args),context=this,row={source,context,id:storageAudio.length+1,stops:0};storageAudio.push(row);
      const stop=source.stop;source.stop=function(...args){row.stops++;return stop.apply(this,args);};return source;
    };
    window.storageSnapshot=()=>{const s=__nir.state();return {state:s,story:{session:s.session,interaction:s.interaction,position:s.position,tick:s.tick_us,history:s.history_count,variables:s.variables},
      loops:storageAudio.filter(r=>r.source.loop).map(r=>({id:r.id,stops:r.stops,clock:r.context.currentTime,state:r.context.state})),
      writes:writeRows.map(({deliver,...r})=>r)};};
    window.storageConnect=async()=>{
      const ch=await(await fetch('/channels/stable.json')).json(),release=await(await fetch(`/releases/${ch.release}.json`)).json();
      const host=await import('/'+release.objects[release.engine.host].path);window.storageDb=await host.openSaveDatabase();window.storageKey=host.profileKey(release.game_id,release.profile);
    };
    window.readStored=kind=>new Promise((ok,no)=>{
      const tx=transaction.call(storageDb,kind,'readonly'),r=tx.objectStore(kind).get(storageKey);tx.oncomplete=()=>ok(r.result??null);tx.onabort=()=>no(tx.error);
    });
    window.lockMetadata=kind=>new Promise(ok=>{
      const tx=transaction.call(storageDb,kind,'readwrite'),store=tx.objectStore(kind),readers=[];let keep=true;
      const done=new Promise(resolve=>{tx.oncomplete=tx.onabort=resolve;});
      window.metadataLock={read:()=>new Promise(resolve=>readers.push(resolve)),release:async()=>{keep=false;await done;}};
      const next=()=>{const r=store.get(storageKey);r.onsuccess=()=>{ok();for(const resolve of readers.splice(0))resolve(r.result??null);if(keep)next();};};next();
    });
  });
  await page.goto(`http://127.0.0.1:4259/?test=1&worker=${worker}&backend=webgl2`);await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
  await page.evaluate(async()=>{
    await storageConnect();for(const [kind,value] of [['preferences',{font_scale:1.4,bgm_volume:.3}],['profile',['seed.prior']]])await new Promise(ok=>{
      const tx=storageDb.transaction(kind,'readwrite');tx.objectStore(kind).put(value,storageKey);tx.oncomplete=ok;
    });storageDb.close();
  });
  await page.reload();await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);await page.evaluate(()=>storageConnect());
  await page.keyboard.press('Enter');await recoverAudioOutput(page);await page.waitForFunction(()=>__nir.state().dialogue?.id==='intro'&&__nir.state().dialogue.ready&&!__nir.state().paused&&!__nir.state().loading);
  expect(await page.evaluate(()=>__nir.state().execution.runtime)).toBe(worker==='required'?'worker':'main');
}
const snapshot=page=>page.evaluate(()=>storageSnapshot());
const loops=s=>s.loops.map(({id,stops})=>({id,stops}));
async function menu(page){await page.evaluate(()=>__nir.action({type:'menu'}));await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);}
async function settings(page){await page.evaluate(()=>__nir.action({type:'settings'}));await page.waitForFunction(()=>__nir.state().screen==='Settings'&&!__nir.state().loading);}
async function record(page,info,name,data){await fs.writeFile(info.outputPath(name+'.json'),JSON.stringify({...data,final:await snapshot(page),scope:'Real Chromium IndexedDB readwrite locks and explicit native commit, with controlled late completion delivery. Audio source/device-clock API evidence; no disk fsync, native GUI, physical audio, Android or latency budget.'},null,2)+'\n');}

for(const worker of ['main','required']) {
  if(baseline) {
    test(`baseline real write lock has no metadata deadline; ${worker}`,async({page},info)=>{
      await boot(page,worker);await settings(page);await page.evaluate(()=>lockMetadata('preferences'));const before=await snapshot(page);
      await page.evaluate(()=>__nir.action({type:'volume',bus:'bgm',delta:-.1}));await page.waitForFunction(()=>writeRows.some(r=>r.kind==='preferences'));await page.waitForTimeout(6000);
      const held=await snapshot(page),old=await page.evaluate(()=>metadataLock.read());expect(held.writes.filter(r=>r.kind==='preferences')).toHaveLength(1);
      expect(held.writes.find(r=>r.kind==='preferences').abortCalls).toBe(0);expect(held.state.diagnostic?.message||'').not.toContain('E_STORAGE_TIMEOUT');expect(old.bgm_volume).toBe(.3);
      expect(held.story).toEqual(before.story);expect(loops(held)).toEqual(loops(before));await page.evaluate(()=>metadataLock.release());
      await expect.poll(()=>page.evaluate(async()=>(await readStored('preferences')).bgm_volume)).toBeCloseTo(.2,5);await record(page,info,'baseline-lock',{worker,before,held,old});
    });
    continue;
  }
  for(const kind of ['preferences','profile'])test(`real ${kind} write lock aborts without overwriting; explicit retry recovers; ${worker}`,async({page},info)=>{
    const errors=[];page.on('pageerror',e=>errors.push(e.message));await boot(page,worker);
    if(kind==='preferences')await settings(page);await page.evaluate(kind=>lockMetadata(kind),kind);
    if(kind==='preferences')await page.evaluate(()=>__nir.action({type:'volume',bus:'bgm',delta:-.1}));
    else {await page.keyboard.press('Enter');await page.waitForFunction(()=>__nir.state().dialogue?.id==='arrival'&&!__nir.state().loading);await menu(page);}
    const before=await snapshot(page);await page.waitForFunction(kind=>__nir.state().diagnostic?.location===kind&&__nir.state().diagnostic.message.includes('E_STORAGE_TIMEOUT'),kind);
    await page.waitForFunction(kind=>writeRows.some(r=>r.kind===kind&&r.aborted),kind);const failed=await snapshot(page),old=await page.evaluate(()=>metadataLock.read());
    expect(failed.story).toEqual(before.story);expect(loops(failed)).toEqual(loops(before));expect(failed.loops[0].clock).toBeGreaterThan(before.loops[0].clock);expect(failed.state.error).toBeNull();
    expect(failed.writes.filter(r=>r.kind===kind)).toHaveLength(1);expect(failed.writes.find(r=>r.kind===kind).values).toEqual([]);
    if(kind==='preferences')expect(old.bgm_volume).toBe(.3);else expect(old).toEqual(['seed.prior']);
    await page.evaluate(()=>metadataLock.release());await page.waitForTimeout(250);expect((await snapshot(page)).writes.filter(r=>r.kind===kind)).toHaveLength(1);
    await page.locator('#nir-storage-retry').click();
    if(kind==='preferences')await expect.poll(()=>page.evaluate(async()=>(await readStored('preferences')).bgm_volume)).toBeCloseTo(.2,5);
    else await expect.poll(()=>page.evaluate(()=>readStored('profile'))).toEqual(['read:intro:1','seed.prior']);
    await page.waitForFunction(()=>document.querySelector('#nir-storage-recovery').hidden);const recovered=await snapshot(page);expect(recovered.story).toEqual(before.story);expect(loops(recovered)).toEqual(loops(before));expect(errors).toEqual([]);
    await record(page,info,'locked-'+kind,{worker,before,failed,old,recovered});
  });
  test(`native committed preferences await confirmation; latest edits are not replayed early; ${worker}`,async({page},info)=>{
    const errors=[];page.on('pageerror',e=>errors.push(e.message));await boot(page,worker);await settings(page);const before=await snapshot(page);
    await page.evaluate(()=>{holdWrite.preferences=true;__nir.action({type:'volume',bus:'bgm',delta:-.1});});
    await page.waitForFunction(()=>writeRows.some(r=>r.kind==='preferences'&&r.nativeCompleted));
    const committed=await page.evaluate(()=>readStored('preferences'));expect(committed.bgm_volume).toBeCloseTo(.2,5);
    await page.waitForFunction(()=>__nir.state().diagnostic?.message.includes('E_STORAGE_UNCERTAIN'));const pending=await snapshot(page);
    expect(pending.writes.filter(r=>r.kind==='preferences')).toHaveLength(1);expect(pending.writes.find(r=>r.kind==='preferences').abortThrows).toBe(1);
    await expect(page.locator('#nir-storage-retry')).toBeDisabled();await page.locator('#nir-storage-retry').evaluate(b=>b.click());
    await page.evaluate(()=>{for(let i=0;i<20;i++)__nir.action({type:'volume',bus:'bgm',delta:.001});});await page.waitForFunction(()=>__nir.state().preferences.bgm_volume>.215);
    const edited=await snapshot(page);expect(edited.writes.filter(r=>r.kind==='preferences')).toHaveLength(1);expect(edited.story).toEqual(before.story);expect(loops(edited)).toEqual(loops(before));
    await page.evaluate(()=>writeRows.find(r=>r.kind==='preferences'&&r.deliver).deliver());
    await expect.poll(()=>page.evaluate(async()=>Math.abs((await readStored('preferences')).bgm_volume-__nir.state().preferences.bgm_volume)<1e-5)).toBe(true);
    await page.waitForFunction(()=>document.querySelector('#nir-storage-recovery').hidden);const recovered=await snapshot(page);
    expect(recovered.writes.filter(r=>r.kind==='preferences')).toHaveLength(2);expect(recovered.story).toEqual(before.story);expect(loops(recovered)).toEqual(loops(before));expect(errors).toEqual([]);
    await record(page,info,'uncertain-preferences',{worker,before,committed,pending,edited,recovered});
  });
  test(`pending progress does not block another kind's explicit recovery; ${worker}`,async({page},info)=>{
    const errors=[];page.on('pageerror',e=>errors.push(e.message));await boot(page,worker);await page.evaluate(()=>{holdWrite.profile=true;});await page.keyboard.press('Enter');
    await page.waitForFunction(()=>__nir.state().dialogue?.id==='arrival'&&!__nir.state().loading);await menu(page);const before=await snapshot(page);
    await page.waitForFunction(()=>__nir.state().diagnostic?.message.includes('E_STORAGE_UNCERTAIN'));await expect(page.locator('#nir-storage-retry')).toBeDisabled();
    await page.evaluate(()=>{abortWrite.preferences=1;__nir.action({type:'volume',bus:'bgm',delta:-.1});});
    await page.waitForFunction(()=>writeRows.some(r=>r.kind==='preferences'&&r.aborted));await expect(page.locator('#nir-storage-retry')).toBeEnabled();const partial=await snapshot(page);
    await page.locator('#nir-storage-retry').click();await expect.poll(()=>page.evaluate(async()=>(await readStored('preferences')).bgm_volume)).toBeCloseTo(.2,5);
    await expect(page.locator('#nir-storage-retry')).toBeDisabled();const healthy=await snapshot(page);
    expect(healthy.writes.filter(r=>r.kind==='profile')).toHaveLength(1);expect(healthy.state.diagnostic.location).toBe('profile');expect(healthy.story).toEqual(before.story);expect(loops(healthy)).toEqual(loops(before));
    await page.evaluate(()=>writeRows.find(r=>r.kind==='profile'&&r.deliver).deliver());await page.waitForFunction(()=>document.querySelector('#nir-storage-recovery').hidden);
    expect(await page.evaluate(()=>readStored('profile'))).toEqual(['read:intro:1','seed.prior']);expect(errors).toEqual([]);
    await record(page,info,'uncertain-profile-partial-recovery',{worker,before,partial,healthy});
  });
}
