import {test,expect} from '@playwright/test';

// The intro cue starts a looping chime and ramps its envelope 1 -> 0.25 over
// 2 s with a linear gain tween. The host applies that as one scheduled
// linearRampToValueAtTime on the voice's dedicated envelope node — separate
// from the event-gain and bus nodes — so the audit can watch the param move
// while the loop itself never stops.
async function start(page){
 await page.addInitScript(()=>{
  window.audioAudit={ramps:[],sources:[],gains:[]};
  const create=AudioContext.prototype.createGain;
  AudioContext.prototype.createGain=function(){const n=create.call(this);window.audioAudit.gains.push(n);window.audioAudit.context=this;return n;};
  const ramp=AudioParam.prototype.linearRampToValueAtTime;
  AudioParam.prototype.linearRampToValueAtTime=function(value,time){window.audioAudit.ramps.push({param:this,value,time});return ramp.call(this,value,time);};
  const source=AudioContext.prototype.createBufferSource;
  AudioContext.prototype.createBufferSource=function(){const n=source.call(this);window.audioAudit.sources.push(n);n.addEventListener('ended',()=>{n.ended=true;});return n;};
 });
 await page.goto('http://127.0.0.1:4226/?test=1&backend=webgl2',{waitUntil:'domcontentloaded'}).catch(e=>{if(!e.message.includes('interrupted'))throw e;});
 await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
 await page.keyboard.press('Enter');
 await page.waitForFunction(()=>window.audioAudit.ramps.length>0);
}
test('a gain tween ramps the instance envelope and keeps playback alive',async({page})=>{
 const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await start(page);
 // Exactly the tween's target lands on an envelope node: one scheduled ramp
 // toward 0.25, distinct from every event gain and bus volume. The live
 // AudioParam never crosses the evaluate boundary — address it by index.
 await page.waitForFunction(()=>window.audioAudit.ramps.some(r=>Math.abs(r.value-.25)<.001));
 const ramp=await page.evaluate(()=>window.audioAudit.ramps.findIndex(r=>Math.abs(r.value-.25)<.001));
 await page.waitForFunction(i=>{const v=window.audioAudit.ramps[i].param.value;return v<.9&&v>.3;},ramp);
 const mid=await page.evaluate(i=>window.audioAudit.ramps[i].param.value,ramp);
 expect(mid).toBeGreaterThan(.3);
 expect(mid).toBeLessThan(.9);
 await page.waitForFunction(i=>Math.abs(window.audioAudit.ramps[i].param.value-.25)<.03,ramp,{timeout:8000});
 // The looping voice never stopped while its envelope moved.
 expect(await page.evaluate(()=>window.audioAudit.sources.some(s=>s.loop&&s.ended!==true))).toBe(true);
 expect(await page.evaluate(()=>window.__nir.state().error)).toBeFalsy();
 expect(errors).toEqual([]);
});
