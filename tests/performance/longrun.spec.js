import { test, expect } from '@playwright/test';
import fs from 'node:fs/promises';
import { installAdapterProbe, assertHardwareAdapter } from './metrics.js';
import { buildScaleFixture, closeScaleFixture } from './fixtures.js';
import { sampleProcessMemory, trend } from './process-memory.js';
import { positiveInteger } from './scenarios.js';

const seconds = positiveInteger(process.env.NIR_PERF_LONGRUN_SECONDS, 120, 'NIR_PERF_LONGRUN_SECONDS');
const act = (page, action) => page.evaluate(action => window.__nir.action(action), action);
const state = page => page.evaluate(() => window.__nir.state());
async function settled(page, screen) {
  const handle = await page.waitForFunction(screen => {
    const api = window.__nir, s = api?.state();
    const diagnostics = screen === 'Title' && api ? api.diagnostics() : null;
    const h = diagnostics?.host_work;
    const ready = s?.ready && !s.loading && !s.locale_pending &&
      (screen !== 'Title' || (api.metrics.activeRequests === 0 && h.resource_pool_active === 0 &&
        h.resource_pool_waiting === 0 && !h.decode_pool_active && !h.decode_pool_waiting &&
        h.upload_pool_active === 0 && h.upload_pool_waiting === 0 && h.request_slots === 0 &&
        h.shared_fetches === 0 && h.content_jobs === 0 && h.media_jobs === 0 && h.pending_owner_callbacks === 0)) &&
      (!screen || s.screen === screen);
    // Capture the same turn that passed cleanup. Another callback can arrive
    // between two separate CDP evaluations even after a successful wait.
    return ready ? { state: s, diagnostics } : false;
  }, screen);
  const snapshot = await handle.jsonValue();
  await handle.dispose();
  expect(snapshot.state.error).toBeNull();
  return snapshot;
}
async function reach(page, predicate, route) {
  for (let i = 0; i < 200; i++) {
    const s = await state(page);
    expect(s.error).toBeNull();
    if (predicate(s)) return s;
    if (s.choice && route) await act(page, { type: 'choose', option: route });
    else if (s.dialogue && !s.dialogue.gate) await act(page, { type: 'advance' });
    await page.waitForTimeout(80);
  }
  throw Error(`longrun route stalled: ${JSON.stringify(await state(page))}`);
}

