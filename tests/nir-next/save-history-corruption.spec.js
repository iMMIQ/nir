import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';

async function boot(page,worker) {
  await page.addInitScript(()=>{
    window.historySources=[];window.historyRpc={active:0,highWater:0,calls:0};
    const create=AudioContext.prototype.createBufferSource;
    AudioContext.prototype.createBufferSource=function(...args){
      const source=create.apply(this,args),row={source,stops:0};historySources.push(row);
      const stop=source.stop;source.stop=function(...args){row.stops++;return stop.apply(this,args);};return source;
    };
    const post=Worker.prototype.postMessage,workers=new WeakMap();
    Worker.prototype.postMessage=function(message,...args){
      let requests=workers.get(this);
      if(!requests){requests=new Set();workers.set(this,requests);this.addEventListener('message',event=>{
        if(requests.delete(event.data.id)){historyRpc.active--;}
      });}
      if(message.kind==='inspect-save'){
        requests.add(message.id);historyRpc.calls++;historyRpc.active++;
        historyRpc.highWater=Math.max(historyRpc.highWater,historyRpc.active);
      }return post.call(this,message,...args);
    };
  });
  await page.goto(`http://127.0.0.1:4259/?test=1&worker=${worker}&backend=webgl2`);
  await page.waitForFunction(()=>window.__nir?.state().ready&&!__nir.state().loading);
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>__nir.state().dialogue?.ready&&!__nir.state().loading);
  await page.evaluate(()=>__nir.action({type:'save',slot:1}));
  await page.waitForFunction(()=>__nir.state().status==='Saved');
  return await page.evaluate(async()=>{
    const channel=await(await fetch('/channels/stable.json')).json(),release=await(await fetch(`/releases/${channel.release}.json`)).json();
    window.archiveHost=await import(new URL(release.objects[release.engine.host].path,new URL('/',location.href)).href);
    window.archiveDb=await archiveHost.openSaveDatabase();window.archiveKey=slot=>archiveHost.saveKey(release.game_id,release.profile,channel.release,slot);
    window.archiveGood=await archiveHost.readSaveRecord(archiveDb,archiveKey(1));
    window.archivePut=(key,value)=>new Promise((ok,no)=>{const tx=archiveDb.transaction('saves','readwrite');tx.objectStore('saves').put(value,key);tx.oncomplete=ok;tx.onabort=()=>no(tx.error);});
    window.archiveStory=()=>{const s=__nir.state();return {session:s.session,tick:s.tick_us,position:s.position,interaction:s.interaction,history:s.history_count};};
    window.archiveLoops=()=>historySources.filter(r=>r.source.loop).map(r=>({stops:r.stops}));
    return {release:channel.release,gameId:release.game_id,profile:release.profile,engine:__nir.diagnostics().engine,execution:__nir.diagnostics().execution};
  });
}
async function openHistory(page) {
  await page.evaluate(()=>__nir.action({type:'menu'}));
  await expect(page.locator('#nir-history-button')).toBeVisible();
  await page.locator('#nir-history-button').click();
}
async function downloadJson(page,button) {
  const pending=page.waitForEvent('download');await button.click();const download=await pending;
  const stream=await download.createReadStream(),chunks=[];for await(const chunk of stream)chunks.push(chunk);
  return {filename:download.suggestedFilename(),json:Buffer.concat(chunks).toString()};
}
for(const worker of ['main','required']) {
  test(`archive isolates raw records and keys, exports current data and keeps healthy navigation; ${worker}`,async({page},info)=>{
    const errors=[];page.on('pageerror',e=>errors.push(e.message));
    const identity=await boot(page,worker);
    const setup=await page.evaluate(async()=>{
      const key=archiveKey(1),raw={...structuredClone(archiveGood),slot:2,version:'Broken metadata',gameId:'wrong-game'};
      await archivePut(key,{...archiveGood,version:'Healthy slot'});
      await archivePut(archiveKey(0),null);await archivePut(archiveKey(2),raw);
      const arrayKey=[key[0],key[1],['damaged'],7],shortKey=key.slice(0,2),cyclicKey=[...key.slice(0,3),9],binaryKey=[...key.slice(0,3),8];
      await archivePut(arrayKey,{version:'Array key',raw:'preserve array key'});
      await archivePut(shortKey,{version:'Short key',raw:'preserve short key'});
      const cyclic={version:'Cyclic value'};cyclic.self=cyclic;await archivePut(cyclicKey,cyclic);
      await archivePut(binaryKey,{version:'Binary value',bytes:new Uint8Array([7,9]).buffer});
      await archivePut([key[0],'another-profile',key[2],0],{version:'Excluded profile'});
      await archivePut(['another-game',key[1],key[2],0],{version:'Excluded game'});
      const entries=await archiveHost.listHistoryRecords(archiveDb,key[0],key[1]);
      return {raw,arrayKey,shortKey,cyclicKey,binaryKey,entries,metadataOnly:entries.every(e=>!('record' in e)&&!('envelope' in e))};
    });
    expect(setup.entries).toHaveLength(7);expect(setup.metadataOnly).toBe(true);
    await openHistory(page);
    const before=await page.evaluate(()=>({story:archiveStory(),loops:archiveLoops()}));
    const rows=page.locator('#nir-history-panel tbody tr');await expect(rows).toHaveCount(7);
    await expect.poll(()=>rows.locator('.nir-history-status').allTextContents()).toEqual(expect.arrayContaining(['Available']));
    await expect.poll(()=>rows.locator('.nir-history-status').allTextContents()).not.toContain('Checking…');
    const healthy=rows.filter({hasText:'Healthy slot'}),broken=rows.filter({hasText:'Broken metadata'});
    await expect(healthy.getByRole('button',{name:'Open release'})).toBeEnabled();
    await expect(broken).toContainText('Unreadable save');await expect(broken.getByRole('button',{name:'Open release'})).toBeDisabled();
    await expect(rows.filter({hasText:'Array key'})).toContainText('Unreadable save');
    await expect(rows.filter({hasText:'Short key'})).toContainText('Unreadable save');
    const nullRow=rows.filter({has:page.locator('td:nth-child(2)',{hasText:/^1$/})});
    const recovered=await downloadJson(page,nullRow.getByRole('button',{name:'Export record',exact:true}));
    expect(recovered.filename).toMatch(/\.nir-save-record\.json$/);
    expect(JSON.parse(recovered.json)).toEqual({format:'nir-save-record-recovery-v1',key:[identity.gameId,identity.profile,identity.release,0],record:null});
    const recoveredMetadata=JSON.parse((await downloadJson(page,broken.getByRole('button',{name:'Export record',exact:true}))).json);
    expect(recoveredMetadata.record).toEqual(setup.raw);
    const exported=await downloadJson(page,healthy.getByRole('button',{name:'Export',exact:true}));
    expect(exported.filename).toMatch(/\.nir-save\.json$/);
    expect(await page.evaluate(json=>__nir.inspectSave(json,archiveKey(1)),exported.json)).toBe(1);
    await rows.filter({hasText:'Cyclic value'}).getByRole('button',{name:'Export record',exact:true}).click();
    await expect(page.locator('#nir-history-panel [role=status]')).toContainText('Unable to export this record');
    await rows.filter({hasText:'Binary value'}).getByRole('button',{name:'Export record',exact:true}).click();
    await expect(page.locator('#nir-history-panel [role=status]')).toContainText('E_HISTORY_EXPORT');
    expect(await page.evaluate(()=>({story:archiveStory(),loops:archiveLoops()}))).toEqual(before);
    // A replacement after listing is exported as recovery data, not the old good save.
    await page.evaluate(()=>archivePut(archiveKey(1),{replaced:'keep the current persisted value'}));
    const replaced=await downloadJson(page,healthy.getByRole('button',{name:'Export',exact:true}));
    expect(replaced.filename).toMatch(/\.nir-save-record\.json$/);
    expect(JSON.parse(replaced.json).record).toEqual({replaced:'keep the current persisted value'});
    await healthy.getByRole('button',{name:'Open release'}).click();
    await expect(healthy).toContainText('Unreadable save');
    await expect(healthy.getByRole('button',{name:'Open release'})).toBeDisabled();
    expect(await page.evaluate(()=>({story:archiveStory(),loops:archiveLoops()}))).toEqual(before);
    const kept=await page.evaluate(async setup=>({
      bad:await archiveHost.readSaveRecord(archiveDb,archiveKey(0)),raw:await archiveHost.readSaveRecord(archiveDb,archiveKey(2)),
      array:await archiveHost.readSaveRecord(archiveDb,setup.arrayKey),short:await archiveHost.readSaveRecord(archiveDb,setup.shortKey),
      cyclic:await archiveHost.readSaveRecord(archiveDb,setup.cyclicKey).then(v=>v.self===v),
      binary:await archiveHost.readSaveRecord(archiveDb,setup.binaryKey).then(v=>[...new Uint8Array(v.bytes)]),state:__nir.state(),execution:__nir.diagnostics().execution,
    }),setup);
    expect(kept.bad).toBeNull();expect(kept.raw).toEqual(setup.raw);expect(kept.cyclic).toBe(true);expect(kept.binary).toEqual([7,9]);expect(kept.state.error).toBeNull();
    // Repair only this fixture before testing actual healthy navigation.
    await page.evaluate(()=>archivePut(archiveKey(1),{...archiveGood,version:'Healthy slot'}));
    await page.locator('#nir-history-panel').getByRole('button',{name:'关闭 / Close',exact:true}).click();
    await page.locator('#nir-history-button').click();
    await expect(healthy.getByRole('button',{name:'Open release'})).toBeEnabled();
    await healthy.getByRole('button',{name:'Open release'}).click();
    await page.waitForURL(`**/releases/${identity.release}/index.html?**`);
    await page.waitForFunction(()=>window.__nir?.state().ready&&!__nir.state().loading);
    expect(errors).toEqual([]);
    await fs.writeFile(info.outputPath('archive-isolation.json'),JSON.stringify({identity,setup,before,kept,recovered,exported,replaced,scope:'Real desktop IndexedDB/UI/export/navigation with deliberate record damage. Source lifetimes do not prove physical sound or Android.'},null,2)+'\n');
  });

  test(`large archive has bounded inspection and metadata-only listing; ${worker}`,async({page},info)=>{
    const identity=await boot(page,worker),count=260;
    await page.evaluate(async count=>{
      const key=archiveKey(1);
      await new Promise((ok,no)=>{const tx=archiveDb.transaction('saves','readwrite'),store=tx.objectStore('saves');
        for(let i=0;i<count;i++){
          const digest=i.toString(16).padStart(64,'0');
          store.put({...archiveGood,releaseDigest:digest,slot:0,version:`Archive ${i}`,envelope:{...archiveGood.envelope,slot:0}},[key[0],key[1],digest,0]);
        }tx.oncomplete=ok;tx.onabort=()=>no(tx.error);
      });
      historyRpc.highWater=historyRpc.active;historyRpc.calls=0;
    },count);
    await openHistory(page);
    const rows=page.locator('#nir-history-panel tbody tr');await expect(rows).toHaveCount(count+1);
    await expect.poll(()=>rows.locator('.nir-history-status').allTextContents(),{timeout:30000}).not.toContain('Checking…');
    const result=await page.evaluate(async()=>({rpc:{...historyRpc},entries:await archiveHost.listHistoryRecords(archiveDb,archiveKey(1)[0],archiveKey(1)[1]),state:__nir.state(),execution:__nir.diagnostics().execution}));
    expect(result.entries.every(e=>!('record' in e)&&!('envelope' in e))).toBe(true);
    expect(result.rpc.highWater).toBeLessThanOrEqual(2);expect(result.rpc.active).toBe(0);
    if(worker==='required'){expect(result.rpc.calls).toBe(count+1);expect(result.execution.runtime).toBe('worker');}
    else {expect(result.rpc.calls).toBe(0);expect(result.execution.runtime).toBe('main');}
    expect(result.state.error).toBeNull();
    await expect(rows.filter({hasText:'E_WORKER_CAPACITY'})).toHaveCount(0);
    await expect(rows.filter({hasText:'Available'})).toHaveCount(1);
    await fs.writeFile(info.outputPath('archive-bound.json'),JSON.stringify({identity,count,...result,scope:'260 deliberately unreadable synthetic release records plus a healthy real save, bounded actual RPC/read path; no physical memory/device budget or true old-package migration claim.'},null,2)+'\n');
  });
}


