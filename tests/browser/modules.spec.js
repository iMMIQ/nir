import { test, expect } from '@playwright/test';
import fs from 'node:fs/promises';
import path from 'node:path';
import { promisify } from 'node:util';
import { execFile } from 'node:child_process';
import {
  buildModulesFixture,
  chapters,
  closeModulesFixture,
  catalogHashesForAssets,
  catalogObjectHashes,
  mediaHashesForAssets,
  moduleContentHashes,
  moduleObjectHashes,
  readRelease,
  requestedNetworkObjects,
  requestedModuleObjects,
  resetNetworkObjects,
  runtimeAssetsForLocales,
  trackObjectRequests,
} from './modules.fixture.js';

const run = promisify(execFile);
const state = page => page.evaluate(() => window.__nir.state());
const action = (page, value) => page.evaluate(v => window.__nir.action(v), value);
let fixture;

async function boot(page) {
  trackObjectRequests(page);
  await page.goto(`${fixture.origin}/?test=1`);
  await page.waitForFunction(() => window.__nir?.state().ready && !window.__nir.state().loading);
  const current=await state(page);
  expect(current.screen).toBe('Title');
  const stages=(await page.evaluate(()=>window.__nir.diagnostics().events)).map(event=>event.stage);
  expect(stages.indexOf('preferences_loaded')).toBeGreaterThanOrEqual(0);
  expect(stages.indexOf('preferences_loaded')).toBeLessThan(stages.indexOf('engine_created'));
  await assertRuntimeContent(page,new Set());
  await assertAssetClosure(page,runtimeAssetsForLocales(fixture.program,current.ui_locale,current.text_locale));
}

async function assertRuntimeContent(page,expected) {
  const moduleHashes=moduleContentHashes(fixture.program);
  const loaded=new Set(await requestedModuleObjects(page)),network=requestedNetworkObjects(page);
  const expectedHashes=[...expected].sort();
  expect([...loaded].filter(hash=>moduleHashes.has(hash)).sort()).toEqual(expectedHashes);
  expect([...network].filter(hash=>moduleHashes.has(hash)).sort()).toEqual(expectedHashes);
}

