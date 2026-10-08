import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import {recoverAudioOutput} from './audio-output-helper.js';
const origin='http://127.0.0.1:4267';
test.use({viewport:{width:390,height:844},locale:'en-US',deviceScaleFactor:1});

for(const rate of [44100,48000,96000])test(`full MP3 endpoint renders intro and 100 cycles at ${rate}Hz`,async({page,request},info)=>{
    const channel=await(await request.get(`${origin}/channels/stable.json`)).json();
    const release=await(await request.get(`${origin}/releases/${channel.release}.json`)).json();
    const host=`${origin}/${release.objects[release.engine.host].path}`;
    const audio=Object.values(release.objects).find(d=>d.media_type==='audio/mpeg'&&d.bytes===161760);
    const bytes=[...await(await request.get(`${origin}/${audio.path}`)).body()];
    await page.goto(`${origin}/channels/stable.json`);
    const proof=await page.evaluate(async({host,bytes,rate})=>{
        const {audioLoopPlayback,audioLoopEndSeconds}=await import(host);
        const decoder=new AudioContext({sampleRate:rate});
        const buffer=await decoder.decodeAudioData(Uint8Array.from(bytes).buffer),pcm=buffer.getChannelData(0);
        await decoder.close();
        const region={start_us:'7800000',end_us:'8000000'};
        const start=Math.round(7.8*rate),end=buffer.length,body=end-start,rows=[];
        for(const position of ['0','8000000','8400000','18446744073709551615']){
            const mapped=audioLoopPlayback(region,position,rate,buffer.length,'8000000');
            const absolute=(BigInt(position)*BigInt(rate)+500000n)/1000000n;
            const first=Number(absolute<BigInt(end)?absolute:BigInt(start)+(absolute-BigInt(end))%BigInt(body));
            const count=position==='0'?end+body*100:body*100;
            const context=new OfflineAudioContext(1,count,rate),source=context.createBufferSource();
            source.buffer=buffer;source.loop=true;source.loopStart=mapped.startFrame/rate;source.loopEnd=audioLoopEndSeconds(mapped.endFrame,rate);
            source.connect(context.destination);source.start(0,mapped.offsetFrame/rate);
            const output=(await context.startRendering()).getChannelData(0);
            let index=first,maxError=0;
            for(const sample of output){maxError=Math.max(maxError,Math.abs(sample-pcm[index]));if(++index===end)index=start;}
            rows.push({position,mapped,first,maxError,frames:output.length});
        }
        const rejects=(region,frames,duration)=>{try{audioLoopPlayback(region,'0',rate,frames,duration);return false;}catch(e){return /E_AUDIO_LOOP/.test(String(e));}};
        return {rate,frames:buffer.length,nominal:8*rate,start,end,rows,
            missingMetadataRejected:buffer.length===8*rate||rejects(region,buffer.length,undefined),
            interiorTruncationRejected:buffer.length===8*rate||rejects(region,buffer.length,'9000000'),
            twoLostFramesRejected:rejects(region,8*rate-2,'8000000'),
            authoredOverflowRejected:rejects({start_us:'7800000',end_us:'8000001'},buffer.length,'8000000')};
    },{host,bytes,rate});
    await fs.writeFile(info.outputPath('loop-endpoint-pcm.json'),JSON.stringify({release:channel.release,host_digest:release.engine.host,cycles:100,...proof},null,2)+'\n');
    expect(proof.nominal-proof.frames).toBeGreaterThanOrEqual(0);
    expect(proof.nominal-proof.frames).toBeLessThanOrEqual(1);
    for(const row of proof.rows){expect(row.mapped).toEqual({startFrame:proof.start,endFrame:proof.end,offsetFrame:row.first});expect(row.maxError).toBeLessThan(.00001);}
    for(const key of ['missingMetadataRejected','interiorTruncationRejected','twoLostFramesRejected','authoredOverflowRejected'])expect(proof[key]).toBe(true);
});

for(const worker of ['required','main'])for(const rate of [44100,48000,96000])test(`authored full endpoint starts and survives menus at ${rate}Hz, ${worker}`,async({page},info)=>{
    await page.addInitScript(rate=>{
        const Context=window.AudioContext,create=Context.prototype.createBufferSource;
        window.AudioContext=class extends Context{constructor(){super({sampleRate:rate});}};
        window.endpointSources=[];
        Context.prototype.createBufferSource=function(...args){
            const source=create.apply(this,args),start=source.start,stop=source.stop;
            const row={stops:0,ended:false};
            source.start=function(...args){Object.assign(row,{loop:source.loop,start:source.loopStart,end:source.loopEnd,frames:source.buffer.length,rate:source.buffer.sampleRate,offset:args[1]});endpointSources.push(row);return start.apply(this,args);};
            source.stop=function(...args){row.stops++;return stop.apply(this,args);};
            source.addEventListener('ended',()=>row.ended=true);return source;
        };
    },rate);
    await page.goto(`${origin}/?test=1&worker=${worker}&backend=webgl2`);
    await page.waitForFunction(()=>window.__nir?.state().ready&&!__nir.state().loading);
    await page.keyboard.press('Enter');
    await page.waitForFunction(()=>__nir.state().error!==null||__nir.state().dialogue?.ready&&!__nir.state().loading&&endpointSources.length===2);
    expect(await page.evaluate(()=>__nir.state().error)).toBeNull();
    await recoverAudioOutput(page);
    const before=await page.evaluate(()=>({state:__nir.state(),sources:endpointSources,diagnostics:__nir.diagnostics()}));
    await fs.writeFile(info.outputPath('loop-endpoint-runtime.json'),JSON.stringify({worker,rate,status:'started',before},null,2)+'\n');
    const region=before.sources.find(s=>s.start>0);
    expect(region.rate).toBe(rate);expect(region.start).toBe(7.8);expect(region.end*rate).toBeLessThanOrEqual(region.frames);expect(region.frames-region.end*rate).toBeLessThanOrEqual(4*Number.EPSILON*region.frames);expect(region.offset).toBe(0);
    expect(8*rate-region.frames).toBeGreaterThanOrEqual(0);
    expect(8*rate-region.frames).toBeLessThanOrEqual(1);
    expect(before.state.error).toBeNull();
    const firstClock=before.diagnostics.host_work.audio_domains.story.buses.bgm.clock_seconds;
    await page.waitForFunction(first=>__nir.diagnostics().host_work.audio_domains.story.buses.bgm.clock_seconds-first>=8.8,firstClock);
    for(let i=0;i<3;i++){
        await page.evaluate(()=>__nir.action({type:'menu'}));
        await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
        await page.evaluate(()=>__nir.action({type:'close'}));
        await page.waitForFunction(()=>__nir.state().screen==='Story');
        await recoverAudioOutput(page);
    }
    const after=await page.evaluate(()=>({state:__nir.state(),sources:endpointSources,diagnostics:__nir.diagnostics()}));
    expect(after.sources).toHaveLength(2);expect(after.sources.every(s=>s.loop&&s.stops===0&&!s.ended)).toBe(true);
    expect(after.state.history_count).toBe(before.state.history_count);expect(after.state.error).toBeNull();
    await fs.writeFile(info.outputPath('loop-endpoint-runtime.json'),JSON.stringify({worker,rate,status:'passed',before,after},null,2)+'\n');
});
