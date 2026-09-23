import { test, expect } from '@playwright/test';
import fs from 'node:fs/promises';
import { buildScaleFixture, closeScaleFixture } from './fixtures.js';
import {
  distribution, installAdapterProbe, assertHardwareAdapter,
  startCapture, checkpoint, measureAction, stopCapture,
} from './metrics.js';
import { networkProfiles, applyNetwork, positiveInteger } from './scenarios.js';

const smoke = process.env.NIR_PERF_SMOKE === '1';
const repetitions = positiveInteger(process.env.NIR_PERF_SCALE_REPETITIONS, 5, 'NIR_PERF_SCALE_REPETITIONS');
const hardware = process.env.NIR_PERF_MODE === 'hardware';
const dwellMs = 1000;
const state = page => page.evaluate(() => window.__nir.state());
const action = (page, value) => page.evaluate(value => window.__nir.action(value), value);

async function quiescent(page) {
  await page.waitForFunction(() => {
    const api = window.__nir, staging = api.diagnostics().content_staging;
    return !api.state().loading && api.metrics.activeRequests === 0 &&
      staging.encoded_bytes === 0 && staging.reservations === 0 && staging.waiting_demands === 0;
  });
}

async function lifecycle(page, fixture) {
  const saved = await state(page);
  await action(page, { type: 'saves' });
  await action(page, { type: 'save', slot: 0 });
  await page.waitForFunction(() => /已保存|Saved/.test(window.__nir.state().status));
  await action(page, { type: 'close' });
  // Checkpoints include the driver's lead-in. A single rollback must move to it.
  const leadIn = fixture.steps.at(-2).textId;
  const rollback = await measureAction(page, {
    label: 'rollback', action: { type: 'rollback' }, expectedTextId: leadIn, requireReady: false,
  });
  const rolled = await state(page);
  expect(rolled.paused).toBe(true);
  expect(rolled.variables.visit_count.value).toBeLessThan(saved.variables.visit_count.value);
  await checkpoint(page, 'after-rollback');
  await action(page, { type: 'title' });
  await quiescent(page);
  await checkpoint(page, 'title-before-restore');
  await action(page, { type: 'saves' });
  const restore = await measureAction(page, {
    label: 'restore', action: { type: 'load', slot: 0 }, expectedTextId: saved.dialogue.id,
  });
  const restored = await state(page);
  expect(restored.paused).toBe(true);
  expect(restored.variables).toEqual(saved.variables);
  expect(restored.dialogue).toEqual(saved.dialogue);
  expect(restored.position).toBe(saved.position);
  await checkpoint(page, 'after-restore');
  await action(page, { type: 'title' });
  await quiescent(page);
  const title = await state(page);
  expect(title.screen).toBe('Title');
  expect(title.error).toBeNull();
  await checkpoint(page, 'title-cleanup');
  return { rollback, restore };
}

