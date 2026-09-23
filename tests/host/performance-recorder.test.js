import { test } from 'node:test';
import assert from 'node:assert/strict';
import { PerformanceRecorder } from '../../crates/nir-platform-web/host.js';

test('performance recorder keeps raw nested intervals in a bounded 64 turn ring', () => {
  const recorder = new PerformanceRecorder();
  for (let index = 0; index < 65; index++) {
    const start = index * 100;
    const turn = recorder.beginTurn(start);
    recorder.record('event_handling', start + 1, start + 21, turn);
    recorder.record('projection', start + 5, start + 12, turn);
    recorder.endTurn(turn, start + 25);
  }

  const snapshot = recorder.snapshot();
  assert.equal(snapshot.enabled, true);
  assert.equal(snapshot.turn_capacity, 64);
  assert.equal(snapshot.total_turns, 65);
  assert.equal(snapshot.dropped_turns, 1);
  assert.equal(snapshot.stage_semantics, 'inclusive_non_additive');
  assert.equal(snapshot.turns.length, 64);
  assert.equal(snapshot.turns[0].id, 2);
  assert.equal(snapshot.turns.at(-1).id, 65);

  const first = snapshot.turns[0];
  assert.equal(first.total_us, 25);
  assert.deepEqual(first.stages, [
    { stage: 'event_handling', start_us: 101, end_us: 121, duration_us: 20 },
    { stage: 'projection', start_us: 105, end_us: 112, duration_us: 7 },
  ]);
  // The independently accumulated stage durations overlap and are not used as
  // an additive owner-turn total.
  assert.equal(snapshot.stages.event_handling.total_us, 65 * 20);
  assert.equal(snapshot.stages.projection.total_us, 65 * 7);
  assert.ok(snapshot.stages.event_handling.total_us + snapshot.stages.projection.total_us > 65 * 25);

  snapshot.turns[0].stages[0].start_us = -1;
  snapshot.stages.projection.total_us = 0;
  const fresh = recorder.snapshot();
  assert.equal(fresh.turns[0].stages[0].start_us, 101);
  assert.equal(fresh.stages.projection.total_us, 65 * 7);
});

test('performance recorder drops malformed timing intervals without affecting totals', () => {
  const recorder = new PerformanceRecorder(4);
  const turn = recorder.beginTurn(100);
  recorder.record('invalid_nan', Number.NaN, 110, turn);
  recorder.record('invalid_order', 120, 119, turn);
  recorder.record('invalid_infinite', 100, Number.POSITIVE_INFINITY, turn);
  recorder.record('valid_zero', 105, 105, turn);
  recorder.endTurn(turn, 125);

  const snapshot = recorder.snapshot();
  assert.equal(snapshot.turns.length, 1);
  assert.deepEqual(snapshot.turns[0].stages, [
    { stage: 'valid_zero', start_us: 105, end_us: 105, duration_us: 0 },
  ]);
  assert.deepEqual(snapshot.stages, {
    valid_zero: { count: 1, total_us: 0, min_us: 0, max_us: 0 },
  });
});

test('performance recorder never grows its turn ring above 64 slots', () => {
  const recorder = new PerformanceRecorder(10_000);
  assert.equal(recorder.snapshot().turn_capacity, 64);
  for (let index = 0; index < 80; index++) {
    const turn = recorder.beginTurn(index);
    recorder.endTurn(turn, index + 1);
  }
  const snapshot = recorder.snapshot();
  assert.equal(snapshot.total_turns, 80);
  assert.equal(snapshot.dropped_turns, 16);
  assert.equal(snapshot.turns.length, 64);
  assert.equal(snapshot.turns[0].id, 17);
  assert.equal(snapshot.turns.at(-1).id, 80);
});
