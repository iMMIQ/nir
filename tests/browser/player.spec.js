import { test, expect, chromium } from '@playwright/test';
import fs from 'node:fs/promises';
import { expectPainted } from './pixels.js';
import { spawn } from 'node:child_process';

const state = page => page.evaluate(() => window.__nir.state());
const act = (page, action) => page.evaluate(a => window.__nir.action(a), action);
async function boot(page) {
  await page.goto('/?test=1');
  await page.waitForFunction(() => window.__nir?.state().ready && !window.__nir.state().loading);
  await expect(page.locator('#shell')).toBeHidden();
}
async function start(page) {
  await page.keyboard.press('Space');
  await page.waitForFunction(() => window.__nir.state().dialogue && !window.__nir.state().loading);
}
async function advanceUntil(page, predicate, route) {
  for (let n = 0; n < 150; n++) {
    const s = await state(page);
    expect(s.error).toBeNull();
    if (predicate(s)) return s;
    if (s.choice && route) await act(page, { type: 'choose', option: route });
    else if (s.dialogue && !s.dialogue.gate) await page.keyboard.press('Space');
    await page.waitForTimeout(120);
  }
  throw Error(`Story stalled: ${JSON.stringify(await state(page))}`);
}
test.beforeEach(async ({ page }) => {
  page.on('pageerror', e => { throw e; });
});

test('WebGPU canvas, Chinese layout, static sleep and both endings', async ({ page }) => {
  await boot(page);
  const adapter = await page.evaluate(async () => {
    const a = await navigator.gpu.requestAdapter();
    return { vendor:a.info.vendor, architecture:a.info.architecture, device:a.info.device, description:a.info.description, fallback:a.info.isFallbackAdapter };
  });
  const first = await state(page);
  await page.waitForTimeout(600);
  expect((await state(page)).frames).toBe(first.frames);
  expectPainted(await page.screenshot({ path: 'reports/title.png' }));
  await start(page);
  await page.keyboard.press('Space');
  expectPainted(await page.screenshot({ path: 'reports/dialogue.png' }));
  expect((await state(page)).dialogue.visible).toContain('末班电车');
  for (const [route, outcome, affection] of [['walk','walk_home',1],['stay','read_letter',0]]) {
    const choice = await advanceUntil(page, s => !!s.choice);
    await page.screenshot({ path: `reports/choice-${route}.png` });
    await page.evaluate(s => window.__nir.rawAction({type:'choose',option:'walk'},s.interaction-1,s.sequence+10,s.session),choice);
    expect((await state(page)).choice).not.toBeNull();
    await act(page,{type:'choose',option:route});
    const end = await advanceUntil(page, s => !!s.outcome, route);
    expect(end.outcome).toBe(outcome);
    expect(end.variables.affection.value).toBe(affection);
    await page.screenshot({ path: `reports/ending-${route}.png` });
    if (route === 'walk') { await act(page,{type:'new_game'}); }
  }
  const results = await page.evaluate(() => ({metrics:window.__nir.metrics,state:window.__nir.state(),traces:window.__nir.traces}));
  expect(results.metrics.audioStarts).toBeGreaterThan(2);
  expect(results.metrics.deviceRecoveries).toBe(0);
  await fs.writeFile('reports/browser-metrics.json', JSON.stringify({adapter,...results},null,2));
});

