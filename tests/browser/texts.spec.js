import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import path from 'node:path';
import {execFile,spawn} from 'node:child_process';
import {promisify} from 'node:util';
import {expectPainted} from './pixels.js';
const run=promisify(execFile), cli=path.resolve('dist/novelc');
const state=p=>p.evaluate(()=>window.__nir.state());
const act=(p,a)=>p.evaluate(a=>window.__nir.action(a),a);
const ready=p=>p.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
async function start(page){await page.keyboard.press('Space');await page.waitForFunction(()=>window.__nir.state().dialogue?.ready&&!window.__nir.state().loading);}
let root;
test.beforeAll(async()=>{await fs.mkdir('target/tmp',{recursive:true});root=await fs.mkdtemp(path.resolve('target/tmp/text-revisions-'));});
test.afterAll(async()=>{await fs.rm(root,{recursive:true,force:true});});
test('dev preserves a session until source revision and translation review both complete',async({page,request})=>{
  const story=`${root}/story`;await run(cli,['init',story]);await run(cli,['-p',story,'resolve']);
  const proc=spawn(cli,['-p',story,'dev','--port','4175'],{stdio:'ignore'}),url='http://127.0.0.1:4175';
  const status=async()=>{try{return await(await request.get(`${url}/__nir_dev/status`)).json();}catch{return null;}};
  try {
    await expect.poll(async()=>!!(await status())?.release).toBe(true);
    await page.goto(`${url}/?test=1`);await ready(page);await start(page);
    const before=await state(page),release=(await status()).release;
    // Recovery must wake the watcher even when only the ignored .nir journal changes.
    await fs.writeFile(`${story}/.nir/text-transaction.json`,'[]');
    await expect.poll(async()=>(await status())?.error||'').toContain('E_TEXT_TRANSACTION');
    expect((await state(page)).dialogue).toEqual(before.dialogue);
    await run(cli,['-p',story,'text','recover']);
    await expect.poll(async()=>(await status())?.error??'').toBe('');
    expect((await status()).release).toBe(release);
    const file=`${story}/content/main/texts/zh-Hans.json`,doc=JSON.parse(await fs.readFile(file,'utf8'));
    doc.intro.spans[0].text='春天的花开了。';await fs.writeFile(file,JSON.stringify(doc,null,2));
    await expect.poll(async()=>(await status())?.error||'').toContain('E_TEXT_SOURCE_CHANGED');
    expect((await status()).release).toBe(release);expect((await state(page)).dialogue).toEqual(before.dialogue);
    await run(cli,['-p',story,'text','update','--id','intro','--meaning','preserve']);
    await expect.poll(async()=>(await status())?.error||'').toContain('E_TRANSLATION_STALE');
    const report=JSON.parse((await run(cli,['-p',story,'text','status','--json'])).stdout);
    expect(report.ready).toBe(false);expect(report.issues.some(i=>i.text_id==='intro'&&i.locale==='en')).toBe(true);
    await fs.writeFile('reports/text-stale-status.json',JSON.stringify(report,null,2));
    expect((await state(page)).dialogue).toEqual(before.dialogue);
    expectPainted(await page.screenshot({path:'reports/text-stale-preview.png'}));
    const english=`${story}/content/main/texts/en.json`,en=JSON.parse(await fs.readFile(english,'utf8'));
    en.intro.spans[0].text='The spring flowers have opened.';await fs.writeFile(english,JSON.stringify(en,null,2));
    await run(cli,['-p',story,'text','review','--id','intro','--locale','en']);
    await expect.poll(async()=>(await status())?.release).not.toBe(release);
    await page.waitForFunction(()=>window.__nir?.state().ready&&window.__nir.state().screen==='Title');
    await start(page);expect((await state(page)).dialogue.visible).toBe('春天的花开了。');
    await act(page,{type:'settings'});await act(page,{type:'locale',locale:'en'});await act(page,{type:'close'});
    expect((await state(page)).dialogue.locale).toBe('zh-Hans');
    await act(page,{type:'saves'});await act(page,{type:'save',slot:0});await page.waitForFunction(()=>/已保存|Saved/.test(window.__nir.state().status));
    await page.reload();await ready(page);await act(page,{type:'saves'});await act(page,{type:'load',slot:0});
    await page.waitForFunction(()=>window.__nir.state().paused&&!window.__nir.state().loading&&!!window.__nir.state().dialogue);
    expect((await state(page)).dialogue.visible).toBe('春天的花开了。');expect((await state(page)).dialogue.locale).toBe('zh-Hans');
    await act(page,{type:'title'});await act(page,{type:'new_game'});await page.waitForFunction(()=>window.__nir.state().dialogue?.ready&&!window.__nir.state().loading);
    expect((await state(page)).dialogue.visible).toBe('The spring flowers have opened.');
    expectPainted(await page.screenshot({path:'reports/text-reviewed-english.png'}));
    expect(JSON.parse((await run(cli,['-p',story,'text','status','--json'])).stdout).ready).toBe(true);
  } finally {if(proc.exitCode===null&&proc.signalCode===null){const done=new Promise(r=>proc.once('exit',r));proc.kill('SIGTERM');await done;}}
});
