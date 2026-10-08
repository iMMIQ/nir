import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import {recoverAudioOutput} from '../nir-next/audio-output-helper.js';

const ports={parallel:4271,after_voice:4272,sampled_remaining:4273,parallel_interaction:4274};
const pageErrors=new WeakMap();
test.beforeEach(async({page})=>{const errors=[];pageErrors.set(page,errors);page.on('pageerror',e=>errors.push(e.message));});
test.afterEach(async({page},info)=>{
  const final=await page.evaluate(()=>globalThis.modeSnapshot&&globalThis.__nir ? modeSnapshot() : null).catch(()=>null);
  await fs.writeFile(info.outputPath('terminal.json'),JSON.stringify({status:info.status,errors:pageErrors.get(page),final},null,2)+'\n');
});
async function activate(page,type,fields={}) {
  await page.waitForFunction(({type,fields})=>[...document.querySelectorAll('#actions button')].some(b=>{const a=JSON.parse(b.dataset.action);return !b.disabled&&a.type===type&&Object.entries(fields).every(([k,v])=>a[k]===v);}),{type,fields});
  const rect=await page.evaluate(({type,fields})=>JSON.parse([...document.querySelectorAll('#actions button')].find(b=>{const a=JSON.parse(b.dataset.action);return a.type===type&&Object.entries(fields).every(([k,v])=>a[k]===v);}).dataset.rect),{type,fields});
  await page.mouse.click(rect[0]+rect[2]/2,rect[1]+rect[3]/2);
}
async function boot(page,worker,policy='parallel',seen=[]) {
  await page.addInitScript(()=>{
    globalThis.modeAudio=[];globalThis.modePromptRecoveryRaces=0;
    const create=AudioContext.prototype.createBufferSource;
    AudioContext.prototype.createBufferSource=function(...args){
      const source=create.apply(this,args),row={id:modeAudio.length+1,source,context:this,stops:0,ended:false,started:null,endedAt:null};modeAudio.push(row);
      const start=source.start,stop=source.stop;
      source.start=function(...args){const result=start.apply(this,args);row.started=this.context.currentTime;row.offset=args[1]||0;row.duration=this.buffer.duration;return result;};
      source.stop=function(...args){row.stops++;return stop.apply(this,args);};
      source.addEventListener('ended',()=>{row.ended=true;row.endedAt=this.currentTime;});return source;
    };
    globalThis.modeSnapshot=()=>({state:__nir.state(),promptRecoveryRaces:modePromptRecoveryRaces,audio:modeAudio.filter(r=>r.started!==null).map(r=>({id:r.id,loop:r.source.loop,stops:r.stops,ended:r.ended,started:r.started,endedAt:r.endedAt,clock:r.context.currentTime,contextState:r.context.state,duration:r.duration})),domains:__nir.diagnostics().host_work.audio_domains});
  });
  const origin=`http://127.0.0.1:${ports[policy]}`,fixture=await(await page.request.get(`${origin}/__reading_fixture/delay`)).json();
  for(const path of fixture.voiceObjects)await page.request.get(`${origin}/__reading_fixture/delay?path=${encodeURIComponent(path)}&ms=0`);
  await page.goto(`${origin}/?test=1&worker=${worker}&backend=webgl2`);await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
  if(seen.length){
    await page.evaluate(async seen=>{
      const channel=await(await fetch('/channels/stable.json')).json(),release=await(await fetch(`/releases/${channel.release}.json`)).json();
      const r=indexedDB.open('nir-player-isolated-v1',1),db=await new Promise(ok=>{r.onsuccess=()=>ok(r.result);});
      try{await new Promise((ok,no)=>{const tx=db.transaction('profile','readwrite');tx.objectStore('profile').put(seen,[release.game_id,release.profile]);tx.oncomplete=ok;tx.onabort=()=>no(tx.error);});}finally{db.close();}
    },seen);
    await page.reload();await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
  }
  await activate(page,'new_game');await recoverAudioOutput(page);
  await page.waitForFunction(()=>{const s=__nir.state();return s.dialogue?.id==='intro'&&!s.loading&&!s.paused&&modeAudio.some(r=>r.started!==null&&!r.source.loop&&!r.ended);});
  expect(await page.evaluate(()=>__nir.state().execution.runtime)).toBe(worker==='required'?'worker':'main');
  return fixture;
}
const snapshot=page=>page.evaluate(()=>modeSnapshot());
const identity=s=>({session:s.state.session,interaction:s.state.interaction,position:s.state.position,history:s.state.history_count,tick:s.state.tick_us});
// Audio offsets keep advancing while a choice waits; its plot must stay put.
const plotIdentity=s=>{const {tick,...plot}=identity(s);return {...plot,choice:s.state.choice,variables:s.state.variables};};
const loops=s=>s.audio.filter(r=>r.loop).map(r=>({id:r.id,stops:r.stops}));
async function resumeAutoOutput(page) {
  // Auto may legitimately advance once output is restored. Its voice/plot
  // assertions below check that transition, rather than freezing interaction.
  await page.waitForFunction(()=>!__nir.state().paused||document.querySelector('#nir-audio-resume')?.hidden===false);
  const prompt=page.locator('#nir-audio-resume');
  if(await prompt.isVisible()) {
    await page.waitForFunction(()=>{const b=document.querySelector('#nir-audio-resume');return b.hidden||!b.disabled;});
    if(await prompt.isVisible()) {
      try{await prompt.click({timeout:1000});}
      catch(error){
        // Output may recover after the visibility check. A completed recovery
        // is valid; leave real gesture failures and paused output as failures.
        const recovered=await page.evaluate(()=>!__nir.state().paused&&document.querySelector('#nir-audio-resume').hidden);
        if(error.name!=='TimeoutError'||!recovered)throw error;
        await page.evaluate(()=>modePromptRecoveryRaces++);
      }
    }
  }
  await page.waitForFunction(()=>!__nir.state().paused&&document.querySelector('#nir-audio-resume').hidden);
}
async function complete(page) {
  const before=await snapshot(page);expect(before.state.dialogue.ready).toBe(false);
  await page.keyboard.press('Space');await page.waitForFunction(()=>__nir.state().dialogue?.ready&&!__nir.state().dialogue.gate&&!__nir.state().loading);
  expect((await snapshot(page)).state.interaction).toBe(before.state.interaction);
}
async function evidence(page,info,name,data) {
  await fs.writeFile(info.outputPath(name+'.json'),JSON.stringify({...data,final:await snapshot(page),scope:'Current SDK neutral MP3 fixture, native audio sources/clocks and trusted canvas/keyboard. Not commercial full-route, physical audio, Android or a latency budget.'},null,2)+'\n');
}

