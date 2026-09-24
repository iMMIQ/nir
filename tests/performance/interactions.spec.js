import { test, expect } from '@playwright/test';
import fs from 'node:fs/promises';
import path from 'node:path';
import { buildScaleFixture, closeScaleFixture } from './fixtures.js';
import { distribution, installAdapterProbe, assertHardwareAdapter } from './metrics.js';

const samples = 30;
const networkTrace = process.env.NIR_PERF_NETWORK_TRACE === '1';
const disableHttpCache = process.env.NIR_PERF_DISABLE_HTTP_CACHE === '1';
const pressureLog = process.env.NIR_PERF_PRESSURE_LOG;
function pressureTotals(text) {
  return Object.fromEntries(text.trim().split('\n').map(line => {
    const [kind] = line.split(' ');
    return [kind, Number(line.match(/\btotal=(\d+)/)?.[1])];
  }));
}
const act = (page, action) => page.evaluate(action => window.__nir.action(action), action);
async function settled(page, screen) {
  await page.waitForFunction(screen => {
    const api = window.__nir, s = api?.state();
    return s?.ready && !s.loading && api.metrics.activeRequests === 0 && (!screen || s.screen === screen);
  }, screen);
}
async function measure(page, action, screen, scrollDelta = 0) {
  const before = await page.evaluate(({ action, scrollDelta }) => {
    const api = window.__nir, s = api.state();
    const history = s.scrolls.find(v => v.region === 'history');
    // Use the recorder's integer-microsecond resolution. Comparing rounded
    // events to raw floating-point milliseconds can reject the same instant.
    const start = Math.round(performance.now() * 1000);
    api.action(action);
    return { start, frames: s.frames, historyOffset: history?.offset, scrollDelta };
  }, { action, scrollDelta });
  await page.waitForFunction(({ before, screen }) => {
    const s = window.__nir.state(), history = s.scrolls.find(v => v.region === 'history');
    return s.screen === screen && !s.loading && s.frames > before.frames &&
      (!before.scrollDelta || (history && Math.sign(history.offset - before.historyOffset) === before.scrollDelta));
  }, { before, screen });
  return page.evaluate(({ before, screen }) => {
    const api = window.__nir, s = api.state(), d = api.diagnostics();
    const input = d.events.findLast(e => e.stage === 'input_received' && Number(e.at_us) >= before.start);
    const submit = input && d.events.find(e => e.stage === 'render_submitted' && e.frames === s.frames && BigInt(e.at_us) >= BigInt(input.at_us));
    if (!submit) throw Error('missing input/target frame timing');
    return { screen, latencyMs: (Number(submit.at_us) - Number(input.at_us)) / 1000,
      inputAtMs: Number(input.at_us) / 1000,
      events: d.events.filter(e => BigInt(e.at_us) >= BigInt(input.at_us)),
      shapes: s.shapes, frames: s.frames, session: s.session,
      performance: d.performance ?? null };
  }, { before, screen });
}

