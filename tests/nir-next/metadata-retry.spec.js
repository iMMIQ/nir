import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import {recoverAudioOutput} from './audio-output-helper.js';

async function records(page,key) {
  return page.evaluate(async key=>{
    const r=indexedDB.open('nir-player-isolated-v1',1),db=await new Promise((ok,no)=>{r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});
    const read=kind=>new Promise((ok,no)=>{const tx=db.transaction(kind),r=tx.objectStore(kind).get(key);tx.oncomplete=()=>ok(r.result);tx.onabort=()=>no(tx.error);});
    try{return {preferences:await read('preferences'),profile:await read('profile')};}finally{db.close();}
  },key);
}
async function bootSeeded(page,worker) {
  await page.goto(`http://127.0.0.1:4259/?test=1&worker=${worker}&backend=webgl2`);await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
  const seed=await page.evaluate(async()=>{
    const channel=await(await fetch('/channels/stable.json')).json(),release=await(await fetch(`/releases/${channel.release}.json`)).json();
    const r=indexedDB.open('nir-player-isolated-v1',1),db=await new Promise(ok=>{r.onsuccess=()=>ok(r.result);});
    const key=[release.game_id,release.profile],preferences={...__nir.state().preferences,font_scale:1.4,bgm_volume:.2},profile=['fixture.original'];
    try{await new Promise((ok,no)=>{const tx=db.transaction(['preferences','profile'],'readwrite');tx.objectStore('preferences').put(preferences,key);tx.objectStore('profile').put(profile,key);tx.oncomplete=ok;tx.onabort=()=>no(tx.error);});}finally{db.close();}
    return {key,preferences,profile};
  });
  await page.addInitScript(()=>{
    globalThis.retryReadAborts={preferences:1,profile:1};globalThis.retryWriteAborts={preferences:0,profile:0};globalThis.retryReads={preferences:0,profile:0};globalThis.retryWrites=[];globalThis.retrySources=[];
    const get=IDBObjectStore.prototype.get,put=IDBObjectStore.prototype.put,create=AudioContext.prototype.createBufferSource;
    IDBObjectStore.prototype.get=function(...args){const r=get.apply(this,args);if(this.name in retryReadAborts&&this.transaction.mode==='readonly'){retryReads[this.name]++;if(retryReadAborts[this.name]>0){retryReadAborts[this.name]--;const tx=this.transaction;queueMicrotask(()=>tx.abort());}}return r;};
    IDBObjectStore.prototype.put=function(value,...args){const r=put.call(this,value,...args);if(this.name in retryWriteAborts){retryWrites.push({kind:this.name,value:structuredClone(value)});if(retryWriteAborts[this.name]>0){retryWriteAborts[this.name]--;const tx=this.transaction;queueMicrotask(()=>tx.abort());}}return r;};
    AudioContext.prototype.createBufferSource=function(...args){const source=create.apply(this,args),row={source,stops:0};retrySources.push(row);const stop=source.stop;source.stop=function(...args){row.stops++;return stop.apply(this,args);};return source;};
    globalThis.retryIdentity=()=>{const s=__nir.state();return {session:s.session,interaction:s.interaction,position:s.position,tick:s.tick_us,history:s.history_count,paused:s.paused,screen:s.screen};};
    globalThis.retryLoops=()=>retrySources.filter(r=>r.source.loop).map((r,index)=>({index,stops:r.stops}));
  });
  await page.reload();await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);await expect(page.locator('#nir-storage-recovery')).toBeVisible();return seed;
}
async function pausedMenu(page) {
  await page.keyboard.press('Enter');await recoverAudioOutput(page);await page.waitForFunction(()=>__nir.state().dialogue?.ready&&!__nir.state().loading&&!__nir.state().paused&&!__nir.state().dialogue.gate&&[...document.querySelectorAll('#actions button')].some(b=>!b.disabled&&JSON.parse(b.dataset.action).type==='advance'));
  await expect(page.locator('#nir-storage-recovery')).toBeHidden();
  await page.keyboard.press('Enter');await page.waitForFunction(()=>__nir.state().dialogue?.id==='arrival'&&__nir.state().dialogue.ready&&!__nir.state().loading);
  await page.keyboard.press('Escape');await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading&&__nir.state().paused);
  await expect(page.locator('#nir-storage-recovery')).toBeVisible();
}
for(const worker of ['main','required']) {
  test(`metadata retry preserves edited defaults and newly read progress across partial failure; ${worker}`,async({page},info)=>{
    const errors=[];page.on('pageerror',e=>errors.push(e.message));const seed=await bootSeeded(page,worker);await pausedMenu(page);
    const temporary=await page.evaluate(()=>__nir.state().preferences.bgm_volume);
    await page.evaluate(async()=>{await __nir.action({type:'volume',bus:'bgm',delta:-.1});await __nir.action({type:'volume',bus:'bgm',delta:.1});});
    await page.waitForFunction(()=>__nir.state().diagnostic?.details.operation==='persist');
    const before={identity:await page.evaluate(()=>retryIdentity()),loops:await page.evaluate(()=>retryLoops()),records:await records(page,seed.key)};
    expect(before.records.preferences).toEqual(seed.preferences);expect(before.records.profile).toContain('fixture.original');expect(before.records.profile).toContain('read:intro:1');
    expect(await page.evaluate(()=>retryWrites.filter(r=>r.kind==='preferences').length)).toBe(0);
    await page.evaluate(()=>{retryReadAborts.profile=1;});await page.locator('#nir-storage-retry').click();
    await expect.poll(()=>page.evaluate(()=>__nir.state().diagnostic?.location)).toBe('profile');
    await expect(page.locator('#nir-storage-retry')).toBeEnabled();await expect(page.locator('#nir-storage-recovery')).toBeVisible();
    const partial=await records(page,seed.key);expect(partial.preferences.font_scale).toBeCloseTo(1.4);expect(partial.preferences.bgm_volume).toBeCloseTo(temporary);expect(partial.profile).toEqual(before.records.profile);
    expect(await page.evaluate(()=>retryIdentity())).toEqual(before.identity);expect(await page.evaluate(()=>retryLoops())).toEqual(before.loops);
    // Keyboard activation belongs to the host retry control and cannot close
    // the menu or become an Advance/NewGame action.
    await page.locator('#nir-storage-retry').focus();await page.keyboard.press('Enter');
    await expect(page.locator('#nir-storage-recovery')).toBeHidden();await page.waitForFunction(()=>!__nir.state().diagnostic);
    expect(await page.evaluate(()=>retryIdentity())).toEqual(before.identity);expect(await page.evaluate(()=>retryLoops())).toEqual(before.loops);
    const recovered=await records(page,seed.key);expect(recovered.profile).toEqual(before.records.profile);expect(recovered.preferences).toEqual(partial.preferences);expect(errors).toEqual([]);
    await fs.writeFile(info.outputPath('metadata-retry-partial.json'),JSON.stringify({worker,before,partial,recovered,state:await page.evaluate(()=>__nir.state()),scope:'Native IDB abort and successful read/write commits; trusted Retry click and keyboard, current scene/loop source retained. Not physical sound or Android.'},null,2)+'\n');
  });

  for(const changed of [true,false])test(`a failed recovery write remains visible and explicit retry saves without another setting change; ${worker}; changed=${changed}`,async({page},info)=>{
    const errors=[];page.on('pageerror',e=>errors.push(e.message));const seed=await bootSeeded(page,worker);await pausedMenu(page);
    await page.evaluate(changed=>__nir.action({type:'volume',bus:'bgm',delta:changed?-.1:0}),changed);await page.waitForFunction(()=>__nir.state().diagnostic?.details.operation==='persist');
    const before=await page.evaluate(()=>retryIdentity());await page.evaluate(()=>{retryWriteAborts.preferences=1;});await page.locator('#nir-storage-retry').click();
    await page.waitForFunction(()=>retryWrites.some(r=>r.kind==='preferences')&&__nir.state().diagnostic?.location==='preferences'&&__nir.state().diagnostic.details.operation==='persist');
    await expect(page.locator('#nir-storage-recovery')).toBeVisible();await expect(page.locator('#nir-storage-retry')).toBeEnabled();
    const failed=await records(page,seed.key);expect(failed.preferences).toEqual(seed.preferences);expect(await page.evaluate(()=>retryIdentity())).toEqual(before);
    await page.locator('#nir-storage-retry').click();await expect(page.locator('#nir-storage-recovery')).toBeHidden();await page.waitForFunction(()=>!__nir.state().diagnostic);
    const recovered=await records(page,seed.key);expect(recovered.preferences.font_scale).toBeCloseTo(1.4);expect(recovered.preferences.bgm_volume).toBeCloseTo(await page.evaluate(()=>__nir.state().preferences.bgm_volume));expect(recovered.profile).toContain('read:intro:1');
    if(!changed){
      expect(Object.keys(recovered.preferences).sort()).toEqual(Object.keys(seed.preferences).sort());
      for(const [key,value] of Object.entries(seed.preferences)){
        if(typeof value==='number'){expect(typeof recovered.preferences[key]).toBe('number');expect(Math.fround(recovered.preferences[key])).toBe(Math.fround(value));}
        else expect(recovered.preferences[key]).toEqual(value);
      }
    }
    expect(await page.evaluate(()=>retryIdentity())).toEqual(before);expect(errors).toEqual([]);
    await fs.writeFile(info.outputPath('metadata-retry-write.json'),JSON.stringify({worker,before,failed,recovered,state:await page.evaluate(()=>__nir.state())},null,2)+'\n');
  });
}

