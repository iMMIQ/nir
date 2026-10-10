import {test,expect} from '@playwright/test';
import {recoverAudioOutput} from '../nir-next/audio-output-helper.js';

async function saved(page) {
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
async function windowPixels(page) {
  const png=await page.screenshot();
  return page.evaluate(async png=>{
    const bitmap=await createImageBitmap(new Blob([Uint8Array.from(atob(png),c=>c.charCodeAt(0))],{type:'image/png'}));
    const canvas=document.createElement('canvas');canvas.width=bitmap.width;canvas.height=bitmap.height;
    const ctx=canvas.getContext('2d');ctx.drawImage(bitmap,0,0);bitmap.close();
    const color=Array.from(ctx.getImageData(755,185,1,1).data);
    const text=ctx.getImageData(770,200,260,140).data;let dark=0;
    for(let i=0;i<text.length;i+=4)if(text[i+1]<160&&text[i+2]<170)dark++;
    return {color,dark};
  },png.toString('base64'));
}
for(const worker of ['required','main']) {
  test(`prepared dialogue window changes actual background and text area, preserves music and cold restore; ${worker}`,async({page})=>{
    test.skip(process.env.NIR_TEST_DIALOGUE_STYLE!=='1','Run the separate dialogue-style fixture');
    const errors=[];page.on('pageerror',error=>errors.push(error.message));
    await page.addInitScript(()=>{
      window.fixtureAudioStarts=0;const start=AudioBufferSourceNode.prototype.start;
      AudioBufferSourceNode.prototype.start=function(...args){window.fixtureAudioStarts++;return start.apply(this,args);};
    });
    await page.setViewportSize({width:1280,height:720});
    await page.goto(`http://127.0.0.1:4268/?test=1&worker=${worker}&backend=webgl2`);
    await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    await page.locator('canvas').first().click({position:{x:640,y:310}});
    await page.waitForFunction(()=>__nir.state().dialogue?.id==='intro'&&!__nir.state().loading);
    await recoverAudioOutput(page);
    const starts=await page.evaluate(()=>window.fixtureAudioStarts);
    for(let i=0;i<4;i++) {
      if(await page.evaluate(()=>__nir.state().dialogue?.id==='arrival'))break;
      await page.locator('canvas').first().click({position:{x:300,y:550}});
      await page.waitForTimeout(100);
    }
    await page.waitForFunction(()=>__nir.state().dialogue?.id==='arrival'&&!__nir.state().loading);
    await expect.poll(async()=>{const c=(await windowPixels(page)).color;return Math.max(c[0],Math.abs(c[1]-255),Math.abs(c[2]-255));}).toBeLessThanOrEqual(3);
    expect((await windowPixels(page)).dark).toBeGreaterThan(20);
    expect(await page.evaluate(()=>window.fixtureAudioStarts)).toBe(starts);
    await page.evaluate(()=>__nir.action({type:'menu'}));
    await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
    await page.evaluate(()=>__nir.action({type:'save',slot:1}));
    await expect.poll(async()=>(await saved(page))?.revision||0).toBeGreaterThan(0);
    const snapshot=(await saved(page)).snapshot;
    expect(snapshot.dialogue_style).toBe('alternate');
    const music=snapshot.handles.music;expect(snapshot.tasks[music].state).toBe('running');
    await page.reload();await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    const session=await page.evaluate(()=>__nir.state().session);
    await page.evaluate(()=>__nir.action({type:'load',slot:1}));
    await page.waitForFunction(s=>__nir.state().session>s&&!__nir.state().loading&&__nir.state().paused,session);
    await expect.poll(async()=>{const c=(await windowPixels(page)).color;return Math.max(c[0],Math.abs(c[1]-255),Math.abs(c[2]-255));}).toBeLessThanOrEqual(3);
    expect((await windowPixels(page)).dark).toBeGreaterThan(20);
    await page.evaluate(()=>__nir.action({type:'continue'}));await recoverAudioOutput(page);
    await page.evaluate(()=>__nir.action({type:'menu'}));
    await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
    await page.evaluate(()=>__nir.action({type:'save',slot:1}));
    await expect.poll(async()=>(await saved(page))?.revision||0).toBeGreaterThan(1);
    expect((await saved(page)).snapshot.handles.music).toBe(music);
    expect((await saved(page)).snapshot.dialogue_style).toBe('alternate');
    expect(await page.evaluate(()=>__nir.state().error)).toBeNull();expect(errors).toEqual([]);
  });
}
