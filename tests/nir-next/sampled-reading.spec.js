import {test,expect} from '@playwright/test';

test('sampled Auto retains the device remainder after early voice completion',async({page})=>{
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.addInitScript(()=>{
    window.voiceAudit=[];
    const create=AudioContext.prototype.createBufferSource;
    AudioContext.prototype.createBufferSource=function(){
      const source=create.call(this),start=source.start,context=this;
      source.start=function(when,offset=0,...rest){
        if(!source.loop)window.voiceAudit.push({source,context,started:context.currentTime,offset,duration:source.buffer.duration});
        return start.call(this,when,offset,...rest);
      };
      return source;
    };
  });
  await page.goto('http://127.0.0.1:4210/?test=1&backend=webgl2',{waitUntil:'domcontentloaded'});
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>window.__nir.state().dialogue&&!window.__nir.state().loading&&window.voiceAudit.length);
  const before=await page.evaluate(async()=>{
    await window.__nir.action({type:'advance'});
    const s=window.__nir.state(),v=window.voiceAudit.at(-1);
    const remaining=Math.max(0,v.duration-(v.context.currentTime-v.started+v.offset));
    await window.__nir.action({type:'toggle_auto'});
    v.source.stop();
    return {interaction:s.interaction,tick:Number(s.tick_us),remaining};
  });
  expect(before.remaining).toBeGreaterThan(.2);
  await page.waitForFunction(i=>window.__nir.state().interaction!==i,before.interaction);
  const elapsed=await page.evaluate(t=>(Number(window.__nir.state().tick_us)-t)/1e6,before.tick);
  expect(elapsed).toBeGreaterThanOrEqual(before.remaining+.5-.15);
  expect(elapsed).toBeLessThan(before.remaining+.5+.45);
  expect(errors).toEqual([]);
});
