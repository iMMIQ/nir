import { test, expect } from '@playwright/test';
import fs from 'node:fs/promises';
import path from 'node:path';
import { execFile, spawn } from 'node:child_process';
import { promisify } from 'node:util';
import { expectPainted } from './pixels.js';

const run = promisify(execFile);
const cli = path.resolve('dist/novelc');
const state = page => page.evaluate(() => window.__nir.state());
const act = (page, action) => page.evaluate(a => window.__nir.action(a), action);
const view = (s, region) => s.scrolls.find(v => v.region === region);
async function ready(page) {
  await page.waitForFunction(() => window.__nir?.state().ready && !window.__nir.state().loading);
  await expect(page.locator('#shell')).toBeHidden();
}
async function start(page) {
  await page.keyboard.press('Space');
  await page.waitForFunction(() => window.__nir.state().dialogue && !window.__nir.state().loading);
}
async function nextUntil(page, predicate) {
  for (let i = 0; i < 120; i++) {
    const s = await state(page);
    expect(s.error).toBeNull();
    if (predicate(s)) return s;
    if (!s.dialogue?.gate) await act(page, {type:'advance'});
    await page.waitForTimeout(80);
  }
  throw Error(`Unreachable author content: ${JSON.stringify(await state(page))}`);
}

let root;
test.beforeAll(async () => {
  await fs.mkdir('target/tmp', {recursive:true});
  root = await fs.mkdtemp(path.resolve('target/tmp/author-reading-'));
  await run(cli, ['init', `${root}/story`, '--template', 'web-basic']);
  const story = `${root}/story`;
  for (const locale of ['zh-Hans', 'en']) {
    const file = `${story}/content/ch01/texts/${locale}.json`;
    const texts = JSON.parse(await fs.readFile(file, 'utf8'));
    texts.intro.spans[0].text = Array.from({length:24}, (_, i) => `${i+1}. ${texts.intro.spans[0].text}`).join('\n');
    texts.letter.spans[0].text = (texts.letter.spans[0].text + '\n').repeat(16);
    texts.walk.spans[0].text = texts.walk.spans[0].text.repeat(16);
    await fs.writeFile(file, JSON.stringify(texts, null, 2));
  }
  const file = `${story}/content/ch01/story.nir.json`;
  const fragment = JSON.parse(await fs.readFile(file, 'utf8'));
  fragment.choices.route.options = Array.from({length:24}, (_, i) => ({
    id:`option${i}`, text:i === 0 ? 'walk' : 'stay',
    ...(i === 7 ? {enabled:{type:'const',value:{type:'bool',value:false}}} : {}),
  }));
  fragment.functions.main.blocks.choose.terminator.branches = Object.fromEntries(
    fragment.choices.route.options.map(o => [o.id, 'stay_begin']));
  await fs.writeFile(file, JSON.stringify(fragment, null, 2));
  const prefs = `${story}/config/player.toml`;
  await fs.writeFile(prefs, (await fs.readFile(prefs,'utf8')).replace('font_scale = 1.0','font_scale = 1.5'));
  await run(cli, ['-p', story, 'resolve']);
  await run(cli, ['-p', story, 'build', '--locked', '--out', path.resolve('dist/author-reading-web')]);
});
test.afterAll(async () => { await fs.rm(root, {recursive:true,force:true}); });

