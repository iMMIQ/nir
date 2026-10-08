import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import {recoverAudioOutput} from './audio-output-helper.js';
import {installClosedPointerProbe} from './closed-output-pointer-probe.js';

for(const worker of ['required','main'])test(`prepared restore retains its explicit Continue control, ${worker}`,async({browser},info)=>{
  const context=await browser.newContext({viewport:{width:390,height:844},deviceScaleFactor:2,hasTouch:true}),page=await context.newPage();
  try {
    await page.addInitScript(()=>{
      const Native=AudioContext;
      globalThis.AudioContext=class extends Native {constructor(options={}){super({...options,sinkId:{type:'none'}});}};
    });
    await page.goto(`http://127.0.0.1:4252/?test=1&worker=${worker}&backend=webgl2`);
    await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    await page.keyboard.press('Enter');
    await page.waitForFunction(()=>__nir.state().screen==='Story'&&__nir.state().dialogue&&!__nir.state().loading);
    await recoverAudioOutput(page);
    await page.evaluate(()=>__nir.action({type:'saves'}));
    await page.waitForFunction(()=>__nir.state().screen==='Saves'&&!__nir.state().loading);
    const saved=await page.evaluate(()=>__nir.state());
    await page.evaluate(()=>__nir.action({type:'save',slot:0}));
    await page.waitForFunction(()=>/已保存|Saved/.test(__nir.state().status));
    await page.evaluate(()=>__nir.action({type:'load',slot:0}));
    await page.waitForFunction(session=>__nir.state().session!==session&&__nir.state().screen==='Story'&&__nir.state().paused&&!__nir.state().loading,saved.session);
    const control=page.locator('#actions button[data-action=\'{"type":"continue"}\']');
    await expect(control).toHaveCount(1);await expect(control).toBeEnabled();
    const restored=await page.evaluate(()=>__nir.state());
    expect(restored.variables).toEqual(saved.variables);expect(restored.position).toEqual(saved.position);
    expect(restored.history_count).toBe(saved.history_count);
    await expect(page.locator('#nir-audio-recovery')).toBeHidden();
    const rect=JSON.parse(await control.getAttribute('data-rect'));
    await page.touchscreen.tap(rect[0]+rect[2]/2,rect[1]+rect[3]/2);
    await recoverAudioOutput(page);await expect(control).toHaveCount(0);
    const resumed=await page.evaluate(()=>__nir.state());
    expect(resumed.session).toBe(restored.session);expect(resumed.interaction).toBe(restored.interaction);
    expect(resumed.history_count).toBe(restored.history_count);expect(resumed.error).toBeNull();
    await fs.writeFile(info.outputPath('restore-continue.json'),JSON.stringify({worker,saved,restored,resumed,rect},null,2));
  }finally{await context.close();}
});

