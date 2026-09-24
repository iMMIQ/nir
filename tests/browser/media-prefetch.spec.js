import { test, expect } from '@playwright/test';
import { buildScaleFixture, closeScaleFixture } from '../performance/fixtures.js';

let enabled,disabled,unavailable;
const state=page=>page.evaluate(()=>window.__nir.state());
const action=(page,value)=>page.evaluate(value=>window.__nir.action(value),value);
const events=page=>page.evaluate(()=>window.__nir.diagnostics().events);

function mediaPaths(fixture,id='ch01') {
  return ['image','audio'].map(kind=>{
    const hash=fixture.program.assets[`media.${id}.${kind}`].object;
    return fixture.manifest.objects[hash].path;
  });
}

async function boot(page,fixture) {
  await page.goto(`${fixture.origin}/?test=1`);
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  expect((await state(page)).screen).toBe('Title');
}

async function chapter(page) {
  await page.keyboard.press('Space');
  await page.waitForFunction(()=>window.__nir.state().dialogue?.id==='driver.lead'&&!window.__nir.state().loading);
  await page.keyboard.press('Space');
  await page.waitForFunction(()=>window.__nir.state().dialogue?.id==='ch01.line'&&window.__nir.state().dialogue.ready&&!window.__nir.state().loading);
  const current=await state(page);
  expect(current.error).toBeNull();
  expect(current.variables.visit_count.value).toBe(1);
  expect(current.dialogue.ready).toBe(true);
}

async function media(page) {
  await page.keyboard.press('Space');
  await page.waitForFunction(()=>window.__nir.state().dialogue?.id==='ch01.next'&&window.__nir.state().dialogue.ready&&!window.__nir.state().loading);
  const current=await state(page);
  expect(current.error).toBeNull();
  expect(current.variables.visit_count.value).toBe(1);
}

async function clean(page) {
  await page.waitForFunction(()=>{
    const api=window.__nir,s=api.state(),d=api.diagnostics();
    return s.screen==='Title'&&!s.loading&&api.metrics.activeRequests===0&&
      !d.host_work.media_jobs&&!d.host_work.decode_pool_active&&!d.host_work.decode_pool_waiting&&
      !d.host_work.upload_pool_active&&!d.host_work.upload_pool_waiting&&
      !d.host_work.shared_fetches&&!d.host_work.pending_owner_callbacks&&d.content_staging.encoded_bytes===0;
  });
  const {s,m,d}=await page.evaluate(()=>({s:window.__nir.state(),m:window.__nir.metrics,d:window.__nir.diagnostics()}));
  expect(s.error).toBeNull();
  expect(m.acceptedRequests).toBe(m.completedRequests+m.cancelledRequests);
  expect(d.host_work.pending_media).toEqual([]);
}

async function holdAudioDecode(page) {
  await page.addInitScript(()=>{
    const original=BaseAudioContext.prototype.decodeAudioData;
    window.__holdMediaDecode=false;
    window.__heldMediaDecode=0;
    window.__pendingMediaDecodes=[];
    window.__releaseMediaDecode=()=>{
      window.__holdMediaDecode=false;
      for(const release of window.__pendingMediaDecodes.splice(0))release();
    };
    BaseAudioContext.prototype.decodeAudioData=function(...args){
      if(!window.__holdMediaDecode)return original.apply(this,args);
      window.__heldMediaDecode++;
      return new Promise((resolve,reject)=>{
        window.__pendingMediaDecodes.push(()=>Promise.resolve(original.apply(this,args)).then(resolve,reject));
      });
    };
  });
}

test.beforeAll(async()=>{
  test.setTimeout(300000);
  disabled=await buildScaleFixture({moduleCount:3,mediaScenario:true,prefetchMedia:false,prefetchContent:true,port:4201});
  enabled=await buildScaleFixture({moduleCount:3,mediaScenario:true,prefetchMedia:true,prefetchContent:true,port:4202});
  unavailable=await buildScaleFixture({moduleCount:3,mediaScenario:true,prefetchMedia:true,mediaCatalogResident:false,prefetchContent:true,port:4203});
});

test.afterAll(async()=>{
  if(disabled)await closeScaleFixture(disabled);
  if(enabled)await closeScaleFixture(enabled);
  if(unavailable)await closeScaleFixture(unavailable);
});

test('media lookahead sends real image and audio requests before demand without advancing the story',async({page})=>{
  for(const fixture of [disabled,enabled]){
    const requested=[];
    const observe=request=>requested.push(new URL(request.url()).pathname.slice(1));
    page.on('request',observe);
    try{
      await boot(page,fixture);
      await chapter(page);
      const before=await state(page);
      const paths=mediaPaths(fixture);
      if(fixture.prefetchMedia){
        await expect.poll(()=>paths.every(path=>requested.includes(path))).toBe(true);
        expect((await events(page)).some(e=>e.stage==='media_lookahead_requested')).toBe(true);
      }else{
        await page.waitForTimeout(250);
        expect(paths.filter(path=>requested.includes(path))).toEqual([]);
        expect((await events(page)).some(e=>e.stage==='media_lookahead_requested')).toBe(false);
      }
      const waiting=await state(page);
      expect(waiting.dialogue.id).toBe('ch01.line');
      expect(waiting.position).toBe(before.position);
      expect(waiting.history_count).toBe(before.history_count);
      await media(page);
      expect(paths.every(path=>requested.includes(path))).toBe(true);
      await action(page,{type:'title'});
      await clean(page);
    }finally{page.off('request',observe);}
  }
});

