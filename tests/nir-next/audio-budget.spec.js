import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import {recoverAudioOutput} from './audio-output-helper.js';
const origin='http://127.0.0.1:4266';
test.use({viewport:{width:390,height:844},deviceScaleFactor:1,locale:'en-US'});
async function prepare(page,worker,rate,oversize=false) {
    await page.addInitScript(({rate,oversize})=>{
        const Context=window.AudioContext,decode=Context.prototype.decodeAudioData,create=Context.prototype.createBufferSource;
        window.failDecodedVoice=oversize;window.budgetDecodes=[];window.budgetSources=[];
        window.AudioContext=class extends Context {constructor(){super({sampleRate:rate});}};
        Context.prototype.decodeAudioData=async function(...args){
            let b=await decode.apply(this,args),injected=false;
            if(failDecodedVoice&&Math.abs(b.duration-1.2)<.01){
                const nominal=Math.round(b.duration*rate);
                b=failDecodedVoice===true?this.createBuffer(2,nominal+2,rate):this.createBuffer(1,nominal+(failDecodedVoice==='short'?-2:2),rate);
                injected=true;
            }
            budgetDecodes.push({length:b.length,channels:b.numberOfChannels,rate:b.sampleRate,bytes:b.length*b.numberOfChannels*4,injected});return b;
        };
        Context.prototype.createBufferSource=function(...args){
            const source=create.apply(this,args),start=source.start,stop=source.stop,row={loop:false,bytes:0,stops:0,ended:false};
            source.start=function(...args){row.loop=source.loop;row.bytes=source.buffer.length*source.buffer.numberOfChannels*4;budgetSources.push(row);return start.apply(this,args);};
            source.stop=function(...args){row.stops++;return stop.apply(this,args);};
            source.addEventListener('ended',()=>row.ended=true);return source;
        };
    },{rate,oversize});
    await page.goto(`${origin}/?test=1&worker=${worker}&backend=webgl2`);
    await page.waitForFunction(()=>window.__nir?.state().ready&&!__nir.state().loading);
    const title=await page.evaluate(()=>__nir.state());
    await page.keyboard.press('Enter');
    await page.waitForFunction(()=>__nir.state().dialogue?.ready&&!__nir.state().loading&&budgetSources.filter(s=>s.loop).length===2);
    // Output becomes relevant only after these sources are scheduled; an
    // earlier check can miss the recovery prompt that arrives with them.
    try {await recoverAudioOutput(page);} catch(error) {
        throw new Error(`Output recovery failed after source scheduling: ${JSON.stringify(await snapshot(page))}; ${error}`);
    }
    // Keep authored lookahead enabled and include its decoded voice in the
    // resident set. A rejected speculative decode must settle without a fault.
    await page.waitForFunction(()=>{
        const work=__nir.diagnostics().host_work;
        return work.media_jobs===0&&work.decode_pool_active===0&&work.decode_pool_waiting===0&&work.shared_fetches===0;
    });
    return title;
}
const snapshot=page=>page.evaluate(()=>({state:__nir.state(),memory:__nir.diagnostics().resource_memory,decodes:budgetDecodes,sources:budgetSources,diagnostics:__nir.diagnostics()}));
async function pixel(page) {
    const png=await page.screenshot();
    return page.evaluate(async data=>{
        const bitmap=await createImageBitmap(new Blob([Uint8Array.from(atob(data),c=>c.charCodeAt(0))],{type:'image/png'}));
        const canvas=document.createElement('canvas');canvas.width=bitmap.width;canvas.height=bitmap.height;
        const c=canvas.getContext('2d');c.drawImage(bitmap,0,0);bitmap.close();
        return [...c.getImageData(8,Math.floor(canvas.height/2),1,1).data];
    },png.toString('base64'));
}
for(const worker of ['required','main'])for(const rate of [44100,48000,96000])test(`PCM admission follows actual ${rate}Hz decoder before first prepare, ${worker}`,async({page},testInfo)=>{
    const title=await prepare(page,worker,rate),story=await snapshot(page);
    expect(title.audio_decode_sample_rate).toBe(rate);expect(story.state.audio_decode_sample_rate).toBe(rate);
    expect(story.decodes.map(d=>d.rate)).toEqual([rate,rate,rate]);
    expect(story.decodes.every(d=>!d.injected)).toBe(true);
    const actual=story.decodes.reduce((n,d)=>n+d.bytes,0),reserved=[8000000n,800000n,1200000n].reduce((n,us)=>n+Number((us*BigInt(rate)+999999n)/1000000n+1n)*8,0);
    // Native resampling can round a track down by one output frame. Verify
    // actual per-track frames, while retaining the admitted upper bound.
    const lengths=story.decodes.map(d=>d.length).sort((a,b)=>a-b),targets=[Math.round(.8*rate),Math.round(1.2*rate),8*rate];
    for(let i=0;i<3;i++)expect(Math.abs(lengths[i]-targets[i])).toBeLessThanOrEqual(1);
    expect(actual).toBeLessThanOrEqual(reserved);
    expect(story.memory.media.decoded_audio_bytes).toBe(actual);
    expect(story.sources).toHaveLength(2);
    expect(story.sources.every(s=>s.loop)).toBe(true);
    expect(story.memory.media.active_audio_bytes).toBe(story.sources.reduce((n,s)=>n+s.bytes,0));
    expect(story.state.error).toBeNull();expect(story.sources.every(s=>s.stops===0&&!s.ended)).toBe(true);
    await page.evaluate(()=>__nir.action({type:'title'}));
    await page.waitForFunction(()=>__nir.state().screen==='Title'&&!__nir.state().loading&&__nir.metrics.activeRequests===0);
    await expect.poll(()=>page.evaluate(()=>__nir.diagnostics().resource_memory.media.decoded_audio_bytes)).toBe(0);
    await fs.writeFile(testInfo.outputPath('audio-budget.json'),JSON.stringify({worker,rate,title,story,actual,reserved,final:await snapshot(page)},null,2)+'\n');
});
for(const worker of ['required','main'])for(const injection of [true,'short','long'])test(`${injection===true?'oversized':injection} decode preserves scene/music and allows a clean retry, ${worker}`,async({page},testInfo)=>{
    await prepare(page,worker,48000,injection);const before=await snapshot(page),red=await pixel(page);
    expect(before.state.error).toBeNull();
    expect(before.decodes.filter(d=>d.injected)).toHaveLength(1);
    expect(red.slice(0,3)).toEqual([255,0,0]);
    await page.keyboard.press('Enter');
    await page.waitForFunction(()=>__nir.state().error!==null||__nir.state().history_count===2);
    const failed=await snapshot(page);
    const failedPixel=await pixel(page);
    await fs.writeFile(testInfo.outputPath('audio-budget-failure.json'),JSON.stringify({worker,injection,before,failed,red,failedPixel},null,2)+'\n');
    expect(failed.state.diagnostic?.code).toBe(injection===true?'E_AUDIO_MEMORY':'E_AUDIO_DURATION');
    expect(failed.decodes.filter(d=>d.injected)).toHaveLength(2);
    expect(failed.state.history_count).toBe(before.state.history_count);
    expect(failed.memory.media.decoded_audio_bytes).toBe(before.memory.media.decoded_audio_bytes);
    expect(failed.sources.filter(s=>s.loop).every(s=>s.stops===0&&!s.ended)).toBe(true);
    expect(failedPixel).toEqual(red);
    const stoppedTick=failed.state.tick_us;await page.waitForTimeout(400);
    const waiting=await snapshot(page);
    expect(waiting.state.tick_us).toBe(stoppedTick);
    expect(waiting.diagnostics.host_work.audio_domains.story.buses.bgm.clock_seconds).toBeGreaterThan(failed.diagnostics.host_work.audio_domains.story.buses.bgm.clock_seconds);
    await page.evaluate(()=>{window.failDecodedVoice=false;return __nir.action({type:'retry'});});
    await page.waitForFunction(()=>__nir.state().error===null&&!__nir.state().loading&&__nir.state().history_count===2);
    const ready=await snapshot(page);
    expect(ready.state.session).toBe(before.state.session);
    expect(ready.sources.filter(s=>s.loop).length).toBe(2);
    expect(ready.sources.filter(s=>s.loop).every(s=>s.stops===0&&!s.ended)).toBe(true);
    expect(ready.decodes.filter(d=>d.injected)).toHaveLength(2);
    expect(ready.decodes.filter(d=>!d.injected&&d.length===57600)).toHaveLength(1);
    expect((await pixel(page)).slice(0,3)).toEqual([0,0,255]);
    await page.screenshot({path:testInfo.outputPath('recovered.png')});
    await fs.writeFile(testInfo.outputPath('audio-budget-failure.json'),JSON.stringify({worker,injection,before,failed,waiting,ready,red,failedPixel,final:await snapshot(page)},null,2)+'\n');
});
