import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import {recoverAudioOutput} from '../nir-next/audio-output-helper.js';
import {installDeferredInputProbe} from './deferred-input-probe.js';
const origin='http://127.0.0.1:4275',traces=new WeakMap();
const identity=s=>({session:s.session,interaction:s.interaction,position:s.position,history:s.history_count,sequence:s.sequence});
const snapshot=page=>page.evaluate(()=>({state:__nir.state(),probe:deferredInputProbe.snapshot(),audio:deferredInputAudio.filter(r=>r.start!==null).map(r=>({loop:r.source.loop,start:r.start,duration:r.duration,stops:r.stops,ended:r.ended,clock:r.context.currentTime,state:r.context.state}))}));
const bgm=s=>s.audio.find(r=>r.loop);
async function checkpoint(page,info,name){const row=await snapshot(page);traces.get(page).stages[name]=row;await fs.writeFile(info.outputPath('stages.json'),JSON.stringify(traces.get(page),null,2)+'\n');return row;}
async function activate(page,type){
  await page.waitForFunction(type=>[...document.querySelectorAll('#actions button')].some(b=>!b.disabled&&JSON.parse(b.dataset.action).type===type),type);
  const rect=await page.evaluate(type=>JSON.parse([...document.querySelectorAll('#actions button')].find(b=>!b.disabled&&JSON.parse(b.dataset.action).type===type).dataset.rect),type);
  await page.mouse.click(rect[0]+rect[2]/2,rect[1]+rect[3]/2);
}
async function boot(page,worker){
  await installDeferredInputProbe(page);await page.goto(`${origin}/?test=1&worker=${worker}&backend=webgl2`);
  await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);await activate(page,'new_game');
  await page.waitForFunction(()=>__nir.state().dialogue?.id==='letter'&&!__nir.state().loading&&deferredInputAudio.some(r=>r.start!==null&&!r.source.loop));
  await recoverAudioOutput(page);await page.waitForFunction(()=>!__nir.state().paused&&Object.values(__nir.diagnostics().host_work.audio_domains.story.buses).every(b=>b.state==='running'&&!b.resume_pending));
  expect(await page.evaluate(()=>__nir.state().execution.runtime)).toBe(worker==='required'?'worker':'main');
}
async function ready(page){await page.waitForFunction(()=>__nir.state().dialogue?.id==='letter'&&__nir.state().dialogue.ready&&!__nir.state().loading&&!__nir.state().paused);}
async function input(page,kind,point=[300,300]){if(kind==='mouse')await page.mouse.click(...point);else if(kind==='touch')await page.touchscreen.tap(...point);else await page.keyboard.press(kind);}
async function flush(page){await page.evaluate(()=>deferredInputProbe.flush());await page.waitForFunction(()=>!deferredInputProbe.snapshot().pending);await page.waitForTimeout(500);}
test.use({hasTouch:true});
test.beforeEach(({page})=>{const row={errors:[],stages:{},scope:'Real compiled MP3 author Gate, native clocks, trusted inputs. Worker UI transport deliberately deferred, bytes/real owner retained; Main uses actual local dispatch. Not physical audio or Android proof.'};traces.set(page,row);page.on('pageerror',e=>row.errors.push(e.message));});
test.afterEach(async({page},info)=>{await fs.writeFile(info.outputPath('terminal.json'),JSON.stringify({...traces.get(page),status:info.status,final:await snapshot(page).catch(()=>null)},null,2)+'\n');});

for(const worker of ['required','main'])for(const kind of ['Space','Enter','mouse','touch'])test(`loading input cannot advance after its deferred query; ${kind}; ${worker}`,async({page},info)=>{
  const fixture=await(await page.request.get(`${origin}/__reading_fixture/delay`)).json(),path=fixture.voiceObjects[1];expect(path.endsWith('.mp3')).toBe(true);
  let release;const media=new Promise(ok=>release=ok);await page.context().route(`**/${path}`,async route=>{await media;await route.continue().catch(()=>{});});
  try{
    await boot(page,worker);await page.keyboard.press('Space');await page.waitForFunction(()=>__nir.state().loading&&__nir.state().dialogue?.gate);
    await page.locator('#stage').focus();const loading=await checkpoint(page,info,'loading');
    await page.evaluate(mode=>deferredInputProbe.hold(mode),['mouse','touch'].includes(kind)?'pointer':'primary');await input(page,kind);await page.waitForTimeout(150);const pressed=await checkpoint(page,info,'pressed');
    const event=pressed.probe.rows.filter(r=>r.type===(['mouse','touch'].includes(kind)?'pointerdown':'keydown')).at(-1);expect(event.trusted).toBe(true);expect(event.state.loading).toBe(true);
    release();await ready(page);const prepared=await checkpoint(page,info,'prepared');
    expect(prepared.state.interaction).toBe(loading.state.interaction);expect(prepared.state.history_count).toBe(loading.state.history_count);
    await flush(page);const held=await checkpoint(page,info,'held');expect(identity(held.state)).toEqual(identity(prepared.state));
    expect(bgm(held).start).toBe(bgm(loading).start);expect(bgm(held).stops).toBe(0);expect(bgm(held).clock-bgm(prepared).clock).toBeGreaterThan(.3);
    expect(held.audio.filter(r=>!r.loop&&Math.abs(r.duration-3)<.01)).toHaveLength(0);expect(held.audio.find(r=>Math.abs(r.duration-2)<.01).stops).toBe(0);
    await input(page,kind);await page.waitForFunction(()=>__nir.state().dialogue?.id==='after_gate'&&!__nir.state().loading&&!__nir.state().paused);
    const fresh=await checkpoint(page,info,'fresh');expect(fresh.state.history_count).toBe(prepared.state.history_count+1);expect(fresh.audio.filter(r=>!r.loop&&Math.abs(r.duration-3)<.01)).toHaveLength(1);expect(traces.get(page).errors).toEqual([]);
  }finally{release();await page.context().unroute(`**/${path}`).catch(error=>{if(!page.isClosed())throw error;});}
});