// The fault model uses Chrome's explicit silent sink, so this test exercises
// decoding, real-time clocks and UI recovery without depending on local speakers.
// The existing default-output recovery tests remain separate and unchanged.
for(const worker of ['required','main'])for(const locale of ['en-US','zh-CN'])for(const fault of ['pending','reject','closed']) {
  test(`${fault} audio output keeps reading frozen and exposes actionable recovery/saves, ${worker}, ${locale}`,async({browser},info)=>{
    const context=await browser.newContext({locale,viewport:{width:390,height:844},deviceScaleFactor:2,hasTouch:true}),page=await context.newPage();
    try {
    const errors=[];page.on('pageerror',e=>errors.push(e.message));
    if(fault==='closed')await installClosedPointerProbe(page);
    await page.setViewportSize({width:390,height:844});
    await page.addInitScript(fault=>{
      globalThis.failureContexts=[];globalThis.failureSources=[];
      const Native=AudioContext;
      globalThis.AudioContext=class extends Native {
        constructor(options={}){super({...options,sinkId:{type:'none'}});this.routeIndex=failureContexts.length;this.resumeCalls=0;failureContexts.push(this);}
        get state(){return this.routeIndex===2&&fault==='closed'?'closed':super.state;}
        resume(){this.resumeCalls++;if(this.routeIndex!==2)return super.resume();return fault==='pending'?new Promise(()=>{}):Promise.reject(new DOMException(fault==='closed'?'Closed output':'Gesture denied',fault==='closed'?'InvalidStateError':'NotAllowedError'));}
      };
      const create=Native.prototype.createBufferSource;
      Native.prototype.createBufferSource=function(...args){
        const source=create.apply(this,args),start=source.start,stop=source.stop;
        const record={source,stops:0};source.start=function(...args){failureSources.push(record);return start.apply(this,args);};
        source.stop=function(...args){record.stops++;return stop.apply(this,args);};return source;
      };
    },fault);
    await page.goto(`http://127.0.0.1:4252/?test=1&worker=${worker}&backend=webgl2`);
    await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    await page.keyboard.press('Enter');
    await page.waitForFunction(()=>__nir.state().screen==='Story'&&__nir.state().dialogue&&__nir.state().paused&&!__nir.state().loading);
    const status=page.locator('#nir-audio-recovery-status'),resume=page.locator('#nir-audio-resume'),saves=page.locator('#nir-audio-recovery-saves');
    const chinese=locale==='zh-CN';
    await expect(status).toContainText(fault==='pending'?(chinese?'仍在等待声音恢复':'Still waiting for sound'):
      fault==='reject'?(chinese?'声音尚未恢复':'Sound has not resumed'):(chinese?'声音不可用':'Sound is unavailable'));
    await expect(saves).toBeVisible();
    if(fault==='reject')await expect(resume).toBeEnabled();else await expect(resume).toBeDisabled();
    const layouts=[];
    for(const viewport of [{width:390,height:844},{width:844,height:390},{width:390,height:844}]) {
      await page.setViewportSize(viewport);
      await expect.poll(()=>page.evaluate(()=>{
        const panel=document.querySelector('#nir-audio-recovery').getBoundingClientRect();
        const controls=[...document.querySelectorAll('#actions button')].map(b=>({
          action:JSON.parse(b.dataset.action),rect:JSON.parse(b.dataset.rect),
        })).filter(n=>['menu','history','toggle_auto','toggle_skip','toggle_interface'].includes(n.action.type));
        return {wide:panel.width>=Math.min(480,innerWidth-32)-1,
          rotated:controls.find(n=>n.action.type==='toggle_interface')?.rect[1]===(innerWidth<650?68:16),
          below:controls.every(n=>panel.top>=n.rect[1]+n.rect[3]+8),
          reachable:controls.every(n=>document.elementFromPoint(n.rect[0]+n.rect[2]/2,n.rect[1]+n.rect[3]/2)?.id==='stage'),
          bounded:panel.left>=15&&panel.right<=innerWidth-15&&panel.bottom<=innerHeight-15,
          continued:[...document.querySelectorAll('#actions button')].some(b=>JSON.parse(b.dataset.action).type==='continue')};
      })).toEqual({wide:true,rotated:true,below:true,reachable:true,bounded:true,continued:false});
      layouts.push(await page.evaluate(()=>{
        const panel=document.querySelector('#nir-audio-recovery'),r=panel.getBoundingClientRect();
        return {width:innerWidth,height:innerHeight,panel:{x:r.x,y:r.y,width:r.width,height:r.height},
          scrollable:panel.scrollHeight>panel.clientHeight};
      }));
      if(viewport.width===844)await page.screenshot({path:info.outputPath('audio-recovery-landscape.png')});
    }
    if(fault==='closed'){
      // Stress short landscape and enlarged host text independently of the
      // game's font preference. Overflow must scroll inside the card.
      await page.setViewportSize({width:844,height:260});
      await page.locator('#nir-audio-recovery').evaluate(p=>p.style.fontSize='32px');
      await expect.poll(()=>page.evaluate(()=>{
        const p=document.querySelector('#nir-audio-recovery'),r=p.getBoundingClientRect();
        return r.bottom<=innerHeight-15&&p.scrollHeight>p.clientHeight;
      })).toBe(true);
      await saves.evaluate(b=>b.scrollIntoView({block:'nearest'}));
      await expect.poll(()=>page.evaluate(()=>{
        const p=document.querySelector('#nir-audio-recovery').getBoundingClientRect(),s=document.querySelector('#nir-audio-recovery-saves').getBoundingClientRect();
        return s.top>=p.top&&s.bottom<=p.bottom&&document.elementFromPoint(s.x+s.width/2,s.y+s.height/2)?.id==='nir-audio-recovery-saves';
      })).toBe(true);
      layouts.push(await page.evaluate(()=>{
        const p=document.querySelector('#nir-audio-recovery'),r=p.getBoundingClientRect();
        return {width:innerWidth,height:innerHeight,fontSize:32,panel:{x:r.x,y:r.y,width:r.width,height:r.height},scrollable:p.scrollHeight>p.clientHeight};
      }));
      await page.screenshot({path:info.outputPath('audio-recovery-compact.png')});
      await page.locator('#nir-audio-recovery').evaluate(p=>{p.style.fontSize='16px';p.scrollTop=0;});
      await page.setViewportSize({width:390,height:844});
      await expect.poll(()=>page.evaluate(()=>JSON.parse([...document.querySelectorAll('#actions button')]
        .find(b=>JSON.parse(b.dataset.action).type==='toggle_interface').dataset.rect)[1])).toBe(68);
    }
    await fs.writeFile(info.outputPath('audio-recovery-layout.json'),JSON.stringify({worker,locale,fault,layouts},null,2));
    const frozen=await page.evaluate(()=>({tick:__nir.state().tick_us,interaction:__nir.state().interaction,
      history:__nir.state().history_count,variables:__nir.state().variables,position:__nir.state().position}));
    const attempts=await page.evaluate(()=>failureContexts[2].resumeCalls);
    // Key unlock precedes the async primary action. Immediate denial settles
    // between them; an unresolved request instead coalesces both calls.
    const initialAttempts=fault==='closed'?0:fault==='pending'?1:2;
    expect(attempts).toBe(initialAttempts);
    const output=await page.evaluate(()=>__nir.diagnostics().host_work.audio_domains.story.buses.voice);
    expect(output.resume_attempts).toBe(initialAttempts);expect(output.resume_pending).toBe(fault==='pending');
    if(fault==='pending'){
      expect(output.pending_gesture).toBe(true);expect(output.resume_wait_ms).toBeGreaterThanOrEqual(1500);expect(output.last_resume_ms).toBeNull();
    }else if(fault==='reject')expect(output.resume_error).toBe(true);
    else expect(output.state).toBe('closed');
    // A context may render a constructor quantum before its initial suspend.
    // Diagnostics must report that real clock, and the blocked source must
    // retain it; the baseline is not necessarily zero.
    expect(output.clock_seconds).toBe(await page.evaluate(()=>failureContexts[2].currentTime));
    for(let i=0;i<20;i++)await page.dispatchEvent('body','keydown',{key:'Enter',repeat:true,bubbles:true});
    for(let i=0;i<3;i++){
      if(fault==='reject')await resume.click();
      else await resume.evaluate(button=>button.click());
    }
    expect(await page.evaluate(()=>failureContexts[2].resumeCalls)).toBe(attempts+(fault==='reject'?3:0));
    expect(await page.evaluate(()=>__nir.diagnostics().host_work.audio_domains.story.buses.voice.clock_seconds)).toBe(output.clock_seconds);
    expect(await page.evaluate(()=>({tick:__nir.state().tick_us,interaction:__nir.state().interaction,
      history:__nir.state().history_count,variables:__nir.state().variables,position:__nir.state().position}))).toEqual(frozen);
    expect(await page.evaluate(()=>failureSources.find(r=>r.source.loop).stops)).toBe(0);
    if(fault!=='reject') {
      const tapAction=async type=>{
        const rect=await page.evaluate(type=>{
          const b=[...document.querySelectorAll('#actions button')].find(b=>JSON.parse(b.dataset.action).type===type);
          return JSON.parse(b.dataset.rect);
        },type);
        await page.touchscreen.tap(rect[0]+rect[2]/2,rect[1]+rect[3]/2);
      };
      const beforeMenu=fault==='closed'?await page.evaluate(()=>closedPointerProbe()):null;
      await tapAction('menu');
      try{await page.waitForFunction(()=>__nir.state().screen==='Menu',null,{timeout:10000});}
      catch(error){
        if(fault==='closed'){
          await fs.writeFile(info.outputPath('closed-pointer-failure.json'),JSON.stringify({before:beforeMenu,after:await page.evaluate(()=>closedPointerProbe())},null,2)+'\n');
          await page.screenshot({path:info.outputPath('closed-pointer-failure.png')});
        }
        throw error;
      }

      await expect(page.locator('#nir-audio-recovery')).toBeHidden();
      await tapAction('close');await page.waitForFunction(()=>__nir.state().screen==='Story'&&__nir.state().paused);
      await expect(saves).toBeVisible();
      expect(await page.evaluate(()=>({tick:__nir.state().tick_us,interaction:__nir.state().interaction,
        history:__nir.state().history_count,variables:__nir.state().variables,position:__nir.state().position}))).toEqual(frozen);
      expect(await page.evaluate(()=>failureContexts[2].resumeCalls)).toBe(attempts+(fault==='pending'?1:0));
      if(fault==='pending'){
        const resumed=await page.evaluate(()=>__nir.diagnostics().host_work.audio_domains.story.buses.voice);
        expect(resumed.pending_gesture).toBe(true);expect(resumed.pending_suspended).toBe(false);
        await expect(resume).toBeDisabled();
      }
      expect(await page.evaluate(()=>failureSources.find(r=>r.source.loop).stops)).toBe(0);
    }
    expect((await saves.boundingBox()).height).toBeGreaterThanOrEqual(44);
    await page.screenshot({path:info.outputPath('audio-recovery.png')});
    const recovery=await page.evaluate(()=>({state:__nir.state(),diagnostics:__nir.diagnostics(),button:{disabled:document.querySelector('#nir-audio-resume').disabled,text:document.querySelector('#nir-audio-resume').textContent},status:document.querySelector('#nir-audio-recovery-status').textContent}));
    await saves.focus();await page.keyboard.press('Enter');
    await page.waitForFunction(()=>__nir.state().screen==='Saves'&&!__nir.state().loading);
    const save=page.getByRole('button',{name:chinese?'保存':'Save',exact:true}).first();
    await expect(save).toBeVisible();
    await save.focus();await page.keyboard.press('Enter');
    const saved=()=>page.evaluate(async()=>{
      for(const {name} of await indexedDB.databases()) {
        const db=await new Promise((ok,no)=>{const r=indexedDB.open(name);r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});
        if(!db.objectStoreNames.contains('saves')){db.close();continue;}
        const rows=await new Promise((ok,no)=>{const tx=db.transaction('saves'),r=tx.objectStore('saves').getAll();tx.oncomplete=()=>ok(r.result);tx.onerror=()=>no(tx.error);});db.close();
        const row=rows.find(r=>r.envelope?.slot===0);if(row)return row.envelope.snapshot;
      }return null;
    });
    await expect.poll(saved).not.toBeNull();
    const stored=await saved();expect(stored.variables).toEqual(frozen.variables);expect(stored.history).toHaveLength(frozen.history);
    expect(stored.history[0].voices).toHaveLength(1);
    expect(await page.evaluate(()=>({tick:__nir.state().tick_us,interaction:__nir.state().interaction,position:__nir.state().position})))
      .toEqual({tick:frozen.tick,interaction:frozen.interaction,position:frozen.position});
    expect(errors).toEqual([]);
    await fs.writeFile(info.outputPath('audio-recovery-state.json'),JSON.stringify({worker,locale,fault,frozen,output,recovery,stored,final:await page.evaluate(()=>({state:__nir.state(),diagnostics:__nir.diagnostics()}))},null,2)+'\n');
    } finally {await context.close();}
  });
}

