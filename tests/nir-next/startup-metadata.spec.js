import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import {recoverAudioOutput} from './audio-output-helper.js';

async function seed(page,worker) {
  await page.addInitScript(()=>{
    const ids=new WeakMap();let next=0;window.startupAudioCalls=[];
    const id=context=>{if(!ids.has(context))ids.set(context,++next);return ids.get(context);};
    const record=(context,operation,extra={})=>{if(startupAudioCalls.length<128)startupAudioCalls.push({context:id(context),operation,time:performance.now(),state:context.state,active:navigator.userActivation.isActive,...extra});};
    for(const method of ['resume','suspend']){
      const original=AudioContext.prototype[method];
      AudioContext.prototype[method]=function(...args){record(this,method);const p=original.apply(this,args);p.then(()=>record(this,method+'_resolved'),e=>record(this,method+'_rejected',{error:String(e)}));return p;};
    }
    const start=AudioBufferSourceNode.prototype.start;
    AudioBufferSourceNode.prototype.start=function(...args){record(this.context,'source_start',{loop:this.loop});return start.apply(this,args);};
  });
  await page.goto(`http://127.0.0.1:4259/?test=1&worker=${worker}&backend=webgl2`);
  await page.waitForFunction(()=>window.__nir?.state().ready&&!__nir.state().loading);
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>__nir.state().dialogue?.ready&&!__nir.state().loading);
  await page.evaluate(()=>__nir.action({type:'saves'}));
  await page.waitForFunction(()=>__nir.state().screen==='Saves'&&!__nir.state().loading);
  await page.evaluate(()=>__nir.action({type:'save',slot:0}));
  await page.waitForFunction(()=>__nir.state().status==='Saved');
  return page.evaluate(async()=>{
    const channel=await(await fetch('/channels/stable.json')).json();
    const release=await(await fetch(`/releases/${channel.release}.json`)).json();
    const host=await import(new URL(release.objects[release.engine.host].path,new URL('/',location.href)).href);
    const db=await host.openSaveDatabase(),key=host.profileKey(release.game_id,release.profile);
    const preferences={...__nir.state().preferences,font_scale:1.4,bgm_volume:.25};
    await new Promise((ok,no)=>{const tx=db.transaction(['preferences','profile'],'readwrite');tx.objectStore('preferences').put(preferences,key);tx.objectStore('profile').put(['fixture.prior'],key);tx.oncomplete=ok;tx.onabort=()=>no(tx.error);});
    const saved=await host.readSaveRecord(db,host.saveKey(release.game_id,release.profile,channel.release,0));db.close();return {key,preferences,saveRevision:saved.envelope.revision,tick:saved.envelope.snapshot.tick_us};
  });
}
async function records(page,key) {
  return page.evaluate(async key=>{
    const r=indexedDB.open('nir-player-isolated-v1',1),db=await new Promise((ok,no)=>{r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});
    const read=store=>new Promise((ok,no)=>{const tx=db.transaction(store),r=tx.objectStore(store).get(key);tx.oncomplete=()=>ok(r.result);tx.onabort=()=>no(tx.error);});
    const preferences=await read('preferences'),profile=await read('profile');db.close();
    return {preferences,profile,profileShape:Array.isArray(profile)?{length:profile.length,ownsFirst:Object.hasOwn(profile,0)}:null};
  },key);
}
async function readyAfterReload(page,info) {
  try {await page.waitForFunction(()=>window.__nir?.state().ready&&!__nir.state().loading,null,{timeout:10000});}
  catch(error) {await fs.writeFile(info.outputPath('failed-startup.json'),JSON.stringify({body:await page.locator('body').innerText(),state:await page.evaluate(()=>window.__nir?.state()||null)},null,2)+'\n');throw error;}
}
for(const worker of ['main','required']) {
  test(`metadata read aborts use only ephemeral defaults and preserve stored records; ${worker}`,async({page},info)=>{
    const errors=[];page.on('pageerror',e=>errors.push(e.message));const fixture=await seed(page,worker);
    await page.addInitScript(()=>{
      const pending=new Set(['preferences','profile']),get=IDBObjectStore.prototype.get;
      IDBObjectStore.prototype.get=function(...args){const r=get.apply(this,args);if(this.transaction.mode==='readonly'&&pending.delete(this.name)){const tx=this.transaction;queueMicrotask(()=>tx.abort());}return r;};
    });
    await page.reload();await readyAfterReload(page,info);
    await page.waitForFunction(()=>__nir.state().diagnostic?.code==='E_STORAGE');
    const startup=await page.evaluate(()=>__nir.state()),stored=await records(page,fixture.key);
    expect(startup.error).toBeNull();expect(startup.diagnostic.details.operation).toBe('load_metadata');
    expect(stored.preferences).toEqual(fixture.preferences);expect(stored.profile).toEqual(['fixture.prior']);
    await page.evaluate(()=>__nir.action({type:'volume',bus:'bgm',delta:-.1}));
    // Temporary preference defaults cannot replace unread disk settings.
    expect((await records(page,fixture.key)).preferences).toEqual(fixture.preferences);
    await page.locator('#nir-storage-retry').click();
    await expect(page.locator('#nir-storage-recovery')).toBeHidden();
    await page.waitForFunction(()=>!__nir.state().diagnostic);
    await page.keyboard.press('Enter');await page.waitForFunction(()=>__nir.state().dialogue?.ready&&!__nir.state().loading);
    await page.keyboard.press('Enter');
    await expect.poll(async()=> (await records(page,fixture.key)).profile).toEqual(['fixture.prior','read:intro:1']);
    await page.waitForFunction(()=>!__nir.state().diagnostic);expect(errors).toEqual([]);
    await fs.writeFile(info.outputPath('metadata-read-recovery.json'),JSON.stringify({startup,stored,recovered:await page.evaluate(()=>__nir.state())},null,2)+'\n');
  });
  for(const kind of ['preferences','profile','both']) {
    test(`bad startup ${kind} preserves originals, healthy load and recovery; ${worker}`,async({page},info)=>{
      const errors=[];page.on('pageerror',e=>errors.push(e.message));const fixture=await seed(page,worker);
      await page.evaluate(async({key,kind})=>{
        const r=indexedDB.open('nir-player-isolated-v1',1),db=await new Promise(ok=>{r.onsuccess=()=>ok(r.result);});
        await new Promise((ok,no)=>{const tx=db.transaction(['preferences','profile'],'readwrite');if(kind!=='profile')tx.objectStore('preferences').put({font_scale:'broken',original:'keep'},key);if(kind!=='preferences')tx.objectStore('profile').put(new Array(1),key);tx.oncomplete=ok;tx.onabort=()=>no(tx.error);});db.close();
      },{key:fixture.key,kind});
      await page.reload();await readyAfterReload(page,info);await page.waitForFunction(()=>__nir.state().diagnostic?.code==='E_STORAGE');
      const startup=await page.evaluate(()=>__nir.state()),original=await records(page,fixture.key);
      expect(startup.error).toBeNull();expect(startup.diagnostic.details.operation).toBe('load_metadata');
      if(kind==='profile'){expect(startup.preferences.font_scale).toBeCloseTo(1.4);expect(startup.preferences.bgm_volume).toBeCloseTo(.25);}
      if(kind!=='preferences')expect(original.profileShape).toEqual({length:1,ownsFirst:false});
      // Use the real menu gesture on the new document before loading audio.
      await page.keyboard.press('Escape');
      await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
      await page.evaluate(()=>__nir.action({type:'saves'}));await page.waitForFunction(()=>__nir.state().screen==='Saves'&&!__nir.state().loading);
      expect(fixture.saveRevision).toBe(1);
      await page.evaluate(()=>__nir.action({type:'load',slot:0}));await page.waitForFunction(()=>__nir.state().screen==='Story'&&!__nir.state().loading);
      const loaded=await page.evaluate(()=>__nir.state());expect(loaded.error).toBeNull();expect(loaded.paused).toBe(true);expect(loaded.tick_us).toBe(fixture.tick);expect(loaded.diagnostic.code).toBe('E_STORAGE');
      if(kind!=='profile'){
        await page.evaluate(()=>__nir.action({type:'volume',bus:'bgm',delta:-.1}));
        await page.waitForFunction(()=>__nir.state().diagnostic?.details.operation==='persist');
        const failed=await page.evaluate(()=>__nir.state());
        for(const key of ['session','tick_us','position','interaction','paused'])expect(failed[key]).toEqual(loaded[key]);
        expect((await records(page,fixture.key)).preferences).toEqual(original.preferences);
      }
      if(kind!=='preferences'){
        const audioBefore=await page.evaluate(()=>({state:__nir.state(),audio:__nir.diagnostics().host_work.audio_domains,activation:{active:navigator.userActivation.isActive,ever:navigator.userActivation.hasBeenActive}}));
        await page.keyboard.press('Enter');
        try {await page.waitForFunction(()=>__nir.state().dialogue?.ready&&!__nir.state().loading&&!__nir.state().paused,null,{timeout:15000});}
        catch(error){await fs.writeFile(info.outputPath('audio-restore-stall.json'),JSON.stringify({before:audioBefore,after:await page.evaluate(()=>({state:__nir.state(),audio:__nir.diagnostics().host_work.audio_domains,activation:{active:navigator.userActivation.isActive,ever:navigator.userActivation.hasBeenActive},notice:document.querySelector('#nir-audio-recovery')?.innerText,audioCalls:startupAudioCalls}))},null,2)+'\n');throw error;}

        // A ready text box can still have an authored presentation gate, and
        // the Worker state can precede publication of its actionable controls.
        await page.waitForFunction(()=>{const s=__nir.state();return s.dialogue?.ready&&!s.dialogue.gate&&!s.loading&&!s.paused&&[...document.querySelectorAll('#actions button')].some(b=>!b.disabled&&JSON.parse(b.dataset.action).type==='advance');});
        const beforeAdvance=await page.evaluate(()=>({state:__nir.state(),audio:__nir.diagnostics().host_work.audio_domains,controls:[...document.querySelectorAll('#actions button')].map(b=>({disabled:b.disabled,action:JSON.parse(b.dataset.action)}))}));
        await page.keyboard.press('Enter');
        try{await page.waitForFunction(()=>__nir.state().diagnostic?.location==='profile'&&__nir.state().diagnostic.details.operation==='persist',null,{timeout:15000});}
        catch(error){await fs.writeFile(info.outputPath('profile-advance-stall.json'),JSON.stringify({before:beforeAdvance,after:await page.evaluate(()=>({state:__nir.state(),audio:__nir.diagnostics().host_work.audio_domains,controls:[...document.querySelectorAll('#actions button')].map(b=>({disabled:b.disabled,action:JSON.parse(b.dataset.action)})),notice:document.querySelector('#nir-audio-recovery')?.innerText,audioCalls:startupAudioCalls}))},null,2)+'\n');throw error;}
        expect((await records(page,fixture.key)).profileShape).toEqual({length:1,ownsFirst:false});
      }
      // Explicit fixture repair; the player never resets or deletes the records.
      await page.evaluate(async({key,kind,preferences})=>{
        const r=indexedDB.open('nir-player-isolated-v1',1),db=await new Promise(ok=>{r.onsuccess=()=>ok(r.result);});
        await new Promise((ok,no)=>{const tx=db.transaction(['preferences','profile'],'readwrite');if(kind!=='profile')tx.objectStore('preferences').put(preferences,key);if(kind!=='preferences')tx.objectStore('profile').put(['fixture.prior'],key);tx.oncomplete=ok;tx.onabort=()=>no(tx.error);});db.close();
      },{...fixture,kind});
      const retryFromStory=await page.evaluate(()=>__nir.state().screen==='Story');
      if(retryFromStory){await page.keyboard.press('Escape');await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);}
      await page.locator('#nir-storage-retry').click();
      await expect(page.locator('#nir-storage-recovery')).toBeHidden();
      if(kind!=='profile'){
        await page.evaluate(()=>__nir.action({type:'volume',bus:'bgm',delta:.1}));
        await expect.poll(async()=>Math.abs((await records(page,fixture.key)).preferences.bgm_volume-(await page.evaluate(()=>__nir.state().preferences.bgm_volume)))<1e-5).toBe(true);
      }
      if(retryFromStory){
        await page.keyboard.press('Escape');await page.waitForFunction(()=>__nir.state().screen==='Story'&&!__nir.state().loading);
        if(kind!=='preferences')await recoverAudioOutput(page);
      }
      if(kind!=='preferences'){
        await page.waitForFunction(()=>__nir.state().dialogue?.id==='arrival'&&__nir.state().dialogue.ready&&!__nir.state().loading);
        await page.keyboard.press('Enter');await expect.poll(async()=> (await records(page,fixture.key)).profile).toEqual(['fixture.prior','read:arrival:1','read:intro:1']);
      }
      await page.waitForFunction(()=>!__nir.state().diagnostic);expect(errors).toEqual([]);
      await fs.writeFile(info.outputPath('startup-corruption-recovery.json'),JSON.stringify({kind,startup,original,loaded,recovered:await page.evaluate(()=>__nir.state()),records:await records(page,fixture.key)},null,2)+'\n');
    });
  }
}