for(const worker of ['main','required'])test(`edits during a pending native read win and duplicate retry clicks do not add requests; ${worker}`,async({page},info)=>{
  const errors=[];page.on('pageerror',e=>errors.push(e.message));const seed=await bootSeeded(page,worker);await pausedMenu(page);
  const before=await page.evaluate(()=>({identity:retryIdentity(),loops:retryLoops(),reads:{...retryReads},volume:__nir.state().preferences.bgm_volume}));
  await page.evaluate(async()=>{
    const r=indexedDB.open('nir-player-isolated-v1',1),db=await new Promise(ok=>{r.onsuccess=()=>ok(r.result);});
    globalThis.retryHold={released:false,completed:false,requests:0};
    const tx=db.transaction('preferences','readwrite'),store=tx.objectStore('preferences');tx.oncomplete=()=>{retryHold.completed=true;db.close();};
    const keep=()=>{retryHold.requests++;const r=store.get('__nir_retry_fixture_lock');r.onsuccess=()=>{if(!retryHold.released)keep();};};keep();
  });
  await page.locator('#nir-storage-retry').click();await expect(page.locator('#nir-storage-retry')).toBeDisabled();
  await page.waitForFunction(before=>retryReads.preferences===before.preferences+1&&retryReads.profile===before.profile+1,before.reads);
  await page.evaluate(async()=>{for(let i=0;i<5;i++)document.querySelector('#nir-storage-retry').click();await __nir.action({type:'volume',bus:'bgm',delta:-.1});});
  await page.waitForFunction(volume=>Math.abs(__nir.state().preferences.bgm_volume-(volume-.1))<1e-5,before.volume);
  const pending=await page.evaluate(()=>({identity:retryIdentity(),reads:{...retryReads},lock:{...retryHold},preferences:__nir.state().preferences}));
  expect(pending.identity).toEqual(before.identity);expect(pending.reads).toEqual({preferences:before.reads.preferences+1,profile:before.reads.profile+1});expect(pending.lock.completed).toBe(false);
  await page.evaluate(()=>{retryHold.released=true;});await expect(page.locator('#nir-storage-recovery')).toBeHidden();await page.waitForFunction(()=>!__nir.state().diagnostic);
  const recovered=await records(page,seed.key);expect(recovered.preferences.font_scale).toBeCloseTo(1.4);expect(recovered.preferences.bgm_volume).toBeCloseTo(before.volume-.1);expect(recovered.profile).toContain('fixture.original');expect(recovered.profile).toContain('read:intro:1');
  expect(await page.evaluate(()=>retryIdentity())).toEqual(before.identity);expect(await page.evaluate(()=>retryLoops())).toEqual(before.loops);expect(errors).toEqual([]);
  await fs.writeFile(info.outputPath('metadata-retry-during-read.json'),JSON.stringify({worker,before,pending,recovered,scope:'Actual native IDB lock, edit while read pending and duplicate trusted control activation suppressed; no hardware timing/Android/audio-output claim.'},null,2)+'\n');
});
