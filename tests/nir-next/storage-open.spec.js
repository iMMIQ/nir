import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import {recoverAudioOutput} from './audio-output-helper.js';

async function activate(page,type,fields={}) {
  await page.waitForFunction(({type,fields})=>[...document.querySelectorAll('#actions button')].some(b=>{const a=JSON.parse(b.dataset.action);return !b.disabled&&a.type===type&&Object.entries(fields).every(([k,v])=>a[k]===v);}),{type,fields});
  const rect=await page.evaluate(({type,fields})=>JSON.parse([...document.querySelectorAll('#actions button')].find(b=>{const a=JSON.parse(b.dataset.action);return a.type===type&&Object.entries(fields).every(([k,v])=>a[k]===v);}).dataset.rect),{type,fields});
  await page.mouse.click(rect[0]+rect[2]/2,rect[1]+rect[3]/2);
}
async function screen(page,name){await page.waitForFunction(name=>__nir.state().screen===name&&!__nir.state().loading,name);}
async function records(page,key) {
  return page.evaluate(async key=>{
    const open=globalThis.storageOpenNative||IDBFactory.prototype.open;
    const r=open.call(indexedDB,'nir-player-isolated-v1'),db=await new Promise((ok,no)=>{r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});
    try{
      const read=store=>new Promise((ok,no)=>{const tx=db.transaction(store),r=tx.objectStore(store).get(key);tx.oncomplete=()=>ok(r.result);tx.onabort=()=>no(tx.error);});
      const preferences=await read('preferences'),profile=await read('profile');
      const slots=await new Promise((ok,no)=>{const tx=db.transaction('saves'),r=tx.objectStore('saves').getAll();tx.oncomplete=()=>ok(r.result);tx.onabort=()=>no(tx.error);});
      return {preferences,profile,saves:slots.map(s=>({slot:s.envelope.slot,revision:s.envelope.revision})),version:db.version};
    }finally{db.close();}
  },key);
}
async function seed(page,worker) {
  await page.goto(`http://127.0.0.1:4259/?test=1&worker=${worker}&backend=webgl2`);await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);await page.keyboard.press('Enter');await screen(page,'Story');await recoverAudioOutput(page);
  await activate(page,'menu');await screen(page,'Menu');await activate(page,'saves');await screen(page,'Saves');await activate(page,'save',{slot:0});await page.waitForFunction(()=>__nir.state().status==='Saved');
  return page.evaluate(async()=>{
    const channel=await(await fetch('/channels/stable.json')).json(),release=await(await fetch(`/releases/${channel.release}.json`)).json();
    const host=await import(new URL(release.objects[release.engine.host].path,new URL('/',location.href)).href),db=await host.openSaveDatabase(),key=host.profileKey(release.game_id,release.profile);
    const preferences={...__nir.state().preferences,font_scale:1.4,bgm_volume:.2},profile=['fixture.original'];
    try{await new Promise((ok,no)=>{const tx=db.transaction(['preferences','profile'],'readwrite');tx.objectStore('preferences').put(preferences,key);tx.objectStore('profile').put(profile,key);tx.oncomplete=ok;tx.onabort=()=>no(tx.error);});}finally{db.close();}
    return {key,preferences,profile};
  });
}
for(const worker of ['main','required']) {
  for(const fault of ['denied','blocked','timeout']) {
    test(`storage open ${fault} keeps startup playable and prior records; ${worker}`,async({page},info)=>{
      const errors=[];page.on('pageerror',e=>errors.push(e.message));const original=await seed(page,worker),before=await records(page,original.key);
      await page.addInitScript(fault=>{
        const native=IDBFactory.prototype.open;window.storageOpenNative=native;window.storageOpenFault=fault;window.storageOpenCalls=0;window.storageLateRequests=[];
        IDBFactory.prototype.open=function(name,...args){
          if(name!=='nir-player-isolated-v1'||!storageOpenFault)return native.call(this,name,...args);
          storageOpenCalls++;
          if(storageOpenFault==='denied')throw new DOMException('fixture storage denied','SecurityError');
          const request={transaction:null,result:null,error:null};storageLateRequests.push(request);
          if(storageOpenFault==='blocked')queueMicrotask(()=>request.onblocked?.());return request;
        };
      },fault);
      const started=Date.now();await page.reload();await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading,null,{timeout:15000});
      await page.waitForFunction(()=>{const h=__nir.diagnostics().host_work;return storageOpenCalls===2&&!__nir.metrics.activeRequests&&!h.pending_owner_callbacks;});
      const startup=await page.evaluate(()=>({state:__nir.state(),calls:storageOpenCalls,execution:__nir.diagnostics().execution,metadataFailures:__nir.diagnostics().events.filter(e=>e.stage==='diagnostic'&&e.operation==='load_metadata')}));
      expect(startup.state.error).toBeNull();expect([...new Set(startup.metadataFailures.map(e=>e.location))].sort()).toEqual(['preferences','profile']);expect(startup.state.preferences.font_scale).not.toBe(original.preferences.font_scale);expect(startup.calls).toBe(2);expect(startup.execution.runtime).toBe(worker==='required'?'worker':'main');
      // Metadata plus the Player's one initial ListSaves request. Idle must
      // not spin retries even after both have actually settled.
      await page.waitForTimeout(200);expect(await page.evaluate(()=>storageOpenCalls)).toBe(2);
      const preserved=await records(page,original.key);expect(preserved).toEqual(before);
      await page.keyboard.press('Enter');await screen(page,'Story');await recoverAudioOutput(page);await page.waitForFunction(()=>__nir.state().dialogue?.ready&&!__nir.state().paused);const readable=await page.evaluate(()=>__nir.state());expect(readable.error).toBeNull();
      // New explicit operations reconnect. No automatic replay of the failed
      // startup read and no preference write from temporary defaults.
      await page.evaluate(()=>{storageOpenFault=null;});await activate(page,'menu');await screen(page,'Menu');await activate(page,'saves');await screen(page,'Saves');
      await page.waitForFunction(()=>[...document.querySelectorAll('#actions button')].some(b=>{const a=JSON.parse(b.dataset.action);return !b.disabled&&a.type==='load'&&a.slot===0;}));await activate(page,'save',{slot:1});await page.waitForFunction(()=>__nir.state().status==='Saved');
      const recovered=await records(page,original.key);expect(recovered.preferences).toEqual(before.preferences);expect(recovered.profile).toContain('fixture.original');expect(recovered.saves.find(s=>s.slot===0)).toEqual({slot:0,revision:1});expect(recovered.saves.find(s=>s.slot===1)).toEqual({slot:1,revision:1});
      const late=await page.evaluate(()=>{let closed=0;for(const r of storageLateRequests){r.result={close(){closed++;}};r.onsuccess?.();}return {requests:storageLateRequests.length,closed};});expect(late.closed).toBe(late.requests);expect(errors).toEqual([]);
      await fs.writeFile(info.outputPath('storage-open.json'),JSON.stringify({fault,worker,elapsedMs:Date.now()-started,startup,readable,before,preserved,recovered,late,scope:'Controlled open exception/blocked event/no-callback boundary, with actual IndexedDB records and user controls. Not physical permission/lock failure or Android.'},null,2)+'\n');
    });
  }
  test(`future database version is preserved without preventing reading; ${worker}`,async({page},info)=>{
    const errors=[];page.on('pageerror',e=>errors.push(e.message));const original=await seed(page,worker),before=await records(page,original.key);
    await page.evaluate(async()=>{const r=indexedDB.open('nir-player-isolated-v1',2);const db=await new Promise((ok,no)=>{r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});db.close();});
    await page.reload();await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);await page.waitForFunction(()=>__nir.state().diagnostic?.message.includes('VersionError'));
    const startup=await page.evaluate(()=>__nir.state());expect(startup.error).toBeNull();await page.keyboard.press('Enter');await screen(page,'Story');await recoverAudioOutput(page);await page.waitForFunction(()=>__nir.state().dialogue?.ready&&!__nir.state().paused);
    const after=await records(page,original.key);expect(after.preferences).toEqual(before.preferences);expect(after.profile).toEqual(before.profile);expect(after.saves).toEqual(before.saves);expect(after.version).toBe(2);expect(errors).toEqual([]);
    await fs.writeFile(info.outputPath('future-database.json'),JSON.stringify({worker,before,startup,after,scope:'Actual IndexedDB VersionError from a seeded future schema, not device permissions or hardware storage.'},null,2)+'\n');
  });
}
