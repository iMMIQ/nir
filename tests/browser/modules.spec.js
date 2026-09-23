import { test, expect } from '@playwright/test';
import fs from 'node:fs/promises';
import path from 'node:path';
import { promisify } from 'node:util';
import { execFile } from 'node:child_process';
import {
  buildModulesFixture,
  chapters,
  closeModulesFixture,
  moduleObjectHashes,
  readRelease,
  requestedModuleObjects,
} from './modules.fixture.js';

const run = promisify(execFile);
const state = page => page.evaluate(() => window.__nir.state());
const action = (page, value) => page.evaluate(v => window.__nir.action(v), value);
let fixture;

async function boot(page) {
  await page.goto(`${fixture.origin}/?test=1`);
  await page.waitForFunction(() => window.__nir?.state().ready && !window.__nir.state().loading);
  expect((await state(page)).screen).toBe('Title');
  expect(await requestedModuleObjects(page)).toEqual([]);
}

async function start(page) {
  await page.keyboard.press('Space');
  await page.waitForFunction(() => {
    const s = window.__nir.state();
    return s.dialogue?.ready && !s.loading;
  });
}

async function advance(page, expectedText) {
  await page.keyboard.press('Space');
  await page.waitForFunction(text => {
    const s = window.__nir.state();
    return s.dialogue?.visible === text && s.dialogue.ready && !s.loading;
  }, expectedText);
  return state(page);
}

const expectedText = (chapter, locale) => locale === 'en'
  ? `${chapter.english}: the shared road continues.`
  : `${chapter.number} 雨`;
const requiredHashes = hashes => Object.values(hashes).flatMap(value => [value.code, ...Object.values(value.locales)]);

test.beforeAll(async () => {
  fixture = await buildModulesFixture();
});

test.afterAll(async () => {
  if (fixture) await closeModulesFixture(fixture);
});

test('startup and module calls fetch only the selected chapter and language; chapter two saves restore without re-entry', async ({ page }) => {
  const hashes = moduleObjectHashes(fixture.program);
  const sharedImage = fixture.program.assets['bg.station'].object;
  let imageRequests = 0;
  page.on('request', request => { if (request.url().includes(sharedImage)) imageRequests++; });
  await boot(page);
  await start(page);
  expect((await state(page)).dialogue.visible).toBe(expectedText(chapters[0], 'zh-Hans'));
  expect((await state(page)).variables.visit_count.value).toBe(1);
  const chapterOneImageRequests = imageRequests;
  expect(chapterOneImageRequests).toBeGreaterThan(0);

  let requested = new Set(await requestedModuleObjects(page));
  expect([...requested].sort()).toEqual([
    hashes.ch01.code,
    hashes.ch01.locales['zh-Hans'],
  ].sort());
  expect(requiredHashes(hashes).filter(hash => !requested.has(hash))).toHaveLength(7);

  await action(page, { type: 'text_locale', locale: 'en' });
  await page.waitForFunction(() => window.__nir.state().text_locale === 'en' && !window.__nir.state().locale_pending);
  expect((await state(page)).dialogue.locale).toBe('zh-Hans');
  expect((await state(page)).dialogue.visible).toBe(expectedText(chapters[0], 'zh-Hans'));
  requested = new Set(await requestedModuleObjects(page));
  expect(requested.has(hashes.ch01.locales.en)).toBe(true);
  expect(requested.has(hashes.ch02.locales.en)).toBe(false);

  await advance(page, expectedText(chapters[1], 'en'));
  expect((await state(page)).variables.visit_count.value).toBe(2);
  requested = new Set(await requestedModuleObjects(page));
  expect(requested.has(hashes.ch02.code)).toBe(true);
  expect(requested.has(hashes.ch02.locales.en)).toBe(true);
  expect(requested.has(hashes.ch03.code)).toBe(false);
  expect(requested.has(hashes.ch03.locales.en)).toBe(false);
  expect(imageRequests).toBe(chapterOneImageRequests);

  await action(page, { type: 'saves' });
  await action(page, { type: 'save', slot: 0 });
  await page.waitForFunction(() => /已保存|Saved/.test(window.__nir.state().status));
  await page.reload();
  await page.waitForFunction(() => window.__nir?.state().ready && !window.__nir.state().loading);
  expect(await requestedModuleObjects(page)).toEqual([]);
  await action(page, { type: 'saves' });
  await action(page, { type: 'load', slot: 0 });
  await page.waitForFunction(() => {
    const s = window.__nir.state();
    return s.screen === 'Story' && s.paused && !s.loading && s.dialogue?.ready;
  });
  expect((await state(page)).dialogue.visible).toBe(expectedText(chapters[1], 'en'));
  expect((await state(page)).variables.visit_count.value).toBe(2);
  requested = new Set(await requestedModuleObjects(page));
  expect(requested.has(hashes.ch01.code)).toBe(true);
  expect(requested.has(hashes.ch02.code)).toBe(true);
  expect(requested.has(hashes.ch02.locales.en)).toBe(true);
  expect(requested.has(hashes.ch03.code)).toBe(false);
  expect(requested.has(hashes.ch03.locales.en)).toBe(false);
  const restoredImageRequests = imageRequests;

  await action(page, { type: 'continue' });
  await advance(page, expectedText(chapters[2], 'en'));
  expect((await state(page)).variables.visit_count.value).toBe(3);
  requested = new Set(await requestedModuleObjects(page));
  expect(requested.has(hashes.ch03.code)).toBe(true);
  expect(requested.has(hashes.ch03.locales.en)).toBe(true);
  expect(imageRequests).toBe(restoredImageRequests);
});

