const ACCOUNTING = Object.freeze({
  encodedResidency: 'state.content_residency.resident_bytes and budget_bytes; engine encoded-content accounting',
  contentStaging: 'diagnostics.content_staging.encoded_bytes and budget_encoded_bytes; encoded bytes held while preparing content',
  mediaResourceTimings: 'PerformanceResourceTiming transferSize, encodedBodySize, and decodedBodySize; browser network estimates, not decoded media residency',
  wasmMemoryCapacity: 'state.wasm_memory_bytes; WASM linear-memory buffer capacity',
  physicalMemory: 'unmeasured',
  gpuTime: 'unmeasured',
});

const finiteNonnegative = value => typeof value === 'number' && Number.isFinite(value) && value >= 0;
const safeObject = value => value && typeof value === 'object' && !Array.isArray(value) ? value : {};

export function distribution(values) {
  const input = Array.isArray(values) ? values : [];
  const sorted = input.filter(finiteNonnegative).sort((a, b) => a - b);
  if (!sorted.length) {
    return { samples: 0, invalidSamples: input.length, min: null, median: null, p95: null, max: null };
  }
  const middle = Math.floor(sorted.length / 2);
  const median = sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
  return {
    samples: sorted.length,
    invalidSamples: input.length - sorted.length,
    min: sorted[0], median,
    p95: sorted[Math.ceil(sorted.length * 0.95) - 1],
    max: sorted[sorted.length - 1],
  };
}

const SOFTWARE_ADAPTER = /swiftshader|llvmpipe|lavapipe|software|unknown/i;
const UNKNOWN_IDENTITY = /^(?:|unknown|other|none|null|n\/a)$/i;
function requireValue(condition, message) { if (!condition) throw new Error(`hardware adapter check failed: ${message}`); }

// `actualAdapters` must be the records captured from the engine's own requestAdapter calls.
export function assertHardwareAdapter(engineAdapter, actualAdapters) {
  if (engineAdapter && typeof engineAdapter === 'object' && !Array.isArray(engineAdapter)) {
    actualAdapters ??= engineAdapter.actualAdapters ?? engineAdapter.probes;
    engineAdapter = engineAdapter.engineAdapter ?? engineAdapter.engine ?? engineAdapter.adapter;
  }
  requireValue(typeof engineAdapter === 'string' && engineAdapter.trim().length > 0, 'engine adapter string is missing');
  requireValue(!SOFTWARE_ADAPTER.test(engineAdapter), `software or unknown engine adapter: ${engineAdapter}`);
  requireValue(Array.isArray(actualAdapters), 'actual engine requestAdapter records are missing');
  const returned = actualAdapters.filter(record => record && record.adapterAvailable !== false && record.available !== false);
  requireValue(returned.length > 0, 'the engine returned no adapter record');
  const normalized = returned.map((record, index) => {
    const probe = safeObject(record);
    const vendor = typeof probe.vendor === 'string' ? probe.vendor.trim() : '';
    const architecture = typeof probe.architecture === 'string' ? probe.architecture.trim() : '';
    const device = typeof probe.device === 'string' ? probe.device.trim() : '';
    const description = typeof probe.description === 'string' ? probe.description.trim() : '';
    const fallback = probe.fallback ?? probe.isFallbackAdapter;
    requireValue(fallback === false, `adapter ${index} did not explicitly report fallback=false`);
    requireValue(!SOFTWARE_ADAPTER.test([vendor, architecture, device, description].join(' ')), `adapter ${index} identifies a software or unknown renderer`);
    requireValue(!UNKNOWN_IDENTITY.test(vendor) && !UNKNOWN_IDENTITY.test(architecture), `adapter ${index} lacks positive vendor and architecture identity`);
    return { vendor, architecture, device, description, fallback: false };
  });
  return { engineAdapter: engineAdapter.trim(), actualAdapters: normalized };
}