test('save commit, refresh restore, locale boundary, history and rollback', async ({ page }) => {
  await boot(page); await start(page); await page.keyboard.press('Space');
  await act(page,{type:'saves'});
  const saved = await state(page);
  await act(page,{type:'save',slot:0});
  await page.waitForFunction(() => /已保存|Saved/.test(window.__nir.state().status));
  await page.reload();
  await page.waitForFunction(() => window.__nir?.state().ready);
  await act(page,{type:'saves'}); await act(page,{type:'load',slot:0});
  await page.waitForFunction(() => window.__nir.state().screen==='Story' && window.__nir.state().paused && !window.__nir.state().loading);
  expect((await state(page)).dialogue).toEqual(saved.dialogue);
  expect((await state(page)).tick_us).toBe(saved.tick_us);
  await act(page,{type:'settings'}); await act(page,{type:'locale',locale:'en'});
  expect((await state(page)).dialogue.locale).toBe('zh-Hans');
  await act(page,{type:'font_size',delta:.2});
  await page.screenshot({path:'reports/settings.png'});
  await act(page,{type:'close'}); await act(page,{type:'continue'});
  const next = await advanceUntil(page,s=>s.dialogue?.id!==saved.dialogue.id && !!s.dialogue);
  expect(next.dialogue.locale).toBe('en');
  await act(page,{type:'history'});
  await page.screenshot({path:'reports/history.png'});
  await act(page,{type:'close'}); await act(page,{type:'rollback'});
  await page.waitForFunction(() => window.__nir.state().paused && !window.__nir.state().loading);
  const rolled = await state(page);
  expect(Number(rolled.tick_us)).toBeLessThanOrEqual(Number(next.tick_us));
  expect(rolled.error).toBeNull();
  // Stage cues are also checkpoints; another rollback reaches the previous line.
  if (!rolled.dialogue) {
    await act(page,{type:'rollback'});
    await page.waitForFunction(() => !!window.__nir.state().dialogue && !window.__nir.state().loading);
  }
  expect((await state(page)).dialogue.id).toBe(saved.dialogue.id);
});

test('independent pauses, viewport changes, touch and actual device recovery', async ({ page }) => {
  await boot(page); await start(page);
  await act(page,{type:'menu'});
  await page.evaluate(()=>window.__nir.hidden(true));
  const before=await state(page);
  await act(page,{type:'close'});
  await page.waitForTimeout(250);
  expect((await state(page)).tick_us).toBe(before.tick_us);
  expect((await state(page)).paused).toBe(true);
  await page.evaluate(()=>window.__nir.hidden(false));
  await act(page,{type:'menu'});
  const paused=await state(page);
  await page.setViewportSize({width:390,height:844});
  await page.waitForTimeout(300);
  expect((await state(page)).dialogue.id).toBe(paused.dialogue.id);
  await page.screenshot({path:'reports/narrow-menu.png'});
  await page.evaluate(()=>window.__nir.loseDevice());
  await page.waitForFunction(d=>window.__nir.state().device>d && window.__nir.state().ready && !window.__nir.state().loading,paused.device);
  const recovered=await state(page);
  expect(recovered.tick_us).toBe(paused.tick_us);
  expect(recovered.dialogue).toEqual(paused.dialogue);
  expect(recovered.paused).toBe(true);
  await act(page,{type:'close'}); await act(page,{type:'continue'});
  await page.screenshot({path:'reports/narrow-dialogue.png'});
  expect((await state(page)).error).toBeNull();
});

test('required resource failure keeps scene and retry succeeds', async ({ page }) => {
  await boot(page);
  let failed=false;
  await page.route('**/objects/*.wav', async route => {
    if(!failed){failed=true;await route.abort('failed');}else await route.continue();
  });
  await page.keyboard.press('Space');
  await page.waitForFunction(()=>!!window.__nir.state().error);
  expect((await state(page)).paused).toBe(true);
  await act(page,{type:'retry'});
  await page.waitForFunction(()=>!!window.__nir.state().dialogue&&!window.__nir.state().loading&&!window.__nir.state().error);
});

test('real tab visibility freezes Story and resumes through visibilitychange',async()=>{
  // Playwright's normal context forces all tabs visible. Attach without its
  // default emulation overrides to exercise real browser visibility instead.
  const dir=await fs.mkdtemp(`${process.env.TMPDIR || '/tmp'}/nir-visibility-`);
  const proc=spawn(process.env.CHROMIUM || '/usr/bin/chromium',['--no-sandbox','--no-first-run','--no-default-browser-check','--remote-debugging-port=0',`--user-data-dir=${dir}`,'--enable-unsafe-webgpu',...(process.env.NIR_CHROME_ARGS||'').split(' ').filter(Boolean),'--disable-backgrounding-occluded-windows','--disable-renderer-backgrounding','about:blank'],{stdio:'ignore'});
  let browser;
  try {
    let port;
    for(let n=0;n<100;n++){try{port=(await fs.readFile(`${dir}/DevToolsActivePort`,'utf8')).split('\n')[0];break;}catch{await new Promise(r=>setTimeout(r,100));}}
    expect(port).toBeTruthy();
    browser=await chromium.connectOverCDP(`http://127.0.0.1:${port}`,{noDefaults:true});
    const context=browser.contexts()[0],page=context.pages()[0];
    await page.goto('http://127.0.0.1:4173/?test=1');
    await page.bringToFront();
    await page.waitForFunction(()=>window.__nir?.state().ready,null,{polling:100,timeout:15000});
    await page.keyboard.press('Space');
    await page.waitForFunction(()=>!!window.__nir.state().dialogue,null,{polling:100,timeout:15000});
    const other=await context.newPage();await other.goto('about:blank');await other.bringToFront();
    await page.waitForFunction(()=>document.hidden&&window.__nir.state().paused,null,{timeout:10000,polling:100});
    const hidden=await state(page);await page.waitForTimeout(500);
    expect((await state(page)).tick_us).toBe(hidden.tick_us);
    await page.bringToFront();
    await page.waitForFunction(()=>!document.hidden&&!window.__nir.state().paused,null,{timeout:10000,polling:100});
    expect((await state(page)).session).toBe(hidden.session);
  } finally {await browser?.close();proc.kill('SIGTERM');}
});