test('prepared interactions with long history', async ({ browser }) => {
  const fixture = await buildScaleFixture({ moduleCount: 32, prefetchContent: true, textRepetitions: 40, port: 4195 });
  const context = await browser.newContext({ viewport: { width: 1280, height: 800 }, locale: 'zh-CN' });
  await installAdapterProbe(context);
  const page = await context.newPage(), errors = [];
  const network = [], maxNetworkEvents = 20000;
  const pressure = [];
  let pressurePending;
  const samplePressure = () => {
    if (!pressureLog || pressurePending) return pressurePending;
    pressurePending = Promise.all([
      fs.readFile('/proc/pressure/io', 'utf8'), fs.readFile('/proc/pressure/memory', 'utf8'),
    ]).then(([io, memory]) => pressure.push({ wallTime: Date.now() / 1000,
      io: pressureTotals(io), memory: pressureTotals(memory) }))
      .finally(() => { pressurePending = undefined; });
    return pressurePending;
  };
  await samplePressure();
  const pressureTimer = pressureLog ? setInterval(samplePressure, 500) : undefined;
  let cdp;
  if (networkTrace) {
    await context.addInitScript(() => performance.setResourceTimingBufferSize(20000));
    cdp = await context.newCDPSession(page);
    await cdp.send('Network.enable');
    if (disableHttpCache) await cdp.send('Network.setCacheDisabled', { cacheDisabled: true });
    const record = row => { if (network.length < maxNetworkEvents) network.push(row); };
    cdp.on('Network.requestWillBeSent', e => record({ type: 'request', id: e.requestId, at: e.timestamp,
      wallTime: e.wallTime, url: new URL(e.request.url).pathname }));
    cdp.on('Network.responseReceived', e => record({ type: 'response', id: e.requestId, at: e.timestamp,
      status: e.response.status, timing: e.response.timing, fromDiskCache: e.response.fromDiskCache,
      serveId: Object.entries(e.response.headers ?? {}).find(([name]) => name.toLowerCase() === 'x-nir-serve-id')?.[1],
      connectionId: e.response.connectionId, connectionReused: e.response.connectionReused }));
    cdp.on('Network.loadingFinished', e => record({ type: 'finished', id: e.requestId, at: e.timestamp, encodedBytes: e.encodedDataLength }));
    cdp.on('Network.loadingFailed', e => record({ type: 'failed', id: e.requestId, at: e.timestamp, cancelled: e.canceled, error: e.errorText }));
    await cdp.send('Tracing.start', { categories: 'devtools.timeline,loading,blink.user_timing', transferMode: 'ReturnAsStream', streamCompression: 'gzip' });
  }
  page.on('pageerror', error => errors.push(String(error)));
  const report = { format: 1, samples, moduleCount: 32, textRepetitions: 40,
    instrumentation: { runtimeDiagnostics: true, playwrightTrace: test.info().project.use.trace,
      networkTrace, disableHttpCache: networkTrace && disableHttpCache,
      serveMode: fixture.serveMode, serveCli: fixture.serveCli,
      serveLog: fixture.serveLog, pressureLog },
    mode: process.env.NIR_PERF_MODE, gpuTime: 'unmeasured', status: 'incomplete', runs: {} };
  try {
    await page.goto(`${fixture.origin}/?test=1`);
    await settled(page, 'Title');
    if (process.env.NIR_PERF_MODE === 'hardware') {
      assertHardwareAdapter(...await page.evaluate(() => [window.__nir.state().adapter, window.__nirActualAdapters]));
    }
    for (const [i, step] of fixture.steps.entries()) {
      // Advance browses overflow before changing dialogue; finish that reading
      // path explicitly so this setup does not bypass authored input behavior.
      for (let scrolls = 0; i > 0; scrolls++) {
        const view = await page.evaluate(() => window.__nir.state().scrolls.find(v => v.region === 'dialogue'));
        if (!view || view.offset >= view.max - 1) break;
        if (scrolls >= 100) throw Error('dialogue scroll did not converge');
        await act(page, { type: 'scroll', region: 'dialogue', delta: 1 });
      }
      await act(page, { type: i === 0 ? 'new_game' : 'advance' });
      await page.waitForFunction(id => {
        const s = window.__nir.state(); return !s.loading && s.dialogue?.id === id && s.dialogue.ready;
      }, step.textId);
    }
    await settled(page, 'Story');
    const saved = await page.evaluate(() => window.__nir.state());
    expect(saved.history_count).toBeGreaterThanOrEqual(60);
    report.release = await page.evaluate(() => window.__nir.diagnostics().release);
    report.engine = await page.evaluate(() => window.__nir.diagnostics().engine);
    report.historyCount = saved.history_count;
    report.actualAdapters = await page.evaluate(() => window.__nirActualAdapters);
    report.runs.menuOpen = []; report.runs.menuClose = [];
    for (let i = 0; i < samples; i++) {
      report.runs.menuOpen.push(await measure(page, { type: 'menu' }, 'Menu'));
      report.runs.menuClose.push(await measure(page, { type: 'close' }, 'Story'));
    }
    await act(page, { type: 'history' }); await settled(page, 'History');
    expect(await page.evaluate(() => window.__nir.state().scrolls.find(v => v.region === 'history')?.max)).toBeGreaterThan(0);
    report.runs.historyScroll = []; report.runs.historyPage = [];
    for (let i = 0; i < samples; i++) {
      const delta = i % 2 ? -1 : 1;
      report.runs.historyScroll.push(await measure(page, { type: 'scroll', region: 'history', delta }, 'History', delta));
    }
    for (let i = 0; i < samples; i++) {
      report.runs.historyPage.push(await measure(page, { type: 'history_page', delta: i % 2 ? -3 : 3 }, 'History'));
    }
    await act(page, { type: 'close' }); await settled(page, 'Story');
    await act(page, { type: 'saves' }); await settled(page, 'Saves');
    await act(page, { type: 'save', slot: 0 });
    await page.waitForFunction(() => /已保存|Saved/.test(window.__nir.state().status));
    await act(page, { type: 'close' }); await settled(page, 'Story');
    report.runs.restore = [];
    for (let i = 0; i < samples; i++) {
      await act(page, { type: 'saves' }); await settled(page, 'Saves');
      report.runs.restore.push(await measure(page, { type: 'load', slot: 0 }, 'Story'));
      const restored = await page.evaluate(() => window.__nir.state());
      expect(restored.variables).toEqual(saved.variables);
      expect(restored.dialogue).toEqual(saved.dialogue);
      expect(restored.history_count).toBe(saved.history_count);
      expect(restored.position).toBe(saved.position);
    }
    await act(page, { type: 'title' }); await settled(page, 'Title');
    const cleanup = await page.evaluate(() => ({ state: window.__nir.state(), diagnostics: window.__nir.diagnostics() }));
    expect(cleanup.state.error).toBeNull();
    expect(cleanup.diagnostics.content_staging.encoded_bytes).toBe(0);
    expect(cleanup.diagnostics.content_staging.reservations).toBe(0);
    expect(errors).toEqual([]);
    report.performance = cleanup.diagnostics.performance ?? null;
    report.status = 'passed';
  } catch (error) {
    report.error = String(error);
    report.failure = await page.evaluate(() => ({ state: window.__nir?.state(), diagnostics: window.__nir?.diagnostics() })).catch(() => null);
    throw error;
  } finally {
    if (pressureTimer) clearInterval(pressureTimer);
    await pressurePending;
    await samplePressure();
    if (pressureLog) {
      await fs.mkdir(path.dirname(pressureLog), { recursive: true });
      await fs.writeFile(pressureLog, JSON.stringify({ format: 1, sampleIntervalMs: 500, samples: pressure }, null, 2));
    }
    if (cdp) {
      report.network = network;
      report.networkTruncated = network.length >= maxNetworkEvents;
      report.resources = await page.evaluate(() => performance.getEntriesByType('resource').map(e => e.toJSON())).catch(() => []);
      const completed = new Promise(resolve => cdp.once('Tracing.tracingComplete', resolve));
      await cdp.send('Tracing.end');
      const { stream } = await completed;
      const chunks = [];
      try {
        while (true) {
          const chunk = await cdp.send('IO.read', { handle: stream });
          chunks.push(Buffer.from(chunk.data, chunk.base64Encoded ? 'base64' : 'utf8'));
          if (chunk.eof) break;
        }
      } finally { await cdp.send('IO.close', { handle: stream }); }
      await fs.mkdir('reports', { recursive: true });
      await fs.writeFile('reports/performance-interactions-trace.json.gz', Buffer.concat(chunks));
    }
    report.summary = Object.fromEntries(Object.entries(report.runs).map(([name, rows]) => [name, distribution(rows.map(row => row.latencyMs))]));
    await fs.mkdir('reports', { recursive: true });
    await fs.writeFile('reports/performance-interactions.json', JSON.stringify(report, null, 2));
    await context.close(); await closeScaleFixture(fixture);
  }
});
