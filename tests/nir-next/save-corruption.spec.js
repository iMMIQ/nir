import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';

async function boot(page,worker) {
  await page.addInitScript(()=>{
    window.slotAudio=[];
    const create=AudioContext.prototype.createBufferSource;
    AudioContext.prototype.createBufferSource=function(...args){
      const source=create.apply(this,args),row={source,stops:0};slotAudio.push(row);
      const stop=source.stop;source.stop=function(...args){row.stops++;return stop.apply(this,args);};return source;
    };
  });
  await page.goto(`http://127.0.0.1:4259/?test=1&worker=${worker}&backend=webgl2`);
  await page.waitForFunction(()=>window.__nir?.state().ready&&!__nir.state().loading);
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>__nir.state().screen==='Story'&&__nir.state().dialogue?.ready&&!__nir.state().loading);
  await page.evaluate(()=>__nir.action({type:'save',slot:1}));
  await page.waitForFunction(()=>__nir.state().status==='Saved');
  return await page.evaluate(async()=>{
    const channel=await(await fetch('/channels/stable.json')).json();
    const release=await(await fetch(`/releases/${channel.release}.json`)).json();
    const url=new URL(release.objects[release.engine.host].path,new URL('/',location.href)).href;
    window.slotHost=await import(url);window.slotDb=await slotHost.openSaveDatabase();
    window.slotKey=slot=>slotHost.saveKey(release.game_id,release.profile,channel.release,slot);
    window.slotGood=await slotHost.readSaveRecord(slotDb,slotKey(1));
    window.slotPut=(slot,value)=>new Promise((ok,no)=>{
      const tx=slotDb.transaction('saves','readwrite');tx.objectStore('saves').put(value,slotKey(slot));
      tx.oncomplete=ok;tx.onabort=()=>no(tx.error);
    });
    window.slotStory=()=>{const s=__nir.state();return {session:s.session,tick:s.tick_us,position:s.position,interaction:s.interaction};};
    window.slotLoops=()=>slotAudio.filter(r=>r.source.loop).map(r=>({stops:r.stops}));
    return {negativeZero:slotGood.envelope.snapshot.scene.some(node=>Object.is(node.x,-0)),release:channel.release,engine:__nir.diagnostics().engine,execution:__nir.diagnostics().execution};
  });
}
async function reopen(page) {
  await page.evaluate(()=>__nir.action({type:'close'}));
  await page.waitForFunction(()=>__nir.state().screen==='Story');
  const completed=await page.evaluate(()=>__nir.metrics.completedRequests);
  await page.evaluate(()=>__nir.action({type:'saves'}));
  // Live loop audio owns reservations; zero total requests is not list completion.
  await page.waitForFunction(n=>__nir.state().screen==='Saves'&&__nir.metrics.completedRequests>n,completed);
}
for(const worker of ['main','required']) {
  test(`failed Web write preserves progress, retries and exports signed zero; ${worker}`,async({page},info)=>{
    const identity=await boot(page,worker);
    expect(identity.negativeZero).toBe(true);
    await reopen(page);
    const before=await page.evaluate(()=>({story:slotStory(),loops:slotLoops(),record:slotGood}));
    await page.evaluate(()=>{
      window.slotWriteFailures=0;
      const put=IDBObjectStore.prototype.put;
      IDBObjectStore.prototype.put=function(value,...args){
        if(this.name==='saves'&&value.slot===1&&value.envelope?.revision===2&&!slotWriteFailures){
          slotWriteFailures++;throw new DOMException('Injected write capacity failure','QuotaExceededError');
        }return put.call(this,value,...args);
      };
    });
    await page.evaluate(()=>__nir.action({type:'save',slot:1}));
    await page.waitForFunction(()=>slotWriteFailures===1&&__nir.state().status?.includes('Storage operation failed'));
    const failed=await page.evaluate(async()=>({story:slotStory(),loops:slotLoops(),record:await slotHost.readSaveRecord(slotDb,slotKey(1)),state:__nir.state()}));
    expect(failed.record).toEqual(before.record);expect(failed.story).toEqual(before.story);
    expect(failed.loops).toEqual(before.loops);expect(failed.state.error).toBeNull();
    await page.evaluate(()=>__nir.action({type:'save',slot:1}));
    await page.waitForFunction(()=>__nir.state().status==='Saved');
    expect(await page.evaluate(async()=> (await slotHost.readSaveRecord(slotDb,slotKey(1))).envelope.revision)).toBe(2);
    await page.evaluate(()=>__nir.action({type:'close'}));
    await page.waitForFunction(()=>__nir.state().screen==='Story');
    await page.evaluate(()=>__nir.action({type:'menu'}));
    await expect(page.locator('#nir-history-button')).toBeVisible();
    await page.locator('#nir-history-button').click();
    const row=page.locator('#nir-history-panel tr').filter({hasText:identity.release});
    await expect(row).toHaveCount(1);
    const downloadPromise=page.waitForEvent('download');await row.getByRole('button',{name:'Export'}).click();
    const download=await downloadPromise,stream=await download.createReadStream(),chunks=[];
    for await(const chunk of stream)chunks.push(chunk);
    const json=Buffer.concat(chunks).toString(),envelope=JSON.parse(json);
    expect(envelope.snapshot.scene.some(node=>Object.is(node.x,-0))).toBe(true);
    expect(await page.evaluate(json=>__nir.inspectSave(json,slotKey(1)),json)).toBe(2);
    await fs.writeFile(info.outputPath('web-slot-write-failure-export.json'),JSON.stringify({identity,before,failed,exportedRevision:envelope.revision,scope:'Injected IDB write exception, actual abort/retry/export and Rust inspection; not a physical disk quota or Android proof.'},null,2)+'\n');
  });
  test(`unreadable Web slots preserve records, healthy slots, story and BGM; ${worker}`,async({page},info)=>{
    const errors=[];page.on('pageerror',error=>errors.push(error.message));
    const identity=await boot(page,worker),observations=[];
    await page.evaluate(async()=>{
      await slotPut(2,{...slotGood,slot:2,envelope:{...slotGood.envelope,slot:2}});
    });
    for(const variant of ['missing-envelope','bad-digest','wrong-release','wrong-game','wrong-metadata','null-record']) {
      const bad=await page.evaluate(async variant=>{
        const record={...slotGood,slot:0,envelope:{...structuredClone(slotGood.envelope),slot:0}};
        if(variant==='missing-envelope')delete record.envelope;
        if(variant==='bad-digest')record.envelope.digest='0'.repeat(64);
        if(variant==='wrong-release')record.envelope.snapshot.release='a'.repeat(64);
        if(variant==='wrong-game')record.envelope.snapshot.game_id='another-game';
        if(variant==='wrong-metadata')record.gameId='another-game';
        const value=variant==='null-record'?null:record;
        await slotPut(0,value);return value;
      },variant);
      await reopen(page);
      const before=await page.evaluate(()=>({story:slotStory(),loops:slotLoops(),state:__nir.state(),
        actions:[...document.querySelectorAll('#actions button')].map(n=>({action:JSON.parse(n.dataset.action),enabled:!n.disabled}))}));
      expect(before.actions.find(r=>r.action.type==='save'&&r.action.slot===0)?.enabled).toBe(false);
      expect(before.actions.find(r=>r.action.type==='load'&&r.action.slot===0)?.enabled).toBe(true);
      expect(before.actions.find(r=>r.action.type==='load'&&r.action.slot===2)?.enabled).toBe(true);
      expect(before.loops.length).toBeGreaterThan(0);
      const commit=await page.evaluate(async()=>{
        let error=null;
        try{await slotHost.commitSaveRecord(slotDb,slotKey(0),{...slotGood.envelope,slot:0},{...slotGood,slot:0},0,__nir.inspectSave);}catch(e){error=String(e);}
        return {error,stored:await slotHost.readSaveRecord(slotDb,slotKey(0))};
      });
      expect(commit.error).not.toBeNull();expect(commit.stored).toEqual(bad);
      const completed=await page.evaluate(()=>__nir.metrics.completedRequests);
      await page.evaluate(()=>__nir.action({type:'load',slot:0}));
      await page.waitForFunction(n=>__nir.metrics.completedRequests>n,completed);
      const after=await page.evaluate(async()=>({story:slotStory(),loops:slotLoops(),state:__nir.state(),stored:await slotHost.readSaveRecord(slotDb,slotKey(0))}));
      expect(after.story).toEqual(before.story);expect(after.loops).toEqual(before.loops);
      expect(after.state.screen).toBe('Saves');expect(after.state.error).toBeNull();
      expect(after.state.status).toMatch(/Storage operation failed/);expect(after.stored).toEqual(bad);
      observations.push({variant,before,commit,after});
    }
    // Recovery is explicit fixture repair; production never deletes the bad data.
    await page.evaluate(()=>slotPut(0,{...slotGood,slot:0,envelope:{...slotGood.envelope,slot:0}}));
    await reopen(page);
    expect(await page.evaluate(()=>[...document.querySelectorAll('#actions button')].some(n=>{
      const a=JSON.parse(n.dataset.action);return a.type==='save'&&a.slot===0&&!n.disabled;
    }))).toBe(true);
    const session=await page.evaluate(()=>__nir.state().session);
    await page.evaluate(()=>__nir.action({type:'load',slot:2}));
    await page.waitForFunction(n=>__nir.state().session>n&&!__nir.state().loading&&__nir.state().screen==='Story',session);
    expect(await page.evaluate(()=>__nir.state().paused)).toBe(true);
    expect(errors).toEqual([]);
    await fs.writeFile(info.outputPath('web-slot-preservation.json'),JSON.stringify({identity,observations,restored:await page.evaluate(()=>__nir.state()),scope:'Real desktop Chromium IndexedDB and Worker/main engine; instrumented source lifetime, not physical audio or Android.'},null,2)+'\n');
  });

  test(`atomic Web save rejects same-revision tamper during async inspection; ${worker}`,async({page},info)=>{
    const identity=await boot(page,worker);
    const result=await page.evaluate(async()=>{
      const incoming={...structuredClone(slotGood.envelope),revision:2};
      const tampered={...structuredClone(slotGood),envelope:{...slotGood.envelope,digest:'0'.repeat(64)}};
      let raced=false,error=null;
      const inspect=async(json,key)=>{
        const revision=await __nir.inspectSave(json,key);
        if(revision===1&&!raced){raced=true;await slotPut(1,tampered);}return revision;
      };
      try{await slotHost.commitSaveRecord(slotDb,slotKey(1),incoming,slotGood,1,inspect);}catch(e){error=String(e);}
      const after=await slotHost.readSaveRecord(slotDb,slotKey(1));
      return {raced,error,tampered,after,state:__nir.state()};
    });
    expect(result.raced).toBe(true);expect(result.error).toMatch(/E_SAVE_CONFLICT/);
    expect(result.after).toEqual(result.tampered);expect(result.state.error).toBeNull();
    await fs.writeFile(info.outputPath('web-slot-race.json'),JSON.stringify({identity,...result},null,2)+'\n');
  });
}
