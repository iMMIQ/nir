import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';

async function boot(page,worker) {
  await page.addInitScript(()=>{
    window.persistenceWrites=[];window.persistenceAborts={preferences:0,profile:0};window.persistenceAudio=[];
    const put=IDBObjectStore.prototype.put;
    IDBObjectStore.prototype.put=function(value,...args){
      const result=put.call(this,value,...args);
      if(['preferences','profile'].includes(this.name)){
        const row={kind:this.name,value:structuredClone(value),aborted:false};persistenceWrites.push(row);
        if(persistenceAborts[this.name]>0){
          persistenceAborts[this.name]--;row.aborted=true;const tx=this.transaction;
          queueMicrotask(()=>tx.abort());
        }
      }
      return result;
    };
    const create=AudioContext.prototype.createBufferSource;
    AudioContext.prototype.createBufferSource=function(...args){
      const source=create.apply(this,args),row={source,stops:0};persistenceAudio.push(row);
      const stop=source.stop;source.stop=function(...args){row.stops++;return stop.apply(this,args);};return source;
    };
    window.persistenceStory=()=>{const s=__nir.state();return {session:s.session,interaction:s.interaction,tick:s.tick_us,position:s.position};};
    window.persistenceLoops=()=>persistenceAudio.filter(row=>row.source.loop).map(row=>({stops:row.stops}));
  });
  await page.goto(`http://127.0.0.1:4259/?test=1&worker=${worker}&backend=webgl2`);
  await page.waitForFunction(()=>window.__nir?.state().ready&&!__nir.state().loading);
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>__nir.state().dialogue?.ready&&!__nir.state().loading);
  await page.evaluate(async()=>{
    const channel=await(await fetch('/channels/stable.json')).json();
    const release=await(await fetch(`/releases/${channel.release}.json`)).json();
    const host=await import(new URL(release.objects[release.engine.host].path,new URL('/',location.href)).href);
    window.persistenceDb=await host.openSaveDatabase();window.persistenceKey=host.profileKey(release.game_id,release.profile);
    window.persistenceRead=store=>new Promise((ok,no)=>{
      const tx=persistenceDb.transaction(store),r=tx.objectStore(store).get(persistenceKey);
      tx.oncomplete=()=>ok(r.result??null);tx.onabort=()=>no(tx.error);
    });
  });
}