for(const worker of ['required','main'])test(`recovery helper observes authorized pending output without clicking its disabled control, ${worker}`,async({page},info)=>{
  await page.addInitScript(()=>{
    window.helperContexts=[];
    const Native=AudioContext;
    window.AudioContext=class extends Native{
      constructor(options={}){super({...options,sinkId:{type:'none'}});this.routeIndex=helperContexts.length;this.calls=0;this.held=false;helperContexts.push(this);}
      get state(){return this.held?'suspended':super.state;}
      resume(){
        this.calls++;
        if(this.routeIndex!==2)return super.resume();
        // Grant native permission on the real gesture, then hold its silent
        // clock before source playback; this test owns the deferred result.
        this.held=true;
        const permitted=super.resume().then(()=>super.suspend());
        return new Promise((resolve,reject)=>{
          window.finishHeld=()=>permitted.then(()=>super.resume()).then(()=>{this.held=false;resolve();},reject);
        });
      }
    };
    window.helperClicks=0;
    document.addEventListener('click',e=>{if(e.target.id==='nir-audio-resume')helperClicks++;},true);
  });
  await page.goto(`http://127.0.0.1:4252/?test=1&worker=${worker}&backend=webgl2`);
  await page.waitForFunction(()=>window.__nir?.state().ready&&!__nir.state().loading);
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>__nir.state().screen==='Story'&&__nir.state().dialogue&&__nir.state().paused&&!__nir.state().loading&&typeof finishHeld==='function');
  await expect(page.locator('#nir-audio-resume')).toBeDisabled();
  const before=await page.evaluate(()=>({tick:__nir.state().tick_us,history:__nir.state().history_count,interaction:__nir.state().interaction}));
  await page.evaluate(()=>setTimeout(()=>finishHeld(),200));
  const start=Date.now();await recoverAudioOutput(page);const elapsedMs=Date.now()-start;
  expect(elapsedMs).toBeLessThan(2000);
  expect(await page.evaluate(()=>helperClicks)).toBe(0);
  expect(await page.evaluate(()=>helperContexts[2].calls)).toBe(1);
  expect(await page.evaluate(()=>({history:__nir.state().history_count,interaction:__nir.state().interaction}))).toEqual({history:before.history,interaction:before.interaction});
  await fs.writeFile(info.outputPath('audio-recovery-helper.json'),JSON.stringify({worker,elapsedMs,before,after:await page.evaluate(()=>({state:__nir.state(),diagnostics:__nir.diagnostics()}))},null,2)+'\n');
});
