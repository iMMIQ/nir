import {test,expect} from '@playwright/test';

// Exercise the distributed production source scheduler against native rendered
// stereo samples. Short synthetic buffers expose the same endpoint rounding as
// a long decoded MP3 without retaining a commercial fixture in the repository.
for(const [rate,endFrame] of [[44100,13],[48000,7],[96000,7]])for(const region of [false,true]) {
 test(`production ${region?'region':'whole'} loop keeps every stereo frame at ${rate}Hz`,async({page,request},info)=>{
  const origin='http://127.0.0.1:4265';
  const channel=await (await request.get(origin+'/channels/stable.json')).json();
  const manifest=await (await request.get(`${origin}/releases/${channel.release}.json`)).json();
  const hostPath='/'+manifest.objects[manifest.engine.host].path;
  const host=await (await request.get(origin+hostPath)).text();
  const start=host.indexOf('    function stopVoice(id) {'),end=host.indexOf('    function sampleAudioPositions()',start);
  expect(start).toBeGreaterThanOrEqual(0);expect(end).toBeGreaterThan(start);
  await page.goto(origin+'/__native_loop_probe__');
  const proof=await page.evaluate(async({hostPath,code,rate,endFrame,region})=>{
   const helpers=await import(hostPath),frames=region?128:endFrame;
   const context=new OfflineAudioContext(2,endFrame*200+128,rate),buffer=context.createBuffer(2,frames,rate);
   for(let c=0;c<2;c++)for(let i=0;i<frames;i++)buffer.getChannelData(c)[i]=(c===0?1:-1)*(.1+i*.0023);
   const voices=new Map(),buffers=new Map([['loop',buffer]]),preferences={bgm_volume:.5},metrics={audioStarts:0};
   const slot={cancel(){}},inbox={reserve:()=>slot},post=(_slot,fn)=>fn(),mutateEngine=fn=>fn();
   const engine={audio_failed_in(){throw new Error('Production audio failed');},audio_ended_in(){throw new Error('Loop ended');}};
   const audioDomains={context:()=>context},assetDescriptors=new Map([['loop',{duration_us:String(Math.ceil(frames*1e6/rate))}]]);
   const names=['voices','buffers','preferences','metrics','inbox','post','mutateEngine','engine','audioDomains','assetDescriptors','audioVoiceKey','audioLoopPlayback','characterVoiceGain','audioLoopEndSeconds'];
   const owner=new Function(...names,code+';return {playVoice,stopVoice};')(voices,buffers,preferences,metrics,inbox,post,mutateEngine,engine,audioDomains,assetDescriptors,helpers.audioVoiceKey,helpers.audioLoopPlayback,helpers.characterVoiceGain,helpers.audioLoopEndSeconds);
   const startFrame=region?2:0,command={domain:'story',bus:'bgm',session:1,task:1,asset:'loop',looped:true,gain:.7,envelope:1,position_us:'0',...(region?{loop_region:{start_us:String(Math.round(startFrame*1e6/rate)),end_us:String(Math.round(endFrame*1e6/rate))}}:{})};
   owner.playVoice(command);const source=voices.get(helpers.audioVoiceKey(command)),gain=source.gain.gain.value;
   const rendered=await context.startRendering();let maxError=0,worstFrame=0,worstChannel=0;const joins=[];
   for(let i=0;i<rendered.length;i++)for(let c=0;c<2;c++){
    const at=i<endFrame?i:startFrame+(i-endFrame)%(endFrame-startFrame),expected=buffer.getChannelData(c)[at]*gain,actual=rendered.getChannelData(c)[i],error=Math.abs(actual-expected);
    if(error>maxError){maxError=error;worstFrame=i;worstChannel=c;}
    if(c===0&&i>=endFrame-2&&i<=endFrame+2)joins.push({i,actual,expected});
   }
   const proof={rate,endFrame,startFrame,region,renderedFrames:rendered.length,channels:rendered.numberOfChannels,maxError,worstFrame,worstChannel,loopEndSeconds:source.source.loopEnd,loopEndFrames:source.source.loopEnd*rate,joins};
   owner.stopVoice(helpers.audioVoiceKey(command));return proof;
  },{hostPath,code:host.slice(start,end),rate,endFrame,region});
  await info.attach('native-loop-rounding',{body:JSON.stringify(proof,null,2),contentType:'application/json'});
  expect(proof.channels).toBe(2);expect(proof.renderedFrames).toBeGreaterThan(endFrame*100);
  expect(proof.maxError).toBeLessThan(2e-6);
 });
}
