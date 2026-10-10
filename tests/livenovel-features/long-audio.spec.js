import {test,expect} from '@playwright/test';
import {recoverAudioOutput} from '../nir-next/audio-output-helper.js';

for(const worker of ['required','main']) {
  test(`nine minute MP3 fits admission and keeps its source across menu pause; ${worker}`,async({page})=>{
    test.skip(!process.env.NIR_TEST_LONG_AUDIO_SOURCE,'Private long-audio experiment');
    const errors=[];page.on('pageerror',e=>errors.push(e.message));
    await page.addInitScript(()=>{
      window.longAudioStarts=[];
      const start=AudioBufferSourceNode.prototype.start;
      AudioBufferSourceNode.prototype.start=function(...args){
        if(this.buffer?.duration>530) window.longAudioStarts.push({duration:this.buffer.duration,channels:this.buffer.numberOfChannels,rate:this.buffer.sampleRate,bytes:this.buffer.length*this.buffer.numberOfChannels*4});
        return start.apply(this,args);
      };
    });
    await page.goto(`http://127.0.0.1:4268/?test=1&worker=${worker}&backend=webgl2`);
    await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    await page.getByRole('button',{name:'Start',exact:true}).focus();await page.keyboard.press('Enter');
    await page.waitForFunction(()=>__nir.state().error||(__nir.state().dialogue?.id==='intro'&&!__nir.state().loading),null,{timeout:60000});
    expect(await page.evaluate(()=>__nir.state().diagnostic)).toBeNull();
    await recoverAudioOutput(page);
    const started=await page.evaluate(()=>window.longAudioStarts);
    expect(started).toHaveLength(1);
    expect(started[0].duration).toBeCloseTo(539.45469,3);
    expect(started[0].channels).toBe(2);
    expect(started[0].bytes).toBeGreaterThan(180*1024*1024);
    expect(await page.evaluate(()=>__nir.state().resident_bytes)).toBeLessThan(256*1024*1024);
    await page.evaluate(()=>__nir.action({type:'menu'}));
    await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
    const tick=await page.evaluate(()=>__nir.state().tick_us);
    await page.waitForTimeout(100);
    expect(await page.evaluate(()=>__nir.state().tick_us)).toBe(tick);
    await page.evaluate(()=>__nir.action({type:'close'}));
    await page.waitForFunction(()=>__nir.state().screen==='Story'&&!__nir.state().loading);
    await recoverAudioOutput(page);
    expect(await page.evaluate(()=>window.longAudioStarts)).toEqual(started);
    const largeBytes=await page.evaluate(()=>__nir.state().resident_bytes);
    await page.setViewportSize({width:640,height:360});
    await expect.poll(()=>page.evaluate(()=>__nir.state().resident_bytes)).toBeLessThan(largeBytes);
    await page.setViewportSize({width:1280,height:720});
    await expect.poll(()=>page.evaluate(()=>__nir.state().resident_bytes)).toBe(largeBytes);
    expect(await page.evaluate(()=>window.longAudioStarts)).toEqual(started);
    expect(await page.evaluate(()=>__nir.state().error)).toBeNull();expect(errors).toEqual([]);
  });
}
