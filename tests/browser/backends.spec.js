import { test, expect } from '@playwright/test';
import { expectPainted, readPng } from './pixels.js';

const state = page => page.evaluate(() => window.__nir.state());
const act = (page, action) => page.evaluate(a => window.__nir.action(a), action);
async function ready(page) {
  await page.waitForFunction(() => (window.__nir?.state().ready && !window.__nir.state().loading) || document.querySelector('#reload')?.hidden===false);
  expect(await page.locator('#shell-message').textContent(), 'bootstrap must initialize the renderer').not.toMatch(/E_[A-Z_]+/);
  expect((await state(page)).error).toBeNull();
}
async function boot(page, backend) {
  await page.goto(`/?test=1&backend=${backend}`);
  await ready(page);
  expect((await state(page)).backend).toBe(backend);
  await expect(page.locator('#shell')).toBeHidden();
}

test('backend renders Chinese, choices, transitions and audio; saves survive refresh', async ({ page }, info) => {
  const errors=[]; page.on('pageerror', error => errors.push(String(error)));
  const backend=info.project.metadata.backend;
  await boot(page, backend);
  expectPainted(await page.locator('#stage').screenshot());
  await page.keyboard.press('Space');
  await expect.poll(async () => (await page.evaluate(() => window.__nir.diagnostics())).host_work.audio_state,
    { message: 'AudioContext must run after trusted input; CI requires an audio output', timeout: 10000 }).toBe('running');
  await page.waitForFunction(() => window.__nir.state().dialogue && !window.__nir.state().loading);
  await page.keyboard.press('Space');
  await expect.poll(async()=> (await state(page)).dialogue?.visible).toContain('末班电车');
  expectPainted(await page.locator('#stage').screenshot());
  await act(page, {type:'saves'});
  const saved=await state(page);
  await act(page, {type:'save',slot:0});
  await page.waitForFunction(() => /已保存|Saved/.test(window.__nir.state().status));
  await page.reload(); await ready(page);
  await act(page, {type:'saves'}); await act(page, {type:'load',slot:0});
  await page.waitForFunction(() => window.__nir.state().screen==='Story' && window.__nir.state().paused && !window.__nir.state().loading);
  expect((await state(page)).dialogue).toEqual(saved.dialogue);
  await page.keyboard.press('Space');
  let reachedChoice=false;
  for(let step=0;step<150;step++) {
    const s=await state(page); expect(s.error).toBeNull();
    if(s.choice) { reachedChoice=true; break; }
    if(s.dialogue&&!s.dialogue.gate) await page.keyboard.press('Space');
    await page.waitForTimeout(120);
  }
  if(!reachedChoice)await info.attach('stalled-diagnostics',{body:JSON.stringify(await page.evaluate(()=>window.__nir.diagnostics()),null,2),contentType:'application/json'});
  expect(reachedChoice).toBe(true);
  expectPainted(await page.locator('#stage').screenshot());
  await act(page,{type:'choose',option:'walk'});
  for(let step=0;step<150;step++) {
    const s=await state(page); expect(s.error).toBeNull();
    if(s.outcome) break;
    if(s.dialogue&&!s.dialogue.gate) await page.keyboard.press('Space');
    await page.waitForTimeout(120);
  }
  expect((await state(page)).outcome).toBe('walk_home');
  expect(await page.evaluate(() => window.__nir.metrics.audioStarts)).toBeGreaterThan(0);
  expect(errors).toEqual([]);
  await info.attach('diagnostics', {body:JSON.stringify(await page.evaluate(() => window.__nir.diagnostics()),null,2),contentType:'application/json'});
});

test('device loss recovers on the selected backend', async ({ page }, info) => {
  const backend=info.project.metadata.backend;
  await boot(page,backend);
  await page.keyboard.press('Space');
  await page.waitForFunction(() => window.__nir.state().dialogue && !window.__nir.state().loading);
  const before=await state(page);
  if(backend==='webgl2') {
    await page.evaluate(() => {
      const gl=document.querySelector('#stage').getContext('webgl2');
      const extension=gl.getExtension('WEBGL_lose_context');
      if(!extension) throw Error('WEBGL_lose_context unavailable');
      extension.loseContext();
      setTimeout(() => extension.restoreContext(),300);
    });
  } else await page.evaluate(() => window.__nir.loseDevice());
  await page.waitForFunction(device => window.__nir.state().device>device && window.__nir.state().ready && !window.__nir.state().loading,before.device);
  expect((await state(page)).backend).toBe(backend);
  expect((await state(page)).error).toBeNull();
  expectPainted(await page.locator('#stage').screenshot());
});

test('auto falls back when WebGPU initialization fails; forced WebGPU fails explicitly', async ({ page }, info) => {
  test.skip(info.project.name!=='chromium-webgl2','Startup fallback is exercised once on Chromium.');
  await page.addInitScript(() => {
    if(navigator.gpu){
      const request=navigator.gpu.requestAdapter.bind(navigator.gpu);
      navigator.gpu.requestAdapter=async options=>{
        const adapter=await request(options);
        if(adapter)adapter.requestDevice=async()=>{throw Error('injected GPU device initialization failure');};
        return adapter;
      };
    }
  });
  await page.goto('/?test=1'); await ready(page);
  expect((await state(page)).backend).toBe('webgl2');
  expectPainted(await page.locator('#stage').screenshot());
  await page.goto('/?test=1&backend=webgpu');
  await expect(page.locator('#shell-message')).toContainText(/GPU|adapter|backend/i);
  expect(await page.evaluate(() => Boolean(window.__nir))).toBe(false);
});

test('WebGL2 preserves WebGPU title color and transparent overlay blending',async({page},info)=>{
  test.skip(info.project.name!=='chromium-webgl2','Compare both renderers in one Chromium instance.');
  const captures=[];
  for(const backend of ['webgpu','webgl2']){
    await boot(page,backend);
    captures.push(readPng(await page.locator('#stage').screenshot()));
  }
  const [gpu,gl]=captures;
  expect([gl.width,gl.height]).toEqual([gpu.width,gpu.height]);
  // Text-free patches include the scene behind the translucent title overlay.
  // Every RGB channel is checked so a gamma or alpha regression cannot pass by
  // merely retaining a nonblank canvas.
  for(const [fx,fy] of [[.9,.1],[.5,.2],[.1,.7],[.9,.8]]){
    let error=0,count=0;
    for(let dy=0;dy<16;dy++)for(let dx=0;dx<16;dx++){
      const x=Math.floor(gpu.width*fx)+dx,y=Math.floor(gpu.height*fy)+dy;
      const a=(y*gpu.width+x)*gpu.channels,b=(y*gl.width+x)*gl.channels;
      for(let c=0;c<3;c++){error+=Math.abs(gpu.pixels[a+c]-gl.pixels[b+c]);count++;}
    }
    expect(error/count,`RGB difference at ${fx},${fy}`).toBeLessThan(3);
  }
});
