import {test, expect} from '@playwright/test';
import {recoverAudioOutput} from '../nir-next/audio-output-helper.js';

async function envelope(page) {
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
async function panelPixel(page) {
  const shot=await page.screenshot();
  return page.evaluate(async data=>{
    const bitmap=await createImageBitmap(new Blob([Uint8Array.from(atob(data),c=>c.charCodeAt(0))],{type:'image/png'}));
    const canvas=document.createElement('canvas');canvas.width=bitmap.width;canvas.height=bitmap.height;
    const ctx=canvas.getContext('2d');ctx.drawImage(bitmap,0,0);bitmap.close();
    return Array.from(ctx.getImageData(170,460,1,1).data);
  },shot.toString('base64'));
}
for(const worker of ['required','main']) {
  test(`reserved window flip shares the scene clock and cold restore; ${worker}`,async({page})=>{
    test.skip(process.env.NIR_TEST_WINDOW_FLIP!=='1','Run the separate reserved-window fixture');
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
    await page.evaluate(()=>__nir.hidden(true));
    await page.waitForFunction(()=>__nir.state().paused);
    expect(await page.evaluate(()=>__nir.state().window)).toBeNull();
    const visible=await panelPixel(page);
    expect(visible[0]+visible[1]+visible[2]).toBeGreaterThan(0);
    const fade=page.evaluate(()=>new Promise((resolve,reject)=>{
      const deadline=performance.now()+15000;
      __nir.hidden(false);__nir.action({type:'continue'});
      (function check(){
        const s=__nir.state(),p=s.window;
        if(p!==null&&p>.45){__nir.hidden(true);resolve();return;}
        if(performance.now()>deadline){reject(new Error(JSON.stringify({tick:s.tick_us,window:p,paused:s.paused,position:s.position,error:s.error})));return;}
        // OffscreenCanvas paints do not guarantee a main-page animation frame.
        // Poll the published Worker state without depending on DOM repaint.
        setTimeout(check,16);
      })();
    }));
    await recoverAudioOutput(page);
    await fade;
    await page.waitForFunction(()=>__nir.state().paused);
    const progress=await page.evaluate(()=>__nir.state().window);
    expect(progress).toBeGreaterThan(.45);expect(progress).toBeLessThan(.65);
    expect(await page.evaluate(()=>__nir.state().transition)).toBe(progress);
    const fading=await panelPixel(page);
    expect(fading[0]+fading[1]+fading[2]).toBeGreaterThan(0);
    expect(fading[0]+fading[1]+fading[2]).toBeLessThan(visible[0]+visible[1]+visible[2]);
    await page.evaluate(()=>__nir.action({type:'menu'}));
    await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
    await page.evaluate(()=>__nir.action({type:'save',slot:1}));
    await expect.poll(async()=>(await envelope(page))?.revision||0).toBeGreaterThan(0);
    const saved=(await envelope(page)).snapshot;
    const music=saved.handles.music;
    await page.reload();await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    const session=await page.evaluate(()=>__nir.state().session);
    await page.evaluate(()=>__nir.action({type:'load',slot:1}));
    await page.waitForFunction(s=>__nir.state().session>s&&!__nir.state().loading&&__nir.state().paused,session);
    expect(await page.evaluate(()=>__nir.state().window)).toBe(progress);
    expect(await panelPixel(page)).toEqual(fading);
    const tick=await page.evaluate(()=>__nir.state().tick_us);
    await page.waitForTimeout(150);
    expect(await page.evaluate(()=>__nir.state().tick_us)).toBe(tick);
    expect(await panelPixel(page)).toEqual(fading);
    await page.evaluate(()=>{__nir.hidden(false);__nir.action({type:'continue'});});
    await page.waitForFunction(()=>!__nir.state().paused);await recoverAudioOutput(page);
    const starts=await page.evaluate(()=>window.fixtureAudioStarts);
    await page.waitForFunction(()=>__nir.state().window===null);
    expect(await panelPixel(page)).toEqual([0,0,0,255]);
    expect(await page.evaluate(()=>window.fixtureAudioStarts)).toBe(starts);
    await page.evaluate(()=>__nir.action({type:'menu'}));
    await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
    await page.evaluate(()=>__nir.action({type:'save',slot:1}));
    await expect.poll(async()=>(await envelope(page))?.revision||0).toBeGreaterThan(1);
    const final=(await envelope(page)).snapshot;
    expect(final.handles.music).toBe(music);expect(final.tasks[music].state).toBe('running');
    expect(final.tasks[final.handles.window_flip].state).toBe('finished');
    expect(await page.evaluate(()=>__nir.state().error)).toBeNull();expect(errors).toEqual([]);
  });
}
