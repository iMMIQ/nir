import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import {recoverAudioOutput} from './audio-output-helper.js';
test.use({viewport:{width:1280,height:720},deviceScaleFactor:1});

async function boot(page,port,worker) {
    await page.goto(`http://127.0.0.1:${port}/?test=1&worker=${worker}&backend=webgl2`);
    await page.waitForFunction(()=>window.__nir?.state().ready&&!__nir.state().loading);
}
const memory=page=>page.evaluate(()=>__nir.diagnostics().resource_memory);
for(const worker of ['required','main'])for(const [kind,port,key,field,multiplier] of [
    ['transition',4262,'transition','transition_texture_bytes',2],
    ['window',4263,'window','window_texture_bytes',1],
    ['menu',4264,'menu_transition','menu_texture_bytes',2],
]) {
    test(`finished ${kind} releases offscreen textures, ${worker}`,async({page},testInfo)=>{
        const proof={worker,kind};
        try {
            await boot(page,port,worker);
            if(kind!=='menu')await page.keyboard.press('Enter');
            await page.waitForFunction(({key,field})=>__nir.state()[key]!==null&&__nir.state().renderer_memory[field]>0,{key,field});
            proof.active=await memory(page);
            expect(proof.active.renderer[field]).toBe(1280*720*4*multiplier);
            if(kind!=='menu')await recoverAudioOutput(page);
            await page.waitForFunction(key=>__nir.state()[key]===null,key);
            proof.completed=await memory(page);
            await expect.poll(async()=> (await memory(page)).renderer[field]).toBe(0);
            proof.released=await memory(page);
            if(kind==='menu') {
                await page.keyboard.press('Enter');
                await recoverAudioOutput(page);
                await page.waitForFunction(()=>__nir.state().dialogue?.ready&&!__nir.state().loading);
                await page.keyboard.press('Escape');
                await page.waitForFunction(()=>__nir.state().menu_transition!==null);
                proof.reentered=await memory(page);
                expect(proof.reentered.renderer[field]).toBe(1280*720*4*multiplier);
                await page.waitForFunction(()=>__nir.state().menu_transition===null);
                await expect.poll(async()=> (await memory(page)).renderer[field]).toBe(0);
                await page.evaluate(()=>__nir.action({type:'close'}));
                await page.waitForFunction(()=>__nir.state().screen==='Story');
                await expect.poll(async()=> (await memory(page)).renderer[field]).toBe(0);
            }
            await page.screenshot({path:testInfo.outputPath('released.png')});
        } finally {
            proof.final=await memory(page).catch(()=>null);
            await fs.writeFile(testInfo.outputPath('resource-memory.json'),JSON.stringify(proof,null,2)+'\n');
        }
    });
}

