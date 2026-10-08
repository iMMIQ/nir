import {test, expect} from '@playwright/test';
import {recoverAudioOutput} from './audio-output-helper.js';

const origin = 'http://127.0.0.1:4252';
const voiceButton = page => page.getByRole('button', {name:/Auto waits for voice|自动等待语音/});
const continueButton = page => page.getByRole('button', {name:/Continue voice on advance|翻页语音续播/});

async function boot(page, worker) {
  await page.goto(`${origin}/?test=1&worker=${worker}&backend=webgl2`);
  await page.waitForFunction(() => globalThis.__nir?.state().ready && !__nir.state().loading);
}
async function showVoicePreference(page) {
  await page.evaluate(() => __nir.action({type:'settings'}));
  await page.waitForFunction(() => __nir.state().screen === 'Settings' && !__nir.state().loading);
  for (let i = 0; i < 30; i++) {
    // Cropped controls remain in the accessible mirror, but cannot activate.
    // Scroll until the actual canvas control is complete and enabled.
    if (await voiceButton(page).count() && await voiceButton(page).isEnabled()) break;
    const offset=await page.evaluate(()=>__nir.state().scrolls.find(s=>s.region==='settings')?.offset);
    await page.evaluate(() => __nir.action({type:'scroll', region:'settings', delta:1}));
    await page.waitForFunction(old=>__nir.state().scrolls.find(s=>s.region==='settings')?.offset!==old,offset);
  }
  await expect(voiceButton(page)).toBeVisible();
  await expect(voiceButton(page)).toBeEnabled();
  expect(JSON.parse(await voiceButton(page).getAttribute('data-rect'))[3]).toBeGreaterThanOrEqual(44);
}
async function showContinuationPreference(page) {
  await page.evaluate(() => __nir.action({type:'settings'}));
  await page.waitForFunction(() => __nir.state().screen === 'Settings' && !__nir.state().loading);
  for (let i=0;i<30;i++) {
    const rect=await continueButton(page).count() ? JSON.parse(await continueButton(page).getAttribute('data-rect')) : null;
    if (rect?.[3]>=44) break;
    await page.evaluate(() => __nir.action({type:'scroll',region:'settings',delta:1}));
    await page.waitForTimeout(60);
  }
  await expect(continueButton(page)).toBeVisible();
  // These DOM buttons are hidden keyboard/screen-reader mirrors. The canvas
  // uses the projected hit rectangle, not the mirror's CSS box.
  expect(JSON.parse(await continueButton(page).getAttribute('data-rect'))[3]).toBeGreaterThanOrEqual(44);
}
async function committed(page, store, predicate) {
  await expect.poll(()=>page.evaluate(async ({store, predicate}) => {
    for (const {name} of await indexedDB.databases()) {
      const db = await new Promise((ok,no) => {const r=indexedDB.open(name);r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});
      if (!db.objectStoreNames.contains(store)) {db.close();continue;}
      const records = await new Promise((ok,no) => {const tx=db.transaction(store),r=tx.objectStore(store).getAll();tx.oncomplete=()=>ok(r.result);tx.onerror=()=>no(tx.error);});
      db.close();
      if (predicate === 'voice-off' ? records.some(r=>r.auto_wait_voice===false) : predicate==='continue-off' ? records.some(r=>r.voice_continue===false) : records.length>0) return true;
    }
    return false;
  }, {store, predicate})).toBe(true);
}

