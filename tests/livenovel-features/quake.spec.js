import {test, expect} from '@playwright/test';
import {recoverAudioOutput} from '../nir-next/audio-output-helper.js';

async function stored(page, slot) {
  return page.evaluate(async slot => {
    for (const {name} of await indexedDB.databases()) {
      const db = await new Promise((ok,no) => {const r=indexedDB.open(name);r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});
      if (!db.objectStoreNames.contains('saves')) {db.close();continue;}
      const rows = await new Promise((ok,no) => {const t=db.transaction('saves'),r=t.objectStore('saves').getAll();t.oncomplete=()=>ok(r.result);t.onerror=()=>no(t.error);});
      db.close();const row=rows.find(r=>r.envelope?.slot===slot);if(row)return row.envelope;
    }
    return null;
  },slot);
}
async function imageState(page) {
  const image=await page.screenshot();
  return page.evaluate(async base64 => {
    const bitmap=await createImageBitmap(new Blob([Uint8Array.from(atob(base64),c=>c.charCodeAt(0))],{type:'image/png'}));
    const canvas=document.createElement('canvas');canvas.width=bitmap.width;canvas.height=bitmap.height;
    const ctx=canvas.getContext('2d');ctx.drawImage(bitmap,0,0);bitmap.close();
    const data=ctx.getImageData(0,0,canvas.width,canvas.height).data;
    const bounds=[Infinity,Infinity,0,0];let count=0;
    for(let y=440;y<650;y++)for(let x=180;x<1100;x++){
      const i=(y*canvas.width+x)*4;
      if(data[i]>230&&data[i+1]<30&&data[i+2]>230){count++;bounds[0]=Math.min(bounds[0],x);bounds[1]=Math.min(bounds[1],y);bounds[2]=Math.max(bounds[2],x);bounds[3]=Math.max(bounds[3],y);}
    }
    return {bounds,count,edge:Array.from(ctx.getImageData(170,450,10,195).data)};
  },image.toString('base64'));
}
for(const worker of ['required','main']) {
  test(`dialogue quake paints frozen offsets after cold restore; ${worker}`,async({page})=>{
    test.skip(process.env.NIR_TEST_QUAKE!=='1','Run the separate finite-quake fixture');
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
    await page.waitForFunction(()=>Math.abs(__nir.state().dialogue_appearance.text_offset?.[0]||0)>=4);
    await page.evaluate(()=>__nir.action({type:'menu'}));
    await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
    const offset=await page.evaluate(()=>__nir.state().dialogue_appearance.text_offset);
    expect(offset.some(v=>v!==0)).toBe(true);
    await page.evaluate(()=>__nir.action({type:'save',slot:1}));
    await expect.poll(async()=>(await stored(page,1))?.revision||0).toBeGreaterThan(0);
    const snapshot=(await stored(page,1)).snapshot;
    expect(snapshot.tasks[snapshot.handles.quake].shake.targets).toHaveLength(40);
    await page.reload();await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    const session=await page.evaluate(()=>__nir.state().session);
    await page.evaluate(()=>__nir.action({type:'load',slot:1}));
    await page.waitForFunction(s=>__nir.state().session>s&&!__nir.state().loading&&__nir.state().paused,session);
    expect(await page.evaluate(()=>__nir.state().dialogue_appearance.text_offset)).toEqual(offset);
    const frozen=await imageState(page);
    const tick=await page.evaluate(()=>__nir.state().tick_us);
    await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
    expect(await page.evaluate(()=>__nir.state().tick_us)).toBe(tick);
    expect(await imageState(page)).toEqual(frozen);
    await page.evaluate(()=>__nir.action({type:'continue'}));
    await page.waitForFunction(()=>!__nir.state().paused);
    await recoverAudioOutput(page);
    const starts=await page.evaluate(()=>window.fixtureAudioStarts);
    await page.waitForFunction(()=>(__nir.state().dialogue_appearance.text_offset||[0,0]).every(v=>v===0)&&Number(__nir.state().tick_us)>=10000000);
    const stationary=await imageState(page);
    expect(stationary.count).toBe(32*24);
    // Ruby reserves 0.6 of the authored 23px font above the body. The
    // WebGL scissor starts at its integer pixel boundary.
    const clipped=[Math.max(200,stationary.bounds[0]+offset[0]),Math.max(Math.floor(490+23*.6),stationary.bounds[1]+offset[1]),
      Math.min(1079,stationary.bounds[2]+offset[0]),Math.min(609,stationary.bounds[3]+offset[1])];
    if(clipped[2]>=clipped[0]&&clipped[3]>=clipped[1]) {
      expect(frozen.bounds).toEqual(clipped);
      expect(frozen.count).toBe((clipped[2]-clipped[0]+1)*(clipped[3]-clipped[1]+1));
    } else {
      expect(frozen.count).toBe(0);
      expect(frozen.bounds).toEqual([Infinity,Infinity,0,0]);
    }
    expect(stationary.edge).toEqual(frozen.edge);
    expect(await page.evaluate(()=>window.fixtureAudioStarts)).toBe(starts);
    expect(await page.evaluate(()=>__nir.state().error)).toBeNull();expect(errors).toEqual([]);
  });
}
