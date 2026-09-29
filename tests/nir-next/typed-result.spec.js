import {test, expect} from '@playwright/test';

// story.typed-result.v1: the VM owns the typed write behind an interaction,
// the semantic selection cursor lives in the snapshot while hover and focus
// stay presentation transients, and a declared cancel path branches without
// writing anything.
const url = 'http://127.0.0.1:4223/?test=1';
const state = () => window.__nir.state();
const picked = () => window.__nir.state().variables.picked.value;
const stayRow = page => page.locator('#actions button', {hasText: 'Stay at the station'});
const walkRow = page => page.locator('#actions button', {hasText: 'Walk home together'});

async function start(page) {
  await page.goto(url, {waitUntil: 'domcontentloaded'});
  await page.waitForFunction(() => window.__nir?.state().ready && !window.__nir.state().loading);
  await page.keyboard.press('Enter');
  await page.waitForFunction(() => window.__nir.state().choice && !window.__nir.state().loading);
}

test('the chosen option writes its declared value and the value drives the dialogue', async ({page}) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await start(page);
  const choice = await page.evaluate(() => window.__nir.state().choice);
  expect(choice.result).toBe('picked');
  expect(choice.on_cancel).toBe('typed_cancel');
  expect(choice.values).toEqual({
    walk: {type: 'i32', value: 1},
    stay: {type: 'i32', value: 2},
  });
  // The cursor starts at the first enabled row; nothing is written yet.
  expect(choice.selected).toBe('walk');
  expect(await page.evaluate(picked)).toBe(0);
  await stayRow(page).focus();
  await page.keyboard.press('Enter');
  await page.waitForFunction(() => window.__nir.state().dialogue?.id === 'stay_line');
  expect(await page.evaluate(picked)).toBe(2);

  // The other row writes the other value and reaches the other dialogue.
  await start(page);
  await walkRow(page).focus();
  await page.keyboard.press('Enter');
  await page.waitForFunction(() => window.__nir.state().dialogue?.id === 'walk_line');
  expect(await page.evaluate(picked)).toBe(1);
  expect(await page.evaluate(() => window.__nir.state().error)).toBeFalsy();
  expect(errors).toEqual([]);
});

test('keyboard focus moves the semantic cursor without progressing the story', async ({page}) => {
  await start(page);
  const before = await page.evaluate(() => ({
    input: window.__nir.state().sequence,
    id: window.__nir.state().choice.interaction,
  }));
  // Focusing the DOM control rides the host focus path into the engine,
  // which mirrors it as a cursor observation on the suspended interaction.
  await stayRow(page).focus();
  await page.waitForFunction(() => window.__nir.state().choice?.selected === 'stay');
  const after = await page.evaluate(() => ({
    input: window.__nir.state().sequence,
    id: window.__nir.state().choice.interaction,
  }));
  expect(after.input).toBe(before.input);
  expect(after.id).toBe(before.id);
  expect(await page.evaluate(picked)).toBe(0);
});

test('Escape cancels through the declared target without a typed write', async ({page}) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await start(page);
  await page.keyboard.press('Escape');
  await page.waitForFunction(() => !window.__nir.state().choice && window.__nir.state().dialogue?.id === 'arrival');
  expect(await page.evaluate(picked)).toBe(0);
  expect(await page.evaluate(() => window.__nir.state().error)).toBeFalsy();
  expect(errors).toEqual([]);
});

test('saving and loading mid-interaction restores the pending selection', async ({page}) => {
  await start(page);
  await stayRow(page).focus();
  await page.waitForFunction(() => window.__nir.state().choice?.selected === 'stay');
  await page.evaluate(() => window.__nir.action({type: 'save', slot: 0}));
  await page.waitForFunction(() => /Saved|已保存/.test(window.__nir.state().status));
  const session = await page.evaluate(() => window.__nir.state().session);
  await page.evaluate(() => window.__nir.action({type: 'load', slot: 0}));
  await page.waitForFunction(s => {
    const state = window.__nir.state();
    return state.session > s && !state.loading && state.paused;
  }, session);
  // The interaction comes back pending with its cursor, on a fresh identity.
  const restored = await page.evaluate(state);
  expect(restored.choice.result).toBe('picked');
  expect(restored.choice.selected).toBe('stay');
  await page.evaluate(() => window.__nir.action({type: 'continue'}));
  await page.waitForFunction(() => {
    const state = window.__nir.state();
    return !state.paused && state.choice;
  });
  await stayRow(page).focus();
  await page.keyboard.press('Enter');
  await page.waitForFunction(() => window.__nir.state().dialogue?.id === 'stay_line');
  expect(await page.evaluate(picked)).toBe(2);
});