test('actual device loss during a dissolve preserves progress', async ({ page }) => {
  await boot(page); await start(page);
  // Observe before triggering the transition: remote-control round trips may
  // otherwise miss its entire lifetime on a busy browser.
  await page.evaluate(()=>{
    window.__transitionPause=new Promise(resolve=>{
      const observe=()=>{const t=window.__nir.state().transition;
        if(t!==null&&t>0&&t<1)window.__nir.action({type:'menu'}).then(resolve);
        else requestAnimationFrame(observe);
      };
      requestAnimationFrame(observe);
    });
  });
  await page.keyboard.press('Space'); await page.keyboard.press('Space');
  // Deliberately outlast the transition to verify that browser-side observation
  // freezes it even when the test driver is late returning for the result.
  await page.waitForTimeout(700);
  expect(await page.evaluate(()=>window.__transitionPause)).toBe(true);
  await page.waitForFunction(()=>window.__nir.state().screen==='Menu');
  const before=await state(page);
  expect(before.transition).toBeGreaterThan(0);expect(before.transition).toBeLessThan(1);
  expect(before.loading).toBe(false);
  expectPainted(await page.screenshot({path:'reports/transition.png'}));
  await page.evaluate(()=>window.__nir.loseDevice());
  await page.waitForFunction(d=>window.__nir.state().device>d && window.__nir.state().ready && !window.__nir.state().loading,before.device);
  const after=await state(page);
  expect(after.transition).toBe(before.transition);
  expect(after.position).toBe(before.position);
  expect(after.history_count).toBe(before.history_count);
  expect(after.tick_us).toBe(before.tick_us);
});

test('corrupt WASM is rejected before instantiation', async ({ page }) => {
  await page.route('**/objects/*.wasm',async route=>{
    const response=await route.fetch();const bytes=await response.body();bytes[bytes.length-1]^=1;
    await route.fulfill({response,body:bytes});
  });
  await page.goto('/?test=1');
  await expect(page.locator('#shell-message')).toContainText('E_OBJECT_DIGEST');
  expect(await page.evaluate(()=>!!window.__nir)).toBe(false);
});

test('save export/import and tamper rejection preserve independent preferences',async({page})=>{
  await boot(page);await start(page);await act(page,{type:'menu'});
  const before=await state(page);
  const downloaded=page.waitForEvent('download');await act(page,{type:'export'});
  const path=await (await downloaded).path();const bytes=await fs.readFile(path);
  const envelope=JSON.parse(bytes.toString());expect(envelope.snapshot.release).toHaveLength(64);
  await act(page,{type:'settings'});await act(page,{type:'locale',locale:'en'});
  let chosen=page.waitForEvent('filechooser');await act(page,{type:'import'});
  await (await chosen).setFiles({name:'save.json',mimeType:'application/json',buffer:bytes});
  await page.waitForFunction(epoch=>window.__nir.state().session>epoch&&!window.__nir.state().loading,before.session);
  const restored=await state(page);
  expect(restored.paused).toBe(true);expect(restored.dialogue.locale).toBe('zh-Hans');expect(restored.locale).toBe('en');
  envelope.snapshot.variables.affection.value=999;
  chosen=page.waitForEvent('filechooser');await act(page,{type:'import'});
  await (await chosen).setFiles({name:'tampered.json',mimeType:'application/json',buffer:Buffer.from(JSON.stringify(envelope))});
  await page.waitForFunction(()=>window.__nir.state().error?.includes('E_SAVE_DIGEST'));
  expect((await state(page)).session).toBe(restored.session);
  expect((await state(page)).variables.affection.value).toBe(0);
});