test('module fetch failure keeps the active scene and retries; cancelled work cannot replace a newer title session', async ({ page }) => {
  const hashes = moduleObjectHashes(fixture.program);
  const ch02Code = hashes.ch02.code;
  let ch02Attempts = 0;
  let rejectFirstAttempt;
  const firstAttemptGate = new Promise(resolve => { rejectFirstAttempt = resolve; });
  let firstAttemptSeen;
  const firstAttemptStarted = new Promise(resolve => { firstAttemptSeen = resolve; });
  await page.route(`**/objects/${ch02Code}.json`, async route => {
    ch02Attempts++;
    if (ch02Attempts === 1) {
      firstAttemptSeen();
      await firstAttemptGate;
      await route.fulfill({ status: 503, body: 'temporary module failure' });
    }
    else await route.continue();
  });

  await boot(page);
  await start(page);
  await page.keyboard.press('Space');
  await firstAttemptStarted;
  await page.waitForFunction(hash => window.__nir.diagnostics().events.some(event =>
    event.stage === 'module_requested' && event.object === hash), ch02Code);
  const blocked = await state(page);
  rejectFirstAttempt();
  await page.waitForFunction(() => window.__nir.diagnostics().events.some(event => event.stage === 'module_failed'));
  const failed = await state(page);
  expect(failed.screen).toBe('Story');
  expect(failed.session).toBe(blocked.session);
  expect(failed.variables.visit_count.value).toBe(1);
  expect(failed.position).toEqual(blocked.position);

  await action(page, { type: 'retry' });
  await page.waitForFunction(text => {
    const s = window.__nir.state();
    return s.dialogue?.visible === text && s.dialogue.ready && !s.loading;
  }, expectedText(chapters[1], 'zh-Hans'));
  expect((await state(page)).variables.visit_count.value).toBe(2);

  const ch03Code = hashes.ch03.code;
  let routeSeen;
  const routeStarted = new Promise(resolve => { routeSeen = resolve; });
  let releaseRoute;
  const waitForRoute = new Promise(resolve => { releaseRoute = resolve; });
  await page.route(`**/objects/${ch03Code}.json`, async route => {
    routeSeen();
    await waitForRoute;
    try { await route.continue(); } catch {}
  });
  const oldSession = (await state(page)).session;
  await page.keyboard.press('Space');
  await routeStarted;
  await action(page, { type: 'title' });
  await page.waitForFunction(session => {
    const s = window.__nir.state();
    return s.screen === 'Title' && s.session > session && !s.loading;
  }, oldSession);
  const titleSession = (await state(page)).session;
  releaseRoute();
  await page.waitForFunction(() => window.__nir.metrics.activeRequests === 0);
  await page.waitForTimeout(300);
  const afterLateFetch = await state(page);
  expect(afterLateFetch.screen).toBe('Title');
  expect(afterLateFetch.session).toBe(titleSession);
  expect(afterLateFetch.variables.visit_count.value).toBe(2);
});

test('editing chapter two English changes only its localized object', async () => {
  const before = fixture.program;
  const beforeHashes = moduleObjectHashes(before);
  const english = path.join(fixture.project, 'content/ch02/texts/en.json');
  const doc = JSON.parse(await fs.readFile(english, 'utf8'));
  doc.line.spans[0].text = 'Chapter two: the road shared continues.';
  await fs.writeFile(english, `${JSON.stringify(doc, null, 2)}\n`);
  await run(fixture.cli, ['-p', fixture.project, 'text', 'review', '--id', 'ch02.line', '--locale', 'en']);
  const updatedWeb = path.join(fixture.temp, 'web-ch02-english');
  await run(fixture.cli, ['-p', fixture.project, 'build', '--locked', '--out', updatedWeb]);
  const after = (await readRelease(updatedWeb)).program;
  const afterHashes = moduleObjectHashes(after);

  for (const chapter of ['ch01', 'ch02', 'ch03']) {
    expect(afterHashes[chapter].code, `${chapter} code`).toBe(beforeHashes[chapter].code);
    for (const locale of ['zh-Hans', 'en']) {
      if (chapter === 'ch02' && locale === 'en') continue;
      expect(afterHashes[chapter].locales[locale], `${chapter}/${locale}`).toBe(beforeHashes[chapter].locales[locale]);
    }
  }
  expect(afterHashes.ch02.locales.en).not.toBe(beforeHashes.ch02.locales.en);
  expect(Object.fromEntries(Object.entries(after.assets).map(([id, asset]) => [id, asset.object])))
    .toEqual(Object.fromEntries(Object.entries(before.assets).map(([id, asset]) => [id, asset.object])));
});