test('long author content remains readable across advance, input, reflow, restore and many choices', async ({page}) => {
  page.on('pageerror', e => { throw e; });
  await page.setViewportSize({width:390,height:600});
  await page.goto('http://127.0.0.1:4174/author-reading-web/?test=1');
  await ready(page); await start(page);
  await act(page,{type:'advance'});
  let s = await state(page);
  expect(s.dialogue.ready).toBe(true);
  expect(view(s,'dialogue').offset).toBeLessThan(100);
  expect(view(s,'dialogue').max).toBeGreaterThan(1000);
  const original = s;
  // Several commands queued before a submitted frame must still browse current layout.
  await page.evaluate(async () => { await Promise.all([window.__nir.action({type:'advance'}),window.__nir.action({type:'advance'})]); });
  s = await state(page);
  expect(s.dialogue).toEqual(original.dialogue);
  expect(s.interaction).toBe(original.interaction);
  expect(view(s,'dialogue').offset).toBeGreaterThan(0);
  const offset = view(s,'dialogue').offset;
  await page.evaluate(s => window.__nir.rawAction({type:'scroll',region:'dialogue',delta:1},s.interaction-1,s.sequence+100,s.session),s);
  expect(view(await state(page),'dialogue').offset).toBe(offset);
  await page.keyboard.press('PageUp');
  await expect.poll(async()=>view(await state(page),'dialogue').offset).toBeLessThan(offset);
  const v = view(await state(page),'dialogue');
  await page.mouse.move(v.rect[0]+30,v.rect[1]+30); await page.mouse.wheel(0,120);
  await expect.poll(async () => view(await state(page),'dialogue').offset).toBeGreaterThan(v.offset);
  // Pointer swipes browse text without activating the underlying Advance target.
  const swipeBefore = await state(page), r = view(swipeBefore,'dialogue').rect;
  await page.locator('#stage').dispatchEvent('pointerdown',{pointerType:'touch',clientX:r[0]+50,clientY:r[1]+100});
  await page.locator('#stage').dispatchEvent('pointerup',{pointerType:'touch',clientX:r[0]+50,clientY:r[1]+40});
  await expect.poll(async () => view(await state(page),'dialogue').offset).toBeGreaterThan(view(swipeBefore,'dialogue').offset);
  expect((await state(page)).dialogue).toEqual(original.dialogue);
  await page.setViewportSize({width:844,height:390});
  await expect.poll(async () => view(await state(page),'dialogue').rect[2]).toBeGreaterThan(600);
  expect(view(await state(page),'dialogue').offset).toBeGreaterThan(0);
  expectPainted(await page.screenshot({path:'reports/author-long-dialogue.png'}));
  await act(page,{type:'saves'}); await act(page,{type:'save',slot:0});
  await page.waitForFunction(() => /已保存|Saved/.test(window.__nir.state().status));
  await page.reload(); await ready(page);
  await act(page,{type:'saves'}); await act(page,{type:'load',slot:0});
  await page.waitForFunction(() => window.__nir.state().paused && window.__nir.state().screen==='Story' && !window.__nir.state().loading);
  expect((await state(page)).dialogue).toEqual(original.dialogue);
  await act(page,{type:'continue'});
  await act(page,{type:'advance'});
  await nextUntil(page, s => !!s.dialogue && s.dialogue.id !== 'intro');
  await act(page,{type:'history'});
  expect(view(await state(page),'history').max).toBeGreaterThan(0);
  await page.keyboard.press('PageDown');
  await expect.poll(async()=>view(await state(page),'history').offset).toBeGreaterThan(0);
  await act(page,{type:'close'});
  // Freeze at the authored Gate before its audio completion. Browsing cannot release it.
  await nextUntil(page,s=>s.dialogue?.id==='letter');
  await page.evaluate(async()=>{await window.__nir.action({type:'advance'}); await window.__nir.hidden(true);});
  const gated = await state(page);
  expect(gated.dialogue.gate).toBe(true);
  expect(view(gated,'dialogue').max).toBeGreaterThan(0);
  await page.keyboard.press('PageDown');
  expect((await state(page)).dialogue).toEqual(gated.dialogue);
  await page.evaluate(()=>window.__nir.hidden(false));
  await nextUntil(page,s=>!!s.choice);
  await page.setViewportSize({width:390,height:600});
  await expect.poll(async()=>view(await state(page),'choices')?.max || 0).toBeGreaterThan(1000);
  const interaction = (await state(page)).interaction;
  const options = new Set(); let disabledSeen = false;
  for(let n=0;n<100;n++) {
    const visible = await page.locator('#actions button').evaluateAll(buttons=>buttons.map(b=>({action:JSON.parse(b.dataset.action),disabled:b.disabled})));
    for(const b of visible.filter(b=>b.action.type==='choose')) {
      options.add(b.action.option);
      if(b.action.option==='option7') { expect(b.disabled).toBe(true); disabledSeen=true; }
    }
    s=await state(page); expect(s.interaction).toBe(interaction);
    if(view(s,'choices').offset>=view(s,'choices').max-.5) break;
    const previousOffset=view(s,'choices').offset;
    await page.keyboard.press('PageDown');
    await expect.poll(async()=>view(await state(page),'choices').offset).toBeGreaterThan(previousOffset);
  }
  expect(options.size).toBe(24); expect(disabledSeen).toBe(true);
  expectPainted(await page.screenshot({path:'reports/author-many-choices.png'}));
  const last = page.locator('#actions button').filter({hasText:'留在车站'}).last();
  expect(JSON.parse(await last.getAttribute('data-action')).option).toBe('option23');
  await last.focus(); await page.keyboard.press('Enter');
  await expect.poll(async()=>(await state(page)).choice).toBeNull();
  expect((await nextUntil(page,s=>!!s.outcome)).outcome).toBe('read_letter');
  await act(page,{type:'settings'}); await act(page,{type:'locale',locale:'en'});
  await act(page,{type:'font_size',delta:-.2}); await act(page,{type:'close'});
  await act(page,{type:'new_game'});
  await page.waitForFunction(()=>!!window.__nir.state().dialogue&&!window.__nir.state().loading);
  await act(page,{type:'advance'});
  expect((await state(page)).dialogue.locale).toBe('en');
  expect(view(await state(page),'dialogue').max).toBeGreaterThan(0);
  await expect(page.getByRole('button',{name:'Page down',exact:true})).toBeEnabled();
  expectPainted(await page.screenshot({path:'reports/author-english-dialogue.png'}));
});

