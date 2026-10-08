import {test,expect} from '@playwright/test';

for(const worker of ['required','main'])test(`reading overlays preserve the same music source, ${worker}`,async({page})=>{
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.addInitScript(()=>{
    globalThis.menuAudioAudit={sources:[],stops:0};
    const create=AudioContext.prototype.createBufferSource;
    AudioContext.prototype.createBufferSource=function(...args){
      const source=create.apply(this,args),stop=source.stop;
      menuAudioAudit.sources.push(source);
      source.stop=function(...args){menuAudioAudit.stops++;return stop.apply(this,args);};
      return source;
    };
  });
  // Neutral reader fixture: persistent looping music plus a bound short voice.
  await page.goto(`http://127.0.0.1:4199/?test=1&worker=${worker}&backend=webgl2`);
  await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>__nir.state().dialogue&&menuAudioAudit.sources.some(s=>s.loop&&s.context.state==='running'));
  const sample=()=>page.evaluate(()=>{
    const music=menuAudioAudit.sources.find(s=>s.loop),host=__nir.diagnostics().host_work;
    return {tick:__nir.state().tick_us,screen:__nir.state().screen,musicTime:music.context.currentTime,musicState:music.context.state,
      loops:menuAudioAudit.sources.filter(s=>s.loop).length,stops:menuAudioAudit.stops,buses:host.audio_domains.story.buses,error:__nir.state().error};
  });
  const initial=await sample();
  for(const [type,screen] of [['menu','Menu'],['settings','Settings'],['history','History'],['saves','Saves']]){
    await page.evaluate(type=>__nir.action({type}),type);
    await page.waitForFunction(screen=>__nir.state().screen===screen&&!__nir.state().loading,screen);
    const before=await sample();await page.waitForTimeout(220);const after=await sample();
    expect(after.musicState).toBe('running');expect(after.musicTime-before.musicTime).toBeGreaterThan(.1);
    expect(after.tick).toBe(before.tick);expect(after.loops).toBe(initial.loops);expect(after.stops).toBe(initial.stops);
    expect(after.buses.voice.state).toBe('suspended');expect(after.buses.sfx.state).toBe('suspended');
    await page.evaluate(()=>__nir.action({type:'close'}));
    await page.waitForFunction(()=>__nir.state().screen==='Story'&&!__nir.state().paused);
  }
  await page.evaluate(()=>__nir.action({type:'menu'}));
  await page.waitForFunction(()=>__nir.state().screen==='Menu');
  await page.evaluate(()=>__nir.hidden(true));
  await page.waitForFunction(()=>__nir.diagnostics().host_work.audio_domains.story.buses.bgm.state==='suspended');
  const hidden=await sample();await page.waitForTimeout(180);
  expect((await sample()).musicTime).toBe(hidden.musicTime);
  // Closing while backgrounded must not wake music or voice.
  await page.evaluate(()=>__nir.action({type:'close'}));
  expect((await sample()).musicState).toBe('suspended');
  await page.evaluate(()=>__nir.hidden(false));
  // A real browser may revoke automatic resume after backgrounding. In that
  // case the explicit recovery gesture must keep the original story sources.
  await page.waitForFunction(()=>__nir.diagnostics().host_work.audio_domains.story.buses.bgm.state==='running'||
    document.querySelector('#nir-audio-resume')?.hidden===false);
  if (await page.locator('#nir-audio-resume').isVisible()) {
    const interaction=await page.evaluate(()=>__nir.state().interaction);
    await page.locator('#nir-audio-resume').focus();await page.keyboard.press('Enter');
    expect(await page.evaluate(()=>__nir.state().interaction)).toBe(interaction);
  }
  await page.waitForFunction(()=>__nir.diagnostics().host_work.audio_domains.story.buses.bgm.state==='running');
  const final=await sample();expect(final.loops).toBe(initial.loops);expect(final.error).toBeNull();expect(errors).toEqual([]);
});