for(const worker of ['main','required']) {
  test(`completing text keeps its voice; held skip stops at unread; ${worker}`,async({page},info)=>{
    const errors=[];page.on('pageerror',e=>errors.push(e.message));await boot(page,worker);const before=await snapshot(page);await complete(page);
    const filled=await snapshot(page),voice=before.audio.find(r=>!r.loop);expect(filled.audio.find(r=>r.id===voice.id).stops).toBe(0);expect(filled.audio.find(r=>r.id===voice.id).ended).toBe(false);
    await page.keyboard.down('ControlLeft');await page.waitForTimeout(250);await page.keyboard.up('ControlLeft');
    const stopped=await snapshot(page);expect(stopped.state.interaction).toBe(before.state.interaction);expect(stopped.state.skip).toBe(false);expect(stopped.audio.find(r=>r.id===voice.id).stops).toBe(0);expect(loops(stopped)).toEqual(loops(before));expect(errors).toEqual([]);
    await evidence(page,info,'unread-and-reveal',{worker,before,filled,stopped});
  });
  test(`releasing held skip during asset loading keeps the next seen line; ${worker}`,async({page},info)=>{
    const errors=[];page.on('pageerror',e=>errors.push(e.message));const fixture=await boot(page,worker,'parallel',['read:intro:1','read:arrival:1']);await complete(page);const before=await snapshot(page);
    await page.request.get(`http://127.0.0.1:4271/__reading_fixture/delay?path=${encodeURIComponent(fixture.voiceObjects[1])}&ms=1200`);
    await page.keyboard.down('ControlLeft');await page.waitForFunction(()=>__nir.state().loading);await page.keyboard.up('ControlLeft');
    await page.waitForFunction(()=>__nir.state().dialogue?.id==='arrival'&&!__nir.state().loading&&!__nir.state().paused);await page.waitForTimeout(300);
    const after=await snapshot(page),old=after.audio.find(r=>r.id===before.audio.find(r=>!r.loop).id);expect(after.state.dialogue.id).toBe('arrival');expect(after.state.skip).toBe(false);
    // A delayed barrier can outlast the finite old voice; a naturally ended
    // source needs no redundant native stop. Neither path can leave it live.
    expect(old.stops===1||old.ended).toBe(true);expect(old.stops).toBeLessThanOrEqual(1);expect(loops(after)).toEqual(loops(before));expect(errors).toEqual([]);
    await evidence(page,info,'skip-release-loading',{worker,before,after});
  });
  for(const option of ['walk','stay'])test(`seen skip stops at choices and does not spill into ${option}; ${worker}`,async({page},info)=>{
    const errors=[];page.on('pageerror',e=>errors.push(e.message));await boot(page,worker,'parallel',['read:intro:1','read:arrival:1','read:after_gate:1']);await complete(page);const before=await snapshot(page);
    await activate(page,'toggle_skip');await page.waitForFunction(()=>__nir.state().choice&&!__nir.state().loading);const choice=await snapshot(page);expect(choice.state.skip).toBe(false);
    await page.waitForTimeout(300);const held=await snapshot(page);expect(plotIdentity(held)).toEqual(plotIdentity(choice));expect(loops(held)).toEqual(loops(before));expect(held.audio.filter(r=>!r.loop&&!r.ended&&r.stops===0)).toHaveLength(0);
    await activate(page,'choose',{option});await page.waitForFunction(option=>__nir.state().dialogue?.id===`${option}_line`&&!__nir.state().loading&&!__nir.state().paused,option);
    const branch=await snapshot(page);expect(branch.state.skip).toBe(false);expect(branch.state.variables.affection.value).toBe(option==='walk'?1:0);expect(errors).toEqual([]);
    await evidence(page,info,'seen-choice-'+option,{worker,before,choice,branch});
  });
  for(const policy of ['parallel','after_voice','sampled_remaining'])test(`Auto follows its voice policy, freezes in menus and waits at choices; ${policy}; ${worker}`,async({page},info)=>{
    const errors=[];page.on('pageerror',e=>errors.push(e.message));await boot(page,worker,policy);await complete(page);const before=await snapshot(page),voice=before.audio.find(r=>!r.loop);
    await activate(page,'toggle_auto');await page.waitForFunction(()=>__nir.state().auto&&!__nir.state().paused);await activate(page,'menu');await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
    const menu=await snapshot(page);await page.waitForTimeout(300);const held=await snapshot(page);expect(identity(held)).toEqual(identity(menu));expect(loops(held)).toEqual(loops(before));expect(held.domains.story.buses.bgm.clock_seconds).toBeGreaterThan(menu.domains.story.buses.bgm.clock_seconds);
    await activate(page,'close');await resumeAutoOutput(page);
    await page.waitForFunction(()=>__nir.state().dialogue?.id==='arrival'&&!__nir.state().loading&&!__nir.state().paused);const advanced=await snapshot(page),finished=advanced.audio.find(r=>r.id===voice.id);
    expect(finished.stops).toBe(0);
    if(policy==='sampled_remaining') {
      // This policy freezes a device-position sample, then runs a Story
      // timer. A busy/headless device need not finish at that same wall time.
      // The input observation may precede the committed sample by a few turns.
      const remainder=voice.duration-(voice.clock-voice.started);
      const elapsed=(Number(advanced.state.tick_us)-Number(before.state.tick_us))/1e6;
      expect(elapsed).toBeGreaterThanOrEqual(remainder+.6-.25);
    } else expect(finished.ended).toBe(true);
    if(policy==='after_voice')expect(finished.clock-finished.endedAt).toBeGreaterThanOrEqual(.5);
    await page.waitForFunction(()=>__nir.state().choice&&!__nir.state().loading);const choice=await snapshot(page);await page.waitForTimeout(300);expect(plotIdentity(await snapshot(page))).toEqual(plotIdentity(choice));expect(loops(await snapshot(page))).toEqual(loops(before));expect(errors).toEqual([]);
    await evidence(page,info,'auto-'+policy,{worker,before,menu,held,advanced,choice});
  });
  test(`Auto voice-wait preference can advance while the old voice continues; ${worker}`,async({page},info)=>{
    const errors=[];page.on('pageerror',e=>errors.push(e.message));await boot(page,worker);await complete(page);const before=await snapshot(page),voice=before.audio.find(r=>!r.loop);
    await page.evaluate(()=>__nir.action({type:'auto_wait_voice',enabled:false}));await page.waitForFunction(()=>__nir.state().preferences.auto_wait_voice===false);await activate(page,'toggle_auto');
    await page.waitForFunction(()=>__nir.state().dialogue?.id==='arrival'&&!__nir.state().loading&&!__nir.state().paused);await activate(page,'toggle_auto');await page.waitForFunction(()=>!__nir.state().auto);
    const after=await snapshot(page),continued=after.audio.find(r=>r.id===voice.id);expect(continued.ended).toBe(false);expect(continued.stops).toBe(0);expect(after.state.preferences.voice_continue).toBe(true);expect(loops(after)).toEqual(loops(before));expect(errors).toEqual([]);
    await evidence(page,info,'auto-without-voice-wait',{worker,before,after});
  });
  for(const fixture of ['parallel','parallel_interaction'])test(`Auto respects ${fixture==='parallel'?'disabled continuation':'authored interaction voice lifetime'}; ${worker}`,async({page},info)=>{
    await boot(page,worker,fixture);await complete(page);const before=await snapshot(page),voice=before.audio.find(r=>!r.loop);
    expect(voice.duration-(voice.clock-voice.started)).toBeGreaterThan(2);
    await page.evaluate(disable=>{__nir.action({type:'auto_wait_voice',enabled:false});if(disable)__nir.action({type:'voice_continue',enabled:false});},fixture==='parallel');
    await page.waitForFunction(disable=>!__nir.state().preferences.auto_wait_voice&&__nir.state().preferences.voice_continue===!disable,fixture==='parallel');
    await activate(page,'toggle_auto');await page.waitForFunction(()=>__nir.state().dialogue?.id==='arrival'&&!__nir.state().loading&&!__nir.state().paused);await activate(page,'toggle_auto');
    const after=await snapshot(page),ended=after.audio.find(r=>r.id===voice.id);expect(ended.stops).toBe(1);expect(loops(after)).toEqual(loops(before));expect(pageErrors.get(page)).toEqual([]);
    await evidence(page,info,'auto-continuation-'+fixture,{worker,before,after});
  });
}