// Wrap only the call the engine makes. This does not issue a second adapter request.
export async function installAdapterProbe(context) {
  await context.addInitScript(() => {
    window.__nirActualAdapters = [];
    const gpu = navigator.gpu;
    if (!gpu || typeof gpu.requestAdapter !== 'function') {
      window.__nirAdapterProbeError = 'navigator.gpu.requestAdapter unavailable';
      return;
    }
    const readInfo = adapter => {
      if (!adapter) return { adapterAvailable: false, fallback: null, vendor: '', architecture: '', device: '', description: '' };
      let info = {};
      try { info = adapter.info || {}; } catch {}
      const value = key => { try { return typeof info[key] === 'string' ? info[key] : ''; } catch { return ''; } };
      let fallback = null;
      try { if (typeof info.isFallbackAdapter === 'boolean') fallback = info.isFallbackAdapter; } catch {}
      return { adapterAvailable: true, vendor: value('vendor'), architecture: value('architecture'), device: value('device'), description: value('description'), fallback, isFallbackAdapter: fallback };
    };
    const patch = target => {
      const original = target?.requestAdapter;
      if (typeof original !== 'function') return false;
      const wrapped = async function(options) {
        const adapter = await original.call(this, options);
        let copiedOptions = null;
        try { copiedOptions = options ? { powerPreference: options.powerPreference, forceFallbackAdapter: options.forceFallbackAdapter } : {}; } catch {}
        window.__nirActualAdapters.push({ atMs: performance.now(), options: copiedOptions, ...readInfo(adapter) });
        return adapter;
      };
      try { Object.defineProperty(target, 'requestAdapter', { configurable: true, value: wrapped }); return true; } catch { return false; }
    };
    if (!patch(gpu) && !patch(Object.getPrototypeOf(gpu))) window.__nirAdapterProbeError = 'could not wrap navigator.gpu.requestAdapter';
  });
}

