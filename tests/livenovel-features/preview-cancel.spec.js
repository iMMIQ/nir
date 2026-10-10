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
async function pixel(page) {
  const png=await page.screenshot();
  return page.evaluate(async png=>{
    const bitmap=await createImageBitmap(new Blob([Uint8Array.from(atob(png),c=>c.charCodeAt(0))],{type:'image/png'}));
    const canvas=document.createElement('canvas');canvas.width=bitmap.width;canvas.height=bitmap.height;
    const ctx=canvas.getContext('2d');ctx.drawImage(bitmap,0,0);bitmap.close();
    return Array.from(ctx.getImageData(180,180,1,1).data);
  },png.toString('base64'));
}
for(const worker of ['required','main']) for(const cancel of [false,true]) {
  test(`picture menu survives cold restore, ${cancel?'cancels cleanly':'retains selected image'}, music continues; ${worker}`,async({page})=>{
    test.skip(process.env.NIR_TEST_PREVIEW_CANCEL!=='1','Run the separate preview-cancel fixture');
    const errors=[];page.on('pageerror',error=>errors.push(error.message));
    await page.addInitScript(()=>{
      window.fixtureAudioStarts=0;const start=AudioBufferSourceNode.prototype.start;
      AudioBufferSourceNode.prototype.start=function(...args){window.fixtureAudioStarts++;return start.apply(this,args);};
    });
    await page.setViewportSize({width:1280,height:720});
    await page.goto(`http://127.0.0.1:4268/?test=1&worker=${worker}&backend=webgl2`);
    await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    await page.locator('canvas').first().click({position:{x:640,y:310}});
    await page.waitForFunction(()=>__nir.state().choice&&!__nir.state().loading);
    await recoverAudioOutput(page);
    await expect(page.getByRole('button',{name:'cancel',exact:true})).toHaveCount(1);
    const token=await page.evaluate(()=>__nir.state().interaction);
    await page.evaluate(()=>__nir.action({type:'menu'}));
    await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
    await page.evaluate(()=>__nir.action({type:'save',slot:1}));
    await expect.poll(async()=>(await saved(page))?.revision||0).toBeGreaterThan(0);
    const music=(await saved(page)).snapshot.handles.music;
    await page.reload();await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    const session=await page.evaluate(()=>__nir.state().session);
    await page.evaluate(()=>__nir.action({type:'load',slot:1}));
    await page.waitForFunction(s=>__nir.state().session>s&&!__nir.state().loading&&__nir.state().paused,session);
    expect(await page.evaluate(()=>__nir.state().interaction)).not.toBe(token);
    await page.evaluate(()=>__nir.action({type:'continue'}));await recoverAudioOutput(page);
    const starts=await page.evaluate(()=>window.fixtureAudioStarts);
    if(cancel) {
      await page.locator('canvas').first().click(worker==='required'
        ?{position:{x:300,y:300},button:'right'}:{position:{x:640,y:630}});
    } else await page.locator('canvas').first().click({position:{x:180,y:180}});
    await page.waitForFunction(()=>__nir.state().dialogue?.id==='arrival'&&!__nir.state().loading);
    const expected=cancel?[0,0,0]:[0,255,255];
    await expect.poll(async()=>{const c=await pixel(page);return Math.max(...expected.map((v,i)=>Math.abs(c[i]-v)));}).toBeLessThanOrEqual(3);
    expect(await page.evaluate(()=>window.fixtureAudioStarts)).toBe(starts);
    await page.evaluate(()=>__nir.action({type:'menu'}));
    await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
    await page.evaluate(()=>__nir.action({type:'save',slot:1}));
    await expect.poll(async()=>(await saved(page))?.revision||0).toBeGreaterThan(1);
    const snapshot=(await saved(page)).snapshot;
    expect(snapshot.handles.music).toBe(music);
    expect(snapshot.tasks[music].state).toBe('running');
    expect(snapshot.variables.preview_value.value).toBe(cancel?'':'walk');
    expect(snapshot.scene.some(n=>n.id==='preview.root')).toBe(!cancel);
    expect(await page.evaluate(()=>__nir.state().error)).toBeNull();expect(errors).toEqual([]);
  });
}
