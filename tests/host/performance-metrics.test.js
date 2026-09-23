import { test } from 'node:test';
import assert from 'node:assert/strict';
import { TraceRecorder } from '../../crates/nir-platform-web/host.js';
import {
  assertHardwareAdapter, correlateRenderSubmitted, distribution, measureAction, startCapture, stopCapture,
} from '../performance/metrics.js';

test('distribution reports an explicit empty result and counts invalid samples', () => {
  assert.deepEqual(distribution([]), { samples: 0, invalidSamples: 0, min: null, median: null, p95: null, max: null });
  assert.deepEqual(distribution([1, 2, NaN, -1, '3', Infinity]), {
    samples: 2, invalidSamples: 4, min: 1, median: 1.5, p95: 2, max: 2,
  });
});

test('hardware validation requires a known engine string and an identified non-fallback engine adapter', () => {
  const probe = { vendor: 'nvidia', architecture: 'ampere', device: '', description: '', fallback: false };
  assert.deepEqual(assertHardwareAdapter('BrowserWebGpu / Other /  / ', [probe]), {
    engineAdapter: 'BrowserWebGpu / Other /  /',
    actualAdapters: [{ vendor: 'nvidia', architecture: 'ampere', device: '', description: '', fallback: false }],
  });
  for (const renderer of ['SwiftShader WebGPU', 'llvmpipe', 'lavapipe', 'unknown']) {
    assert.throws(() => assertHardwareAdapter(renderer, [probe]), /hardware adapter check failed/);
  }
  assert.throws(() => assertHardwareAdapter('BrowserWebGpu / Other', [{ ...probe, fallback: true }]), /fallback=false/);
  assert.throws(() => assertHardwareAdapter('BrowserWebGpu / Other', [{ ...probe, fallback: undefined }]), /fallback=false/);
  assert.throws(() => assertHardwareAdapter('BrowserWebGpu / Other', [{ ...probe, architecture: 'unknown' }]), /software or unknown/);
  assert.throws(() => assertHardwareAdapter('BrowserWebGpu / Other', [{ adapterAvailable: false }]), /no adapter record/);
});

test('action correlation selects the submission for the exact expected dialogue frame', () => {
  const input = { stage: 'input_received', at_us: '1000' };
  const events = [
    { stage: 'render_submitted', at_us: '900', frames: 12 },
    { stage: 'render_submitted', at_us: '1100', frames: 11 },
    { stage: 'render_submitted', at_us: '1200', frames: 12 },
  ];
  assert.equal(correlateRenderSubmitted(input, events, 12), events[2]);
  assert.equal(correlateRenderSubmitted(input, events, 13), null);
  assert.equal(correlateRenderSubmitted({ stage: 'input_received', at_us: 'bad' }, events, 12), null);
});

function makeBrowserPage({ capacity = 32 } = {}) {
  let now = 1;
  const trace = new TraceRecorder({ enabled: true, capacity, now: () => now++ / 1000 });
  const state = {
    ready: true, screen: 'Story', session: 1, device: 1, interaction: 1, frames: 10,
    paused: true, loading: false, dialogue: { id: 'old', ready: true, locale: 'en' },
    resident_bytes: 20, content_residency: { resident_bytes: 10, budget_bytes: 100 }, wasm_memory_bytes: 4096,
    adapter: 'BrowserWebGpu / Other /  / ',
  };
  const metrics = { frames: 10, activeRequests: 0, peakResidentBytes: 20 };
  const api = {
    state: () => structuredClone(state),
    metrics,
    needsClock: () => false,
    diagnostics: () => ({ ...trace.snapshot(), content_staging: { encoded_bytes: 0, peak_encoded_bytes: 0, budget_encoded_bytes: 100, reservations: 0, waiting_demands: 0 } }),
    action: () => {
      trace.record('input_received', { sequence: 1 });
      trace.record('render_submitted', { frames: 11 });
      state.frames = metrics.frames = 11;
      state.dialogue = { id: 'intermediate', ready: true, locale: 'en' };
      trace.record('render_submitted', { frames: 12 });
      state.frames = metrics.frames = 12;
      state.dialogue = { id: 'target', ready: true, locale: 'en' };
      return Promise.resolve(true);
    },
  };
  const window = { __nir: api, __nirActualAdapters: [] };
  const performance = {
    now: () => now,
    setResourceTimingBufferSize() {},
    addEventListener() {}, removeEventListener() {}, getEntriesByType: () => [],
  };
  let rafId = 0;
  const globals = { window, performance, requestAnimationFrame: () => ++rafId, cancelAnimationFrame() {}, location: { href: 'http://127.0.0.1/' } };
  const withGlobals = async callback => {
    const old = new Map();
    for (const [key, value] of Object.entries(globals)) {
      old.set(key, Object.getOwnPropertyDescriptor(globalThis, key));
      Object.defineProperty(globalThis, key, { configurable: true, writable: true, value });
    }
    try { return await callback(); }
    finally {
      for (const key of Object.keys(globals)) {
        const descriptor = old.get(key);
        if (descriptor) Object.defineProperty(globalThis, key, descriptor);
        else delete globalThis[key];
      }
    }
  };
  return {
    trace,
    page: {
      evaluate: (fn, arg) => withGlobals(() => fn(arg)),
      waitForFunction: (fn, arg) => withGlobals(() => assert.equal(fn(arg), true)),
    },
  };
}

test('measureAction waits for the ready target dialogue and matches its exact frame event', async () => {
  const { page } = makeBrowserPage();
  await startCapture(page);
  const result = await measureAction(page, { label: 'advance', action: { type: 'advance' }, expectedTextId: 'target' });
  assert.equal(result.after.dialogueId, 'target');
  assert.equal(result.after.dialogueReady, true);
  assert.equal(result.renderSubmitted.frames, 12);
  assert.equal(result.latencyMs, 0.002);
  assert.deepEqual(result.requestStats.module_requested, 0);
});

test('incremental trace draining detects only events lost since the previous snapshot', async () => {
  const { page, trace } = makeBrowserPage({ capacity: 2 });
  await startCapture(page);
  trace.record('one'); trace.record('two'); trace.record('three');
  const capture = await stopCapture(page);
  assert.equal(capture.traceIncomplete, true);
  assert.equal(capture.trace.missingEvents, 1);
  assert.deepEqual(capture.events.map(event => event.stage), ['two', 'three']);
});