export async function startCapture(page) {
  return page.evaluate(() => {
    if (!window.__nir) throw new Error('window.__nir is not available; open the player with ?test=1');
    if (window.__nirPerfCapture?.active) throw new Error('performance capture is already active');
    const api = window.__nir;
    const maxSamples = 10000, maxIntervals = 20000, maxEvents = 50000;
    const maxResources = 20000, sampleEveryMs = 100, traceEveryMs = 100;
    const valid = value => typeof value === 'number' && Number.isFinite(value) && value >= 0;
    const clone = value => { try { return JSON.parse(JSON.stringify(value)); } catch { return null; } };
    const capture = {
      active: true, startedAtMs: performance.now(), events: [], checkpoints: [], samples: [], activeFrameIntervalsMs: [],
      violations: [], captureErrors: [], peaks: { playerResidentBytes: null, encodedResidencyBytes: null, encodedResidencyBudgetBytes: null, wasmMemoryCapacityBytes: null, stagingEncodedBytes: null, stagingPeakEncodedBytes: null, stagingBudgetEncodedBytes: null },
      prefetchStats: { requested: 0, delivered: 0, skipped: 0, failed: 0 },
      trace: { enabled: false, capacity: null, droppedAtStart: 0, droppedDuringCapture: 0, missingEvents: 0, snapshots: 0, incomplete: false, reasons: [] },
      lastTotal: null, lastTraceAtMs: -Infinity, lastSampleAtMs: -Infinity, lastActiveAtMs: null, lastWasActive: false,
      lastBudgetViolation: Object.create(null), samplerTruncated: false, resourceTimingsIncomplete: false, resourceBufferFull: false,
      appendEvents(rows) {
        for (const row of rows) {
          if (this.events.length >= maxEvents) {
            this.trace.incomplete = true;
            if (!this.trace.reasons.includes('collector_event_limit')) this.trace.reasons.push('collector_event_limit');
            break;
          }
          const event = clone(row);
          if (!event || typeof event.stage !== 'string') continue;
          this.events.push(event);
          if (event.stage === 'module_requested' && event.kind === 'prefetch') this.prefetchStats.requested++;
          if (event.stage === 'module_delivered' && event.kind === 'prefetch') this.prefetchStats.delivered++;
          if (event.stage === 'module_skipped' && String(event.code || '').startsWith('E_PREFETCH')) this.prefetchStats.skipped++;
          if (event.stage === 'prefetch_failed' || event.code === 'E_PREFETCH_FAILED') this.prefetchStats.failed++;
        }
      },
      markIncomplete(reason, missing = 0) {
        this.trace.incomplete = true;
        if (!this.trace.reasons.includes(reason)) this.trace.reasons.push(reason);
        if (valid(missing)) { this.trace.missingEvents += missing; this.trace.droppedDuringCapture += missing; }
      },
      drainTrace() {
        let report;
        try { report = api.diagnostics(); } catch (error) {
          this.captureErrors.push({ type: 'diagnostics-error', message: String(error) });
          this.markIncomplete('diagnostics_error');
          return null;
        }
        const snapshot = report && typeof report === 'object' ? report : {};
        const ring = snapshot.events;
        const rows = Array.isArray(ring) ? ring : [];
        const dropped = Number.isSafeInteger(snapshot.dropped) && snapshot.dropped >= 0 ? snapshot.dropped : 0;
        const total = dropped + rows.length;
        this.trace.snapshots++;
        this.trace.enabled = snapshot.enabled === true;
        if (Number.isSafeInteger(snapshot.capacity)) this.trace.capacity = snapshot.capacity;
        if (snapshot.enabled !== true) this.markIncomplete('trace_disabled');
        if (this.lastTotal === null) {
          this.trace.droppedAtStart = dropped;
          if (dropped > 0) this.markIncomplete('trace_dropped_before_capture', dropped);
          this.appendEvents(rows);
          this.lastTotal = total;
        } else {
          const count = total - this.lastTotal;
          if (count < 0) {
            this.markIncomplete('trace_counter_reset');
            this.appendEvents(rows);
            this.lastTotal = total;
          } else if (count > 0) {
            const missing = Math.max(0, count - rows.length);
            if (missing > 0) this.markIncomplete('trace_ring_overwritten_between_drains', missing);
            this.appendEvents(rows.slice(Math.max(0, rows.length - count)));
            this.lastTotal = total;
          }
        }
        const staging = snapshot.content_staging;
        if (staging && typeof staging === 'object') this.updateStaging(staging);
        return snapshot;
      },
      updatePeak(name, value) {
        if (valid(value)) this.peaks[name] = this.peaks[name] === null ? value : Math.max(this.peaks[name], value);
      },
      checkBudget(name, type, value, budget, atMs) {
        if (!valid(value) || !valid(budget)) return;
        this.updatePeak(name.peak, value);
        this.updatePeak(name.budget, budget);
        if (value <= budget) return;
        const key = `${type}:${budget}`;
        if (this.lastBudgetViolation[type] === key) return;
        this.lastBudgetViolation[type] = key;
        this.violations.push({ type, atMs, bytes: value, budgetBytes: budget });
      },
      updateStaging(staging) {
        const now = performance.now();
        this.updatePeak('stagingEncodedBytes', staging.encoded_bytes);
        this.updatePeak('stagingPeakEncodedBytes', staging.peak_encoded_bytes);
        this.updatePeak('stagingBudgetEncodedBytes', staging.budget_encoded_bytes);
        this.checkBudget({ peak: 'stagingPeakEncodedBytes', budget: 'stagingBudgetEncodedBytes' }, 'content-staging-budget', staging.encoded_bytes, staging.budget_encoded_bytes, now);
        this.checkBudget({ peak: 'stagingPeakEncodedBytes', budget: 'stagingBudgetEncodedBytes' }, 'content-staging-peak-budget', staging.peak_encoded_bytes, staging.budget_encoded_bytes, now);
      },
      recordState(state, now, includeSample) {
        const s = state && typeof state === 'object' ? state : {};
        const residency = s.content_residency && typeof s.content_residency === 'object' ? s.content_residency : {};
        this.updatePeak('playerResidentBytes', s.resident_bytes);
        this.updatePeak('wasmMemoryCapacityBytes', s.wasm_memory_bytes);
        this.checkBudget({ peak: 'encodedResidencyBytes', budget: 'encodedResidencyBudgetBytes' }, 'encoded-residency-budget', residency.resident_bytes, residency.budget_bytes, now);
        if (includeSample && this.samples.length < maxSamples) {
          this.samples.push({ atMs: now, frames: valid(s.frames) ? s.frames : null, active: this.lastWasActive,
            playerResidentBytes: valid(s.resident_bytes) ? s.resident_bytes : null,
            encodedResidencyBytes: valid(residency.resident_bytes) ? residency.resident_bytes : null,
            wasmMemoryCapacityBytes: valid(s.wasm_memory_bytes) ? s.wasm_memory_bytes : null });
        } else if (includeSample) this.samplerTruncated = true;
      },
      captureState() { return clone(api.state()); },
      captureMetrics() { return clone(api.metrics); },
      stopSampler() { this.active = false; if (this.raf) cancelAnimationFrame(this.raf); if (this.resourceFullHandler) performance.removeEventListener('resourcetimingbufferfull', this.resourceFullHandler); },
    };
    capture.drainTrace();
    const initial = api.state();
    capture.recordState(initial, capture.startedAtMs, true);
    try { performance.setResourceTimingBufferSize?.(maxResources); } catch {}
    capture.resourceFullHandler = () => { capture.resourceBufferFull = true; capture.resourceTimingsIncomplete = true; };
    performance.addEventListener?.('resourcetimingbufferfull', capture.resourceFullHandler);
    const sample = now => {
      if (!capture.active) return;
      try {
        const current = api.state();
        const active = !!api.needsClock();
        if (active && capture.lastWasActive && capture.lastActiveAtMs !== null && capture.activeFrameIntervalsMs.length < maxIntervals) {
          capture.activeFrameIntervalsMs.push(now - capture.lastActiveAtMs);
        }
        capture.lastWasActive = active;
        capture.lastActiveAtMs = active ? now : null;
        capture.recordState(current, now, now - capture.lastSampleAtMs >= sampleEveryMs);
        if (now - capture.lastSampleAtMs >= sampleEveryMs) capture.lastSampleAtMs = now;
        if (now - capture.lastTraceAtMs >= traceEveryMs) { capture.drainTrace(); capture.lastTraceAtMs = now; }
      } catch (error) {
        capture.captureErrors.push({ type: 'sampler-error', message: String(error) });
        capture.markIncomplete('sampler_error');
      }
      capture.raf = requestAnimationFrame(sample);
    };
    capture.raf = requestAnimationFrame(sample);
    window.__nirPerfCapture = capture;
    return { startedAtMs: capture.startedAtMs, traceEnabled: capture.trace.enabled, traceCapacity: capture.trace.capacity, sampleEveryMs, traceEveryMs };
  });
}