for(const worker of ['main','required']) {
  test(`malformed stored progress fails without hanging or overwriting and recovers after repair; ${worker}`,async({page},info)=>{
    const errors=[];page.on('pageerror',e=>errors.push(e.message));await boot(page,worker);
    const corruptionType=worker==='main'?'object':'sparse';
    // Create a hole in the browser: transporting a sparse array through the
    // automation protocol can change its shape before IndexedDB sees it.
    await page.evaluate(type=>new Promise((ok,no)=>{
      const bad=type==='object'?{unreadable:'original'}:new Array(1);
      const tx=persistenceDb.transaction('profile','readwrite');tx.objectStore('profile').put(bad,persistenceKey);
      tx.oncomplete=ok;tx.onabort=()=>no(tx.error);
    }),corruptionType);
    await page.keyboard.press('Enter');
    await page.waitForFunction(()=>__nir.state().diagnostic?.location==='profile');
    const failed=await page.evaluate(async()=>{
      const stored=await persistenceRead('profile');
      return {stored,shape:Array.isArray(stored)?{length:stored.length,ownsFirst:Object.hasOwn(stored,0)}:null,state:__nir.state()};
    });
    expect(failed.state.error).toBeNull();expect(failed.state.diagnostic.message).toMatch(/E_PROFILE_RECORD/);
    if(corruptionType==='object')expect(failed.stored).toEqual({unreadable:'original'});
    else expect(failed.shape).toEqual({length:1,ownsFirst:false});
    // Only the fixture repairs the bad record; production retains it intact.
    await page.evaluate(()=>new Promise((ok,no)=>{
      const tx=persistenceDb.transaction('profile','readwrite');tx.objectStore('profile').put(['fixture.prior'],persistenceKey);
      tx.oncomplete=ok;tx.onabort=()=>no(tx.error);
    }));
    await page.waitForFunction(()=>__nir.state().dialogue?.id==='arrival'&&__nir.state().dialogue.ready&&!__nir.state().loading);
    await page.keyboard.press('Enter');
    await expect.poll(()=>page.evaluate(()=>persistenceRead('profile'))).toEqual(['fixture.prior','read:arrival:1','read:intro:1']);
    await page.waitForFunction(()=>__nir.state().diagnostic?.location!=='profile');
    const recovered=await page.evaluate(async()=>({stored:await persistenceRead('profile'),state:__nir.state()}));
    expect(recovered.stored).toEqual(['fixture.prior','read:arrival:1','read:intro:1']);expect(errors).toEqual([]);
    await fs.writeFile(info.outputPath('profile-corruption.json'),JSON.stringify({failed,recovered},null,2)+'\n');
  });

  test(`failed read progress is retained through the next real reading boundary; ${worker}`,async({page},info)=>{
    const errors=[];page.on('pageerror',e=>errors.push(e.message));await boot(page,worker);
    await page.evaluate(()=>{persistenceAborts.profile=1;});
    await page.keyboard.press('Enter');
    await page.waitForFunction(()=>persistenceWrites.some(r=>r.kind==='profile'&&r.aborted)&&__nir.state().status.includes('E_STORAGE'));
    const failed=await page.evaluate(async()=>({writes:persistenceWrites.filter(r=>r.kind==='profile'),stored:await persistenceRead('profile'),state:__nir.state()}));
    expect(failed.state.error).toBeNull();expect(failed.writes[0].value.length).toBeGreaterThan(0);
    expect(failed.stored).toBeNull();
    await page.waitForFunction(()=>__nir.state().dialogue?.id==='arrival'&&__nir.state().dialogue.ready&&!__nir.state().loading);
    await page.keyboard.press('Enter');
    await expect.poll(()=>page.evaluate(()=>persistenceRead('profile'))).not.toBeNull();
    const recovered=await page.evaluate(async()=>({writes:persistenceWrites.filter(r=>r.kind==='profile'),stored:await persistenceRead('profile'),state:__nir.state()}));
    for(const key of failed.writes[0].value)expect(recovered.stored).toContain(key);
    expect(recovered.stored.length).toBeGreaterThan(failed.writes[0].value.length);
    await page.waitForFunction(()=>__nir.state().diagnostic?.location!=='profile');
    expect(errors).toEqual([]);
    await fs.writeFile(info.outputPath('profile-recovery.json'),JSON.stringify({failed,recovered,scope:'Actual desktop IndexedDB abort and two author reading boundaries; Worker/main. No physical disk quota, hardware audio or Android.'},null,2)+'\n');
  });

  test(`preference write failure preserves current scene and clears after a later committed change; ${worker}`,async({page},info)=>{
    const errors=[];page.on('pageerror',e=>errors.push(e.message));await boot(page,worker);
    await page.evaluate(()=>__nir.action({type:'settings'}));
    await page.waitForFunction(()=>__nir.state().screen==='Settings'&&!__nir.state().loading);
    const before=await page.evaluate(()=>({story:persistenceStory(),loops:persistenceLoops(),volume:__nir.state().preferences.bgm_volume}));
    await page.evaluate(()=>{persistenceAborts.preferences=1;__nir.action({type:'volume',bus:'bgm',delta:-.1});});
    await page.waitForFunction(()=>__nir.state().status.includes('E_STORAGE')&&persistenceWrites.some(r=>r.kind==='preferences'&&r.aborted));
    const failed=await page.evaluate(async()=>({story:persistenceStory(),loops:persistenceLoops(),stored:await persistenceRead('preferences'),state:__nir.state()}));
    expect(failed.story).toEqual(before.story);expect(failed.loops).toEqual(before.loops);expect(failed.state.error).toBeNull();
    expect(failed.state.preferences.bgm_volume).toBeCloseTo(before.volume-.1,5);expect(failed.stored).toBeNull();
    await page.evaluate(()=>__nir.action({type:'volume',bus:'bgm',delta:.1}));
    await expect.poll(()=>page.evaluate(async()=>Math.abs((await persistenceRead('preferences'))?.bgm_volume-__nir.state().preferences.bgm_volume)<1e-5)).toBe(true);
    await page.waitForFunction(()=>__nir.state().diagnostic?.location!=='preferences'&&!__nir.state().status.includes('E_STORAGE'));
    const recovered=await page.evaluate(async()=>({story:persistenceStory(),loops:persistenceLoops(),stored:await persistenceRead('preferences'),state:__nir.state()}));
    expect(recovered.story).toEqual(before.story);expect(recovered.loops).toEqual(before.loops);expect(errors).toEqual([]);
    await fs.writeFile(info.outputPath('preference-recovery.json'),JSON.stringify({before,failed,recovered},null,2)+'\n');
  });
}