async function assertAssetClosure(page,assetIds) {
  const catalogHashes=new Set(Object.values(catalogObjectHashes(fixture.program)));
  const mediaHashes=new Set(Object.values(fixture.program.assets).map(asset=>asset.object));
  const network=requestedNetworkObjects(page);
  const content=new Set(await requestedModuleObjects(page));
  const expectedCatalogs=[...catalogHashesForAssets(fixture.program,assetIds)].sort();
  expect([...content].filter(hash=>catalogHashes.has(hash)).sort()).toEqual(expectedCatalogs);
  expect([...network].filter(hash=>catalogHashes.has(hash)).sort()).toEqual(expectedCatalogs);
  expect([...network].filter(hash=>mediaHashes.has(hash)).sort())
    .toEqual([...mediaHashesForAssets(fixture.program,assetIds)].sort());
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

async function seedPreferences(page,preferences) {
  const namespace=`${fixture.manifest.game_id}:dev`;
  await page.goto(`${fixture.origin}/channels/stable.json`);
  await page.evaluate(({namespace,value})=>new Promise((resolve,reject)=>{
    const request=indexedDB.open('nir-player-v1',1);
    request.onupgradeneeded=()=>{for(const store of ['saves','preferences','profile'])if(!request.result.objectStoreNames.contains(store))request.result.createObjectStore(store);};
    request.onerror=()=>reject(request.error);
    request.onsuccess=()=>{
      const db=request.result,tx=db.transaction('preferences','readwrite');
      tx.objectStore('preferences').put(value,namespace);
      tx.oncomplete=()=>{db.close();resolve();};
      tx.onabort=tx.onerror=()=>{db.close();reject(tx.error);};
    };
  }),{namespace,value:preferences});
}

async function readSavedEnvelope(page,namespace,slot) {
  return page.evaluate(({key})=>new Promise((resolve,reject)=>{
    const request=indexedDB.open('nir-player-v1',1);
    request.onerror=()=>reject(request.error);
    request.onsuccess=()=>{
      const db=request.result,tx=db.transaction('saves','readonly'),get=tx.objectStore('saves').get(key);
      let value;
      get.onsuccess=()=>{value=get.result;};
      tx.oncomplete=()=>{db.close();resolve(value);};
      tx.onabort=tx.onerror=()=>{db.close();reject(tx.error||get.error);};
    };
  }),{key:`${namespace}:${slot}`});
}

const expectedText = (chapter, locale) => locale === 'en'
  ? `${chapter.english}: the shared road continues.`
  : `${chapter.number} 雨`;
const requiredHashes = hashes => Object.values(hashes).flatMap(value => [value.code, value.static, ...Object.values(value.locales)].filter(Boolean));

test.beforeAll(async () => {
  fixture = await buildModulesFixture();
});

test.afterAll(async () => {
  if (fixture) await closeModulesFixture(fixture);
});

test('startup and module calls fetch only the selected chapter and language; chapter two saves restore without re-entry', async ({ page }) => {
  const hashes = moduleObjectHashes(fixture.program);
  expect(fixture.executable.format).toBe(2);
  for(const chapter of chapters)expect(hashes[chapter.id].static).toMatch(/^[0-9a-f]{64}$/);
  const sharedImage = fixture.program.assets['bg.station'].object;
  let imageRequests = 0;
  page.on('request', request => { if (request.url().includes(sharedImage)) imageRequests++; });
  await boot(page);
  await start(page);
  expect((await state(page)).dialogue.visible).toBe(expectedText(chapters[0], 'zh-Hans'));
  expect((await state(page)).variables.visit_count.value).toBe(1);
  expect(fixture.program.assets['bg.station'].bytes).toBeUndefined();
  expect(Object.keys(fixture.program.catalogs||{}).length).toBeGreaterThan(0);
  const chapterOneImageRequests = imageRequests;
  expect(chapterOneImageRequests).toBeGreaterThan(0);

  let requested = new Set(await requestedModuleObjects(page));
  const chapterOneZh=new Set([
    hashes.ch01.code,
    hashes.ch01.static,
    hashes.ch01.locales['zh-Hans'],
  ]);
  await assertRuntimeContent(page,chapterOneZh);
  expect(requiredHashes(hashes).filter(hash => !requested.has(hash))).toHaveLength(requiredHashes(hashes).length-3);

  await action(page, { type: 'text_locale', locale: 'en' });
  await page.waitForFunction(() => window.__nir.state().text_locale === 'en' && !window.__nir.state().locale_pending);
  expect((await state(page)).dialogue.locale).toBe('zh-Hans');
  expect((await state(page)).dialogue.visible).toBe(expectedText(chapters[0], 'zh-Hans'));
  requested = new Set(await requestedModuleObjects(page));
  const chapterOneBothLocales=new Set([...chapterOneZh,hashes.ch01.locales.en]);
  await assertRuntimeContent(page,chapterOneBothLocales);
  expect(requested.has(hashes.ch01.locales.en)).toBe(true);
  expect(requested.has(hashes.ch02.locales.en)).toBe(false);
  await assertAssetClosure(page,runtimeAssetsForLocales(fixture.program,'zh-Hans','en'));

  await advance(page, expectedText(chapters[1], 'en'));
  expect((await state(page)).variables.visit_count.value).toBe(2);
  requested = new Set(await requestedModuleObjects(page));
  const throughChapterTwo=new Set([...chapterOneBothLocales,hashes.ch02.code,hashes.ch02.static,hashes.ch02.locales.en]);
  await assertRuntimeContent(page,throughChapterTwo);
  expect(requested.has(hashes.ch02.code)).toBe(true);
  expect(requested.has(hashes.ch02.static)).toBe(true);
  expect(requested.has(hashes.ch02.locales.en)).toBe(true);
  expect(requested.has(hashes.ch03.code)).toBe(false);
  expect(requested.has(hashes.ch03.locales.en)).toBe(false);
  expect(imageRequests).toBe(chapterOneImageRequests);

  await action(page, { type: 'saves' });
  await action(page, { type: 'save', slot: 0 });
  await page.waitForFunction(() => /已保存|Saved/.test(window.__nir.state().status));
  const saved=await readSavedEnvelope(page,`${fixture.manifest.game_id}:dev`,0);
  const frozenDialogues=Object.values(saved.snapshot.tasks).map(task=>task.dialogue).filter(Boolean);
  expect(frozenDialogues).toEqual(expect.arrayContaining([
    expect.objectContaining({text_id:'ch01.line',locale:'zh-Hans'}),
    expect.objectContaining({text_id:'ch02.line',locale:'en'}),
  ]));
  resetNetworkObjects(page);
  await page.reload();
  await page.waitForFunction(() => window.__nir?.state().ready && !window.__nir.state().loading);
  await assertRuntimeContent(page,new Set());
  const reloaded=await state(page);
  await assertAssetClosure(page,runtimeAssetsForLocales(fixture.program,reloaded.ui_locale,reloaded.text_locale));
  await action(page, { type: 'saves' });
  await action(page, { type: 'load', slot: 0 });
  await page.waitForFunction(() => {
    const s = window.__nir.state();
    return s.screen === 'Story' && s.paused && !s.loading && s.dialogue?.ready;
  });
  expect((await state(page)).dialogue.visible).toBe(expectedText(chapters[1], 'en'));
  expect((await state(page)).variables.visit_count.value).toBe(2);
  requested = new Set(await requestedModuleObjects(page));
  await assertRuntimeContent(page,new Set([
    hashes.ch01.code,hashes.ch01.static,hashes.ch01.locales['zh-Hans'],
    hashes.ch02.code,hashes.ch02.static,hashes.ch02.locales.en,
  ]));
  expect(requested.has(hashes.ch01.code)).toBe(true);
  expect(requested.has(hashes.ch02.code)).toBe(true);
  expect(requested.has(hashes.ch02.static)).toBe(true);
  expect(requested.has(hashes.ch02.locales.en)).toBe(true);
  expect(requested.has(hashes.ch01.locales.en)).toBe(false);
  expect(requested.has(hashes.ch03.code)).toBe(false);
  expect(requested.has(hashes.ch03.static)).toBe(false);
  expect(requested.has(hashes.ch03.locales['zh-Hans'])).toBe(false);
  expect(requested.has(hashes.ch03.locales.en)).toBe(false);
  const restoredImageRequests = imageRequests;

  await action(page, { type: 'continue' });
  await advance(page, expectedText(chapters[2], 'en'));
  expect((await state(page)).variables.visit_count.value).toBe(3);
  requested = new Set(await requestedModuleObjects(page));
  await assertRuntimeContent(page,new Set([
    hashes.ch01.code,hashes.ch01.static,hashes.ch01.locales['zh-Hans'],
    hashes.ch02.code,hashes.ch02.static,hashes.ch02.locales.en,
    hashes.ch03.code,hashes.ch03.static,hashes.ch03.locales.en,
  ]));
  expect(requested.has(hashes.ch03.code)).toBe(true);
  expect(requested.has(hashes.ch03.static)).toBe(true);
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

test('saved locale is applied before the first Boot fetch and selects only English chapter text', async ({ page }) => {
  const hashes=moduleObjectHashes(fixture.program);
  await seedPreferences(page,{
    ui_locale:'en',text_locale:'en',font_scale:1,bgm_volume:.3,voice_volume:.8,sfx_volume:.5,reduced_motion:false,
  });
  await boot(page);
  expect((await state(page)).ui_locale).toBe('en');
  expect((await state(page)).text_locale).toBe('en');
  await start(page);
  expect((await state(page)).dialogue.locale).toBe('en');
  expect((await state(page)).dialogue.visible).toBe(expectedText(chapters[0],'en'));
  await assertRuntimeContent(page,new Set([hashes.ch01.code,hashes.ch01.static,hashes.ch01.locales.en]));
  expect(new Set(await requestedModuleObjects(page)).has(hashes.ch01.locales['zh-Hans'])).toBe(false);
  await assertAssetClosure(page,runtimeAssetsForLocales(fixture.program,'en','en'));
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
    expect(afterHashes[chapter].static, `${chapter} static`).toBe(beforeHashes[chapter].static);
    for (const locale of ['zh-Hans', 'en']) {
      if (chapter === 'ch02' && locale === 'en') continue;
      expect(afterHashes[chapter].locales[locale], `${chapter}/${locale}`).toBe(beforeHashes[chapter].locales[locale]);
    }
  }
  expect(afterHashes.ch02.locales.en).not.toBe(beforeHashes.ch02.locales.en);
  expect(catalogObjectHashes(after)).toEqual(catalogObjectHashes(before));
  expect(Object.fromEntries(Object.entries(after.assets).map(([id, asset]) => [id, asset.object])))
    .toEqual(Object.fromEntries(Object.entries(before.assets).map(([id, asset]) => [id, asset.object])));
});