test('final release boots below a URL subpath and uses correct HTTP types',async({page,request})=>{
  const prefix='http://127.0.0.1:4174/rain-letters-web/';
  await page.goto(prefix+'?test=1');
  await page.waitForFunction(()=>window.__nir?.state().ready);
  await expect(page.locator('#shell')).toBeHidden();
  const channel=await (await request.get(prefix+'channels/stable.json')).json();
  const manifest=await (await request.get(prefix+`releases/${channel.release}.json`)).json();
  const response=await request.get(prefix+manifest.objects[manifest.engine.wasm].path);
  expect(response.headers()['content-type']).toBe('application/wasm');
  expect(response.headers()['cache-control']).toContain('immutable');
  expect((await request.get(prefix+'objects/missing.wasm')).status()).toBe(404);
});

test.describe('touch input emulation',()=>{
  test.use({hasTouch:true,viewport:{width:390,height:844}});
  test('starts and advances by tapping the drawn canvas',async({page})=>{
    await boot(page);
    await page.locator('#actions button').filter({hasText:'开始阅读'}).focus();
    const box=await page.locator('#focus-ring').boundingBox();
    await page.touchscreen.tap(box.x+box.width/2,box.y+box.height/2);
    await page.waitForFunction(()=>!!window.__nir.state().dialogue);
    await page.touchscreen.tap(160,650);
    await page.waitForFunction(()=>window.__nir.state().dialogue?.ready);
    await page.screenshot({path:'reports/touch-dialogue.png'});
  });
});

test('owner inbox defers actions, drains bursts and keeps turn work bounded', async ({ page }) => {
  await boot(page); await start(page);
  const result=await page.evaluate(async()=>{
    const before=window.__nir.state().screen;
    const pending=window.__nir.action({type:'menu'});
    const immediate=window.__nir.state().screen;
    await pending;
    const after=window.__nir.state().screen;
    const s=window.__nir.state();
    await Promise.all(Array.from({length:40},(_,i)=>window.__nir.rawAction({type:'advance'},s.interaction-1,s.sequence+i+1,s.session)));
    return {before,immediate,after,state:window.__nir.state(),metrics:window.__nir.metrics};
  });
  expect(result.before).toBe('Story');expect(result.immediate).toBe('Story');expect(result.after).toBe('Menu');
  expect(result.state.error).toBeNull();expect(result.state.pending_events).toBe(0);
  expect(result.metrics.inboxHighWater).toBeGreaterThanOrEqual(40);
  expect(result.metrics.inboxHighWater).toBeLessThanOrEqual(256);
  expect(result.metrics.maxTurnWork).toBeLessThanOrEqual(10000);
  expect(result.metrics.maxTurnUploadBytes).toBeGreaterThan(0);
  expect(result.metrics.maxTurnUploadBytes).toBeLessThanOrEqual(2*1024*1024);
  expect(result.metrics.uploadSteps).toBeGreaterThan(1);
});

test('leaving preparation cancels its fetch and a new request still succeeds', async ({ page }) => {
  await boot(page);
  let intercepted=false,release;
  const gate=new Promise(resolve=>release=resolve);
  const cancelled=[];page.on('requestfailed',request=>{if(request.url().endsWith('.wav'))cancelled.push(request.url());});
  await page.route('**/objects/*.wav',async route=>{
    if(!intercepted){intercepted=true;await gate;}
    await route.continue().catch(()=>{});
  });
  await page.keyboard.press('Space');
  await expect.poll(()=>intercepted).toBe(true);
  await act(page,{type:'title'});
  await page.waitForFunction(()=>window.__nir.state().screen==='Title'&&!window.__nir.state().loading);
  await expect.poll(()=>cancelled.length).toBeGreaterThan(0);
  release();
  await act(page,{type:'new_game'});
  await page.waitForFunction(()=>!!window.__nir.state().dialogue&&!window.__nir.state().loading);
  expect((await state(page)).error).toBeNull();
});