async function runRoute(browser, fixture, profile, repetition) {
  const context = await browser.newContext({ viewport: { width: 1280, height: 800 }, locale: 'zh-CN' });
  await installAdapterProbe(context);
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  const started = Date.now();
  let progress = 'boot';
  try {
    await applyNetwork(context, page, profile);
    await page.goto(`${fixture.origin}/?test=1`);
    await page.waitForFunction(() => window.__nir?.state().ready && !window.__nir.state().loading);
    const adapters = await page.evaluate(() => window.__nirActualAdapters);
    const engineAdapter = (await state(page)).adapter;
    if (hardware) assertHardwareAdapter(engineAdapter, adapters);
    await startCapture(page);
    await checkpoint(page, 'title');
    const route = [], transitions = [];
    for (const [index, step] of fixture.steps.entries()) {
      progress = `${index}:${step.kind}:${step.moduleId}:${step.textId}`;
      const measurement = await measureAction(page, {
        label: `${index}:${step.kind}:${step.moduleId}`,
        action: { type: index === 0 ? 'new_game' : 'advance' }, expectedTextId: step.textId,
      });
      const current = await state(page);
      expect(current.error).toBeNull();
      expect(current.dialogue.ready).toBe(true);
      if (step.kind === 'chapter') {
        transitions.push({ ...measurement, module: step.moduleId });
        route.push({ textId: current.dialogue.id, text: current.dialogue.visible, variables: current.variables });
        expect(current.variables.visit_count.value).toBe(route.length);
      }
      await checkpoint(page, `${index}:${step.kind}`);
      // Fixed user think time, never wait for prefetch completion to bias the A/B run.
      if (step.kind === 'lead-in' && !fixture.capacity) await page.waitForTimeout(dwellMs);
    }
    const reading = await checkpoint(page, 'route-complete');
    progress = 'lifecycle';
    const recovery = await lifecycle(page, fixture);
    const capture = await stopCapture(page);
    expect(errors).toEqual([]);
    expect(capture.violations).toEqual([]);
    expect(capture.traceIncomplete).toBe(false);
    expect(capture.resourceTimingsIncomplete).toBe(false);
    expect(capture.samplerTruncated).toBe(false);
    expect(capture.peaks.encodedResidencyBudgetBytes).toBe(16 * 1024 * 1024);
    expect(capture.peaks.stagingBudgetEncodedBytes).toBe(16 * 1024 * 1024);
    expect(capture.peaks.encodedResidencyBytes).toBeGreaterThan(0);
    expect(capture.peaks.wasmMemoryCapacityBytes).toBeGreaterThan(0);
    const failures = capture.events.filter(event => /^(module_failed|prefetch_failed|prepare_failed|diagnostic)$/.test(event.stage));
    expect(failures).toEqual([]);
    const prefetchRequests = capture.events.filter(event => event.stage === 'prefetch_requested');
    if (fixture.prefetchContent) expect(prefetchRequests.length).toBeGreaterThan(0);
    else expect(prefetchRequests).toEqual([]);
    if (fixture.capacity) {
      expect(fixture.totalContentBytes).toBeGreaterThan(16 * 1024 * 1024);
      const firstCode = fixture.program.modules[fixture.route[0]].code;
      // This is a logical reload; the HTTP cache can satisfy its bytes.
      const reloads = capture.events.filter(event => event.stage === 'module_requested' && event.object === firstCode && Number(event.at_us) / 1000 <= reading.atMs);
      expect(reloads.length).toBeGreaterThanOrEqual(2);
    }
    const allowed = new Set(Object.keys(fixture.manifest.objects));
    for (const event of capture.events.filter(event => event.stage === 'module_requested')) expect(allowed.has(event.object)).toBe(true);
    const locale = route.length ? (await state(page)).text_locale : 'zh-Hans';
    const otherLocales = new Set(Object.values(fixture.program.modules).flatMap(module =>
      Object.entries(module.locales).filter(([key]) => key !== locale).map(([, hash]) => hash)));
    expect(capture.events.filter(event => event.stage === 'module_requested' && otherLocales.has(event.object))).toEqual([]);
    for (const transition of transitions) {
      const code = fixture.program.modules[transition.module].code;
      const request = capture.events.findLast(event => event.stage === 'module_requested' && event.object === code &&
        Number(event.at_us) / 1000 <= transition.inputAtMs + transition.latencyMs);
      const related = request ? capture.events.filter(event => event.request === request.request && event.session === request.session) : [];
      const prefetched = related.find(event => event.stage === 'prefetch_ready');
      const ready = related.find(event => event.stage === 'content_ready' || event.stage === 'prefetch_ready');
      transition.contentRequest = request?.request ?? null;
      transition.prefetchReadyBeforeInput = prefetched ? Number(prefetched.at_us) / 1000 < transition.inputAtMs : false;
      transition.promoted = related.some(event => event.stage === 'content_promoted');
      transition.contentRequestToReadyMs = request && ready ? (Number(ready.at_us) - Number(request.at_us)) / 1000 : null;
    }
    const routeResources = capture.resources.filter(row => row.name.includes('/objects/') && row.startTimeMs <= reading.atMs);
    for (const row of routeResources) {
      expect(Number.isFinite(row.transferSize)).toBe(true);
      expect(Number.isFinite(row.encodedBodySize)).toBe(true);
    }
    const routeRequests = capture.events.filter(event => event.stage === 'module_requested' && Number(event.at_us) / 1000 <= reading.atMs);
    const network = {
      scope: 'reading route; resource timing starts after title; logical events include boot',
      logicalObjectRequests: routeRequests.length,
      uniqueLogicalObjects: new Set(routeRequests.map(event => event.object)).size,
      httpObjectRequests: routeResources.length,
      transferBytes: routeResources.reduce((sum, row) => sum + row.transferSize, 0),
      encodedBodyBytes: routeResources.reduce((sum, row) => sum + row.encodedBodySize, 0),
      zeroTransferObjects: routeResources.filter(row => row.transferSize === 0).length,
    };
    return {
      repetition, prefetch: fixture.prefetchContent, release: fixture.channel.release,
      engine: fixture.manifest.engine.wasm, semanticDigest: fixture.semanticDigest,
      adapter: engineAdapter, actualAdapters: adapters, elapsedMs: Date.now() - started,
      route, transitions, reading, recovery, network, capture,
    };
  } catch(error) {
    const failure=await page.evaluate(()=>({
      state:window.__nir?.state(),metrics:window.__nir?.metrics,
      diagnostics:window.__nir?.diagnostics(),
      checkpoints:window.__nirPerfCapture?.checkpoints,
      events:window.__nirPerfCapture?.events,
      resources:performance.getEntriesByType('resource').slice(-1000).map(entry=>entry.toJSON()),
    })).catch(captureError=>({captureError:String(captureError)}));
    await fs.mkdir('reports',{recursive:true});
    await fs.writeFile(`reports/performance-failure-${profile.id}-${fixture.moduleCount}-${fixture.prefetchContent}-${repetition}-${Date.now()}.json`,JSON.stringify({error:String(error),progress,release:fixture.channel.release,repetition,prefetch:fixture.prefetchContent,...failure},null,2));
    throw error;
  } finally { await context.close(); }
}

