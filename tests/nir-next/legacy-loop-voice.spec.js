import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';

for(const worker of ['required','main']) {
  test(`legacy auto advances after finite speech and preserves a looping Voice, ${worker}`,async({page},testInfo)=>{
    const errors=[];page.on('pageerror',error=>errors.push(error.message));
    await page.addInitScript(()=>{
      window.legacyContexts=[];window.legacyAudio=[];
      const Native=AudioContext;
      window.AudioContext=class extends Native {
        constructor(...args){super(...args);legacyContexts.push(this);}
      };
      const create=Native.prototype.createBufferSource;
      Native.prototype.createBufferSource=function(...args){
        const source=create.apply(this,args),row={source,context:this,startedAt:null,ended:false,stops:0};
        legacyAudio.push(row);
        const start=source.start,stop=source.stop;
        source.start=function(...args){row.startedAt=performance.now();row.duration=source.buffer.duration;return start.apply(this,args);};
        source.stop=function(...args){row.stops++;return stop.apply(this,args);};
        source.addEventListener('ended',()=>{row.ended=true;});
        return source;
      };
    });
    await page.goto(`http://127.0.0.1:4258/?test=1&worker=${worker}&backend=webgl2`);
    await page.waitForFunction(()=>window.__nir?.state().ready&&!__nir.state().loading);
    await page.keyboard.press('Enter');
    await page.waitForFunction(()=>__nir.state().dialogue?.ready&&!__nir.state().loading&&
      legacyContexts.every(context=>context.state==='running')&&
      legacyAudio.filter(row=>row.context===legacyContexts[2]&&row.startedAt!==null).length===2);
    const before=await page.evaluate(async()=>{
      await __nir.action({type:'voice_continue',enabled:false});
      await __nir.action({type:'toggle_auto'});
      return {session:__nir.state().session,interaction:__nir.state().interaction};
    });
    // The 100ms fixed reading timer expires, but finite speech still waits.
    await page.waitForTimeout(400);
    expect(await page.evaluate(()=>__nir.state().interaction)).toBe(before.interaction);
    expect(await page.evaluate(()=>legacyAudio.some(row=>row.context===legacyContexts[2]&&
      !row.source.loop&&!row.ended&&row.stops===0))).toBe(true);
    await page.waitForFunction(()=>legacyAudio.some(row=>row.context===legacyContexts[2]&&
      !row.source.loop&&row.ended),null,{timeout:15000});
    await page.waitForFunction(interaction=>__nir.state().interaction!==interaction&&
      !__nir.state().loading,before.interaction,{timeout:5000});
    const proof=await page.evaluate(()=>({session:__nir.state().session,interaction:__nir.state().interaction,
      preferences:__nir.state().preferences,
      voices:legacyAudio.filter(row=>row.context===legacyContexts[2]).map(row=>({
        loop:row.source.loop,duration:row.duration,stops:row.stops,
        ended:row.ended,state:row.context.state,startedAt:row.startedAt,
      })),error:__nir.state().error,
    }));
    expect(proof.session).toBe(before.session);
    expect(proof.preferences.auto_wait_voice).toBe(true);
    expect(proof.preferences.voice_continue).toBe(false);
    expect(proof.voices.find(voice=>voice.loop)).toMatchObject({stops:0,ended:false,state:'running'});
    expect(proof.voices.find(voice=>!voice.loop)).toMatchObject({stops:0,ended:true});
    expect(proof.error).toBeNull();expect(errors).toEqual([]);
    const output=testInfo.outputPath('legacy-loop-voice.json');
    await fs.writeFile(output,JSON.stringify({before,...proof},null,2)+'\n');
    await testInfo.attach('legacy-loop-voice',{path:output,contentType:'application/json'});
  });
}
