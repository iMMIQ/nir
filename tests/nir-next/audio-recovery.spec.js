import {test, expect} from '@playwright/test';
import fs from 'node:fs/promises';

for (const {worker,delayReply} of [
  {worker:'required',delayReply:false},
  {worker:'main',delayReply:false},
  {worker:'required',delayReply:true},
]) {
  test(`voice output recovery freezes reading and consumes its gesture, ${worker}${delayReply?' with delayed reply':''}`, async ({page},testInfo) => {
    const errors=[];page.on('pageerror', error=>errors.push(error.message));
    await page.addInitScript(({delayReply}) => {
      window.audioContexts=[];window.blockAudioResume=false;
      window.recoveryTiming={armed:false,started:null,ended:null,voiceResumed:null};
      if(delayReply) {
        const NativeWorker=window.Worker;
        window.Worker=class extends NativeWorker {
          constructor(url,options){super(url,options);this.runtime=options?.name==='nir-runtime';this.methods=new Map();}
          postMessage(message,...args){if(message.kind==='batch')this.methods.set(message.id,message.value.map(call=>call.method));return super.postMessage(message,...args);}
          set onmessage(handler){super.onmessage=event=>{
            const methods=this.methods.get(event.data.id);
            if(event.data.kind!=='update')this.methods.delete(event.data.id);
            if(recoveryTiming.armed&&this.runtime&&event.data.kind==='reply'&&methods?.includes('hidden')) {
              recoveryTiming.armed=false;recoveryTiming.started=performance.now();
              setTimeout(()=>{recoveryTiming.ended=performance.now();handler.call(this,event);},400);
            }else handler.call(this,event);
          };}
          get onmessage(){return super.onmessage;}
        };
      }
      const Native=window.AudioContext;
      window.AudioContext=class extends Native {
        constructor(...args){super(...args);audioContexts.push(this);}
        resume(){
          if(blockAudioResume)return Promise.reject(new DOMException('Gesture required','NotAllowedError'));
          const result=super.resume();
          if(this===audioContexts[2])result.then(()=>{recoveryTiming.voiceResumed=performance.now();},()=>{});
          return result;
        }
      };
      window.startedAudio=[];
      const create=Native.prototype.createBufferSource;
      Native.prototype.createBufferSource=function(...args) {
        const source=create.apply(this,args),start=source.start;
        source.start=function(...args){startedAudio.push(source);return start.apply(this,args);};
        return source;
      };
    },{delayReply});
    await page.goto(`http://127.0.0.1:4252/?test=1&backend=webgl2&worker=${worker}`);
    await page.waitForFunction(() => window.__nir?.state().ready&&!__nir.state().loading);
    await page.keyboard.press('Enter');
    await page.waitForFunction(() => __nir.state().dialogue?.ready&&!__nir.state().loading&&audioContexts.every(c=>c.state==='running'));
    await page.evaluate(async () => {
      await __nir.action({type:'toggle_auto'});
      blockAudioResume=true;
      await audioContexts[2].suspend();
    });
    const recovery=page.getByRole('button',{name:/Tap to resume sound|点击恢复声音/});
    await expect(recovery).toBeVisible();
    await page.waitForFunction(() => __nir.state().paused);
    const hide=page.getByRole('button',{name:/^Hide$|^隐藏$/});
    await expect(hide).toBeDisabled();
    const before=await page.evaluate(() => ({
      tick:__nir.state().tick_us,interaction:__nir.state().interaction,
      music:audioContexts[0].currentTime,voice:audioContexts[2].currentTime,
      sourceCount:startedAudio.length,auto:__nir.state().auto,
      clock:__nir.state().story_clock,
    }));
    expect(before.auto).toBe(true);
    await recovery.click(); // Denied recovery must remain actionable.
    await page.waitForTimeout(400);
    expect(await page.evaluate(() => __nir.state().tick_us)).toBe(before.tick);
    expect(await page.evaluate(() => __nir.state().interaction)).toBe(before.interaction);
    expect(await page.evaluate(() => audioContexts[2].currentTime)).toBe(before.voice);
    expect(await page.evaluate(() => audioContexts[0].currentTime)).toBeGreaterThan(before.music);
    await expect(recovery).toBeVisible();
    if(delayReply) {
      await page.evaluate(()=>{recoveryTiming.armed=true;void __nir.hidden(false);});
      await page.waitForFunction(()=>recoveryTiming.started!==null);
    }
    await page.evaluate(() => {blockAudioResume=false;});
    if(worker==='required')await recovery.click();
    else {
      // Recover using a real dialogue hit, so a resumed pointerup would have
      // advanced the story if the host forgot the blocked pointerdown.
      const rect=await page.evaluate(()=>JSON.parse([...document.querySelectorAll('#actions button')]
        .find(b=>JSON.parse(b.dataset.action).type==='advance').dataset.rect));
      await page.locator('#stage').click({position:{x:rect[0]+rect[2]/2,y:rect[1]+rect[3]/2}});
    }
    await expect(recovery).toBeHidden();
    await page.waitForFunction(() => !__nir.state().paused&&audioContexts[2].state==='running');
    await expect(hide).toBeEnabled();
    expect(await page.evaluate(() => __nir.state().interaction)).toBe(before.interaction);
    expect(await page.evaluate(() => startedAudio.length)).toBe(before.sourceCount);
    // Measure the first *actual* VM advance at its owner. Playwright's
    // visibility polling can finish hundreds of milliseconds after recovery,
    // when normal reading has already resumed. Keep the original step limit.
    await page.waitForFunction(revision=>__nir.state().story_clock.resume_revision>revision&&
      __nir.state().story_clock.first_advance_us!==null,before.clock.resume_revision);
    const clock=await page.evaluate(()=>__nir.state().story_clock);
    expect(clock.resume_revision).toBe(before.clock.resume_revision+1);
    expect(clock.paused_advance_us).toBe(0);
    expect(clock.resume_tick_us).toBe(Number(before.tick));
    expect(clock.first_advance_us).toBeGreaterThan(0);
    expect(clock.first_advance_us).toBeLessThan(250_000);
    if(delayReply) {
      const timing=await page.evaluate(()=>recoveryTiming);
      expect(timing.ended-timing.started).toBeGreaterThanOrEqual(400);
      expect(timing.voiceResumed).toBeGreaterThan(timing.started);
      expect(timing.voiceResumed).toBeLessThan(timing.ended);
    }
    const proof=JSON.stringify(await page.evaluate(()=>({
      clock:__nir.state().story_clock,timing:recoveryTiming,
      output:__nir.diagnostics().host_work.audio_domains,
    })),null,2);
    const clockPath=testInfo.outputPath('recovery-clock.json');
    await fs.writeFile(clockPath,proof+'\n');
    await testInfo.attach('recovery-clock',{path:clockPath,contentType:'application/json'});

    // A normal menu Voice/Sfx suspension is not an output failure. Hidden
    // ownership must survive any attempt to unlock via a player action.
    await page.evaluate(() => __nir.action({type:'menu'}));
    await page.waitForFunction(() => __nir.state().screen==='Menu'&&audioContexts[2].state==='suspended');
    await expect(recovery).toBeHidden();
    await page.evaluate(() => __nir.hidden(true));
    await page.waitForFunction(() => audioContexts.every(c=>c.state==='suspended'));
    await page.evaluate(() => __nir.action({type:'settings'}));
    expect(await page.evaluate(() => audioContexts.every(c=>c.state==='suspended'))).toBe(true);
    await expect(recovery).toBeHidden();
    expect(errors).toEqual([]);
  });

  if(delayReply)continue;
  test(`initial gesture denial and deliberate mute have distinct behavior, ${worker}`, async ({page},testInfo) => {
    const errors=[];page.on('pageerror', error=>errors.push(error.message));
    await page.addInitScript(() => {
      window.blockAudioResume=true;
      window.denialContexts=[];window.denialSources=[];
      const Native=window.AudioContext;
      window.AudioContext=class extends Native {
        constructor(...args){super(...args);denialContexts.push(this);}
        resume(){return blockAudioResume?Promise.reject(new DOMException('Gesture required','NotAllowedError')):super.resume();}
      };
      const create=Native.prototype.createBufferSource;
      Native.prototype.createBufferSource=function(...args){const source=create.apply(this,args);denialSources.push(source);return source;};
    });
    await page.goto(`http://127.0.0.1:4252/?test=1&backend=webgl2&worker=${worker}`);
    await page.waitForFunction(() => window.__nir?.state().ready&&!__nir.state().loading);
    await page.keyboard.press('Enter');
    const recovery=page.getByRole('button',{name:/Tap to resume sound|点击恢复声音/});
    try {await expect(recovery).toBeVisible();} catch(error) {
      await testInfo.attach('initial-denial-state',{body:JSON.stringify(await page.evaluate(()=>({state:__nir.state(),
        contexts:denialContexts.map(c=>({state:c.state,time:c.currentTime})),
        sources:denialSources.map(s=>({loop:s.loop,duration:s.buffer?.duration,context:s.context.state})),
        hidden:document.hidden,button:document.querySelector('#nir-audio-resume')?.outerHTML})),null,2),contentType:'application/json'});
      throw error;
    }
    // Output can be blocked before even an instant-reveal line gets its
    // first Story tick. Recovery must not require revealing it first.
    await page.waitForFunction(() => __nir.state().dialogue&&__nir.state().paused);
    const interaction=await page.evaluate(() => __nir.state().interaction);
    await page.evaluate(async () => {
      await __nir.action({type:'volume',bus:'bgm',delta:-1});
      await __nir.action({type:'volume',bus:'voice',delta:-1});
    });
    await expect(recovery).toBeHidden();
    await page.waitForFunction(() => !__nir.state().paused);
    await page.evaluate(() => __nir.action({type:'volume',bus:'voice',delta:1}));
    await expect(recovery).toBeVisible();
    await page.waitForFunction(() => __nir.state().paused);
    await page.evaluate(() => {blockAudioResume=false;});
    await page.keyboard.press('Enter');
    try {await expect(recovery).toBeHidden();} catch(error) {
      await testInfo.attach('denial-recovery-state',{body:JSON.stringify(await page.evaluate(()=>({state:__nir.state(),
        output:__nir.diagnostics().host_work.audio_domains,
        contexts:denialContexts.map(c=>({state:c.state,time:c.currentTime})),hidden:document.hidden})),null,2),contentType:'application/json'});
      throw error;
    }
    await page.waitForFunction(() => !__nir.state().paused);
    expect(await page.evaluate(() => __nir.state().interaction)).toBe(interaction);
    expect(errors).toEqual([]);
  });
}