export async function checkpoint(page, label) {
  return page.evaluate(({ label, accounting }) => {
    const capture = window.__nirPerfCapture;
    if (!capture?.active) throw new Error('performance capture is not active');
    capture.drainTrace();
    const state = capture.captureState() || {};
    const metrics = capture.captureMetrics() || {};
    let report = null;
    try { report = window.__nir.diagnostics(); } catch {}
    const contentStaging = report?.content_staging ? JSON.parse(JSON.stringify(report.content_staging)) : null;
    const checkpoint = {
      label: String(label), atMs: performance.now(),
      state: {
        ready: state.ready ?? null, screen: state.screen ?? null, session: state.session ?? null, device: state.device ?? null,
        interaction: state.interaction ?? null, frames: state.frames ?? null, paused: state.paused ?? null, loading: state.loading ?? null,
        dialogue: state.dialogue ? { id: state.dialogue.id ?? null, ready: state.dialogue.ready ?? null, locale: state.dialogue.locale ?? null } : null,
        choice: state.choice ? true : false, residentBytes: state.resident_bytes ?? null,
        contentResidency: state.content_residency ?? null, wasmMemoryBytes: state.wasm_memory_bytes ?? null, adapter: state.adapter ?? null,
      },
      metrics: {
        frames: metrics.frames ?? null, peakResidentBytes: metrics.peakResidentBytes ?? null,
        activeRequests: metrics.activeRequests ?? null, requestHighWater: metrics.requestHighWater ?? null,
        acceptedRequests: metrics.acceptedRequests ?? null, completedRequests: metrics.completedRequests ?? null,
        cancelledRequests: metrics.cancelledRequests ?? null, inboxHighWater: metrics.inboxHighWater ?? null,
        maxTurnUploadBytes: metrics.maxTurnUploadBytes ?? null, uploadSteps: metrics.uploadSteps ?? null,
        maxTurnWork: metrics.maxTurnWork ?? null, navigationToFirstLineMs: metrics.navigationToFirstLineMs ?? null,
      },
      contentStaging,
      accounting,
    };
    capture.checkpoints.push(checkpoint);
    return checkpoint;
  }, { label, accounting: { ...ACCOUNTING } });
}

