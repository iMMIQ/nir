import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import path from 'node:path';
import {buildModulesFixture,closeModulesFixture,moduleObjectHashes} from '../browser/modules.fixture.js';
let fixture;
test.use({locale:'en-US'});
test.beforeAll(async()=>{
 fixture=await buildModulesFixture({prefetchContent:true,withMusic:true,port:4261});
 const dest=path.resolve('reports/nir-next/browser-weak-content');
 await fs.mkdir(dest,{recursive:true});await fs.cp(fixture.web,path.join(dest,'web'),{recursive:true});
 await fs.writeFile(path.join(dest,'manifest.json'),JSON.stringify({channel:fixture.channel,manifest:fixture.manifest,program:fixture.program},null,2)+'\n');
});
test.afterAll(async()=>{if(fixture)await closeModulesFixture(fixture);});
async function sample(page){return page.evaluate(()=>{const s=__nir.state(),row=contentAudio.find(r=>r.source.loop);return {
 session:s.session,interaction:s.interaction,position:s.position,tick:s.tick_us,history:s.history_count,visits:s.variables.visit_count.value,loading:s.loading,paused:s.paused,error:s.error,retrying:s.retrying,clock:s.story_clock,status:s.status,
 music:{count:contentAudio.filter(r=>r.source.loop).length,stops:row.stops,ended:row.ended,time:row.source.context.currentTime,state:row.source.context.state},
};});}
for(const worker of ['required','main'])for(const kind of ['code','text','cancel']) {
 test(`required chapter ${kind} failure retains session music and a single retry, ${worker}`,async({page},testInfo)=>{
  await page.setViewportSize({width:390,height:844});
  const hashes=moduleObjectHashes(fixture.program).ch02;
  const object=kind==='text'?hashes.locales.en:hashes.code;
  let attempts=0,releaseFirst,releaseRetry;const first=new Promise(ok=>releaseFirst=ok),retry=new Promise(ok=>releaseRetry=ok);
  await page.context().route(`**/objects/${object}.json`,async route=>{
   if(++attempts===1){await first;return route.fulfill({status:503,body:'temporarily unavailable'}).catch(()=>{});}
   await retry;await route.continue().catch(()=>{});
  });
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.addInitScript(()=>{
   window.contentAudio=[];const create=AudioContext.prototype.createBufferSource;
   AudioContext.prototype.createBufferSource=function(...args){const source=create.apply(this,args),stop=source.stop,row={source,stops:0,ended:false};contentAudio.push(row);
    source.stop=function(...args){row.stops++;return stop.apply(this,args);};source.addEventListener('ended',()=>row.ended=true);return source;};
  });
  try {
   await page.goto(`${fixture.origin}/?test=1&worker=${worker}&backend=webgl2`);
   await page.waitForFunction(()=>window.__nir?.state().ready&&!__nir.state().loading);
   await page.keyboard.press('Enter');
   await page.waitForFunction(()=>__nir.state().dialogue?.ready&&!__nir.state().loading&&contentAudio.some(r=>r.source.loop&&r.source.context.state==='running'));
   expect(await page.evaluate(()=>__nir.state().text_locale)).toBe('en');
   const initial=await sample(page);expect(initial.visits).toBe(1);
   await page.keyboard.press('Enter');
   await page.waitForFunction(()=>__nir.state().loading&&__nir.state().paused);
   await expect.poll(()=>attempts).toBe(1);
   const held=await sample(page);
   await page.waitForTimeout(650);releaseFirst();
   await page.waitForFunction(()=>__nir.state().error!==null);
   const failed=await sample(page);expect(failed.position).toEqual(held.position);expect(failed.tick).toBe(held.tick);expect(failed.visits).toBe(1);
   await page.waitForTimeout(700);
   const after=await sample(page);expect(after.music.time-failed.music.time).toBeGreaterThan(.5);expect(after.music.state).toBe('running');expect(after.music.stops).toBe(0);
   const rect=await page.evaluate(()=>JSON.parse([...document.querySelectorAll('#actions button')].find(n=>JSON.parse(n.dataset.action).type==='retry').dataset.rect));
   expect(rect[3]).toBeGreaterThanOrEqual(44);
   await page.locator('#stage').click({position:{x:rect[0]+rect[2]/2,y:rect[1]+rect[3]/2}});
   await page.waitForFunction(()=>__nir.state().loading&&__nir.state().error===null);
   await expect.poll(()=>attempts).toBe(2);
   await page.evaluate(()=>Promise.all(Array.from({length:20},()=>__nir.action({type:'retry'}))));
   expect(attempts).toBe(2);expect((await sample(page)).tick).toBe(held.tick);expect((await sample(page)).status).toBe('');
   await page.screenshot({path:testInfo.outputPath('retrying.png')});
   if(kind==='cancel') {
    await page.evaluate(()=>__nir.action({type:'title'}));
    await page.waitForFunction(s=>__nir.state().screen==='Title'&&__nir.state().session>s&&!__nir.state().loading,initial.session);
    const title=await page.evaluate(()=>({session:__nir.state().session,position:__nir.state().position}));releaseRetry();
    await page.waitForFunction(()=>__nir.metrics.activeRequests===0);
    await page.waitForTimeout(350);
    expect(await page.evaluate(()=>({session:__nir.state().session,position:__nir.state().position}))).toEqual(title);
    expect(await page.evaluate(()=>__nir.state().screen)).toBe('Title');expect(await page.evaluate(()=>__nir.state().error)).toBeNull();
    expect((await sample(page)).music.stops).toBe(1);
   } else {
    await page.evaluate(()=>{void Promise.all(Array.from({length:30},()=>__nir.action({type:'advance'})));});releaseRetry();
    await page.waitForFunction(()=>__nir.state().dialogue?.ready&&!__nir.state().loading&&!__nir.state().paused&&__nir.state().variables.visit_count.value===2);
    const ready=await sample(page);expect(ready.session).toBe(initial.session);expect(ready.error).toBeNull();expect(ready.status).toBe('');expect(ready.history).toBe(2);
    expect(ready.music.count).toBe(1);expect(ready.music.stops).toBe(0);expect(ready.music.ended).toBe(false);expect(ready.clock.paused_advance_us).toBe(0);
    await page.waitForFunction(()=>__nir.state().story_clock.first_advance_us!==null);
    expect((await sample(page)).clock.first_advance_us).toBeLessThan(250_000);
    await page.waitForTimeout(350);expect((await sample(page)).visits).toBe(2);
   }
   expect(errors).toEqual([]);
   await fs.writeFile(testInfo.outputPath('weak-content.json'),JSON.stringify({worker,kind,object,attempts,initial,held,failed,after,final:await sample(page),diagnostics:await page.evaluate(()=>__nir.diagnostics())},null,2)+'\n');
  } finally {releaseFirst();releaseRetry();await page.context().unroute(`**/objects/${object}.json`);}
 });
}