for (const worker of ['required','main']) {
  test(`viewport pages keep the current voice until actual dialogue completion, ${worker}`, async ({page}) => {
    const errors=[];page.on('pageerror',e=>errors.push(e.message));
    await page.addInitScript(() => {
      globalThis.pagingVoice=[];
      const create=AudioContext.prototype.createBufferSource;
      AudioContext.prototype.createBufferSource=function(...args) {
        const source=create.apply(this,args),start=source.start,stop=source.stop;
        let record;
        source.start=function(...args) {
          record={source,stops:0,ended:false,duration:source.buffer.duration};source.addEventListener('ended',()=>{record.ended=true;});pagingVoice.push(record);
          return start.apply(this,args);
        };
        source.stop=function(...args) {if(record)record.stops++;return stop.apply(this,args);};return source;
      };
    });
    await page.setViewportSize({width:390,height:844});
    await page.goto(`http://127.0.0.1:4257/?test=1&worker=${worker}&backend=webgl2`);
    await page.waitForFunction(() => globalThis.__nir?.state().ready && !__nir.state().loading);
    await page.evaluate(() => __nir.action({type:'voice_continue',enabled:false}));
    await page.keyboard.press('Enter');
    await page.waitForFunction(() => __nir.state().screen==='Story' && !__nir.state().loading);
    await recoverAudioOutput(page);
    await page.waitForFunction(() => __nir.state().dialogue?.ready && __nir.state().scrolls.some(s=>s.region==='dialogue' && s.max>0));
    // Instant reveal follows the last visible line. Browse from the top to
    // establish that the next input is a viewport page, not source completion.
    for(let i=0;i<30;i++) {
      const view=await page.evaluate(() => __nir.state().scrolls.find(s=>s.region==='dialogue'));
      if(view.offset===0)break;
      await page.evaluate(() => __nir.action({type:'scroll',region:'dialogue',delta:-1}));
      await page.waitForFunction(old => __nir.state().scrolls.find(s=>s.region==='dialogue')?.offset<old,view.offset);
    }
    const before=await page.evaluate(() => ({interaction:__nir.state().interaction,offset:__nir.state().scrolls.find(s=>s.region==='dialogue').offset}));
    expect(before.offset).toBe(0);
    await page.keyboard.press('Space');
    await page.waitForFunction(offset => __nir.state().scrolls.find(s=>s.region==='dialogue')?.offset>offset,before.offset);
    expect(await page.evaluate(() => __nir.state().interaction)).toBe(before.interaction);
    expect(await page.evaluate(() => pagingVoice.find(v=>!v.source.loop && v.duration>20).stops)).toBe(0);
    expect(await page.evaluate(() => pagingVoice.find(v=>!v.source.loop && v.duration>20).ended)).toBe(false);
    await page.setViewportSize({width:844,height:390});
    await page.waitForFunction(() => document.querySelector('canvas').width===844);
    expect(await page.evaluate(() => __nir.state().interaction)).toBe(before.interaction);
    for(let i=0;i<30;i++) {
      const view=await page.evaluate(() => __nir.state().scrolls.find(s=>s.region==='dialogue'));
      if(!view || view.offset>=view.max-.5)break;
      await page.evaluate(() => __nir.action({type:'scroll',region:'dialogue',delta:1}));
      await page.waitForFunction(old => __nir.state().scrolls.find(s=>s.region==='dialogue').offset>old,view.offset);
    }
    const view=await page.evaluate(() => __nir.state().scrolls.find(s=>s.region==='dialogue'));
    expect(view.offset).toBeGreaterThanOrEqual(view.max-.5);
    expect(await page.evaluate(() => pagingVoice.find(v=>!v.source.loop && v.duration>20).stops)).toBe(0);
    await page.keyboard.press('Space');
    await page.waitForFunction(i => __nir.state().interaction!==i && !__nir.state().loading,before.interaction);
    await page.waitForFunction(() => pagingVoice.find(v=>!v.source.loop && v.duration>20).ended);
    expect(await page.evaluate(() => pagingVoice.find(v=>!v.source.loop && v.duration>20).stops)).toBe(1);
    expect(await page.evaluate(() => __nir.state().error)).toBeNull();expect(errors).toEqual([]);
  });
  test(`voice continuation is reachable, persists and stays global across load, ${worker}`, async ({page},testInfo) => {
    const errors=[];page.on('pageerror',e=>errors.push(e.message));
    await page.setViewportSize({width:390,height:844});
    await boot(page,worker);
    expect(await page.evaluate(() => __nir.state().preferences.voice_continue)).toBe(true);
    await showContinuationPreference(page);
    const rect=JSON.parse(await continueButton(page).getAttribute('data-rect'));
    await page.mouse.click(rect[0]+rect[2]/2,rect[1]+rect[3]/2);
    await expect.poll(() => page.evaluate(() => __nir.state().preferences.voice_continue)).toBe(false);
    await committed(page,'preferences','continue-off');
    await page.screenshot({path:testInfo.outputPath('voice-continuation-390x844.png')});
    await page.setViewportSize({width:844,height:390});
    await showContinuationPreference(page);
    await page.screenshot({path:testInfo.outputPath('voice-continuation-844x390.png')});
    await page.reload();
    await page.waitForFunction(() => globalThis.__nir?.state().ready && !__nir.state().loading);
    expect(await page.evaluate(() => __nir.state().preferences.voice_continue)).toBe(false);
    await page.keyboard.press('Enter');
    await page.waitForFunction(() => __nir.state().screen==='Story' && __nir.state().dialogue && !__nir.state().loading);
    await recoverAudioOutput(page);
    await page.evaluate(() => __nir.action({type:'menu'}));
    await page.waitForFunction(() => __nir.state().screen==='Menu');
    const before=await page.evaluate(() => ({interaction:__nir.state().interaction,tick:__nir.state().tick_us}));
    await page.evaluate(() => __nir.action({type:'save',slot:1}));
    await committed(page,'saves','any');
    await showContinuationPreference(page);
    await continueButton(page).focus();await page.keyboard.press('Enter');
    await expect.poll(() => page.evaluate(() => __nir.state().preferences.voice_continue)).toBe(true);
    expect(await page.evaluate(() => __nir.state().interaction)).toBe(before.interaction);
    expect(await page.evaluate(() => __nir.state().tick_us)).toBe(before.tick);
    const session=await page.evaluate(() => __nir.state().session);
    await page.evaluate(() => __nir.action({type:'load',slot:1}));
    await page.waitForFunction(s => __nir.state().session!==s && !__nir.state().loading && __nir.state().paused,session);
    expect(await page.evaluate(() => __nir.state().preferences.voice_continue)).toBe(true);
    expect(await page.evaluate(() => __nir.state().auto)).toBe(false);
    expect(await page.evaluate(() => __nir.state().error)).toBeNull();
    expect(errors).toEqual([]);
  });

  for (const automatic of [false,true]) {
    test(`voice continuation off stops the real MP3 voice on ${automatic?'auto':'manual'} advance, ${worker}`, async ({page}) => {
      const errors=[];page.on('pageerror',e=>errors.push(e.message));
      await page.addInitScript(() => {
        globalThis.voiceStopAudit=[];
        const create=AudioContext.prototype.createBufferSource;
        AudioContext.prototype.createBufferSource=function(...args) {
          const source=create.apply(this,args),start=source.start,stop=source.stop;
          let record;
          source.start=function(when,offset=0,...rest) {
            record={source,context:source.context,started:source.context.currentTime,offset,stops:0,ended:false,duration:source.buffer.duration};
            source.addEventListener('ended',()=>{record.ended=true;});voiceStopAudit.push(record);
            return start.call(this,when,offset,...rest);
          };
          source.stop=function(...args) {if(record)record.stops++;return stop.apply(this,args);};
          return source;
        };
      });
      await boot(page,worker);
      await page.evaluate(() => {__nir.action({type:'voice_continue',enabled:false});__nir.action({type:'auto_wait_voice',enabled:false});});
      await page.keyboard.press('Enter');
      await page.waitForFunction(() => __nir.state().screen==='Story' && !__nir.state().loading);
      await recoverAudioOutput(page);
      await page.waitForFunction(() => __nir.state().dialogue?.ready && !__nir.state().loading && voiceStopAudit.some(v=>!v.source.loop && v.duration>7 && !v.ended));
      const before=await page.evaluate(() => {
        const v=voiceStopAudit.find(v=>!v.source.loop && v.duration>7);
        return {interaction:__nir.state().interaction,remaining:v.duration-v.offset-v.context.currentTime+v.started};
      });
      expect(before.remaining).toBeGreaterThan(2);
      if(automatic)await page.evaluate(() => __nir.action({type:'toggle_auto'}));
      else await page.keyboard.press('Space');
      await page.waitForFunction(i => __nir.state().interaction!==i && !__nir.state().loading,before.interaction);
      await page.waitForFunction(() => voiceStopAudit.find(v=>!v.source.loop && v.duration>7).ended);
      const after=await page.evaluate(() => {
        const v=voiceStopAudit.find(v=>!v.source.loop && v.duration>7);
        const bgm=voiceStopAudit.find(v=>v.source.loop);
        return {stops:v.stops,remaining:v.duration-v.offset-v.context.currentTime+v.started,bgmStops:bgm.stops,bgmEnded:bgm.ended};
      });
      expect(after.stops).toBe(1);
      expect(after.remaining).toBeGreaterThan(1);
      expect(after.bgmStops).toBe(0);expect(after.bgmEnded).toBe(false);
      expect(await page.evaluate(() => __nir.state().error)).toBeNull();
      expect(errors).toEqual([]);
    });
  }
  test(`voice wait control is reachable, persists and stays global across load, ${worker}`, async ({page}) => {
    const errors=[];page.on('pageerror', e=>errors.push(e.message));
    await page.setViewportSize({width:390,height:844});
    await boot(page, worker);
    expect(await page.evaluate(() => __nir.state().preferences.auto_wait_voice)).toBe(true);
    await showVoicePreference(page);
    await voiceButton(page).focus();await page.keyboard.press('Enter');
    await expect.poll(() => page.evaluate(() => __nir.state().preferences.auto_wait_voice)).toBe(false);
    await committed(page, 'preferences', 'voice-off');
    await page.reload();
    await page.waitForFunction(() => globalThis.__nir?.state().ready && !__nir.state().loading);
    expect(await page.evaluate(() => __nir.state().preferences.auto_wait_voice)).toBe(false);
    await page.keyboard.press('Enter');
    await page.waitForFunction(() => __nir.state().screen === 'Story' && __nir.state().dialogue && !__nir.state().loading);
    await page.evaluate(() => __nir.action({type:'menu'}));
    await page.waitForFunction(() => __nir.state().screen === 'Menu');
    const snapshot = await page.evaluate(() => ({interaction:__nir.state().interaction,tick:__nir.state().tick_us}));
    await page.evaluate(() => __nir.action({type:'save',slot:1}));
    await committed(page, 'saves', 'any');
    await showVoicePreference(page);
    await voiceButton(page).focus();await page.keyboard.press('Enter');
    await expect.poll(() => page.evaluate(() => __nir.state().preferences.auto_wait_voice)).toBe(true);
    expect(await page.evaluate(() => __nir.state().tick_us)).toBe(snapshot.tick);
    expect(await page.evaluate(() => __nir.state().interaction)).toBe(snapshot.interaction);
    const epoch=await page.evaluate(() => __nir.state().session);
    await page.evaluate(() => __nir.action({type:'load',slot:1}));
    await page.waitForFunction(epoch => __nir.state().session!==epoch && !__nir.state().loading && __nir.state().paused, epoch);
    expect(await page.evaluate(() => __nir.state().preferences.auto_wait_voice)).toBe(true);
    expect(await page.evaluate(() => __nir.state().auto)).toBe(false);
    expect(await page.evaluate(() => __nir.state().error)).toBeNull();
    expect(errors).toEqual([]);
  });

  test(`opting out advances while the bound real voice is still playing, ${worker}`, async ({page}) => {
    const errors=[];page.on('pageerror',e=>errors.push(e.message));
    await page.addInitScript(() => {
      globalThis.voicePrefAudit=[];
      const create=AudioContext.prototype.createBufferSource;
      AudioContext.prototype.createBufferSource=function(...args) {
        const source=create.apply(this,args),start=source.start;
        source.start=function(when,offset=0,...rest) {
          if (!source.loop) {
            const record={source,context:source.context,started:source.context.currentTime,offset,ended:false,duration:source.buffer.duration};
            source.addEventListener('ended',()=>{record.ended=true;});
            voicePrefAudit.push(record);
          }
          return start.call(this,when,offset,...rest);
        };
        return source;
      };
    });
    await boot(page, worker);
    await page.evaluate(() => __nir.action({type:'auto_wait_voice',enabled:false}));
    await page.keyboard.press('Enter');
    await page.waitForFunction(() => __nir.state().screen === 'Story' && !__nir.state().loading);
    await recoverAudioOutput(page);
    await page.waitForFunction(() => __nir.state().dialogue?.ready && !__nir.state().loading && voicePrefAudit.some(v=>v.duration>7 && !v.ended));
    const before=await page.evaluate(() => {
      const v=voicePrefAudit.find(v=>v.duration>7);
      return {interaction:__nir.state().interaction,remaining:v.duration-v.offset-v.context.currentTime+v.started};
    });
    expect(before.remaining).toBeGreaterThan(2);
    await page.evaluate(() => __nir.action({type:'toggle_auto'}));
    await page.waitForFunction(i => __nir.state().interaction!==i && !__nir.state().loading, before.interaction);
    expect(await page.evaluate(() => voicePrefAudit.find(v=>v.duration>7).ended)).toBe(false);
    expect(await page.evaluate(() => __nir.state().preferences.auto_wait_voice)).toBe(false);
    expect(await page.evaluate(() => __nir.state().error)).toBeNull();
    expect(errors).toEqual([]);
  });
}