function summary(runs) {
  return Object.fromEntries([false, true].map(prefetch => {
    const selected = runs.filter(run => run.prefetch === prefetch);
    return [String(prefetch), {
      runs: selected.length,
      chapterInputToSubmitMs: distribution(selected.flatMap(run => run.transitions.map(row => row.latencyMs))),
      contentRequestToReadyMs: distribution(selected.flatMap(run => run.transitions.map(row => row.contentRequestToReadyMs).filter(value => value !== null))),
      prefetchReadyBeforeInput: selected.flatMap(run => run.transitions).filter(row => row.prefetchReadyBeforeInput).length,
      promoted: selected.flatMap(run => run.transitions).filter(row => row.promoted).length,
      logicalObjectRequests: distribution(selected.map(run => run.network.logicalObjectRequests)),
      transferBytes: distribution(selected.map(run => run.network.transferBytes)),
      encodedResidencyPeakBytes: distribution(selected.map(run => run.capture.peaks.encodedResidencyBytes)),
      stagingPeakBytes: distribution(selected.map(run => run.capture.peaks.stagingPeakEncodedBytes)),
      wasmCapacityPeakBytes: distribution(selected.map(run => run.capture.peaks.wasmMemoryCapacityBytes)),
      activeFrameIntervalsMs: distribution(selected.flatMap(run => run.capture.activeFrameIntervalsMs)),
      rollbackInputToSubmitMs: distribution(selected.map(run => run.recovery.rollback.latencyMs)),
      restoreInputToSubmitMs: distribution(selected.map(run => run.recovery.restore.latencyMs)),
    }];
  }));
}

const scenarios = [
  ...(smoke ? [networkProfiles[0]] : networkProfiles).flatMap(network =>
    (smoke ? [3] : [3, 32]).map(moduleCount => ({ id: `${network.id}-${moduleCount}`, network, moduleCount, capacity: false }))),
  { id: 'capacity-32', network: networkProfiles[0], moduleCount: 32, capacity: true },
];

for (const scenario of scenarios) test(`module scale ${scenario.id}`, async ({ browser }) => {
  const runs = [], fixtures = [];
  const count = scenario.capacity ? 1 : repetitions;
  const started = Date.now();
  const report = {
    format: 1, workload: scenario, mode: hardware ? 'hardware' : 'ci', browser: browser.version(),
    instrumentation: { runtimeDiagnostics: true, playwrightTrace: test.info().project.use.trace },
    smoke: smoke || (!scenario.capacity && repetitions < 5), repetitions: count,
    dwellMs: scenario.capacity ? 0 : dwellMs, cache: 'isolated-context; shared browser/GPU process',
    synthetic: scenario.capacity, timingGate: false, gpuTime: 'unmeasured', physicalMemory: 'unmeasured',
    runs, status: 'incomplete',
  };
  try {
    for (const prefetchContent of [false, true]) fixtures.push(await buildScaleFixture({
      moduleCount: scenario.moduleCount, capacity: scenario.capacity, prefetchContent, port: prefetchContent ? 4193 : 4192,
    }));
    report.setupMs = Date.now() - started;
    report.totalContentBytes = fixtures.map(fixture => fixture.totalContentBytes);
    expect(fixtures[0].semanticDigest).toBe(fixtures[1].semanticDigest);
    for (let repetition = 0; repetition < count; repetition++) {
      const pair = [];
      for (const fixture of repetition % 2 ? [...fixtures].reverse() : fixtures) {
        const result = await runRoute(browser, fixture, scenario.network, repetition);
        pair.push(result); runs.push(result);
      }
      expect(pair[0].route).toEqual(pair[1].route);
      console.info(`[performance] ${scenario.id}: paired route ${repetition + 1}/${count} complete`);
    }
    report.status = 'passed';
  } catch (error) { report.error = String(error); throw error; }
  finally {
    report.summary = summary(runs);
    await fs.mkdir('reports', { recursive: true });
    await fs.writeFile(`reports/performance-modules-${scenario.id}.json`, JSON.stringify(report, null, 2));
    for (const fixture of fixtures) await closeScaleFixture(fixture);
  }
});