test('dev rebuild reloads valid content and retains the previous session on author errors', async ({page,request}) => {
  const story = `${root}/dev-story`;
  await run(cli,['init',story,'--template','web-basic']); await run(cli,['-p',story,'resolve']);
  const lock = await fs.readFile(`${story}/game.lock`,'utf8');
  const proc = spawn(cli,['-p',story,'dev','--port','4175'],{stdio:'ignore'});
  const url='http://127.0.0.1:4175';
  const status=async()=>{try{return await (await request.get(`${url}/__nir_dev/status`)).json();}catch{return null;}};
  try {
    await expect.poll(async()=>!!(await status())?.release).toBe(true);
    await page.goto(`${url}/?test=1`);await ready(page);await start(page);
    await act(page,{type:'advance'});
    const before=await state(page), release=(await status()).release;
    const file=`${story}/content/ch01/texts/zh-Hans.json`;
    const texts=JSON.parse(await fs.readFile(file,'utf8'));
    await fs.writeFile(file,'{"broken":');
    await expect.poll(async()=>(await status())?.error || '').toContain('E_');
    await expect(page.locator('#nir-dev-status')).toBeVisible();
    expect((await status()).release).toBe(release);
    expect((await state(page)).dialogue).toEqual(before.dialogue);
    expect((await state(page)).interaction).toBe(before.interaction);
    expectPainted(await page.screenshot({path:'reports/author-preview-error.png'}));
    texts.intro.spans[0].text='雨后书简。'+texts.intro.spans[0].text;
    await fs.writeFile(file,JSON.stringify(texts,null,2));
    await expect.poll(async()=>(await status())?.release).not.toBe(release);
    await page.waitForFunction(()=>window.__nir?.state().ready && window.__nir.state().screen==='Title');
    await expect(page.locator('#nir-dev-status')).toBeHidden();
    await start(page); await act(page,{type:'advance'});
    expect((await state(page)).dialogue.visible).toContain('雨后书简。');
    expect(await fs.readFile(`${story}/game.lock`,'utf8')).toBe(lock);
    const stable=(await status()).release;
    await fs.mkdir(`${story}/reports`,{recursive:true});await fs.writeFile(`${story}/reports/note.txt`,'ignored');
    await page.waitForTimeout(1100);
    expect(await status()).toMatchObject({release:stable,building:false,error:null});
    expect((await state(page)).dialogue.visible).toContain('雨后书简。');
    expect((await request.get('http://127.0.0.1:4174/__nir_dev/status')).status()).toBe(404);
    expect(await fs.readFile(`${story}/dist/full/web/index.html`,'utf8')).not.toContain('__nir_dev');
  } finally { if(proc.exitCode===null && proc.signalCode===null){const exited=new Promise(resolve=>proc.once('exit',resolve));proc.kill('SIGTERM');await exited;} }
});