for(const kind of ['mouse','touch'])test(`a deferred Menu close cannot become a Story click; ${kind}`,async({page},info)=>{
  await boot(page,'required');await ready(page);await activate(page,'menu');await page.waitForFunction(()=>__nir.state().screen==='Menu');await page.locator('#stage').focus();
  const menu=await checkpoint(page,info,'menu'),rect=await page.evaluate(()=>JSON.parse([...document.querySelectorAll('#actions button')].find(b=>!b.disabled&&JSON.parse(b.dataset.action).type==='close').dataset.rect));
  await page.evaluate(()=>deferredInputProbe.hold('pointer'));await input(page,kind,[rect[0]+rect[2]/2,rect[1]+rect[3]/2]);await page.waitForFunction(()=>deferredInputProbe.snapshot().held===1);
  const pressed=await checkpoint(page,info,'pressed');expect(pressed.probe.rows.filter(r=>r.type==='pointerdown').at(-1).state.screen).toBe('Menu');
  await page.keyboard.press('Escape');await recoverAudioOutput(page);await ready(page);const closed=await checkpoint(page,info,'closed');expect(closed.state.history_count).toBe(menu.state.history_count);
  await flush(page);const held=await checkpoint(page,info,'held');expect(identity(held.state)).toEqual(identity(closed.state));expect(bgm(held).start).toBe(bgm(menu).start);expect(bgm(held).stops).toBe(0);
  await input(page,kind);await page.waitForFunction(()=>__nir.state().dialogue?.id==='after_gate'&&!__nir.state().loading&&!__nir.state().paused);expect((await checkpoint(page,info,'fresh')).state.history_count).toBe(closed.state.history_count+1);expect(traces.get(page).errors).toEqual([]);
});

test('loading navigation guard keeps mode, Menu and Retry controls usable',async({page},info)=>{
  const fixture=await(await page.request.get(`${origin}/__reading_fixture/delay`)).json(),path=fixture.voiceObjects[1];let release,attempts=0;const media=new Promise(ok=>release=ok);
  await page.context().route(`**/${path}`,async route=>{if(++attempts===1){await media;await route.fulfill({status:503,body:'temporary Sfx failure'}).catch(()=>{});}else await route.continue().catch(()=>{});});
  try{
    await boot(page,'required');await page.keyboard.press('Space');await page.waitForFunction(()=>__nir.state().loading&&__nir.state().dialogue?.gate);const loading=await checkpoint(page,info,'loading');
    await activate(page,'toggle_auto');await page.waitForFunction(()=>__nir.state().auto);await activate(page,'toggle_auto');await page.waitForFunction(()=>!__nir.state().auto);
    await activate(page,'toggle_skip');await page.waitForFunction(()=>__nir.state().skip);await activate(page,'toggle_skip');await page.waitForFunction(()=>!__nir.state().skip);
    await activate(page,'menu');await page.waitForFunction(()=>__nir.state().screen==='Menu');await activate(page,'close');await page.waitForFunction(()=>__nir.state().screen==='Story');
    const controls=await checkpoint(page,info,'controls');expect(identity(controls.state)).toEqual(identity(loading.state));expect(controls.state.loading).toBe(true);expect(bgm(controls).stops).toBe(0);
    release();await page.waitForFunction(()=>__nir.state().error!==null);await activate(page,'retry');await expect.poll(()=>attempts).toBe(2);
    await page.waitForFunction(()=>!__nir.state().loading&&deferredInputAudio.some(r=>r.start!==null&&Math.abs(r.duration-2)<.01));await recoverAudioOutput(page);await ready(page);
    const restored=await checkpoint(page,info,'restored');expect(restored.state.history_count).toBe(loading.state.history_count);expect(restored.state.error).toBeNull();expect(restored.state.story_clock.paused_advance_us).toBe(0);expect(bgm(restored).start).toBe(bgm(loading).start);expect(bgm(restored).stops).toBe(0);expect(traces.get(page).errors).toEqual([]);
  }finally{release();await page.context().unroute(`**/${path}`).catch(error=>{if(!page.isClosed())throw error;});}
});
