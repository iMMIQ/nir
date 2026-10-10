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
async function pixels(page) {
  const png=await page.screenshot();
  return page.evaluate(async png=>{
    const bitmap=await createImageBitmap(new Blob([Uint8Array.from(atob(png),c=>c.charCodeAt(0))],{type:'image/png'}));
    const canvas=document.createElement('canvas');canvas.width=bitmap.width;canvas.height=bitmap.height;
    const ctx=canvas.getContext('2d');ctx.drawImage(bitmap,0,0);bitmap.close();
    return [[110,410],[210,466]].map(([x,y])=>Array.from(ctx.getImageData(x,y,1,1).data).slice(0,3));
  },png.toString('base64'));
}
async function expectColors(page, portrait) {
  await expect.poll(async()=>{
    const colors=await pixels(page);
    const targets=[portrait?[255,0,255]:[0,0,0],[0,255,255]];
    return Math.max(...colors.flatMap((color,i)=>color.map((c,j)=>Math.abs(c-targets[i][j]))));
  }).toBeLessThanOrEqual(3);
}
for(const worker of ['required','main']) {
  test(`live portrait/name replacement, visibility and cold restore preserve music; ${worker}`,async({page})=>{
    test.skip(process.env.NIR_TEST_DECORATION!=='1','Run the dialogue-decoration fixture');
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
    expect(starts).toBeGreaterThan(0);
    await expectColors(page,true);
    const text=await page.evaluate(()=>__nir.state().dialogue.visible);
    expect(text).toContain('Reader');
    await page.evaluate(()=>__nir.action({type:'toggle_interface'}));
    await expect.poll(async()=>{const c=(await pixels(page))[0];return Math.max(...c);}).toBeLessThanOrEqual(3);
    await page.evaluate(()=>__nir.action({type:'toggle_interface'}));
    await expectColors(page,true);
    expect(await page.evaluate(()=>__nir.state().dialogue.visible)).toBe(text);
    expect(await page.evaluate(()=>window.fixtureAudioStarts)).toBe(starts);
    await page.evaluate(()=>__nir.action({type:'menu'}));
    await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
    await page.evaluate(()=>__nir.action({type:'save',slot:1}));
    await expect.poll(async()=>(await saved(page))?.revision||0).toBeGreaterThan(0);
    const snapshot=(await saved(page)).snapshot;
    expect(snapshot.dialogue_decorations.portrait.rect).toEqual([100,400,32,24]);
    expect(snapshot.dialogue_decorations.name.rect).toEqual([200,456,32,24]);
    const music=snapshot.handles.music;expect(snapshot.tasks[music].state).toBe('running');
    await page.reload();await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    const session=await page.evaluate(()=>__nir.state().session);
    await page.evaluate(()=>__nir.action({type:'load',slot:1}));
    await page.waitForFunction(s=>__nir.state().session>s&&!__nir.state().loading&&__nir.state().paused,session);
    await expectColors(page,true);
    await page.evaluate(()=>__nir.action({type:'continue'}));await recoverAudioOutput(page);
    const restoredStarts=await page.evaluate(()=>window.fixtureAudioStarts);
    for(let i=0;i<4;i++) {
      if(await page.evaluate(()=>__nir.state().dialogue?.id==='arrival'))break;
      await page.locator('canvas').first().click({position:{x:300,y:550}});
      await page.waitForTimeout(100);
    }
    await page.waitForFunction(()=>__nir.state().dialogue?.id==='arrival'&&!__nir.state().loading);
    await expectColors(page,false);
    expect(await page.evaluate(()=>window.fixtureAudioStarts)).toBe(restoredStarts);
    await page.evaluate(()=>__nir.action({type:'menu'}));
    await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
    await page.evaluate(()=>__nir.action({type:'save',slot:1}));
    await expect.poll(async()=>(await saved(page))?.revision||0).toBeGreaterThan(1);
    const after=(await saved(page)).snapshot;
    expect(after.handles.music).toBe(music);
    expect(Object.keys(after.dialogue_decorations)).toEqual(['name']);
    expect(await page.evaluate(()=>__nir.state().error)).toBeNull();expect(errors).toEqual([]);
  });
}
