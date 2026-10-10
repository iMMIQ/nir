import {test,expect} from '@playwright/test';
import {recoverAudioOutput} from '../nir-next/audio-output-helper.js';

async function saveEnvelope(page) {
  return page.evaluate(async()=>{
    for(const {name} of await indexedDB.databases()) {
      const db=await new Promise((ok,no)=>{const r=indexedDB.open(name);r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});
      if(!db.objectStoreNames.contains('saves')) {db.close();continue;}
      const rows=await new Promise((ok,no)=>{const t=db.transaction('saves'),r=t.objectStore('saves').getAll();t.oncomplete=()=>ok(r.result);t.onerror=()=>no(t.error);});
      db.close();const row=rows.find(r=>r.envelope?.slot===1);if(row)return row.envelope;
    }
    return null;
  });
}
async function closePage(page) {
  const button=page.getByRole('button',{name:/^(close|back to story|back)$/i});
  await expect(button).toHaveCount(1);
  const rect=JSON.parse(await button.getAttribute('data-rect'));
  await page.locator('canvas').first().click({position:{x:rect[0]+rect[2]/2,y:rect[1]+rect[3]/2}});
}
for(const worker of ['required','main']) test(`story load and gallery resume, cold gallery restoration; ${worker}`,async({page})=>{
  test.skip(process.env.NIR_TEST_STORY_MODAL!=='1','Separate story-modal fixture');
  const errors=[];page.on('pageerror',error=>errors.push(error.message));
  await page.addInitScript(()=>{
    window.fixtureAudioStarts=0;const start=AudioBufferSourceNode.prototype.start;
    AudioBufferSourceNode.prototype.start=function(...args){window.fixtureAudioStarts++;return start.apply(this,args);};
  });
  await page.setViewportSize({width:1280,height:720});
  await page.goto(`http://127.0.0.1:4268/?test=1&worker=${worker}&backend=webgl2`);
  await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
  await page.locator('canvas').first().click({position:{x:640,y:310}});
  await page.waitForFunction(()=>__nir.state().screen==='Saves'&&!__nir.state().loading);
  await recoverAudioOutput(page);
  const starts=await page.evaluate(()=>window.fixtureAudioStarts);
  const saveButtons=page.getByRole('button',{name:/^save$/i});
  await expect(saveButtons).toHaveCount(3);
  for(let i=0;i<3;i++) await expect(saveButtons.nth(i)).toBeDisabled();
  await closePage(page);
  await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
  expect(await page.evaluate(()=>window.fixtureAudioStarts)).toBe(starts);
  await page.evaluate(()=>__nir.action({type:'save',slot:1}));
  await expect.poll(async()=>(await saveEnvelope(page))?.revision||0).toBeGreaterThan(0);
  const saved=await saveEnvelope(page),music=saved.snapshot.handles.music;
  const task=saved.snapshot.handles['modal.gallery'];
  expect(saved.snapshot.tasks[task].state).toBe('running');
  expect(saved.snapshot.tasks[task].modal_interaction).toBeGreaterThan(0);
  await page.reload();await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
  const session=await page.evaluate(()=>__nir.state().session);
  await page.evaluate(()=>__nir.action({type:'load',slot:1}));
  await page.waitForFunction(s=>__nir.state().session>s&&__nir.state().screen==='Menu'&&!__nir.state().loading,session);
  await page.evaluate(()=>__nir.action({type:'continue'}));await recoverAudioOutput(page);
  const resumedStarts=await page.evaluate(()=>window.fixtureAudioStarts);
  await closePage(page);
  await page.waitForFunction(()=>__nir.state().screen==='Story'&&__nir.state().dialogue?.id==='arrival'&&!__nir.state().loading,null,{timeout:15000});
  expect(await page.evaluate(()=>window.fixtureAudioStarts)).toBe(resumedStarts);
  await page.evaluate(()=>__nir.action({type:'menu'}));
  await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
  await page.evaluate(()=>__nir.action({type:'save',slot:1}));
  await expect.poll(async()=>(await saveEnvelope(page))?.revision||0).toBeGreaterThan(1);
  const after=(await saveEnvelope(page)).snapshot;
  expect(after.handles.music).toBe(music);expect(after.tasks[music].state).toBe('running');
  expect(after.tasks[task].state).toBe('finished');
  expect(await page.evaluate(()=>__nir.state().error)).toBeNull();expect(errors).toEqual([]);
});