test('M3 sustained reading restore and device recovery', async ({ browser, context, page }) => {
  test.setTimeout((seconds + 240) * 1000);
  const fixture = await buildScaleFixture({ moduleCount: 3, prefetchContent: true, port: 4197 });
  const report = { format: 1, requestedSeconds: seconds, status: 'incomplete', mode: process.env.NIR_PERF_MODE,
    instrumentation: { runtimeDiagnostics: true, playwrightTrace: test.info().project.use.trace },
    memoryNotes: 'RSS sum may count shared pages repeatedly; PSS covers CDP browser processes including both pages; JS heap and WASM cover the primary page only. GPU memory unmeasured.', rows: [] };
  const browserSession = await browser.newBrowserCDPSession();
  const pageSession = await context.newCDPSession(page);
  await pageSession.send('Performance.enable');
  await installAdapterProbe(context);
  const modulePage = await context.newPage();
  const errors = [];
  for (const p of [page, modulePage]) p.on('pageerror', e => errors.push(String(e)));
  const output = 'reports/performance-m3-longrun.json';
  await fs.mkdir('reports', { recursive: true });
  const samplesFile = `${output}.jsonl`;
  await fs.writeFile(samplesFile, '');
  report.samplesFile = samplesFile;
  let lastCheckpoint = -Infinity;
  const checkpoint = async (force = false) => {
    if (!force && performance.now() - lastCheckpoint < 60000) return;
    await fs.writeFile(`${output}.tmp`, JSON.stringify(report, null, 2));
    await fs.rename(`${output}.tmp`, output);
    lastCheckpoint = performance.now();
  };
  try {
    await page.goto('/?test=1'); await settled(page, 'Title');
    await modulePage.goto(`${fixture.origin}/?test=1`); await settled(modulePage, 'Title');
    report.release = await page.evaluate(() => window.__nir.diagnostics().release);
    report.engine = await page.evaluate(() => window.__nir.diagnostics().engine);
    report.actualAdapters = await page.evaluate(() => window.__nirActualAdapters);
    if (process.env.NIR_PERF_MODE === 'hardware')
      assertHardwareAdapter(...await page.evaluate(() => [window.__nir.state().adapter, window.__nirActualAdapters]));
    const started = performance.now();
    let cycle = 0;
    do {
      await page.bringToFront();
      const route = cycle % 2 ? 'stay' : 'walk';
      await act(page, { type: 'new_game' });
      await reach(page, s => !!s.dialogue);
      await act(page, { type: 'settings' });
      await act(page, { type: 'text_locale', locale: Math.floor(cycle / 2) % 2 ? 'en' : 'zh-Hans' });
      await settled(page, 'Settings');
      await act(page, { type: 'close' });
      await reach(page, s => !!s.choice);
      await act(page, { type: 'saves' });
      const saved = await state(page);
      await act(page, { type: 'save', slot: 0 });
      await page.waitForFunction(() => /已保存|Saved/.test(window.__nir.state().status));
      await act(page, { type: 'load', slot: 0 });
      await settled(page, 'Story');
      const restored = await state(page);
      expect(restored.variables).toEqual(saved.variables);
      expect(restored.position).toEqual(saved.position);
      const { interaction: oldInteraction, ...savedChoice } = saved.choice;
      const { interaction: newInteraction, ...restoredChoice } = restored.choice;
      expect(restoredChoice).toEqual(savedChoice);
      expect(newInteraction).not.toBe(oldInteraction);
      if (cycle % 10 === 9) {
        const device = restored.device;
        await page.evaluate(() => window.__nir.loseDevice());
        await page.waitForFunction(device => window.__nir.state().device > device, device);
        await settled(page, 'Story');
      }
      await act(page, { type: 'continue' });
      await act(page, { type: 'choose', option: route });
      const ending = await reach(page, s => !!s.outcome, route);
      expect(ending.outcome).toBe(route === 'walk' ? 'walk_home' : 'read_letter');
      expect(ending.variables.affection.value).toBe(route === 'walk' ? 1 : 0);
      await act(page, { type: 'rollback' }); await settled(page, 'Story');
      await act(page, { type: 'title' }); await settled(page, 'Title');
      // A separate compiler-built route exercises actual cross-module calls.
      if (cycle % 5 === 0) {
        await modulePage.bringToFront();
        let visits = 0;
        for (const [i, step] of fixture.steps.entries()) {
          await act(modulePage, { type: i ? 'advance' : 'new_game' });
          await modulePage.waitForFunction(id => {
            const s = window.__nir.state(); return !s.loading && s.dialogue?.id === id && s.dialogue.ready;
          }, step.textId);
          if (step.kind === 'chapter') expect((await state(modulePage)).variables.visit_count.value).toBe(++visits);
        }
        await act(modulePage, { type: 'title' }); await settled(modulePage, 'Title');
        await page.bringToFront(); await settled(page, 'Title');
      }
      const clean = await settled(page, 'Title');
      expect(clean.diagnostics.content_staging.encoded_bytes).toBe(0);
      expect(clean.diagnostics.content_staging.reservations).toBe(0);
      expect(clean.state.pending_events).toBe(0);
      expect(clean.state.resident_bytes).toBeLessThanOrEqual(128 * 1024 * 1024);
      expect(clean.state.content_residency.resident_bytes).toBeLessThanOrEqual(clean.state.content_residency.budget_bytes);
      expect(errors).toEqual([]);
      const sampleStarted = performance.now();
      const memory = await sampleProcessMemory(browserSession, pageSession);
      const memorySampleMs = performance.now() - sampleStarted;
      report.rows.push({ cycle: ++cycle, elapsedMs: performance.now() - started, route,
        device: clean.state.device, wasmCapacityBytes: clean.state.wasm_memory_bytes,
        uiLocale: clean.state.ui_locale, textLocale: clean.state.text_locale,
        estimatedMediaBytes: clean.state.resident_bytes, content: clean.state.content_residency,
        staging: clean.diagnostics.content_staging, host: clean.diagnostics.host_work,
        memorySampleMs, ...memory });
      report.elapsedSeconds = (performance.now() - started) / 1000;
      await fs.appendFile(samplesFile, `${JSON.stringify(report.rows.at(-1))}\n`);
      await checkpoint();
    } while (performance.now() - started < seconds * 1000);
    report.cycles = cycle;
    report.memoryCoverage = Object.fromEntries(['rssSumBytes', 'pssSumBytes', 'jsHeapUsedBytes'].map(field =>
      [field, report.rows.filter(row => Number.isFinite(row[field])).length / report.rows.length]));
    if (process.env.NIR_PERF_MODE === 'hardware') {
      for (const [field, coverage] of Object.entries(report.memoryCoverage))
        expect(coverage, `${field} measurement coverage`).toBeGreaterThanOrEqual(0.95);
    }
    const warmupMs = Math.min(10 * 60 * 1000, seconds * 1000 / 4);
    report.warmupMs = warmupMs;
    report.trends = Object.fromEntries(['wasmCapacityBytes', 'rssSumBytes', 'pssSumBytes', 'jsHeapUsedBytes'].map(field =>
      [field, { all: trend(report.rows, field), steady: trend(report.rows.filter(row => row.elapsedMs >= warmupMs), field) }]));
    report.trendsByTextLocale = Object.fromEntries(['zh-Hans', 'en'].map(locale => [locale,
      Object.fromEntries(['wasmCapacityBytes', 'pssSumBytes', 'jsHeapUsedBytes'].map(field =>
        [field, trend(report.rows.filter(row => row.elapsedMs >= warmupMs && row.textLocale === locale), field)]))]));
    report.status = 'passed';
  } catch (error) {
    report.status = 'failed'; report.error = String(error);
    report.failure = await page.evaluate(() => ({ state: window.__nir?.state(), diagnostics: window.__nir?.diagnostics() })).catch(() => null);
    throw error;
  } finally {
    await checkpoint(true);
    await modulePage.close(); await closeScaleFixture(fixture);
    await pageSession.detach(); await browserSession.detach();
  }
});
