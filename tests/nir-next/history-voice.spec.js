import {test, expect} from '@playwright/test';

const origin = 'http://127.0.0.1:4252';
const replay = page => page.getByRole('button', {name:/^Replay voice$|^重播语音$/});
const stop = page => page.getByRole('button', {name:/^Stop voice$|^停止语音$/});

async function audit(page) {
  await page.addInitScript(() => {
    globalThis.historyAudio = [];
    const create = AudioContext.prototype.createBufferSource;
    AudioContext.prototype.createBufferSource = function(...args) {
      if (globalThis.historyRejectNext && globalThis.__nir?.state().screen === 'History') {
        globalThis.historyRejectNext = false;
        throw new Error('injected history source failure');
      }
      const source = create.apply(this, args), start = source.start, stop = source.stop;
      const record = {source, context:source.context, started:0, stops:0, ended:false};
      source.start = function(...args) {
        record.started = source.context.currentTime;
        historyAudio.push(record);
        source.addEventListener('ended', () => {record.ended = true;});
        return start.apply(this, args);
      };
      source.stop = function(...args) {record.stops++;return stop.apply(this, args);};
      return source;
    };
  });
}
async function boot(page, worker) {
  await page.goto(`${origin}/?test=1&worker=${worker}&backend=webgl2`);
  await page.waitForFunction(() => globalThis.__nir?.state().ready && !__nir.state().loading);
  await page.keyboard.press('Enter');
  await page.waitForFunction(() => __nir.state().dialogue?.ready && !__nir.state().loading &&
    historyAudio.some(r => !r.source.loop && r.source.buffer.duration > 7 && !r.ended));
}
async function keyActivate(page, button) {
  await expect(button).toBeVisible();
  await button.focus();
  await page.keyboard.press('Enter');
}
async function openHistory(page) {
  await keyActivate(page, page.getByRole('button', {name:/^History$|^回看$/}));
  await page.waitForFunction(() => __nir.state().screen === 'History');
  await expect(replay(page)).toBeVisible();
}
const snapshot = page => page.evaluate(() => {
  const s = __nir.state();
  return {tick:s.tick_us, interaction:s.interaction, variables:s.variables, count:s.history_count};
});