export async function measureAction(page, { label, action, expectedTextId, requireReady = true }) {
  if (!action || typeof action !== 'object' || typeof expectedTextId !== 'string' || !expectedTextId) throw new TypeError('measureAction requires action and expectedTextId');
  const start = await page.evaluate(action => {
    const capture = window.__nirPerfCapture, api = window.__nir;
    if (!capture?.active || !api) throw new Error('performance capture is not active');
    capture.drainTrace();
    const before = api.state();
    const eventStart = capture.events.length;
    const startedAtMs = performance.now();
    const promise = api.action(action);
    Promise.resolve(promise).catch(error => { capture.captureErrors.push({ type: 'action-error', message: String(error) }); });
    return {
      eventStart, startedAtMs,
      before: { frames: before.frames, dialogueId: before.dialogue?.id ?? null, session: before.session, activeRequests: api.metrics.activeRequests ?? null },
    };
  }, action);
  await page.waitForFunction(({ frames, textId, requireReady }) => {
    const state = window.__nir?.state(), dialogue = state?.dialogue;
    return !!state && !state.loading && state.frames > frames && dialogue?.id === textId && (!requireReady || dialogue.ready === true);
  }, { frames: start.before.frames, textId: expectedTextId, requireReady });
  return page.evaluate(({ label, expectedTextId, eventStart, startedAtMs, before }) => {
    const capture = window.__nirPerfCapture, api = window.__nir;
    capture.drainTrace();
    const afterState = api.state(), afterMetrics = api.metrics;
    const events = capture.events.slice(eventStart);
    const inputEvent = events.find(event => event.stage === 'input_received');
    if (!inputEvent) throw new Error(`no input_received trace event for ${label ?? 'action'}`);
    const frame = afterState.frames;
    const renderSubmitted = events.find(event => event.stage === 'render_submitted' && event.frames === frame && atOrAfter(event.at_us, inputEvent.at_us));
    if (!renderSubmitted) throw new Error(`no render_submitted event for expected dialogue frame ${frame} after input ${inputEvent.at_us}`);
    const latencyMs = (Number(renderSubmitted.at_us) - Number(inputEvent.at_us)) / 1000;
    if (!Number.isFinite(latencyMs) || latencyMs < 0) throw new Error('invalid input-to-submit trace timestamps');
    const counts = Object.create(null);
    for (const event of events) counts[event.stage] = (counts[event.stage] || 0) + 1;
    const requestStages = ['request_reserved', 'request_completed', 'request_cancelled', 'callback_discarded', 'resource_queued', 'resource_admitted', 'fetch_started', 'fetch_verified', 'module_requested', 'module_delivered', 'module_skipped', 'module_failed', 'prefetch_failed'];
    const requestStats = Object.fromEntries(requestStages.map(stage => [stage, counts[stage] || 0]));
    requestStats.activeRequestsBefore = before.activeRequests;
    requestStats.activeRequestsAfter = afterMetrics.activeRequests ?? null;
    requestStats.prefetchRequested = events.filter(event => event.stage === 'module_requested' && event.kind === 'prefetch').length;
    requestStats.prefetchDelivered = events.filter(event => event.stage === 'module_delivered' && event.kind === 'prefetch').length;
    requestStats.prefetchSkipped = events.filter(event => event.stage === 'module_skipped' && String(event.code || '').startsWith('E_PREFETCH')).length;
    requestStats.prefetchFailed = events.filter(event => event.stage === 'prefetch_failed' || event.code === 'E_PREFETCH_FAILED').length;
    return {
      label: label ?? null, expectedTextId,
      inputAtMs: Number(inputEvent.at_us) / 1000, requestedAtMs: startedAtMs, latencyMs,
      before, after: { frames: afterState.frames, dialogueId: afterState.dialogue?.id ?? null, dialogueReady: afterState.dialogue?.ready ?? null, session: afterState.session },
      inputEvent, renderSubmitted, events, requestStats, traceIncomplete: capture.trace.incomplete,
    };
    function atOrAfter(a, b) { return /^\d{1,20}$/.test(String(a)) && /^\d{1,20}$/.test(String(b)) && BigInt(a) >= BigInt(b); }
  }, { label, expectedTextId, eventStart: start.eventStart, startedAtMs: start.startedAtMs, before: start.before });
}

