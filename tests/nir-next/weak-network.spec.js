import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
const origin='http://127.0.0.1:4260';
test.use({locale:'zh-CN'});
async function sample(page) {
 return page.evaluate(()=>{
  const s=__nir.state(),row=networkAudio.find(r=>r.source.loop),ambient=networkAudio.filter(r=>r.source.loop)[1];
  return {screen:s.screen,session:s.session,interaction:s.interaction,tick:s.tick_us,history:s.history_count,position:s.position,loading:s.loading,paused:s.paused,error:s.error,clock:s.story_clock,status:s.status,
   ambience:{stops:ambient.stops,ended:ambient.ended,state:ambient.source.context.state,time:ambient.source.context.currentTime},
   music:{count:networkAudio.filter(r=>r.source.loop).length,stops:row.stops,ended:row.ended,state:row.source.context.state,time:row.source.context.currentTime}};
 });
}
async function pixels(page,path) {
 const screenshot=await page.screenshot({path});
 return page.evaluate(async base64=>{
  const image=await createImageBitmap(new Blob([Uint8Array.from(atob(base64),c=>c.charCodeAt(0))],{type:'image/png'}));
  const canvas=document.createElement('canvas');canvas.width=image.width;canvas.height=image.height;
  const ctx=canvas.getContext('2d');ctx.drawImage(image,0,0);image.close();
  return Array.from(ctx.getImageData(8,Math.floor(canvas.height*.5),1,1).data);
 },screenshot.toString('base64'));
}
async function canvasRetry(page) {
 const rect=await page.evaluate(()=>{const n=[...document.querySelectorAll('#actions button')].find(n=>JSON.parse(n.dataset.action).type==='retry');return n?JSON.parse(n.dataset.rect):null;});
 expect(rect).not.toBeNull();expect(rect[2]).toBeGreaterThanOrEqual(44);expect(rect[3]).toBeGreaterThanOrEqual(44);
 await page.locator('#stage').click({position:{x:rect[0]+rect[2]/2,y:rect[1]+rect[3]/2}});
}
for(const worker of ['required','main'])for(const failure of ['delay','http','offline','integrity','cancel']) {
 test(`required scene ${failure} preserves old pixels/music and retries without queued advances, ${worker}`,async({page,request},testInfo)=>{
  await page.setViewportSize({width:390,height:844});
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  const channel=await (await request.get(`${origin}/channels/stable.json`)).json();
  const release=await (await request.get(`${origin}/releases/${channel.release}.json`)).json();
  // Asset catalogs are separate hashed module-static objects in a lazy root.
  const records=await Promise.all(Object.values(release.objects).filter(d=>d.media_type==='application/json').map(async d=>(await request.get(`${origin}/${d.path}`)).json()));
  const findAsset=(v)=>{
   if(v&&typeof v==='object') {
    if(v['actor.aki']?.object)return v['actor.aki'].object;
    for(const child of Object.values(v)){const found=findAsset(child);if(found)return found;}
   }
  };
  const object=records.map(findAsset).find(Boolean);expect(object).toBeTruthy();
  const path=release.objects[object].path;
  let attempts=0,releaseFirst,releaseRetry;
  const first=new Promise(ok=>releaseFirst=ok),retry=new Promise(ok=>releaseRetry=ok);
  await page.context().route(`**/${path}`,async route=>{
   const attempt=++attempts;
   if(attempt===1) {
    await first;
    if(failure==='http'||failure==='cancel')return route.fulfill({status:503,body:'temporary failure'}).catch(()=>{});
    if(failure==='offline')return route.abort('internetdisconnected').catch(()=>{});
    if(failure==='integrity')return route.fulfill({status:200,contentType:'image/webp',body:Buffer.from('invalid asset bytes')}).catch(()=>{});
   } else await retry;
   await route.continue().catch(()=>{});
  });
  await page.addInitScript(()=>{
   window.networkAudio=[];const create=AudioContext.prototype.createBufferSource;
   AudioContext.prototype.createBufferSource=function(...args){
    const source=create.apply(this,args),stop=source.stop,row={source,stops:0,ended:false};networkAudio.push(row);
    source.stop=function(...args){row.stops++;return stop.apply(this,args);};source.addEventListener('ended',()=>row.ended=true);return source;
   };
  });
  try {
   await page.goto(`${origin}/?test=1&worker=${worker}&backend=webgl2`);
   await page.waitForFunction(()=>window.__nir?.state().ready&&!__nir.state().loading);
   await page.keyboard.press('Enter');
   await page.waitForFunction(()=>__nir.state().dialogue?.ready&&!__nir.state().loading&&networkAudio.filter(r=>r.source.loop).length===2&&networkAudio.filter(r=>r.source.loop).every(r=>r.source.context.state==='running'));
   const initial=await sample(page),red=await pixels(page,testInfo.outputPath('initial.png'));
   expect(red.slice(0,3)).toEqual([255,0,0]);
   await page.keyboard.press('Enter');
   await page.waitForFunction(()=>__nir.state().loading&&__nir.state().paused);
   await expect.poll(()=>attempts).toBe(1);
   const held=await sample(page);
   await page.evaluate(()=>Promise.all(Array.from({length:40},()=>__nir.action({type:'advance'}))));
   await page.waitForTimeout(1000);
   const waiting=await sample(page);
   expect(waiting.tick).toBe(held.tick);expect(waiting.position).toEqual(held.position);expect(waiting.history).toBe(held.history);
   expect(waiting.music.state).toBe('running');expect(waiting.music.time-held.music.time).toBeGreaterThan(.8);
   expect(waiting.ambience.state).toBe('running');expect(waiting.ambience.time-held.ambience.time).toBeGreaterThan(.8);expect(waiting.ambience.stops).toBe(0);expect(waiting.ambience.ended).toBe(false);
   expect(waiting.music.stops).toBe(0);expect(waiting.music.ended).toBe(false);expect(waiting.music.count).toBe(initial.music.count);
   expect(await pixels(page,testInfo.outputPath('waiting.png'))).toEqual(red);
   releaseFirst();
   let failed=null,retrying=null,cancelledTitle=null,titlePixel=null;
   if(failure!=='delay') {
    await page.waitForFunction(()=>__nir.state().error!==null);
    failed=await sample(page);await page.waitForTimeout(900);
    const after=await sample(page);
    expect(after.tick).toBe(held.tick);expect(after.music.time-failed.music.time).toBeGreaterThan(.6);expect(after.music.state).toBe('running');
    expect(after.ambience.state).toBe('running');expect(after.ambience.time-failed.ambience.time).toBeGreaterThan(.6);expect(after.ambience.stops).toBe(0);
    expect(after.music.stops).toBe(0);expect(after.music.ended).toBe(false);expect(after.position).toEqual(held.position);
    expect(await pixels(page,testInfo.outputPath('failed.png'))).toEqual(red);
    await canvasRetry(page);
    await expect.poll(()=>attempts).toBe(2);
    await page.waitForFunction(()=>__nir.state().retrying);
    expect(await page.evaluate(()=>[...document.querySelectorAll('#actions button')].find(n=>JSON.parse(n.dataset.action).type==='retry').disabled)).toBe(true);
    await page.evaluate(()=>Promise.all(Array.from({length:20},()=>__nir.action({type:'retry'}))));
    expect(attempts).toBe(2);
    retrying=await sample(page);expect(retrying.status).toBe('');
    await page.screenshot({path:testInfo.outputPath('retrying.png')});
    await page.waitForTimeout(400);
    expect((await sample(page)).tick).toBe(held.tick);
    if(failure==='cancel') {
     await page.evaluate(()=>__nir.action({type:'title'}));
     await page.waitForFunction(s=>__nir.state().screen==='Title'&&__nir.state().session>s&&!__nir.state().loading,initial.session);
     await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
     cancelledTitle=await sample(page);
     titlePixel=await pixels(page,testInfo.outputPath('title-before-release.png'));
    } else await page.evaluate(()=>{void Promise.all(Array.from({length:30},()=>__nir.action({type:'advance'})));});
    releaseRetry();
   }
   let ready,blue=null;
   if(failure==='cancel') {
    const title=cancelledTitle;
    await page.waitForFunction(()=>__nir.metrics.activeRequests===0);
    await page.waitForTimeout(350);ready=await sample(page);
    expect(ready.screen).toBe('Title');expect(ready.session).toBe(title.session);expect(ready.position).toEqual(title.position);
    expect(ready.history).toBe(title.history);expect(ready.tick).toBe(title.tick);expect(ready.error).toBeNull();expect(ready.music.count).toBe(initial.music.count);expect(ready.music.stops).toBe(1);expect(ready.ambience.stops).toBe(1);
    expect(await pixels(page,testInfo.outputPath('title.png'))).toEqual(titlePixel);
   } else {
   await page.waitForFunction(()=>!__nir.state().loading&&!__nir.state().paused&&__nir.state().history_count===2);
   ready=await sample(page);
   expect(ready.status).toBe('');expect(ready.session).toBe(initial.session);expect(ready.error).toBeNull();expect(ready.music.count).toBe(initial.music.count);expect(ready.ambience.state).toBe('running');expect(ready.ambience.stops).toBe(0);expect(ready.ambience.ended).toBe(false);expect(ready.music.stops).toBe(0);expect(ready.music.ended).toBe(false);
   expect(ready.clock.paused_advance_us).toBe(0);
   await page.waitForFunction(()=>__nir.state().story_clock.first_advance_us!==null);
   expect((await sample(page)).clock.first_advance_us).toBeLessThan(250_000);
   blue=await pixels(page,testInfo.outputPath('ready.png'));expect(blue.slice(0,3)).toEqual([0,0,255]);
   await page.waitForTimeout(350);expect((await sample(page)).history).toBe(2);
   }
   expect(errors).toEqual([]);
   await fs.writeFile(testInfo.outputPath('weak-network.json'),JSON.stringify({worker,failure,attempts,initial,held,waiting,failed,retrying,cancelledTitle,ready,red,blue,titlePixel,diagnostics:await page.evaluate(()=>__nir.diagnostics())},null,2)+'\n');
  } finally {releaseFirst();releaseRetry();await page.context().unroute(`**/${path}`);}
 });
}
