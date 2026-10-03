import {test, expect} from '@playwright/test';

// The motivating sample for composition: while the main VM awaits the
// dialogue (the typewriter still owns the single waiting slot), a sequence
// chain fades the dialogue panel and then the text on its own. Activate/Await
// alone cannot express this — the chain has to be an autonomous task.
test('a sequence chain advances while the VM awaits the dialogue', async ({page}) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto('http://127.0.0.1:4222/?test=1', {waitUntil: 'domcontentloaded'});
  await page.waitForFunction(() => window.__nir?.state().ready && !window.__nir.state().loading);
  await page.keyboard.press('Enter');
  await page.waitForFunction(() => window.__nir.state().dialogue && !window.__nir.state().loading);
  // The first link runs while the reveal is still typing.
  await page.waitForFunction(() => {
    const s = window.__nir.state();
    return !s.dialogue.ready && s.dialogue_appearance.background_opacity < .9;
  });
  expect(await page.evaluate(() => window.__nir.state().dialogue_appearance.text_opacity))
    .toBeGreaterThan(.99);
  // The second link starts only after the first one finished: order holds.
  await page.waitForFunction(() => {
    const s = window.__nir.state();
    return !s.dialogue.ready
      && Math.abs(s.dialogue_appearance.background_opacity - .2) < .00001
      && s.dialogue_appearance.text_opacity < .9;
  });
  await page.waitForFunction(() =>
    Math.abs(window.__nir.state().dialogue_appearance.text_opacity - .35) < .00001);
  expect(await page.evaluate(() => window.__nir.state().error)).toBeFalsy();
  expect(errors).toEqual([]);
});
