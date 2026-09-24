import { test, expect } from '@playwright/test';

async function bootAndProbe(page, trace = true) {
  await page.goto(`/?test=1${trace ? '' : '&trace=0'}`);
  await page.waitForFunction(() => window.__nir?.state().ready && !window.__nir.state().loading);
  await page.evaluate(async () => {
    const root=new URL('../../',location.href);
    const digest=/\/releases\/([a-f0-9]{64})\//.exec(location.pathname)[1];
    const release = await (await fetch(new URL(`releases/${digest}.json`,root))).json();
    const wasm = await import(new URL(release.objects[release.engine.js].path, root).href);
    const original = wasm.Engine.prototype.state;
    window.__fullStateCalls = 0;
    wasm.Engine.prototype.state = function () {
      window.__fullStateCalls++;
      window.__observedEngine = this;
      return original.call(this);
    };
    window.__nir.state();
    window.__fullStateCalls = 0;
  });
}

test('host actions use compact state and preserve full diagnostic state', async ({ page }) => {
  await bootAndProbe(page);
  for (const type of ['menu', 'close']) {
    const frames = await page.evaluate(type => {
      const frames = window.__nir.metrics.frames;
      window.__nir.action({ type });
      return frames;
    }, type);
    await page.waitForFunction(frames => window.__nir.metrics.frames > frames, frames);
  }
  expect(await page.evaluate(() => window.__fullStateCalls)).toBe(0);
  const { compact, full } = await page.evaluate(() => ({
    compact: JSON.parse(window.__observedEngine.host_state()), full: window.__nir.state(),
  }));
  for (const field of ['ready', 'session', 'device', 'interaction', 'sequence', 'screen', 'paused', 'loading', 'frames']) {
    expect(compact[field], field).toEqual(full[field]);
  }
  expect(compact.has_dialogue).toBe(Boolean(full.dialogue));
  for (const field of ['dialogue', 'variables', 'history', 'diagnostic']) expect(compact).not.toHaveProperty(field);
  expect(full).toHaveProperty('variables');
  expect(full).toHaveProperty('dialogue');
  const performance = await page.evaluate(() => window.__nir.diagnostics().performance);
  expect(performance.enabled).toBe(true);
  expect(performance.stage_semantics).toBe('inclusive_non_additive');
  expect(performance.turns.length).toBeGreaterThan(0);
  expect(performance.turns.length).toBeLessThanOrEqual(64);
  expect(performance.stages['queue.submit'].count).toBeGreaterThan(0);
  for (const turn of performance.turns) for (const stage of turn.stages) {
    expect(stage.end_us).toBeGreaterThanOrEqual(stage.start_us);
    expect(stage.duration_us).toBe(stage.end_us - stage.start_us);
  }
});

test('disabled profiling keeps gameplay and diagnostics available', async ({ page }) => {
  await bootAndProbe(page, false);
  await page.evaluate(() => window.__nir.action({ type: 'new_game' }));
  await page.waitForFunction(() => window.__nir.state().dialogue && !window.__nir.state().loading);
  const result = await page.evaluate(() => ({
    profile: JSON.parse(window.__observedEngine.take_profile()),
    diagnostics: window.__nir.diagnostics(), state: window.__nir.state(),
  }));
  expect(result.profile).toEqual([]);
  expect(result.diagnostics.enabled).toBe(false);
  expect(result.diagnostics.events).toEqual([]);
  expect(result.diagnostics.performance.enabled).toBe(false);
  expect(result.diagnostics.performance.turns).toEqual([]);
  expect(result.diagnostics.performance.stages).toEqual({});
  expect(result.state.error).toBeNull();
});
