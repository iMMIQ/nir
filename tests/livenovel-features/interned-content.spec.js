import {test,expect} from '@playwright/test';
import fs from 'node:fs';
import path from 'node:path';
import {recoverAudioOutput} from '../nir-next/audio-output-helper.js';

async function snapshot(page) {
  return page.evaluate(async()=>{
    for(const {name} of await indexedDB.databases()) {
      const db=await new Promise((ok,no)=>{const r=indexedDB.open(name);r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});
      if(!db.objectStoreNames.contains('saves')) {db.close();continue;}
      const rows=await new Promise((ok,no)=>{const t=db.transaction('saves'),r=t.objectStore('saves').getAll();t.oncomplete=()=>ok(r.result);t.onerror=()=>no(t.error);});
      db.close();const row=rows.find(r=>r.envelope?.slot===1);if(row)return row.envelope.snapshot;
    }
    return null;
  });
}
function encodedPackages() {
  const dir=path.resolve('reports/livenovel-features/project/dist/full/web');
  const channel=JSON.parse(fs.readFileSync(path.join(dir,'channels/stable.json')));
  const release=JSON.parse(fs.readFileSync(path.join(dir,`releases/${channel.release}.json`)));
  const root=JSON.parse(fs.readFileSync(path.join(dir,release.objects[release.program].path))).program;
  expect(root.requires).toContain('content.interned-json.v1');
  const reader=root.function_index.reader||root.function_index['ch01.reader'];
  const scope=reader.execution_module||reader.module;
  const keys=[root.modules[scope].static_content,root.modules[reader.module].code];
  for(const hash of keys)expect(JSON.parse(fs.readFileSync(path.join(dir,release.objects[hash].path))).package_encoding).toBe('interned_json_v1');
  return keys;
}

for(const worker of ['required','main'])test(`interned packages keep music, reading and cold restore; ${worker}`,async({page})=>{
  test.skip(process.env.NIR_TEST_INTERNED!=='1','Run separate interned package fixture');
  const hashes=encodedPackages(),requests=[],errors=[];
  page.on('request',request=>requests.push(request.url()));page.on('pageerror',error=>errors.push(error.message));
  await page.addInitScript(()=>{
    window.fixtureAudioStarts=0;const start=AudioBufferSourceNode.prototype.start;
    AudioBufferSourceNode.prototype.start=function(...args){window.fixtureAudioStarts++;return start.apply(this,args);};
  });
  await page.setViewportSize({width:1280,height:720});
  await page.goto(`http://127.0.0.1:4268/?test=1&worker=${worker}&backend=webgl2`);
  await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
  await page.getByRole('button',{name:'Start',exact:true}).focus();await page.keyboard.press('Enter');
  await page.waitForFunction(()=>__nir.state().dialogue?.id==='intro'&&__nir.state().dialogue.ready&&!__nir.state().loading);
  await recoverAudioOutput(page);
  const starts=await page.evaluate(()=>window.fixtureAudioStarts);expect(starts).toBeGreaterThan(0);
  await page.keyboard.press('Space');await page.waitForFunction(()=>__nir.state().dialogue.visible.endsWith(' continues.'));
  await page.keyboard.press('Space');await page.waitForFunction(()=>__nir.state().dialogue?.id==='arrival'&&!__nir.state().loading);
  expect(await page.evaluate(()=>window.fixtureAudioStarts)).toBe(starts);
  await page.evaluate(()=>{__nir.hidden(true);__nir.action({type:'menu'});});
  await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
  await page.evaluate(()=>__nir.action({type:'save',slot:1}));
  await expect.poll(async()=>Boolean(await snapshot(page))).toBe(true);
  const saved=await snapshot(page),music=saved.handles.music;
  expect(saved.tasks[music].state).toBe('running');expect(saved.variables.fixture_payload.value).toBe('x'.repeat(1024));
  expect(await page.evaluate(()=>window.fixtureAudioStarts)).toBe(starts);
  await page.reload();await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
  await page.evaluate(()=>__nir.action({type:'load',slot:1}));
  await page.waitForFunction(()=>__nir.state().dialogue?.id==='arrival'&&!__nir.state().loading&&__nir.state().paused);
  await page.evaluate(()=>{__nir.hidden(false);__nir.action({type:'continue'});});await recoverAudioOutput(page);
  await page.waitForFunction(()=>__nir.state().screen==='Story'&&!__nir.state().loading);
  await page.evaluate(()=>{__nir.hidden(true);__nir.action({type:'menu'});});
  await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
  await page.evaluate(()=>__nir.action({type:'save',slot:1}));
  await expect.poll(async()=>Boolean((await snapshot(page))?.handles.music===music)).toBe(true);
  const restored=await snapshot(page);expect(restored.tasks[music].state).toBe('running');
  expect(restored.variables.fixture_payload.value).toBe('x'.repeat(1024));
  for(const hash of hashes)expect(requests.some(url=>url.includes(`/objects/${hash}.json`))).toBe(true);
  expect(await page.evaluate(()=>__nir.state().error)).toBeNull();expect(errors).toEqual([]);
});