async function delaySaveTransactions(page) {
  await page.addInitScript(()=>{
    const original=IDBDatabase.prototype.transaction;
    const complete=Object.getOwnPropertyDescriptor(IDBTransaction.prototype,'oncomplete');
    window.__heldTransactions=[];
    IDBDatabase.prototype.transaction=function(...args){
      const tx=original.apply(this,args);
      if(Array.from(tx.objectStoreNames).includes('saves'))Object.defineProperty(tx,'oncomplete',{
        configurable:true,
        get(){return complete.get.call(tx);},
        set(fn){complete.set.call(tx,event=>{
          const hold=tx.mode==='readwrite'?window.__holdSaveWrites:window.__holdSaveReads;
          if(hold)window.__heldTransactions.push(()=>fn.call(tx,event));else fn.call(tx,event);
        });}
      });
      return tx;
    };
  });
}

test('reserved save completion survives session replacement and duplicate traffic',async({page})=>{
  await delaySaveTransactions(page);await boot(page);await start(page);
  await page.evaluate(()=>window.__holdSaveWrites=true);
  await act(page,{type:'save',slot:0});
  await page.waitForFunction(()=>window.__heldTransactions.length>0);
  const savedSession=(await state(page)).session;
  await act(page,{type:'title'});
  await page.waitForFunction(()=>!window.__nir.state().loading);
  await page.evaluate(async()=>{
    const s=window.__nir.state();
    await Promise.all(Array.from({length:40},(_,i)=>window.__nir.rawAction({type:'advance'},s.interaction,s.sequence+i+1,s.session-1)));
    window.__holdSaveWrites=false;
    for(const complete of window.__heldTransactions.splice(0))complete();
  });
  await page.waitForFunction(()=>/已保存|Saved/.test(window.__nir.state().status));
  const current=await state(page);expect(current.session).toBeGreaterThan(savedSession);expect(current.screen).toBe('Title');expect(current.error).toBeNull();
  await page.waitForFunction(()=>window.__nir.metrics.activeRequests===0);
  const m=await page.evaluate(()=>window.__nir.metrics);
  expect(m.acceptedRequests).toBe(m.completedRequests+m.cancelledRequests+m.activeRequests);
});

test('a delayed load cannot replace a newer session',async({page})=>{
  await delaySaveTransactions(page);await boot(page);await start(page);
  await act(page,{type:'save',slot:0});await page.waitForFunction(()=>/已保存|Saved/.test(window.__nir.state().status));
  await page.evaluate(()=>window.__holdSaveReads=true);
  await act(page,{type:'load',slot:0});await page.waitForFunction(()=>window.__heldTransactions.length>0);
  await act(page,{type:'title'});await page.waitForFunction(()=>!window.__nir.state().loading);
  const title=await state(page);
  await page.evaluate(()=>{window.__holdSaveReads=false;for(const complete of window.__heldTransactions.splice(0))complete();});
  await page.waitForTimeout(150);
  const after=await state(page);expect(after.screen).toBe('Title');expect(after.session).toBe(title.session);expect(after.error).toBeNull();
  await page.waitForFunction(()=>window.__nir.metrics.activeRequests===0);
});

test('repeated preparation, pause and device replacement release all request slots',async({page})=>{
  await boot(page);
  for(let cycle=0;cycle<12;cycle++){
    await act(page,{type:'new_game'});
    await page.waitForFunction(()=>!!window.__nir.state().dialogue&&!window.__nir.state().loading);
    await act(page,{type:'menu'});await page.evaluate(()=>window.__nir.hidden(true));
    await act(page,{type:'close'});expect((await state(page)).paused).toBe(true);
    await page.evaluate(()=>window.__nir.hidden(false));
    if(cycle%4===3){
      const old=(await state(page)).device;await page.evaluate(()=>window.__nir.loseDevice());
      await page.waitForFunction(d=>window.__nir.state().device>d&&window.__nir.state().ready&&!window.__nir.state().loading,old);
    }
    await act(page,{type:'title'});
    await page.waitForFunction(()=>!window.__nir.state().loading&&window.__nir.metrics.activeRequests===0);
    const s=await state(page),m=await page.evaluate(()=>window.__nir.metrics);
    expect(s.error).toBeNull();expect(s.screen).toBe('Title');
    expect(m.acceptedRequests).toBe(m.completedRequests+m.cancelledRequests);
    expect(m.inboxHighWater).toBeLessThanOrEqual(256);
  }
  await fs.writeFile('reports/request-lifecycle-metrics.json',JSON.stringify(await page.evaluate(()=>window.__nir.metrics),null,2));
});


