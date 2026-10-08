import {test,expect} from '@playwright/test';

// Observe the actual device clock/source while a required image fetch is held.
// A prefetch may reach the gate early; only sample once the page promotes it
// into a required preparation, so speculative loading alone cannot pass.
test('required page loading preserves music while explicit pauses still freeze it',async({page,request})=>{
 const channel=await (await request.get('channels/stable.json')).json();
 const release=await (await request.get(`releases/${channel.release}.json`)).json();
 const index=await (await request.get(release.objects[release.program].path)).json();
 const image=release.objects[index.program.assets['actor.aki'].object].path;
 let releaseImage,intercepted=false;
 const gate=new Promise(resolve=>releaseImage=resolve);
 await page.context().route(`**/${image}`,async route=>{intercepted=true;await gate;await route.continue().catch(()=>{});});
 await page.addInitScript(()=>{
  globalThis.audioLoadingAudit={sources:[],suspends:[]};
  const create=AudioContext.prototype.createBufferSource;
  AudioContext.prototype.createBufferSource=function(...args){
   const source=create.apply(this,args);audioLoadingAudit.sources.push(source);
   source.addEventListener('ended',()=>{source.auditEnded=true;});return source;
  };
  const suspend=AudioContext.prototype.suspend;
  AudioContext.prototype.suspend=function(...args){audioLoadingAudit.suspends.push({context:this,time:performance.now()});return suspend.apply(this,args);};
 });
 const sample=()=>page.evaluate(()=>{
  const s=__nir.state(),a=audioLoadingAudit,source=a.sources.find(n=>n.loop);
  return {loading:s.loading,paused:s.paused,tick:s.tick_us,error:s.error,deviceState:source?.context.state,deviceTime:source?.context.currentTime,ended:source?.auditEnded===true,loops:a.sources.filter(n=>n.loop).length,suspends:a.suspends.filter(n=>n.context===source?.context).length};
 });
 try {
  await page.goto('/?test=1&backend=webgl2');
  await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
  await page.keyboard.press('Space');
  await page.waitForFunction(()=>!!__nir.state().dialogue&&!__nir.state().loading);
  await page.waitForFunction(()=>audioLoadingAudit.sources.some(n=>n.loop&&n.context.state==='running'));
  const initial=await sample();
  await page.evaluate(()=>__nir.action({type:'advance'}));
  await page.evaluate(()=>__nir.action({type:'advance'}));
  await page.waitForFunction(()=>__nir.state().loading&&__nir.state().paused);
  await expect.poll(()=>intercepted).toBe(true);
  const before=await sample();
  expect(before.deviceState).toBe('running');
  await page.waitForTimeout(1100);
  const after=await sample();
  expect(after.loading).toBe(true);expect(after.tick).toBe(before.tick);
  expect(after.deviceTime-before.deviceTime).toBeGreaterThan(.8);
  expect(after.loops).toBe(initial.loops);expect(after.ended).toBe(false);
  expect(after.suspends).toBe(initial.suspends);
  await page.evaluate(()=>__nir.action({type:'menu'}));
  await page.waitForFunction(()=>__nir.state().screen==='Menu');
  const menu=await sample();await page.waitForTimeout(200);
  expect((await sample()).deviceTime-menu.deviceTime).toBeGreaterThan(.1);
  await page.evaluate(()=>__nir.hidden(true));
  await page.waitForFunction(()=>audioLoadingAudit.sources.find(n=>n.loop)?.context.state==='suspended');
  const paused=await sample();await page.waitForTimeout(200);
  expect((await sample()).deviceTime).toBe(paused.deviceTime);
  await page.evaluate(()=>__nir.action({type:'close'}));
  expect((await sample()).deviceState).toBe('suspended');
  await page.evaluate(()=>__nir.hidden(false));
  await page.waitForFunction(()=>audioLoadingAudit.sources.find(n=>n.loop)?.context.state==='running');
  expect((await sample()).loading).toBe(true);
  releaseImage();
  await page.waitForFunction(()=>!__nir.state().loading&&!__nir.state().paused);
  const ready=await sample();
  expect(ready.deviceState).toBe('running');expect(ready.loops).toBe(initial.loops);
  expect(ready.ended).toBe(false);expect(ready.error).toBeNull();
  await page.evaluate(()=>__nir.action({type:'title'}));
  await page.waitForFunction(()=>__nir.state().screen==='Title'&&!__nir.state().loading);
  await page.waitForFunction(()=>audioLoadingAudit.sources.filter(n=>n.loop).every(n=>n.auditEnded));
 }finally{releaseImage();if(!page.isClosed())await page.context().unroute(`**/${image}`);}
});
