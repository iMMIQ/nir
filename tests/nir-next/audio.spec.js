import {test,expect} from '@playwright/test';
import {recoverAudioOutput} from './audio-output-helper.js';
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
 await page.goto('/?test=1&backend=webgl2',{waitUntil:'domcontentloaded'}).catch(e=>{if(!e.message.includes('interrupted'))throw e;});
 await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
 await page.keyboard.press('Enter');
 await page.waitForFunction(()=>window.audioAudit.ramps.length>0);
}
test('event gain, continuous stop envelope, pause and restored remaining segment',async({page})=>{
 const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await start(page);
 await page.waitForFunction(()=>{const p=window.audioAudit.ramps[0].param;return p.value<.9&&p.value>.1;});
 // The audio device continues while the main thread is stalled; input pauses
 // Story before it catches up. Background suspension freezes all buses; saving
 // must preserve the audible ramp, not rewind it.
 await page.evaluate(()=>{const until=performance.now()+800;while(performance.now()<until){}window.__nir.action({type:'menu'});window.__nir.hidden(true);});
 await page.waitForFunction(()=>window.__nir.state().screen==='Menu'&&window.audioAudit.context.state==='suspended');
 const value=await page.evaluate(()=>window.audioAudit.ramps[0].param.value);
 const appearance=await page.evaluate(()=>window.__nir.state().dialogue_appearance);
 await page.waitForTimeout(250);
 expect(await page.evaluate(()=>window.audioAudit.ramps[0].param.value)).toBeCloseTo(value,3);
 expect(await page.evaluate(()=>window.audioAudit.gains.some(n=>Math.abs(n.gain.value-.45)<.001))).toBe(true);
 await page.evaluate(()=>window.__nir.action({type:'volume',bus:'bgm',delta:.2}));
 await page.waitForFunction(()=>window.audioAudit.gains.some(n=>Math.abs(n.gain.value-.75)<.001));
 expect(await page.evaluate(()=>window.audioAudit.context.state)).toBe('suspended');
 await page.evaluate(()=>window.__nir.action({type:'save',slot:1}));
 // Wait on a completed storage transaction through the committed IndexedDB record.
 await expect.poll(()=>page.evaluate(async()=>{
  const databases=await indexedDB.databases();
  for(const d of databases){
   const db=await new Promise((ok,no)=>{const r=indexedDB.open(d.name);r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});
   if(db.objectStoreNames.contains('saves')){
    const count=await new Promise((ok,no)=>{const r=db.transaction('saves').objectStore('saves').count();r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});db.close();if(count)return true;
   }else db.close();
  }return false;
 })).toBe(true);
 await page.evaluate(()=>window.__nir.action({type:'load',slot:1}));
 await page.waitForFunction(()=>window.audioAudit.ramps.length>=2&&!window.__nir.state().loading);
 const restored=await page.evaluate(()=>window.audioAudit.ramps.at(-1).param.value);
 expect(restored).toBeGreaterThan(0);expect(restored).toBeLessThan(.95);
 expect(Math.abs(restored-value)).toBeLessThan(.01);
 const restoredAppearance=await page.evaluate(()=>window.__nir.state().dialogue_appearance);
 expect(Math.abs(restoredAppearance.opacity-appearance.opacity)).toBeLessThan(.03);
 expect(restoredAppearance.background_opacity).toBe(appearance.background_opacity);
 expect(restoredAppearance.text_opacity).toBe(appearance.text_opacity);
 await page.evaluate(()=>window.__nir.hidden(false));
 await page.evaluate(()=>window.__nir.action({type:'close'}));
 await page.evaluate(()=>window.__nir.action({type:'continue'}));
 await recoverAudioOutput(page);
 await page.waitForFunction(()=>window.audioAudit.sources.at(-1).ended===true,{},{timeout:15000});
 expect(await page.evaluate(()=>window.__nir.state().error)).toBeFalsy();
 expect(errors).toEqual([]);
});
test('leaving the session terminates an in-flight fade and rejects old completions',async({page})=>{
 await start(page);
 await page.evaluate(()=>window.__nir.action({type:'title'}));
 await page.waitForFunction(()=>window.__nir.state().screen==='Title'&&!window.__nir.state().loading);
 await page.waitForFunction(()=>window.audioAudit.sources.every(s=>s.ended));
 await page.waitForTimeout(100);
 expect(await page.evaluate(()=>window.__nir.state().error)).toBeFalsy();
});