test('an owner turn detects device loss before the idle watchdog',async({page})=>{
  await page.addInitScript(()=>{
    const interval=window.setInterval.bind(window);
    window.setInterval=(fn,ms,...args)=>interval(fn,ms===500?60000:ms,...args);
    const destroy=GPUDevice.prototype.destroy;
    GPUDevice.prototype.destroy=function(){window.__deviceLoss=this.lost;return destroy.call(this);};
  });
  await boot(page);await start(page);await act(page,{type:'menu'});
  const before=await state(page);
  await page.evaluate(async()=>{
    window.__nir.loseDevice();
    // Device destruction crosses the GPU process boundary; a timer turn does
    // not guarantee that the registered loss callback has run yet.
    await window.__deviceLoss;
    await window.__nir.hidden(true);
  });
  await page.waitForFunction(d=>window.__nir.state().device>d&&window.__nir.state().ready&&!window.__nir.state().loading,before.device,{timeout:15000});
  const after=await state(page);
  expect(after.position).toBe(before.position);expect(after.tick_us).toBe(before.tick_us);
  expect(after.paused).toBe(true);expect(after.error).toBeNull();
  expectPainted(await page.screenshot());
});

test('decode and quota failures provide correlated diagnostics and a private export',async({page})=>{
  await page.addInitScript(()=>{
    const decode=AudioContext.prototype.decodeAudioData;
    let once=true;
    AudioContext.prototype.decodeAudioData=function(...args){if(once){once=false;return Promise.reject(new Error('PRIVATE_AUDIO_SENTINEL'));}return decode.apply(this,args);};
    const put=IDBObjectStore.prototype.put;
    let quota=true;
    IDBObjectStore.prototype.put=function(...args){if(this.name==='saves'&&quota){quota=false;throw new DOMException('PRIVATE_STORAGE_SENTINEL','QuotaExceededError');}return put.apply(this,args);};
  });
  await boot(page);await page.keyboard.press('Space');
  await page.waitForFunction(()=>window.__nir.state().diagnostic?.code==='E_AUDIO_DECODE');
  const failed=await state(page);
  expect(failed.diagnostic.details.domain).toBe('prepare');
  expect(failed.diagnostic.details.request).toBeGreaterThan(0);
  expect(failed.error).not.toContain('PRIVATE_');
  await act(page,{type:'retry'});
  await page.waitForFunction(()=>window.__nir.state().dialogue&&!window.__nir.state().loading&&!window.__nir.state().error);
  await act(page,{type:'saves'});await act(page,{type:'save',slot:0});
  await page.waitForFunction(()=>window.__nir.state().diagnostic?.code==='E_STORAGE_QUOTA');
  expect((await state(page)).status).not.toContain('PRIVATE_');
  await act(page,{type:'save',slot:0});
  await page.waitForFunction(()=>/已保存|Saved/.test(window.__nir.state().status));
  const report=await page.evaluate(()=>window.__nir.diagnostics());
  const stages=new Set(report.events.map(e=>e.stage));
  for(const name of ['bootstrap_started','wasm_initialized','input_received','prepare_requested','fetch_verified','decode_allocate','upload_enqueued','presentation_prepare','lease_ready','prepare_commit','render_submitted','diagnostic','storage_committed'])expect(stages.has(name),name).toBe(true);
  expect(JSON.stringify(report)).not.toMatch(/PRIVATE_|snapshot|variables|visible_text/);
  expect(report.events.find(e=>e.stage==='resource_failed'&&e.code==='E_AUDIO_DECODE').request).toBe(failed.diagnostic.details.request);
  await fs.writeFile('reports/diagnostic-trace.json',JSON.stringify(report,null,2));
  const download=page.waitForEvent('download');
  await page.evaluate(()=>window.nirDiagnostics.download());
  const artifact=await download;expect(artifact.suggestedFilename()).toBe('nir-diagnostics.json');
});