for(const worker of ['required','main'])test(`actual MP3 payloads survive menus and are reclaimed at Title, ${worker}`,async({page},testInfo)=>{
    await page.addInitScript(()=>{
        window.memoryAudio=[];window.memoryAudioBuffers=[];const ids=new WeakMap();let next=0;
        const create=AudioContext.prototype.createBufferSource;
        AudioContext.prototype.createBufferSource=function(...args){
            const source=create.apply(this,args),start=source.start;
            source.start=function(...args){
                const b=source.buffer;if(!ids.has(b)){ids.set(b,++next);memoryAudioBuffers.push({id:ids.get(b),ref:new WeakRef(b)});}
                memoryAudio.push({bufferId:ids.get(b),loop:source.loop,length:b.length,channels:b.numberOfChannels,sampleRate:b.sampleRate});
                return start.apply(this,args);
            };return source;
        };
    });
    await boot(page,4265,worker);
    const title=await memory(page);
    expect(title.media.decoded_audio_bytes).toBe(0);
    await page.keyboard.press('Enter');
    await recoverAudioOutput(page);
    await page.waitForFunction(()=>__nir.state().dialogue?.ready&&!__nir.state().loading&&memoryAudio.filter(s=>s.loop).length===2);
    // The first ready dialogue can overlap speculative image uploads. Require
    // staging to drain before taking the idle payload snapshot.
    await expect.poll(async()=> (await memory(page)).media.image_staging_bytes).toBe(0);
    const story=await memory(page);
    const native=await page.evaluate(()=>{
        const buffers=new Map(memoryAudio.filter(s=>s.loop).map(s=>[s.bufferId,s]));
        return {bytes:[...buffers.values()].reduce((n,b)=>n+b.length*b.channels*4,0),shapes:[...buffers.values()]};
    });
    expect(story.media.active_audio_bytes).toBe(native.bytes);
    expect(story.media.decoded_audio_bytes).toBe(native.bytes);
    expect(story.media.image_staging_bytes).toBe(0);
    const downloadsBefore=await page.evaluate(()=>__nir.resourceTimings());
    for(let i=0;i<3;i++) {
        await page.evaluate(()=>__nir.action({type:'menu'}));
        await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
        expect((await memory(page)).media.decoded_audio_bytes).toBe(native.bytes);
        await page.evaluate(()=>__nir.action({type:'close'}));
        await page.waitForFunction(()=>__nir.state().screen==='Story');
        await recoverAudioOutput(page);
    }
    expect(await page.evaluate(()=>memoryAudio.filter(s=>s.loop).length)).toBe(2);
    const downloadsAfter=await page.evaluate(()=>__nir.resourceTimings());
    const mp3s=rows=>rows.filter(r=>new URL(r.name).pathname.endsWith('.mp3'));
    expect(mp3s(downloadsAfter).length).toBe(mp3s(downloadsBefore).length);
    // The menu freezes the looping SFX context. Stopping this source must
    // release its PCM even while no further audio render quantum can run.
    await page.evaluate(()=>__nir.action({type:'menu'}));
    await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading&&__nir.diagnostics().host_work.audio_domains.story.buses.sfx.state==='suspended');
    await page.evaluate(()=>__nir.hidden(true));
    await page.waitForFunction(()=>Object.values(__nir.diagnostics().host_work.audio_domains.story.buses).every(b=>b.state==='suspended'));
    await page.evaluate(()=>__nir.action({type:'title'}));
    await page.waitForFunction(()=>__nir.state().screen==='Title'&&!__nir.state().loading&&__nir.metrics.activeRequests===0);
    await expect.poll(async()=> (await memory(page)).media.decoded_audio_bytes).toBe(0);
    const final=await memory(page);
    expect(final.media.active_audio_sources).toBe(0);expect(final.media.cached_audio_assets).toBe(0);
    expect(final.media.encoded_cache_bytes).toBe(title.media.encoded_cache_bytes);
    const cdp=await page.context().newCDPSession(page);
    const retiredIds=native.shapes.map(b=>b.bufferId);
    const gcProof={before:await page.evaluate(()=>__nir.diagnostics().host_work.audio_domains.story)};
    await cdp.send('HeapProfiler.collectGarbage');
    // Read WeakRefs in a later task; inspecting them before GC would itself
    // keep their targets alive for the duration of that JavaScript job.
    await page.waitForTimeout(100);
    gcProof.after=await page.evaluate(ids=>({
        domains:__nir.diagnostics().host_work.audio_domains.story,
        retained:memoryAudioBuffers.filter(b=>ids.includes(b.id)).map(b=>({id:b.id,alive:b.ref.deref()!==undefined})),
    }),retiredIds);
    await cdp.detach();
    await fs.writeFile(testInfo.outputPath('retired-pcm.json'),JSON.stringify(gcProof,null,2)+'\n');
    expect(gcProof.after.retained).toHaveLength(2);
    expect(gcProof.after.domains.buses.sfx.state).toBe('suspended');
    expect(gcProof.after.domains.buses.sfx.clock_seconds).toBe(gcProof.before.buses.sfx.clock_seconds);
    expect(gcProof.after.domains.buses.sfx.resume_attempts).toBe(gcProof.before.buses.sfx.resume_attempts);
    expect(gcProof.after.retained.filter(b=>b.alive)).toEqual([]);
    await fs.writeFile(testInfo.outputPath('audio-memory.json'),JSON.stringify({worker,title,story,native,final,downloadsBefore,downloadsAfter,diagnostics:await page.evaluate(()=>__nir.diagnostics())},null,2)+'\n');
});
