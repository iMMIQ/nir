import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import {recoverAudioOutput} from './audio-output-helper.js';

async function records(page,key) {
  return page.evaluate(async key=>{
    const r=indexedDB.open('nir-player-isolated-v1',1),db=await new Promise((ok,no)=>{r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});
    try{
      const read=kind=>new Promise((ok,no)=>{const tx=db.transaction(kind),r=tx.objectStore(kind).get(key);tx.oncomplete=()=>ok(r.result);tx.onabort=()=>no(tx.error);});
      return {preferences:await read('preferences'),profile:await read('profile')};
    }finally{db.close();}
  },key);
}

for(const worker of ['main','required'])for(const kind of ['preferences','profile','both']) {
  test(`native metadata lock ${kind} has a read deadline and preserves originals; ${worker}`,async({page},info)=>{
    const errors=[];page.on('pageerror',e=>errors.push(e.message));
    await page.goto(`http://127.0.0.1:4259/?test=1&worker=${worker}&backend=webgl2`);
    await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    const original=await page.evaluate(async()=>{
      const channel=await(await fetch('/channels/stable.json')).json(),release=await(await fetch(`/releases/${channel.release}.json`)).json();
      const r=indexedDB.open('nir-player-isolated-v1',1),db=await new Promise((ok,no)=>{r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});
      const key=[release.game_id,release.profile],preferences={...__nir.state().preferences,font_scale:1.4,bgm_volume:.2},profile=['fixture.original'];
      try{await new Promise((ok,no)=>{const tx=db.transaction(['preferences','profile'],'readwrite');tx.objectStore('preferences').put(preferences,key);tx.objectStore('profile').put(profile,key);tx.oncomplete=ok;tx.onabort=()=>no(tx.error);});}finally{db.close();}
      return {key,preferences,profile};
    });
    await page.addInitScript(kind=>{
      const native=IDBDatabase.prototype.transaction;
      globalThis.metadataReadLock={started:false,released:false,completed:false,requests:0,reads:0,aborted:0};
      IDBDatabase.prototype.transaction=function(stores,mode,...args){
        const names=typeof stores==='string'?[stores]:[...stores],state=metadataReadLock;
        if(this.name==='nir-player-isolated-v1'&&(mode===undefined||mode==='readonly')&&names.some(n=>n==='preferences'||n==='profile')){
          if(!state.started){
            state.started=true;
            // A real write transaction holds the selected object-store locks.
            // It issues reads only, so it cannot change the seeded originals.
            const held=kind==='both'?['preferences','profile']:[kind],blocker=native.call(this,held,'readwrite');
            blocker.oncomplete=()=>{state.completed=true;};
            const store=blocker.objectStore(held[0]);
            const keepAlive=()=>{state.requests++;const r=store.get('__nir_read_lock_fixture');r.onsuccess=()=>{if(!state.released)keepAlive();};};
            keepAlive();
          }
          state.reads++;
          const tx=native.call(this,stores,mode,...args);tx.addEventListener('abort',()=>{state.aborted++;});return tx;
        }
        return native.call(this,stores,mode,...args);
      };
    },kind);
    const started=Date.now();await page.reload();await page.waitForFunction(()=>metadataReadLock.started&&metadataReadLock.requests>0);
    if(process.env.NIR_READ_TIMEOUT_BASELINE==='1'){
      await page.waitForTimeout(6000);
      const stalled=await page.evaluate(()=>({state:globalThis.__nir?.state()||null,lock:{...metadataReadLock},body:document.body.innerText}));
      expect(stalled.state).toBeNull();expect(stalled.lock.released).toBe(false);expect(stalled.lock.completed).toBe(false);
      await page.evaluate(()=>{metadataReadLock.released=true;});await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
      expect(await records(page,original.key)).toEqual({preferences:original.preferences,profile:original.profile});expect(errors).toEqual([]);
      await fs.writeFile(info.outputPath('baseline-native-read-lock.json'),JSON.stringify({worker,kind,stalled,elapsedMs:Date.now()-started,scope:'Native IDB transaction lock reproduced prechange startup stall; releasing it resumes startup.'},null,2)+'\n');return;
    }
    await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading,null,{timeout:10000});
    const startup=await page.evaluate(()=>({state:__nir.state(),lock:{...metadataReadLock},execution:__nir.diagnostics().execution,failures:__nir.diagnostics().events.filter(e=>e.operation==='load_metadata')}));
    expect(startup.state.error).toBeNull();expect(startup.lock.released).toBe(false);expect(startup.lock.completed).toBe(false);expect(startup.lock.reads).toBe(2);
    expect(startup.execution.runtime).toBe(worker==='required'?'worker':'main');
    const failedKinds=[...new Set(startup.failures.map(e=>e.location))].sort();expect(failedKinds).toEqual(kind==='both'?['preferences','profile']:[kind]);
    expect(startup.state.diagnostic.message).toContain('E_STORAGE_TIMEOUT');
    if(kind==='profile'){expect(startup.state.preferences.font_scale).toBeCloseTo(1.4);expect(startup.state.preferences.bgm_volume).toBeCloseTo(.2);}
    else expect(startup.state.preferences.font_scale).not.toBe(original.preferences.font_scale);
    await page.waitForTimeout(200);expect(await page.evaluate(()=>metadataReadLock.reads)).toBe(2);
    // It is already playable while the native database is still locked.
    await page.keyboard.press('Enter');await recoverAudioOutput(page);
    await page.waitForFunction(()=>__nir.state().screen==='Story'&&__nir.state().dialogue?.ready&&!__nir.state().loading&&!__nir.state().paused);
    const readable=await page.evaluate(()=>__nir.state());
    await page.evaluate(()=>{metadataReadLock.released=true;});await page.waitForFunction(()=>metadataReadLock.completed);
    const preserved=await records(page,original.key);expect(preserved).toEqual({preferences:original.preferences,profile:original.profile});
    // Late unlock cannot apply old preferences or erase a pending read warning.
    expect((await page.evaluate(()=>__nir.state())).preferences).toEqual(startup.state.preferences);
    expect(await page.evaluate(()=>__nir.state().diagnostic?.code)).toBe('E_STORAGE');expect(errors).toEqual([]);
    await fs.writeFile(info.outputPath('native-metadata-read-timeout.json'),JSON.stringify({worker,kind,elapsedMs:Date.now()-started,startup,readable,preserved,lock:await page.evaluate(()=>({...metadataReadLock})),scope:'Real IndexedDB object-store lock; engine startup and reading continue before unlock in both execution modes. Not hardware storage, Android or explicit metadata retry UI.'},null,2)+'\n');
  });
}