test('closing archive cancels queued inspections and discards late results without altering story',async({page},info)=>{
  const identity=await boot(page,'required'),count=30;
  await page.evaluate(async({count,current})=>{
    const key=archiveKey(1);
    await new Promise((ok,no)=>{const tx=archiveDb.transaction('saves','readwrite'),store=tx.objectStore('saves');
      for(let i=0;i<count;i++){
        const digest=i.toString(16).padStart(64,'0');
        store.put({...archiveGood,releaseDigest:digest,slot:0,envelope:{...archiveGood.envelope,slot:0}},[key[0],key[1],digest,0]);
      }tx.oncomplete=ok;tx.onabort=()=>no(tx.error);
    });
    window.archiveHeld=[];window.archiveHolding=true;window.archiveSent=0;
    const post=Worker.prototype.postMessage;
    Worker.prototype.postMessage=function(message,...args){
      if(message.kind==='inspect-save'&&message.value.release!==current){
        if(archiveHolding){archiveHeld.push({worker:this,message,args});return;}
        archiveSent++;
      }return post.call(this,message,...args);
    };
    window.archiveRelease=async()=>{
      archiveHolding=false;
      await Promise.all(archiveHeld.map(({worker,message,args})=>new Promise(ok=>{
        const listener=event=>{if(event.data.id===message.id){worker.removeEventListener('message',listener);setTimeout(ok,0);}};
        worker.addEventListener('message',listener);archiveSent++;post.call(worker,message,...args);
      })));
      await new Promise(requestAnimationFrame);
    };
  },{count,current:identity.release});
  await openHistory(page);
  await page.waitForFunction(()=>archiveHeld.length===2);
  const before=await page.evaluate(()=>({story:archiveStory(),loops:archiveLoops()}));
  await page.locator('#nir-history-panel').getByRole('button',{name:'关闭 / Close',exact:true}).click();
  await page.evaluate(()=>archiveRelease());
  const after=await page.evaluate(()=>({story:archiveStory(),loops:archiveLoops(),sent:archiveSent,held:archiveHeld.length,
    statuses:[...document.querySelectorAll('#nir-history-panel .nir-history-status')].map(n=>n.textContent),state:__nir.state(),execution:__nir.diagnostics().execution}));
  expect(after.sent).toBe(2);expect(after.held).toBe(2);
  expect(after.statuses).toHaveLength(count+1);expect(after.statuses.every(s=>s==='Checking…')).toBe(true);
  expect({story:after.story,loops:after.loops}).toEqual(before);expect(after.state.screen).toBe('Menu');expect(after.state.error).toBeNull();
  await page.locator('#nir-history-button').click();
  const statuses=page.locator('#nir-history-panel .nir-history-status');
  await expect.poll(()=>statuses.allTextContents(),{timeout:30000}).not.toContain('Checking…');
  await expect(page.locator('#nir-history-panel tbody tr').filter({hasText:'Available'})).toHaveCount(1);
  expect(await page.evaluate(()=>archiveSent)).toBe(count+2);
  await fs.writeFile(info.outputPath('archive-close.json'),JSON.stringify({identity,count,before,after,scope:'Actual Runtime Worker replies delayed by outgoing-message interception; close/reopen UI and story/source lifetime checks, not Android or physical sound.'},null,2)+'\n');
});