for(const worker of ['required','main']) {
  for(const mode of ['auto','held','latched'])for(const failure of ['http','integrity'])
  test(`${mode} survives required voice ${failure} without queued advances after retry; ${worker}`,async({page},info)=>{
    const fixture=await boot(page,worker,'parallel',mode==='held'?['read:intro:1','read:arrival:1']:['read:intro:1']);
    await complete(page);const before=await snapshot(page),origin='http://127.0.0.1:4271',path=fixture.voiceObjects[1];
    let attempts=0,releaseFirst,releaseRetry;
    const first=new Promise(ok=>releaseFirst=ok),retry=new Promise(ok=>releaseRetry=ok);
    await page.context().route(`**/${path}`,async route=>{
      if(++attempts===1){await first;return route.fulfill(failure==='http'?{status:503,body:'temporary failure'}:{status:200,contentType:'audio/mpeg',body:Buffer.from('invalid audio object bytes')}).catch(()=>{});}
      await retry;await route.continue().catch(()=>{});
    });
    try {
      if(mode==='auto'){
        await page.evaluate(()=>__nir.action({type:'auto_wait_voice',enabled:false}));
        await page.waitForFunction(()=>!__nir.state().preferences.auto_wait_voice);await activate(page,'toggle_auto');
      }else if(mode==='held')await page.keyboard.down('ControlLeft');
      else await activate(page,'toggle_skip');
      await page.waitForFunction(()=>__nir.state().loading&&__nir.state().paused);
      await expect.poll(()=>attempts).toBe(1);
      if(mode==='held')await page.keyboard.up('ControlLeft');
      const held=await snapshot(page);
      // Real user input and direct owner input both belong to the blocked
      // interaction. Neither may be delivered into the prepared next line.
      for(let i=0;i<5;i++)await page.keyboard.press('Space');
      await page.mouse.click(200,580);
      await page.evaluate(()=>Promise.all(Array.from({length:30},()=>__nir.action({type:'advance'}))));
      await page.waitForTimeout(700);const waiting=await snapshot(page);
      expect(identity(waiting)).toEqual(identity(held));expect(loops(waiting)).toEqual(loops(before));
      expect(waiting.domains.story.buses.bgm.clock_seconds-held.domains.story.buses.bgm.clock_seconds).toBeGreaterThan(.5);
      releaseFirst();await page.waitForFunction(()=>__nir.state().error!==null);
      const failed=await snapshot(page);await page.waitForTimeout(350);
      expect(identity(await snapshot(page))).toEqual(identity(failed));expect(loops(await snapshot(page))).toEqual(loops(before));
      await activate(page,'retry');await expect.poll(()=>attempts).toBe(2);
      await page.waitForFunction(()=>__nir.state().loading&&__nir.state().retrying);
      const retrying=await snapshot(page);
      await page.evaluate(()=>Promise.all(Array.from({length:20},()=>__nir.action({type:'retry'}))));
      await page.waitForTimeout(350);expect(attempts).toBe(2);expect(identity(await snapshot(page))).toEqual(identity(retrying));
      releaseRetry();await page.waitForFunction(()=>__nir.state().dialogue?.id==='arrival'&&!__nir.state().loading);
      if(mode==='auto')await resumeAutoOutput(page);else {await recoverAudioOutput(page);await page.waitForFunction(()=>!__nir.state().skip);}
      const restored=await snapshot(page);expect(restored.state.dialogue.id).toBe('arrival');expect(restored.state.error).toBeNull();
      expect(restored.state.skip).toBe(false);expect(loops(restored)).toEqual(loops(before));
      expect(restored.state.story_clock.paused_advance_us).toBe(0);
      expect(restored.state.history_count).toBe(before.state.history_count+1);
      expect(restored.audio).toHaveLength(before.audio.length+1);
      if(mode==='auto'){await activate(page,'toggle_auto');await page.waitForFunction(()=>!__nir.state().auto);}
      await page.waitForTimeout(350);const settled=await snapshot(page);
      expect(settled.state.dialogue.id).toBe('arrival');expect(settled.state.interaction).toBe(restored.state.interaction);
      expect(settled.state.history_count).toBe(restored.state.history_count);expect(loops(settled)).toEqual(loops(before));
      if(mode!=='auto'){
        const voice=settled.audio.find(r=>r.id===before.audio.find(r=>!r.loop).id);
        expect(voice.stops===1||voice.ended).toBe(true);expect(voice.stops).toBeLessThanOrEqual(1);
      }
      expect(pageErrors.get(page)).toEqual([]);
      await evidence(page,info,`${mode}-${failure}-retry`,{worker,mode,failure,attempts,before,held,waiting,failed,retrying,restored,settled});
    }finally{releaseFirst();releaseRetry();await page.context().unroute(`**/${path}`).catch(error=>{if(!page.isClosed())throw error;});if(!page.isClosed())await page.keyboard.up('ControlLeft').catch(error=>{if(!page.isClosed())throw error;});}
  });
  for(const option of ['walk','stay'])test(`held skip release after choosing ${option} preserves the unread branch; ${worker}`,async({page},info)=>{
    await boot(page,worker,'parallel',['read:intro:1','read:arrival:1','read:after_gate:1']);
    await complete(page);const before=await snapshot(page);
    try{
      await page.keyboard.down('ControlLeft');await page.waitForFunction(()=>__nir.state().choice&&!__nir.state().loading);
      const choice=await snapshot(page);expect(choice.state.skip).toBe(false);
      await page.waitForTimeout(300);expect(plotIdentity(await snapshot(page))).toEqual(plotIdentity(choice));
      expect(choice.audio.filter(r=>!r.loop&&!r.ended&&r.stops===0)).toHaveLength(0);
      await activate(page,'choose',{option});
      await page.waitForFunction(option=>__nir.state().dialogue?.id===`${option}_line`&&!__nir.state().loading,option);
      const branch=await snapshot(page);expect(branch.state.skip).toBe(false);
      // Repeated keydown while the physical key is still held cannot start
      // a new gesture in the branch that replaced the choice interaction.
      await page.keyboard.down('ControlLeft');await page.waitForTimeout(200);
      await page.keyboard.up('ControlLeft');await page.waitForTimeout(250);
      const released=await snapshot(page);
      expect(released.state.interaction).toBe(branch.state.interaction);expect(released.state.position).toEqual(branch.state.position);
      expect(released.state.history_count).toBe(branch.state.history_count);expect(released.state.skip).toBe(false);
      expect(released.state.variables.affection.value).toBe(option==='walk'?1:0);
      expect(loops(released)).toEqual(loops(before));expect(pageErrors.get(page)).toEqual([]);
      await evidence(page,info,'held-choice-'+option,{worker,option,before,choice,branch,released});
    }finally{await page.keyboard.up('ControlLeft');}
  });
}