export async function stopCapture(page) {
  return page.evaluate(accounting => {
    const capture = window.__nirPerfCapture;
    if (!capture) throw new Error('performance capture was not started');
    if (!capture.active) throw new Error('performance capture was already stopped');
    capture.drainTrace();
    capture.stopSampler();
    const stopAtMs = performance.now();
    const resourceRows = [];
    try {
      for (const entry of performance.getEntriesByType('resource')) {
        if (entry.startTime < capture.startedAtMs || resourceRows.length >= 20000) continue;
        let name = entry.name;
        try {
          const url = new URL(entry.name, location.href);
          name = url.protocol === 'data:' ? 'data:' : `${url.origin}${url.pathname}`;
        } catch { name = String(name).slice(0, 512); }
        resourceRows.push({ name, startTimeMs: entry.startTime, durationMs: entry.duration,
          fetchStartMs: entry.fetchStart, domainLookupStartMs: entry.domainLookupStart,
          domainLookupEndMs: entry.domainLookupEnd, connectStartMs: entry.connectStart,
          connectEndMs: entry.connectEnd, requestStartMs: entry.requestStart,
          responseStartMs: entry.responseStart, responseEndMs: entry.responseEnd,
          nextHopProtocol: entry.nextHopProtocol || null,
          transferSize: Number.isFinite(entry.transferSize) ? entry.transferSize : null,
          encodedBodySize: Number.isFinite(entry.encodedBodySize) ? entry.encodedBodySize : null,
          decodedBodySize: Number.isFinite(entry.decodedBodySize) ? entry.decodedBodySize : null,
          initiatorType: entry.initiatorType || null });
      }
    } catch (error) { capture.captureErrors.push({ type: 'resource-timing-error', message: String(error) }); }
    const api = window.__nir;
    const state = api?.state?.() || {};
    const diagnostics = (() => { try { return api?.diagnostics?.() || null; } catch { return null; } })();
    const probes = Array.isArray(window.__nirActualAdapters) ? window.__nirActualAdapters : [];
    const violations = [...capture.violations, ...capture.captureErrors.map(error => ({ type: error.type, message: error.message }))];
    return {
      format: 1, startedAtMs: capture.startedAtMs, stoppedAtMs: stopAtMs,
      events: capture.events, checkpoints: capture.checkpoints,
      activeFrameIntervalsMs: capture.activeFrameIntervalsMs, samples: capture.samples, samplerTruncated: capture.samplerTruncated,
      resources: resourceRows, resourceTimingsIncomplete: capture.resourceTimingsIncomplete || capture.resourceBufferFull,
      adapterProbe: { error: window.__nirAdapterProbeError || null, actualAdapters: probes },
      prefetchStats: capture.prefetchStats,
      peaks: { ...capture.peaks }, violations, traceIncomplete: capture.trace.incomplete,
      trace: { ...capture.trace, reasons: [...capture.trace.reasons] },
      finalState: { playerResidentBytes: state.resident_bytes ?? null, contentResidency: state.content_residency ?? null, wasmMemoryCapacityBytes: state.wasm_memory_bytes ?? null,
        activeRequests: api?.metrics?.activeRequests ?? null, contentStaging: diagnostics?.content_staging ?? null },
      accounting,
    };
  }, { ...ACCOUNTING });
}

// Select the render submission belonging to the exact state frame where the expected dialogue was observed.
export function correlateRenderSubmitted(inputEvent, events, expectedFrames) {
  if (!inputEvent || !Array.isArray(events) || !Number.isSafeInteger(expectedFrames)) return null;
  const inputAt = inputEvent.at_us;
  if (!/^\d{1,20}$/.test(String(inputAt))) return null;
  return events.find(event => event?.stage === 'render_submitted' && event.frames === expectedFrames &&
    /^\d{1,20}$/.test(String(event.at_us)) && BigInt(event.at_us) >= BigInt(inputAt)) ?? null;
}
