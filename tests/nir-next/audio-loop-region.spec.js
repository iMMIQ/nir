import {test, expect} from '@playwright/test';
import fs from 'node:fs/promises';

const origin = 'http://127.0.0.1:4250';

for (const rate of [44100, 48000]) test(`MP3 intro and 100 region cycles match decoded output at ${rate} Hz`, async ({page, request}) => {
  const channel = await (await request.get(`${origin}/channels/stable.json`)).json();
  const release = await (await request.get(`${origin}/releases/${channel.release}.json`)).json();
  const host = `${origin}/${release.objects[release.engine.host].path}`;
  const bytes = [...await fs.readFile(new URL('../../apps/player-desktop/tests/fixtures/gapless-44100-mono.mp3', import.meta.url))];
  // A same-origin static page avoids bootstrap's redirect while rendering
  // offline PCM; import the host shipped in this actual release.
  await page.goto(`${origin}/channels/stable.json`);
  const results = await page.evaluate(async ({host, bytes, rate}) => {
    const {audioLoopPlayback} = await import(host);
    const decoder = new OfflineAudioContext(1, rate, rate);
    const buffer = await decoder.decodeAudioData(Uint8Array.from(bytes).buffer);
    const pcm = buffer.getChannelData(0).slice();
    const region = {start_us:'150000', end_us:'350000'};
    // Expected output indices are independent of the production mapper.
    const start = Math.round(rate * .15), end = Math.round(rate * .35), body = end - start;
    const rows = [];
    for (const position of [0, 900000, 1400000]) {
      const mapped = audioLoopPlayback(region, String(position), buffer.sampleRate, buffer.length);
      const offset = Math.round(position / 1e6 * rate);
      const first = offset < end ? offset : start + (offset - end) % body;
      const count = position === 0 ? end + body * 100 : body * 100;
      const context = new OfflineAudioContext(1, count, rate);
      const source = context.createBufferSource();
      source.buffer = buffer; source.loop = true;
      source.loopStart = mapped.startFrame / rate; source.loopEnd = mapped.endFrame / rate;
      source.connect(context.destination); source.start(0, mapped.offsetFrame / rate);
      const output = (await context.startRendering()).getChannelData(0);
      let maxError = 0, index = first;
      for (let i = 0; i < output.length; i++) {
        maxError = Math.max(maxError, Math.abs(output[i] - pcm[index]));
        if (++index === end) index = start;
      }
      rows.push({position, first, mapped, maxError, frames:output.length});
    }
    return {rate:buffer.sampleRate, rows};
  }, {host, bytes, rate});
  expect(results.rate).toBe(rate);
  for (const row of results.rows) {
    expect(row.mapped.offsetFrame).toBe(row.first);
    expect(row.maxError).toBeLessThan(.00001);
  }
  await fs.writeFile(new URL(`../../reports/experience-loop-pcm-${rate}.json`, import.meta.url), JSON.stringify({
    host_digest:release.engine.host, cycles:100, ...results,
  }, null, 2) + '\n');
});

for (const worker of ['required', 'main']) test(`region music restores the cumulative playhead without replaying intro, ${worker}`, async ({page}) => {
  const errors = []; page.on('pageerror', e => errors.push(e.message));
  await page.addInitScript(() => {
    globalThis.loopAudit = {sources:[], starts:[]};
    const create = AudioContext.prototype.createBufferSource;
    AudioContext.prototype.createBufferSource = function(...args) {
      const source = create.apply(this, args), start = source.start, stop = source.stop;
      loopAudit.sources.push(source);
      source.start = function(when, offset, ...rest) {
        loopAudit.starts.push({source, offset, at:source.context.currentTime});
        return start.call(this, when, offset, ...rest);
      };
      source.stop = function(...args) {source.stopped = true; return stop.apply(this, args);};
      return source;
    };
  });
  await page.goto(`${origin}/?test=1&worker=${worker}&backend=webgl2`);
  await page.waitForFunction(() => globalThis.__nir?.state().ready && !__nir.state().loading);
  await page.keyboard.press('Enter');
  await page.waitForFunction(() => __nir.state().dialogue && loopAudit.starts.some(s => s.source.loop && s.source.context.currentTime - s.at > .95));
  const initial = await page.evaluate(() => {
    const s = loopAudit.starts.find(s => s.source.loop);
    return {offset:s.offset, start:s.source.loopStart, end:s.source.loopEnd};
  });
  expect(initial).toEqual({offset:0, start:.2, end:.6});
  await page.evaluate(() => __nir.action({type:'menu'}));
  await page.waitForFunction(() => __nir.state().screen === 'Menu');
  await page.evaluate(() => __nir.action({type:'save', slot:1}));
  await expect.poll(()=>page.evaluate(async () => {
    for (const {name} of await indexedDB.databases()) {
      const db = await new Promise((ok,no) => {const r=indexedDB.open(name);r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});
      if (!db.objectStoreNames.contains('saves')) {db.close(); continue;}
      const records = await new Promise((ok,no) => {const tx=db.transaction('saves'),r=tx.objectStore('saves').getAll();tx.oncomplete=()=>ok(r.result);tx.onerror=()=>no(tx.error);});
      db.close();
      const task = records.flatMap(r => Object.values(r.envelope.snapshot.tasks)).find(t => t.effect.type === 'audio' && t.effect.loop_region);
      if (task && Number(task.audio_position_us) > 600000) {globalThis.savedLoopTask = task; return true;}
    }
    return false;
  })).toBe(true);
  const saved = await page.evaluate(() => savedLoopTask);
  expect(saved.effect.loop_region).toEqual({start_us:'200000', end_us:'600000'});
  await page.evaluate(() => __nir.action({type:'load', slot:1}));
  await page.waitForFunction(() => !__nir.state().loading && loopAudit.starts.filter(s => s.source.loop).length === 2);
  const restored = await page.evaluate(() => {
    const loops = loopAudit.starts.filter(s => s.source.loop), s = loops[1];
    return {offset:s.offset, start:s.source.loopStart, end:s.source.loopEnd, rate:s.source.buffer.sampleRate,
      oldStopped:loops[0].source.stopped, context:s.source.context.state, paused:__nir.state().paused};
  });
  const frames = Math.round(Number(saved.audio_position_us) / 1e6 * restored.rate);
  const expected = (Math.round(.2 * restored.rate) + (frames - Math.round(.6 * restored.rate)) % Math.round(.4 * restored.rate)) / restored.rate;
  expect(restored.offset).toBeCloseTo(expected, 8);
  expect(restored.offset).toBeGreaterThanOrEqual(.2); expect(restored.offset).toBeLessThan(.6);
  expect(restored.start).toBe(.2); expect(restored.end).toBe(.6);
  expect(restored.oldStopped).toBe(true); expect(restored.paused).toBe(true);
  await page.waitForFunction(() => loopAudit.starts.filter(s=>s.source.loop).at(-1).source.context.state === 'suspended');
  await page.evaluate(() => __nir.action({type:'close'}));
  await page.evaluate(() => __nir.action({type:'continue'}));
  await page.waitForFunction(() => loopAudit.starts.filter(s=>s.source.loop).at(-1).source.context.state === 'running');
  expect(await page.evaluate(() => __nir.state().error)).toBeNull();
  expect(errors).toEqual([]);
});
