import {test,expect} from '@playwright/test';
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
async function pixels(page) {
  const shot=await page.screenshot();
  return page.evaluate(async data=>{
    const bitmap=await createImageBitmap(new Blob([Uint8Array.from(atob(data),c=>c.charCodeAt(0))],{type:'image/png'}));
    const canvas=document.createElement('canvas');canvas.width=bitmap.width;canvas.height=bitmap.height;
    const ctx=canvas.getContext('2d');ctx.drawImage(bitmap,0,0);bitmap.close();
    return [[710,110],[770,110],[830,110]].map(([x,y])=>Array.from(ctx.getImageData(x,y,1,1).data));
  },shot.toString('base64'));
}
for(const worker of ['required','main']) {
  test(`finished movie disappears across later pages and cold restore; ${worker}`,async({page})=>{
    test.skip(process.env.NIR_TEST_SPRITE_LIFECYCLE!=='1','Run the separate timeline fixture');
    const errors=[];page.on('pageerror',e=>errors.push(e.message));
    const requests=[];page.on('request',r=>{if(r.url().includes('/objects/'))requests.push(r.url());});
    await page.addInitScript(()=>{
      window.fixtureAudioStarts=0;const start=AudioBufferSourceNode.prototype.start;
      AudioBufferSourceNode.prototype.start=function(...args){window.fixtureAudioStarts++;return start.apply(this,args);};
    });
    await page.setViewportSize({width:1280,height:720});
    await page.goto(`http://127.0.0.1:4268/?test=1&worker=${worker}&backend=webgl2`);
    await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    await page.getByRole('button',{name:'Start',exact:true}).focus();await page.keyboard.press('Enter');
    await page.waitForFunction(()=>__nir.state().dialogue?.id==='intro'&&!__nir.state().loading);await recoverAudioOutput(page);
    await page.evaluate(()=>new Promise((resolve,reject)=>{
      const deadline=performance.now()+15000;
      (function check(){const s=__nir.state();
        if(s.transition!==null&&s.transition>.45){__nir.hidden(true);resolve();return;}
        if(performance.now()>deadline){reject(new Error('Timeline did not reach scene fade'));return;}
        setTimeout(check,16);
      })();
    }));
    await page.waitForFunction(()=>__nir.state().paused);
    const frozen=await pixels(page);
    expect(frozen[0]).toEqual([0,0,0,255]);expect(frozen[2]).toEqual([0,0,0,255]);
    expect(frozen[1].slice(0,2)).toEqual([0,0]);expect(frozen[1][2]).toBeGreaterThan(245);expect(frozen[1][3]).toBe(255);
    await page.evaluate(()=>__nir.action({type:'menu'}));
    await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
    await page.evaluate(()=>__nir.action({type:'save',slot:1}));
    await expect.poll(async()=>(await envelope(page))?.revision||0).toBeGreaterThan(0);
    const saved=(await envelope(page)).snapshot;
    const movie=saved.tasks[saved.handles.movie];expect(movie.effect.timeline).toBe('movie.frames');
    expect(Number(movie.elapsed_us)).toBeGreaterThan(2400000);expect(Number(movie.elapsed_us)).toBeLessThan(2900000);
    expect(JSON.stringify(saved)).not.toContain('"tracks"');
    const music=saved.handles.music;
    await page.reload();await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    const session=await page.evaluate(()=>__nir.state().session);
    await page.evaluate(()=>__nir.action({type:'load',slot:1}));
    await page.waitForFunction(s=>__nir.state().session>s&&!__nir.state().loading&&__nir.state().paused,session);
    expect(await pixels(page)).toEqual(frozen);
    const tick=await page.evaluate(()=>__nir.state().tick_us);await page.waitForTimeout(150);
    expect(await page.evaluate(()=>__nir.state().tick_us)).toBe(tick);
    await page.evaluate(()=>{__nir.hidden(false);__nir.action({type:'continue'});});
    await page.waitForFunction(()=>!__nir.state().paused);await recoverAudioOutput(page);
    const starts=await page.evaluate(()=>window.fixtureAudioStarts);const fetched=requests.length;
    await page.waitForFunction(()=>Number(__nir.state().tick_us)>5300000);
    const finalPixels=await pixels(page);expect(finalPixels.slice(0,2)).toEqual([[0,0,0,255],[0,0,0,255]]);
    expect(finalPixels[2]).toEqual([0,0,0,255]);
    expect(await page.evaluate(()=>window.fixtureAudioStarts)).toBe(starts);expect(requests.length).toBe(fetched);
    await page.evaluate(()=>__nir.action({type:'menu'}));
    await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
    await page.evaluate(()=>__nir.action({type:'save',slot:1}));
    await expect.poll(async()=>(await envelope(page))?.revision||0).toBeGreaterThan(1);
    const done=(await envelope(page)).snapshot;expect(done.handles.music).toBe(music);
    expect(done.tasks[music].state).toBe('running');expect(done.tasks[done.handles.movie].state).toBe('finished');
    expect(done.scene.some(node=>['movie-root','movie-picture'].includes(node.id))).toBe(false);
    await page.reload();await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    const coldSession=await page.evaluate(()=>__nir.state().session);
    await page.evaluate(()=>__nir.action({type:'load',slot:1}));
    await page.waitForFunction(s=>__nir.state().session>s&&!__nir.state().loading&&__nir.state().paused,coldSession);
    expect(await pixels(page)).toEqual([[0,0,0,255],[0,0,0,255],[0,0,0,255]]);
    expect(await page.evaluate(()=>__nir.state().error)).toBeNull();expect(errors).toEqual([]);
  });
}