test('speculative media fetch failure is silent and a later demand retries',async({page})=>{
  const path=mediaPaths(enabled)[0];let attempts=0;
  await page.route(`**/${path}`,async route=>{
    attempts++;
    if(attempts===1)await route.fulfill({status:503,body:'temporary media failure'});
    else await route.continue();
  });
  await boot(page,enabled);
  await chapter(page);
  await page.waitForFunction(()=>window.__nir.diagnostics().events.some(e=>e.stage==='media_lookahead_cancelled'));
  const current=await state(page);
  expect(current.dialogue.id).toBe('ch01.line');
  expect(current.loading).toBe(false);
  expect(current.error).toBeNull();
  await media(page);
  expect(attempts).toBeGreaterThanOrEqual(2);
  await action(page,{type:'title'});
  await clean(page);
});

test('lookahead skips an unavailable catalog and demand still prepares media',async({page})=>{
  const requested=[];
  page.on('request',request=>requested.push(new URL(request.url()).pathname.slice(1)));
  await boot(page,unavailable);
  await chapter(page);
  await page.waitForTimeout(250);
  const paths=mediaPaths(unavailable);
  expect(paths.filter(path=>requested.includes(path))).toEqual([]);
  expect((await events(page)).some(e=>e.stage==='media_lookahead_requested')).toBe(false);
  expect((await state(page)).dialogue.id).toBe('ch01.line');
  await media(page);
  expect(paths.every(path=>requested.includes(path))).toBe(true);
  await action(page,{type:'title'});
  await clean(page);
});

test('a pending speculative fetch promoted by demand reports a required failure',async({page})=>{
  const path=mediaPaths(enabled)[0];let first,release,attempts=0;
  const started=new Promise(resolve=>{first=resolve;});
  const gate=new Promise(resolve=>{release=resolve;});
  await page.route(`**/${path}`,async route=>{
    attempts++;
    if(attempts===1){first();await gate;await route.fulfill({status:503,body:'promoted media failure'});}
    else await route.continue();
  });
  try{
    await boot(page,enabled);
    await chapter(page);
    await started;
    await page.keyboard.press('Space');
    await page.waitForFunction(()=>window.__nir.state().loading);
    release();
    await page.waitForFunction(()=>window.__nir.diagnostics().events.some(e=>e.stage==='prepare_failed'));
    const failed=await state(page);
    expect(failed.error).toBeTruthy();
    expect(failed.dialogue?.id).not.toBe('ch01.next');
    expect(attempts).toBe(1);
    await action(page,{type:'retry'});
    await page.waitForFunction(()=>window.__nir.state().dialogue?.id==='ch01.next'&&!window.__nir.state().loading);
    expect(attempts).toBe(2);
    await action(page,{type:'title'});
    await clean(page);
  }finally{release?.();}
});

test('title, locale, and device cancellation discard late decode results and release reservations',async({browser})=>{
  for(const change of ['title','locale','device']){
    const context=await browser.newContext();
    const page=await context.newPage();
    try{
      await holdAudioDecode(page);
      await boot(page,enabled);
      const titleBytes=(await state(page)).resident_bytes;
      await page.keyboard.press('Space');
      await page.waitForFunction(()=>window.__nir.state().dialogue?.id==='driver.lead'&&!window.__nir.state().loading);
      await page.evaluate(()=>window.__holdMediaDecode=true);
      await page.keyboard.press('Space');
      await page.waitForFunction(()=>window.__nir.state().dialogue?.id==='ch01.line'&&window.__nir.state().dialogue.ready);
      await page.waitForFunction(()=>window.__heldMediaDecode>0);
      const before=await state(page);
      if(change==='title')await action(page,{type:'title'});
      if(change==='locale')await action(page,{type:'text_locale',locale:'en'});
      if(change==='device')await page.evaluate(()=>window.__nir.loseDevice());
      await page.waitForFunction(()=>window.__nir.diagnostics().events.some(e=>e.stage==='media_lookahead_cancelled'));
      if(change==='title'){
        await page.waitForFunction(()=>window.__nir.state().screen==='Title');
        expect((await state(page)).resident_bytes).toBeGreaterThan(titleBytes);
      }else if(change==='locale'){
        await page.waitForFunction(()=>window.__nir.state().text_locale==='en'&&!window.__nir.state().locale_pending);
      }else{
        await page.waitForFunction(device=>window.__nir.state().device>device&&window.__nir.state().ready,before.device);
      }
      const heldBytes=(await state(page)).resident_bytes;
      await page.evaluate(()=>window.__releaseMediaDecode());
      if(change!=='title'){
        await page.waitForFunction(()=>window.__nir.state().dialogue?.id==='ch01.line'&&!window.__nir.state().loading);
        expect((await state(page)).error).toBeNull();
        await action(page,{type:'title'});
      }
      await clean(page);
      const releasedBytes=(await state(page)).resident_bytes;
      expect(releasedBytes).toBeLessThan(heldBytes);
      if(change==='title')expect(releasedBytes).toBe(titleBytes);
    }finally{
      await page.evaluate(()=>window.__releaseMediaDecode?.()).catch(()=>{});
      await context.close();
    }
  }
});
