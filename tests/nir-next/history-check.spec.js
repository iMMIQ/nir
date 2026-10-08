import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import {createServer} from 'node:http';
import {recoverAudioOutput} from './audio-output-helper.js';
const baseline=process.env.NIR_HISTORY_CHECK_BASELINE==='1';
const fixture='reports/storage-fixture/dist/full/web';
let server,port,manifest,html,httpRows=[],releaseWaiters=[],httpMode='normal';
const timeoutText='Check timed out; refresh to retry.';
test.beforeEach(async({page})=>{
 const channel=JSON.parse(await fs.readFile(`${fixture}/channels/stable.json`,'utf8'));
 manifest=await fs.readFile(`${fixture}/releases/${channel.release}.json`);html=await fs.readFile(`${fixture}/releases/${channel.release}/index.html`);
 httpRows=[];releaseWaiters=[];httpMode='normal';
 server=createServer(async(req,res)=>{
  const stage=req.url.startsWith('/manifest')?'manifest':'player-body',bytes=stage==='manifest'?manifest:html;
  const row={stage,arrived:Date.now(),closed:false,finished:false};httpRows.push(row);res.on('close',()=>{row.closed=true;row.closedAt=Date.now();});
  if(httpMode===stage){
   if(stage==='player-body'){res.writeHead(200,{'Access-Control-Allow-Origin':'*','Content-Type':'text/html'});res.write(bytes.subarray(0,20));}
   await new Promise(resolve=>releaseWaiters.push(resolve));
   if(stage==='player-body'){res.end(bytes.subarray(20));row.finished=true;return;}
  }
  res.writeHead(200,{'Access-Control-Allow-Origin':'*'});res.end(bytes);row.finished=true;
 });
 await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));port=server.address().port;
 await page.addInitScript(({port})=>{
  window.historyCheckErrors=[];window.historyCheckFetches=[];window.historyCheckAudio=[];window.historyCheckRpc=[];window.historyCheckHeld=[];window.historyCheckTerminations=0;
  const fetch=window.fetch,create=AudioContext.prototype.createBufferSource,NativeWorker=window.Worker;
  window.fetch=function(url,options){
   const parsed=new URL(typeof url==='string'||url instanceof URL?url:url.url,location.href);
   if(window.historyHttpEnabled&&/\/releases\/[a-f0-9]{64}(\.json|\/index\.html)$/.test(parsed.pathname)){
    const stage=parsed.pathname.endsWith('.json')?'manifest':'player-body',row={stage,signal:options?.signal,at:performance.now()};historyCheckFetches.push(row);
    return fetch.call(this,`http://127.0.0.1:${port}/${stage}`,options);
   }return fetch.call(this,url,options);
  };
  AudioContext.prototype.createBufferSource=function(...args){const source=create.apply(this,args),row={id:historyCheckAudio.length+1,source,context:this,stops:0};historyCheckAudio.push(row);const stop=source.stop;source.stop=function(...args){row.stops++;return stop.apply(this,args);};return source;};
  window.Worker=class extends NativeWorker{
   constructor(...args){super(...args);this.inspections=new Map();}
   set onmessage(listener){this.listener=listener;super.onmessage=event=>{
    const row=this.inspections.get(event.data.id);
    if(row&&window.holdHistoryInspections){row.replyHeld=true;historyCheckHeld.push(()=>{row.delivered=true;listener(event);});return;}
    if(row)row.delivered=true;listener(event);
   };}
   get onmessage(){return this.listener;}
   postMessage(message,...args){
    if(message.kind==='inspect-save'){const row={id:message.id,replyHeld:false,delivered:false,cancelCalls:0};this.inspections.set(message.id,row);historyCheckRpc.push(row);}
    if(message.kind==='cancel'&&this.inspections.has(message.id))this.inspections.get(message.id).cancelCalls++;
    return super.postMessage(message,...args);
   }
   terminate(){historyCheckTerminations++;return super.terminate();}
  };
  window.releaseHistoryInspections=()=>{window.holdHistoryInspections=false;for(const deliver of historyCheckHeld.splice(0))deliver();};
  window.historyCheckSnapshot=()=>{const s=__nir.state();return {state:s,story:{session:s.session,interaction:s.interaction,position:s.position,tick:s.tick_us,history:s.history_count,variables:s.variables},loops:historyCheckAudio.filter(r=>r.source.loop).map(r=>({id:r.id,stops:r.stops,clock:r.context.currentTime})),fetches:historyCheckFetches.map(r=>({stage:r.stage,aborted:r.signal?.aborted||false,at:r.at})),rpc:structuredClone(historyCheckRpc),terminations:historyCheckTerminations};};
 },{port});
 page.on('pageerror',e=>page.evaluate(message=>historyCheckErrors.push(message),e.message).catch(()=>{}));
});
test.afterEach(async({page},info)=>{
 const terminal=await page.evaluate(()=>({errors:historyCheckErrors,final:globalThis.__nir?historyCheckSnapshot():null})).catch(()=>null);
 await fs.writeFile(info.outputPath('terminal.json'),JSON.stringify({status:info.status,...terminal,httpRows},null,2)+'\n');
 for(const release of releaseWaiters)release();server.closeAllConnections();await new Promise(resolve=>server.close(resolve));
});
async function boot(page,worker,slots=[1]){
 await page.goto(`http://127.0.0.1:4259/?test=1&worker=${worker}&backend=webgl2`);await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
 await page.keyboard.press('Enter');await recoverAudioOutput(page);await page.waitForFunction(()=>__nir.state().dialogue?.ready&&!__nir.state().paused&&!__nir.state().loading);
 for(const slot of slots){await page.evaluate(slot=>__nir.action({type:'save',slot}),slot);await page.waitForFunction(()=>__nir.state().status==='Saved');}
 await page.evaluate(async()=>{
  const channel=await(await fetch('/channels/stable.json')).json(),release=await(await fetch(`/releases/${channel.release}.json`)).json();
  window.historyHost=await import('/'+release.objects[release.engine.host].path);window.historyDb=await historyHost.openSaveDatabase();window.historyIdentity={game:release.game_id,profile:release.profile,release:channel.release};
  window.historyKey=slot=>historyHost.saveKey(historyIdentity.game,historyIdentity.profile,historyIdentity.release,slot);
  window.historyGood=await historyHost.readSaveRecord(historyDb,historyKey(1));
  window.putHistoryRecord=(key,value)=>new Promise((ok,no)=>{const tx=historyDb.transaction('saves','readwrite');tx.objectStore('saves').put(value,key);tx.oncomplete=ok;tx.onabort=()=>no(tx.error);});
 });
 await page.evaluate(()=>__nir.action({type:'close'}));await page.waitForFunction(()=>__nir.state().screen==='Story');await page.evaluate(()=>__nir.action({type:'menu'}));await page.waitForFunction(()=>__nir.state().screen==='Menu');
 expect(await page.evaluate(()=>__nir.state().execution.runtime)).toBe(worker==='required'?'worker':'main');
}
const snapshot=page=>page.evaluate(()=>historyCheckSnapshot());
async function stable(page,before){const after=await snapshot(page);expect(after.story).toEqual(before.story);expect(after.loops.map(({id,stops})=>({id,stops}))).toEqual(before.loops.map(({id,stops})=>({id,stops})));expect(after.loops[0].clock).toBeGreaterThan(before.loops[0].clock);expect(after.state.error).toBeNull();return after;}
async function originalKept(page){expect(await page.evaluate(()=>historyHost.readSaveRecord(historyDb,historyKey(1)))).toEqual(await page.evaluate(()=>historyGood));}
async function record(page,info,name,data){await fs.writeFile(info.outputPath(name+'.json'),JSON.stringify({baseline,...data,final:await snapshot(page),httpRows},null,2)+'\n');}
async function open(page){await page.locator('#nir-history-button').click();}
async function normalRefresh(page){httpMode='normal';await page.evaluate(()=>{window.historyHttpEnabled=false;});for(const release of releaseWaiters.splice(0))release();await page.locator('#nir-history-refresh').click();await expect(page.locator('#nir-history-panel .nir-history-status')).toHaveText('Available');}
for(const worker of ['main','required'])for(const stage of ['manifest','player-body']){
 test(`history resource deadline cancels ${stage} and refresh recovers; ${worker}`,async({page},info)=>{
  await boot(page,worker);httpMode=stage;await page.evaluate(()=>window.historyHttpEnabled=true);const before=await snapshot(page);const start=Date.now();await open(page);
  await expect.poll(()=>httpRows.filter(r=>r.stage===stage).length).toBe(1);
  if(baseline){await page.waitForTimeout(3500);await expect(page.locator('#nir-history-refresh')).toBeDisabled();await expect(page.locator('.nir-history-status')).toHaveText('Checking…');expect(httpRows.find(r=>r.stage===stage).closed).toBe(false);await stable(page,before);await record(page,info,'resource-deadline',{worker,stage,before,pendingAfter3500ms:true});return;}
  await expect(page.locator('.nir-history-status')).toContainText(timeoutText);await expect(page.locator('#nir-history-refresh')).toBeEnabled();await expect.poll(()=>httpRows.find(r=>r.stage===stage).closed).toBe(true);
  const failed=await stable(page,before);expect(failed.fetches.find(r=>r.stage===stage).aborted).toBe(true);expect(Date.now()-start).toBeLessThan(5000);await originalKept(page);
  await normalRefresh(page);await stable(page,before);await record(page,info,'resource-deadline',{worker,stage,before,failed});
 });
 test(`closing history cancels ${stage} before deadline and reopened rows stay current; ${worker}`,async({page},info)=>{
  await boot(page,worker);httpMode=stage;await page.evaluate(()=>window.historyHttpEnabled=true);const before=await snapshot(page);await open(page);await expect.poll(()=>httpRows.filter(r=>r.stage===stage).length).toBe(1);
  await page.locator('#nir-history-panel').getByRole('button',{name:'关闭 / Close',exact:true}).click();await expect.poll(()=>httpRows.find(r=>r.stage===stage).closed).toBe(true);
  const cancelled=await stable(page,before);expect(cancelled.fetches.find(r=>r.stage===stage).aborted).toBe(true);
  httpMode='normal';await page.evaluate(()=>window.historyHttpEnabled=false);await open(page);await expect(page.locator('.nir-history-status')).toHaveText('Available');for(const release of releaseWaiters.splice(0))release();await expect(page.locator('.nir-history-status')).toHaveText('Available');await originalKept(page);await stable(page,before);
  await record(page,info,'resource-close',{worker,stage,before,cancelled});
 });
}
for(const worker of ['main','required']){
 test(`three saved slots share verification of one release; ${worker}`,async({page},info)=>{
  await boot(page,worker,[1,0,2]);await page.evaluate(()=>window.historyHttpEnabled=true);const before=await snapshot(page);await open(page);await expect(page.locator('.nir-history-status')).toHaveText(['Available','Available','Available']);await expect(page.locator('#nir-history-refresh')).toBeEnabled();
  expect(httpRows.map(r=>r.stage).sort()).toEqual(['manifest','player-body']);await originalKept(page);await stable(page,before);await record(page,info,'shared-release',{worker,before});
 });
 test(`database admission delay and resource fetch consume one row budget; ${worker}`,async({page},info)=>{
  await boot(page,worker);await page.evaluate(()=>{const run=historyHost.SaveDatabaseConnection.prototype.run;let calls=0;historyHost.SaveDatabaseConnection.prototype.run=async function(work){if(++calls===2)await new Promise(r=>setTimeout(r,1800));return run.call(this,work);};window.historyHttpEnabled=true;});
  httpMode='manifest';const before=await snapshot(page),start=Date.now();await open(page);await expect.poll(()=>httpRows.length).toBe(1);await expect(page.locator('.nir-history-status')).toContainText(timeoutText);await expect(page.locator('#nir-history-refresh')).toBeEnabled();
  const elapsed=Date.now()-start;expect(elapsed).toBeLessThan(4300);expect(elapsed).toBeGreaterThan(2600);await expect.poll(()=>httpRows[0].closed).toBe(true);await originalKept(page);await stable(page,before);await normalRefresh(page);await record(page,info,'combined-budget',{worker,before,elapsedMs:elapsed});
 });
}
test('late inspection replies do not export or navigate; refresh cancels an export; required',async({page},info)=>{
 await boot(page,'required');const before=await snapshot(page);await page.evaluate(()=>window.holdHistoryInspections=true);await open(page);await expect.poll(()=>page.evaluate(()=>historyCheckHeld.length)).toBeGreaterThan(0);
 await expect(page.locator('.nir-history-status')).toContainText(timeoutText);await expect(page.locator('#nir-history-refresh')).toBeEnabled();const failed=await stable(page,before);expect(failed.terminations).toBe(0);expect(failed.rpc.filter(r=>r.replyHeld&&!r.delivered)).toHaveLength(1);expect(failed.rpc.at(-1).cancelCalls).toBe(1);
 await page.evaluate(()=>releaseHistoryInspections());await expect(page.locator('.nir-history-status')).toContainText(timeoutText);await page.locator('#nir-history-refresh').click();await expect(page.locator('.nir-history-status')).toHaveText('Available');
 let downloads=0;page.on('download',()=>downloads++);await page.evaluate(()=>window.holdHistoryInspections=true);
 await page.locator('#nir-history-panel').getByRole('button',{name:'Export',exact:true}).click();await expect.poll(()=>page.evaluate(()=>historyCheckHeld.length)).toBeGreaterThan(0);
 await page.locator('#nir-history-refresh').click();await page.evaluate(()=>releaseHistoryInspections());await expect(page.locator('.nir-history-status')).toHaveText('Available');expect(downloads).toBe(0);
 await page.evaluate(()=>window.holdHistoryInspections=true);const url=page.url();await page.locator('#nir-history-panel').getByRole('button',{name:'Open release'}).click();await expect(page.locator('.nir-history-status')).toContainText(timeoutText);await expect(page.locator('#nir-history-panel').getByRole('button',{name:'Open release'})).toBeEnabled();await page.evaluate(()=>releaseHistoryInspections());expect(page.url()).toBe(url);expect(downloads).toBe(0);await originalKept(page);await stable(page,before);await record(page,info,'late-inspection',{worker:'required',before,failed,downloads,navigationUnchanged:true});
});
test('whole history refresh stops at its deadline and physical inspections stay bounded; required',async({page},info)=>{
 await boot(page,'required');await page.evaluate(async()=>{for(let i=0;i<10;i++){const key=[historyIdentity.game,historyIdentity.profile,i.toString(16).padStart(64,'0'),1];await putHistoryRecord(key,{...structuredClone(historyGood),version:'Uninspected '+i,releaseDigest:key[2]});}window.holdHistoryInspections=true;});
 const before=await snapshot(page),start=Date.now();await open(page);await expect(page.locator('#nir-history-panel [role=status]')).toContainText(timeoutText,{timeout:14000});await expect(page.locator('#nir-history-refresh')).toBeEnabled();
 const elapsed=Date.now()-start;expect(elapsed).toBeLessThan(12000);expect(elapsed).toBeGreaterThan(9300);await expect.poll(()=>page.locator('.nir-history-status').allTextContents()).not.toContain('Checking…');
 const failed=await stable(page,before);expect(failed.rpc.filter(r=>r.replyHeld&&!r.delivered)).toHaveLength(2);expect(failed.terminations).toBe(0);
 await page.evaluate(()=>releaseHistoryInspections());await page.locator('#nir-history-refresh').click();await expect(page.locator('.nir-history-status',{hasText:'Available'})).toHaveCount(1);await expect.poll(()=>page.locator('.nir-history-status').allTextContents()).not.toContain('Checking…');await originalKept(page);await stable(page,before);await record(page,info,'whole-budget',{worker:'required',before,failed,elapsedMs:elapsed});
});
