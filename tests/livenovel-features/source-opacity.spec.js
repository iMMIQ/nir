import {test, expect} from '@playwright/test';
import {recoverAudioOutput} from '../nir-next/audio-output-helper.js';

async function saveEnvelope(page) {
  return page.evaluate(async()=>{
    for(const {name} of await indexedDB.databases()) {
      const db=await new Promise((resolve,reject)=>{const r=indexedDB.open(name);r.onsuccess=()=>resolve(r.result);r.onerror=()=>reject(r.error);});
      if(!db.objectStoreNames.contains('saves')) {db.close();continue;}
      const rows=await new Promise((resolve,reject)=>{const t=db.transaction('saves'),r=t.objectStore('saves').getAll();t.oncomplete=()=>resolve(r.result);t.onerror=()=>reject(t.error);});
      db.close();const row=rows.find(r=>r.envelope?.slot===1);if(row)return row.envelope;
    }
    return null;
  });
}
async function alphaPixel(page) {
  const shot=await page.screenshot();
  return page.evaluate(async data=>{
    const bitmap=await createImageBitmap(new Blob([Uint8Array.from(atob(data),c=>c.charCodeAt(0))],{type:'image/png'}));
    const canvas=document.createElement('canvas');canvas.width=bitmap.width;canvas.height=bitmap.height;
    const ctx=canvas.getContext('2d');ctx.drawImage(bitmap,0,0);bitmap.close();
    return [...ctx.getImageData(715,112,1,1).data];
  },shot.toString('base64'));
}
async function expectAlpha(page, value) {
  // Rendering blends in linear light, then encodes the canvas as sRGB.
  const linear=value/255;
  const expected=Math.round(255*(linear<=0.0031308?12.92*linear:1.055*Math.pow(linear,1/2.4)-0.055));
  await expect.poll(async()=>{
    const [r,g,b]=await alphaPixel(page);
    return Math.max(Math.abs(r-expected),g,Math.abs(b-expected));
  }).toBeLessThanOrEqual(3);
}
function alpha(task) {
  const t=Math.floor(Number(task.elapsed_us)/1000),duration=Number(task.effect.duration_us)/1000;
  if(t===0)return Math.round(task.captured*255);
  if(t>=duration-1)return task.effect.to;
  return Math.trunc((1-t/duration)*Math.round(task.captured*255)+t/duration*task.effect.to);
}
for(const worker of ['required','main']) {
  test(`source byte opacity keeps actual alpha across overlay and survives cold restore with music; ${worker}`,async({page})=>{
    test.skip(process.env.NIR_TEST_SOURCE_OPACITY!=='1','Run the separate source-opacity fixture');
    const errors=[];page.on('pageerror',error=>errors.push(error.message));
    await page.addInitScript(()=>{
      window.fixtureAudioStarts=0;const start=AudioBufferSourceNode.prototype.start;
      AudioBufferSourceNode.prototype.start=function(...args){window.fixtureAudioStarts++;return start.apply(this,args);};
    });
    await page.setViewportSize({width:1280,height:720});
    await page.goto(`http://127.0.0.1:4268/?test=1&worker=${worker}&backend=webgl2`);
    await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    await page.getByRole('button',{name:'Start',exact:true}).focus();await page.keyboard.press('Enter');
    await page.waitForFunction(()=>__nir.state().dialogue?.id==='intro'&&!__nir.state().loading);
    await recoverAudioOutput(page);
    await page.waitForTimeout(100);
    await page.evaluate(()=>__nir.action({type:'menu'}));
    await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
    await page.evaluate(()=>__nir.action({type:'save',slot:1}));
    await expect.poll(async()=>(await saveEnvelope(page))?.revision||0).toBeGreaterThan(0);
    const saved=(await saveEnvelope(page)).snapshot;
    const task=saved.tasks[saved.handles.source_opacity];
    expect(task.effect.curve).toBe('opacity_linear');expect(task.captured).toBe(1);
    expect(task.state).toBe('running');const expected=alpha(task);
    const music=saved.handles.music;
    await page.reload();await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    const session=await page.evaluate(()=>__nir.state().session);
    await page.evaluate(()=>__nir.action({type:'load',slot:1}));
    await page.waitForFunction(s=>__nir.state().session>s&&!__nir.state().loading&&__nir.state().paused,session);
    await expectAlpha(page,expected);
    const tick=await page.evaluate(()=>__nir.state().tick_us);
    await page.waitForTimeout(100);
    expect(await page.evaluate(()=>__nir.state().tick_us)).toBe(tick);
    await expectAlpha(page,expected);
    await page.evaluate(()=>__nir.action({type:'continue'}));
    await page.waitForFunction(()=>!__nir.state().paused);await recoverAudioOutput(page);
    const starts=await page.evaluate(()=>window.fixtureAudioStarts);
    await page.waitForFunction(()=>Number(__nir.state().tick_us)>=8500000);
    await page.evaluate(()=>__nir.action({type:'menu'}));
    await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
    await page.evaluate(()=>__nir.action({type:'save',slot:1}));
    await expect.poll(async()=>(await saveEnvelope(page))?.revision||0).toBeGreaterThan(1);
    const final=(await saveEnvelope(page)).snapshot;
    expect(final.handles.music).toBe(music);
    expect(final.tasks[music].state).toBe('running');
    expect(final.tasks[final.handles.source_opacity].state).toBe('finished');
    expect(await page.evaluate(()=>window.fixtureAudioStarts)).toBe(starts);
    await page.evaluate(()=>__nir.action({type:'close'}));
    await page.waitForFunction(()=>__nir.state().screen==='Story');
    await recoverAudioOutput(page);
    await expectAlpha(page,64);
    expect(await page.evaluate(()=>__nir.state().error)).toBeNull();expect(errors).toEqual([]);
  });
}
