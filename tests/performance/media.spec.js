import { test, expect } from '@playwright/test';
import fs from 'node:fs/promises';
import { buildScaleFixture, closeScaleFixture } from './fixtures.js';
import { distribution, installAdapterProbe, assertHardwareAdapter, startCapture, checkpoint, measureAction, stopCapture } from './metrics.js';
import { networkProfiles, applyNetwork, positiveInteger } from './scenarios.js';

const repetitions = positiveInteger(process.env.NIR_PERF_SCALE_REPETITIONS, 5, 'NIR_PERF_SCALE_REPETITIONS');
const profile = networkProfiles.find(profile => profile.id === 'shaped');

test('M3 bounded media lookahead paired transitions', async ({ browser }) => {
  const fixtures = [], report = { format: 1, status: 'incomplete', repetitions, dwellMs: 1000,
    mode: process.env.NIR_PERF_MODE, network: profile,
    instrumentation: { runtimeDiagnostics: true, playwrightTrace: test.info().project.use.trace }, runs: [] };
  try {
    for (const enabled of [false, true]) fixtures.push(await buildScaleFixture({ moduleCount: 3,
      prefetchContent: true, mediaScenario: true, prefetchMedia: enabled, port: enabled ? 4199 : 4198 }));
    // Settings and source revisions differ; compare actual story results below.
    for (let repetition = 0; repetition < repetitions; repetition++) {
      for (const fixture of repetition % 2 ? [...fixtures].reverse() : fixtures) {
        const context = await browser.newContext({ viewport: { width: 1280, height: 800 }, locale: 'zh-CN' });
        try {
          await installAdapterProbe(context);
          const page = await context.newPage(), errors = [];
          page.on('pageerror', error => errors.push(String(error)));
          await applyNetwork(context, page, profile);
          await page.goto(`${fixture.origin}/?test=1`);
          await page.waitForFunction(() => window.__nir?.state().ready && !window.__nir.state().loading);
          if (process.env.NIR_PERF_MODE === 'hardware')
            assertHardwareAdapter(...await page.evaluate(() => [window.__nir.state().adapter, window.__nirActualAdapters]));
          await startCapture(page);
          const transitions = [], story = [];
          for (const [index, step] of fixture.steps.entries()) {
            const measured = await measureAction(page, { label: `${index}:${step.kind}:${step.moduleId}`,
              action: { type: index ? 'advance' : 'new_game' }, expectedTextId: step.textId });
            const current = await page.evaluate(() => window.__nir.state());
            expect(current.error).toBeNull();
            if (step.kind === 'media') transitions.push(measured);
            story.push({ id: current.dialogue.id, text: current.dialogue.visible, variables: current.variables });
            await checkpoint(page, `${index}:${step.kind}`);
            // Identical user think time; do not wait on speculative completion.
            if (step.kind === 'chapter' || step.kind === 'lead-in') await page.waitForTimeout(1000);
          }
          await page.evaluate(() => window.__nir.action({ type: 'title' }));
          await page.waitForFunction(() => {
            const api = window.__nir, d = api.diagnostics();
            return api.state().screen === 'Title' && !api.state().loading && api.metrics.activeRequests === 0 &&
              d.content_staging.encoded_bytes === 0 && !d.host_work.media_jobs && !d.host_work.decode_pool_active;
          });
          await checkpoint(page, 'title-cleanup');
          const capture = await stopCapture(page);
          expect(capture.violations).toEqual([]);
          expect(capture.traceIncomplete).toBe(false);
          expect(capture.peaks.playerResidentBytes).toBeLessThanOrEqual(128 * 1024 * 1024);
          expect(errors).toEqual([]);
          const predictions = capture.events.filter(e => e.stage === 'media_lookahead_requested');
          if (fixture.prefetchMedia) expect(predictions.length).toBeGreaterThan(0);
          else expect(predictions).toEqual([]);
          const d = await page.evaluate(() => window.__nir.diagnostics());
          report.runs.push({ repetition, enabled: fixture.prefetchMedia, release: d.release, engine: d.engine,
            actualAdapters: await page.evaluate(() => window.__nirActualAdapters), story, transitions, capture });
        } finally { await context.close(); }
      }
    }
    for (const run of report.runs) expect(run.story).toEqual(report.runs[0].story);
    report.summary = Object.fromEntries([false, true].map(enabled => {
      const rows = report.runs.filter(run => run.enabled === enabled);
      return [String(enabled), { transitionMs: distribution(rows.flatMap(row => row.transitions.map(t => t.latencyMs))),
        requests: rows.map(row => row.capture.events.filter(e => e.stage === 'fetch_started').length),
        verifiedBytes: rows.map(row => row.capture.events.filter(e => e.stage === 'fetch_verified').reduce((sum, e) => sum + Number(e.bytes || 0), 0)),
        predictedBatches: rows.map(row => row.capture.events.filter(e => e.stage === 'media_lookahead_requested').length),
        cancellations: rows.map(row => row.capture.events.filter(e => e.stage === 'media_lookahead_cancelled').length),
        cancelledPredictionBytes: rows.map(row => {
          const cancelled = new Set(row.capture.events.filter(e => e.stage === 'media_lookahead_cancelled').map(e => e.request));
          return row.capture.events.filter(e => e.stage === 'fetch_verified' && cancelled.has(e.request))
            .reduce((sum, e) => sum + Number(e.bytes || 0), 0);
        }),
        peaks: rows.map(row => row.capture.peaks) }];
    }));
    report.status = 'passed';
  } catch (error) { report.error = String(error); throw error; }
  finally {
    await fs.mkdir('reports', { recursive: true });
    await fs.writeFile('reports/performance-m3-media.json', JSON.stringify(report, null, 2));
    for (const fixture of fixtures) await closeScaleFixture(fixture);
  }
});