for (const worker of ['required', 'main']) {
  test(`history audition stays separate, stops and restores the same story sources, ${worker}`, async ({page}) => {
    const errors=[];page.on('pageerror', e => errors.push(e.message));
    await page.setViewportSize({width:390, height:844});
    await audit(page);
    await boot(page, worker);
    await openHistory(page);
    const before = await snapshot(page);
    const clocks = await page.evaluate(() => ({
      music:historyAudio.find(r => r.source.loop).context.currentTime,
      story:historyAudio.find(r => !r.source.loop).context.currentTime,
    }));
    await keyActivate(page, replay(page));
    await page.waitForFunction(() => __nir.state().history_voice && !__nir.state().history_voice.preparing &&
      historyAudio.filter(r => !r.source.loop).length === 2);
    await expect(stop(page)).toBeVisible();
    await page.screenshot({path:`reports/experience-history-portrait-${worker}.png`});
    await page.waitForTimeout(200);
    const playing = await page.evaluate(() => {
      const [story, audition] = historyAudio.filter(r => !r.source.loop), music = historyAudio.find(r => r.source.loop);
      return {music:music.context.currentTime, story:story.context.currentTime,
        storyState:story.context.state, auditionState:audition.context.state,
        independent:audition.context !== story.context, storyStops:story.stops, musicStops:music.stops};
    });
    expect(playing.independent).toBe(true);
    expect(playing.auditionState).toBe('running');
    expect(playing.storyState).toBe('suspended');
    expect(playing.story).toBe(clocks.story);
    expect(playing.music - clocks.music).toBeGreaterThan(.1);
    expect(playing.storyStops).toBe(0);expect(playing.musicStops).toBe(0);
    expect(await snapshot(page)).toEqual(before);
    await page.evaluate(() => __nir.hidden(true));
    await page.waitForFunction(() => historyAudio.filter(r => !r.source.loop)[1].context.state === 'suspended');
    await page.setViewportSize({width:844, height:390});
    await page.evaluate(() => __nir.hidden(false));
    await page.waitForFunction(() => historyAudio.filter(r => !r.source.loop)[1].context.state === 'running');
    expect(await page.evaluate(() => ({count:historyAudio.length, stops:historyAudio.filter(r => !r.source.loop)[1].stops,
      storyState:historyAudio.filter(r => !r.source.loop)[0].context.state}))).toEqual({count:3, stops:0, storyState:'suspended'});
    expect(await snapshot(page)).toEqual(before);
    await keyActivate(page, stop(page));
    await page.waitForFunction(() => __nir.state().history_voice === null);
    expect(await page.evaluate(() => historyAudio.filter(r => !r.source.loop)[1].stops)).toBe(1);
    await keyActivate(page, replay(page));
    await page.waitForFunction(() => historyAudio.filter(r => !r.source.loop).length === 3);
    await keyActivate(page, page.getByRole('button', {name:/^Back to story$|^返回故事$/}));
    await page.waitForFunction(() => __nir.state().screen === 'Story' && !__nir.state().paused &&
      historyAudio.filter(r => !r.source.loop)[0].context.state === 'running');
    expect(await page.evaluate(() => {
      const [story, first, second] = historyAudio.filter(r => !r.source.loop);
      return {storyState:story.context.state, storyStops:story.stops, stopped:[first.stops, second.stops], loops:historyAudio.filter(r => r.source.loop).length};
    })).toEqual({storyState:'running', storyStops:0, stopped:[1,1], loops:1});
    expect(await page.evaluate(() => __nir.state().interaction)).toBe(before.interaction);
    expect(await page.evaluate(() => __nir.state().error)).toBeNull();expect(errors).toEqual([]);
  });

  test(`history voice survives saved-state reload without changing the saved story, ${worker}`, async ({page}) => {
    const errors=[];page.on('pageerror', e => errors.push(e.message));
    await audit(page);await boot(page, worker);await openHistory(page);
    const before = await snapshot(page);
    await page.evaluate(() => __nir.action({type:'save', slot:1}));
    await expect.poll(()=>page.evaluate(async () => {
      for (const {name} of await indexedDB.databases()) {
        const db = await new Promise((ok,no) => {const r=indexedDB.open(name);r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});
        if (!db.objectStoreNames.contains('saves')) {db.close();continue;}
        const records = await new Promise((ok,no) => {const tx=db.transaction('saves'),r=tx.objectStore('saves').getAll();tx.oncomplete=()=>ok(r.result);tx.onerror=()=>no(tx.error);});
        db.close();
        if (records.some(record => record.envelope?.snapshot?.history.some(h => h.voices?.length))) return true;
      }
      return false;
    })).toBe(true);
    await page.reload();
    await page.waitForFunction(() => globalThis.__nir?.state().ready && !__nir.state().loading);
    const session=await page.evaluate(() => __nir.state().session);
    await page.evaluate(() => __nir.action({type:'load', slot:1}));
    try {
      await page.waitForFunction(session => __nir.state().session !== session && !__nir.state().loading && __nir.state().paused, session, {timeout:15000});
    } catch (error) {
      throw new Error(`Restore did not settle: ${JSON.stringify(await page.evaluate(() => __nir.state()))}; ${error}`);
    }
    const loaded = await snapshot(page);
    expect(loaded.interaction).not.toBe(before.interaction);
    expect({...loaded, interaction:before.interaction}).toEqual(before);
    await page.evaluate(() => __nir.action({type:'continue'}));
    await page.waitForFunction(() => __nir.state().screen === 'Story' && !__nir.state().paused);
    await openHistory(page);
    const restored = await snapshot(page);
    await keyActivate(page, replay(page));
    await page.waitForFunction(() => __nir.state().history_voice && !__nir.state().history_voice.preparing);
    await page.waitForTimeout(100);
    expect(await snapshot(page)).toEqual(restored);
    await keyActivate(page, stop(page));
    expect(await page.evaluate(() => __nir.state().error)).toBeNull();expect(errors).toEqual([]);
  });

  test(`failed history output retries locally and natural completion keeps the story frozen, ${worker}`, async ({page}) => {
    const errors=[];page.on('pageerror', e => errors.push(e.message));
    await audit(page);await boot(page, worker);await openHistory(page);
    const before = await snapshot(page);
    await page.evaluate(() => {globalThis.historyRejectNext = true;});
    await keyActivate(page, replay(page));
    await page.waitForFunction(() => __nir.state().history_voice?.failed);
    expect(await page.evaluate(() => ({error:__nir.state().error, loading:__nir.state().loading}))).toEqual({error:null, loading:false});
    expect(await snapshot(page)).toEqual(before);
    await keyActivate(page, page.getByRole('button', {name:/^Retry voice$|^重试语音$/}));
    await page.waitForFunction(() => __nir.state().history_voice && !__nir.state().history_voice.preparing && !__nir.state().history_voice.failed);
    await page.waitForFunction(() => __nir.state().history_voice === null && historyAudio.filter(r => !r.source.loop).length === 2 && historyAudio.filter(r => !r.source.loop)[1].ended);
    await expect(replay(page)).toBeVisible();
    expect(await snapshot(page)).toEqual(before);
    expect(await page.evaluate(() => historyAudio.filter(r => !r.source.loop)[0].context.state)).toBe('suspended');
    expect(await page.evaluate(() => __nir.state().error)).toBeNull();expect(errors).toEqual([]);
  });
}
