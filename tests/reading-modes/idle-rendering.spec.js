import {test,expect} from '@playwright/test';
import {recoverAudioOutput} from '../nir-next/audio-output-helper.js';

async function activate(page,type) {
  await page.waitForFunction(type=>[...document.querySelectorAll('#actions button')].some(b=>!b.disabled&&JSON.parse(b.dataset.action).type===type),type);
  const rect=await page.evaluate(type=>JSON.parse([...document.querySelectorAll('#actions button')].find(b=>!b.disabled&&JSON.parse(b.dataset.action).type===type).dataset.rect),type);
  await page.mouse.click(rect[0]+rect[2]/2,rect[1]+rect[3]/2);
}
async function snapshot(page) {
  return page.evaluate(()=>{
    const state=__nir.state(),diagnostics=__nir.diagnostics(),stages=diagnostics.performance.stages;
    return {
      plot:{session:state.session,interaction:state.interaction,position:state.position,history:state.history_count,dialogue:state.dialogue},
      frames:state.frames,tick:Number(state.tick_us),paused:state.paused,
      projections:stages.projection?.count||0,draws:stages.draw?.count||0,
      bgmClock:diagnostics.host_work.audio_domains.story.buses.bgm.clock_seconds,
    };
  });
}

for(const worker of ['required','main'])test(`audio-only clock reuses the idle picture and still advances playback; ${worker}`,async({page})=>{
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto(`http://127.0.0.1:4271/?test=1&worker=${worker}&backend=webgl2`);
  await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
  await activate(page,'new_game');await recoverAudioOutput(page);
  await page.waitForFunction(()=>__nir.state().dialogue?.id==='intro'&&!__nir.state().loading&&!__nir.state().paused);
  await page.keyboard.press('Space');
  await page.waitForFunction(()=>__nir.state().dialogue?.id==='intro'&&__nir.state().dialogue.ready&&!__nir.state().loading&&!__nir.state().paused);
  await page.mouse.move(1,1);await page.waitForTimeout(500);
  const before=await snapshot(page);await page.waitForTimeout(1200);const after=await snapshot(page);
  expect(after.plot).toEqual(before.plot);expect(after.frames).toBe(before.frames);
  expect(after.paused).toBe(false);expect(after.tick-before.tick).toBeGreaterThan(200000);
  expect(after.bgmClock-before.bgmClock).toBeGreaterThan(.2);
  // Allow the periodic safety projection and a finite voice ending. Count
  // work rather than CPU milliseconds, which vary across browser runners.
  expect(after.projections-before.projections).toBeLessThanOrEqual(Math.max(8,Math.ceil((after.draws-before.draws)/4)));
  await activate(page,'menu');await page.waitForFunction(()=>__nir.state().screen==='Menu');
  await activate(page,'close');await recoverAudioOutput(page);
  await page.waitForFunction(()=>__nir.state().screen==='Story'&&!__nir.state().paused);
  expect((await snapshot(page)).plot).toEqual(before.plot);expect(errors).toEqual([]);
});