test('tracing on and off preserves deterministic story trace and outcome',async({page})=>{
  const runs=[];
  for(const enabled of [true,false]){
    await page.goto(`/?test=1${enabled?'':'&trace=0'}`);
    await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
    await start(page);
    await advanceUntil(page,s=>!!s.choice);
    await act(page,{type:'choose',option:'stay'});
    const end=await advanceUntil(page,s=>!!s.outcome,'stay');
    // Logical IDs/order are deterministic; wall-clock-derived Story times are not.
    runs.push({outcome:end.outcome,variables:end.variables,trace:await page.evaluate(()=>window.__nir.traces)});
    if(!enabled)expect(await page.evaluate(()=>window.__nir.diagnostics().events.length)).toBe(0);
  }
  expect(runs[1]).toEqual(runs[0]);
});

test('author theme and player defaults run on the unchanged SDK and keep saved preferences',async({page})=>{
  const {execFile}=await import('node:child_process');
  const {promisify}=await import('node:util');
  const run=promisify(execFile);
  const dir=await fs.mkdtemp('target/tmp/theme-contract-');
  const project=`${dir}/story`;
  await run('dist/novelc',['init',project,'--template','web-basic']);
  const theme=`${project}/themes/rain/theme.toml`;
  await fs.writeFile(theme,(await fs.readFile(theme,'utf8')).replace('builtin.dialogue"','builtin.dialogue.top"').replace('builtin.choice"','builtin.choice.compact"').replace('height = 240.0','height = 260.0'));
  await fs.writeFile(`${project}/config/player.toml`,'format = 1\n[defaults]\nfont_scale = 1.2\nbgm_volume = 0.12\nreduced_motion = true\n');
  await run('dist/novelc',['-p',project,'resolve']);
  const lock=await fs.readFile(`${project}/game.lock`,'utf8');
  await run('dist/novelc',['-p',project,'build','--locked','--out','dist/theme-contract-web']);
  expect(await fs.readFile(`${project}/game.lock`,'utf8')).toBe(lock);
  const resolved=JSON.parse((await run('dist/novelc',['-p',project,'config'])).stdout);
  expect(resolved['theme.slots.dialogue.main'].value).toBe('builtin.dialogue.top');
  await page.goto('http://127.0.0.1:4174/theme-contract-web/?test=1');
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  expect((await state(page)).preferences.font_scale).toBeCloseTo(1.2);
  expect((await state(page)).preferences.bgm_volume).toBeCloseTo(.12);
  expect((await state(page)).preferences.reduced_motion).toBe(true);
  await start(page);await page.keyboard.press('Space');
  const advance=page.locator('#actions button').filter({hasText:'继续阅读'});
  await advance.focus();
  expect(await page.locator('#focus-ring').evaluate(e=>parseFloat(e.style.top))).toBe(64);
  expectPainted(await page.screenshot({path:'reports/theme-top-dialogue.png'}));
  await act(page,{type:'settings'});await act(page,{type:'font_size',delta:.1});
  await act(page,{type:'title'}); // Looping BGM legitimately owns a terminal slot until stopped.
  await page.waitForFunction(()=>window.__nir.metrics.activeRequests===0);
  await page.reload();
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading&&window.__nir.metrics.activeRequests===0);
  expect((await state(page)).preferences.font_scale).toBeCloseTo(1.3);
  await start(page);
  await advanceUntil(page,s=>!!s.choice);
  await page.setViewportSize({width:390,height:844});
  // Both components keep their original labels, stable option IDs and keyboard focus.
  const buttons=page.locator('#actions button');
  const options=(await state(page)).choice.options;
  const chosen=buttons.filter({hasText:options[0].label});
  await chosen.focus();
  await expect.poll(()=>page.locator('#focus-ring').evaluate(e=>parseFloat(e.style.width))).toBe(350);
  const firstY=await page.locator('#focus-ring').evaluate(e=>parseFloat(e.style.top));
  await buttons.filter({hasText:options[1].label}).focus();
  expect(await page.locator('#focus-ring').evaluate(e=>parseFloat(e.style.top))).toBe(firstY+64);
  await chosen.focus();
  expectPainted(await page.screenshot({path:'reports/theme-compact-choice.png'}));
  await chosen.press('Enter');
  const end=await advanceUntil(page,s=>!!s.outcome,'walk');
  expect(end.outcome).toBe('walk_home');
  await fs.rm(dir,{recursive:true,force:true});
});
