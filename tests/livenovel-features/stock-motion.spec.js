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
async function markerBounds(page, base) {
  const shot=await page.screenshot();
  return page.evaluate(async ({data,base})=>{
    const bitmap=await createImageBitmap(new Blob([Uint8Array.from(atob(data),c=>c.charCodeAt(0))],{type:'image/png'}));
    const canvas=document.createElement('canvas');canvas.width=bitmap.width;canvas.height=bitmap.height;
    const ctx=canvas.getContext('2d');ctx.drawImage(bitmap,0,0);bitmap.close();
    const pixels=ctx.getImageData(0,0,canvas.width,canvas.height).data;
    const bounds=[Infinity,Infinity,0,0];let count=0;
    for(let y=40;y<200;y++)for(let x=base-70;x<base+100;x++) {
      const i=(y*canvas.width+x)*4;
      if(pixels[i]>230&&pixels[i+1]<30&&pixels[i+2]>230) {
        count++;bounds[0]=Math.min(bounds[0],x);bounds[1]=Math.min(bounds[1],y);
        bounds[2]=Math.max(bounds[2],x);bounds[3]=Math.max(bounds[3],y);
      }
    }
    return {bounds,count};
  },{data:shot.toString('base64'),base});
}
function position(task) {
  const t=Math.floor(Number(task.elapsed_us)/1000), duration=Number(task.effect.duration_us)/1000;
  if(t===0)return task.captured;
  if(t>=duration-1)return task.effect.to;
  const p=task.effect.curve==='inc'?1-Math.sin((duration-t-1)*Math.PI/2/duration):Math.sin(t*Math.PI/2/duration);
  return Math.trunc((1-p)*task.captured+p*task.effect.to);
}
for(const worker of ['required','main']) {
  test(`stock INC/DEC motion keeps integer positions across overlay and survives cold restore with music; ${worker}`,async({page})=>{
    test.skip(process.env.NIR_TEST_STOCK_MOTION!=='1','Run the separate stock-motion fixture');
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
    const tasks=['source_inc','source_dec'].map(id=>saved.tasks[saved.handles[id]]);
    expect(tasks.map(t=>t.effect.curve)).toEqual(['inc','dec']);
    expect(tasks.map(t=>t.captured)).toEqual([700,950]);
    expect(tasks.every(t=>t.state==='running')).toBe(true);
    const positions=tasks.map(position);
    const music=saved.handles.music;
    await page.reload();await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    const session=await page.evaluate(()=>__nir.state().session);
    await page.evaluate(()=>__nir.action({type:'load',slot:1}));
    await page.waitForFunction(s=>__nir.state().session>s&&!__nir.state().loading&&__nir.state().paused,session);
    const frozen=[];
    for(const [index,base] of [700,950].entries()) {
      const expected=[positions[index],100,positions[index]+31,123];
      frozen.push(await markerBounds(page,base));
      expect(frozen[index]).toEqual({bounds:expected,count:32*24});
    }
    const tick=await page.evaluate(()=>__nir.state().tick_us);
    await page.waitForTimeout(100);
    expect(await page.evaluate(()=>__nir.state().tick_us)).toBe(tick);
    for(const [index,base] of [700,950].entries()) expect(await markerBounds(page,base)).toEqual(frozen[index]);
    await page.evaluate(()=>__nir.action({type:'continue'}));
    await page.waitForFunction(()=>!__nir.state().paused);await recoverAudioOutput(page);
    const starts=await page.evaluate(()=>window.fixtureAudioStarts);
    await page.waitForFunction(()=>Number(__nir.state().tick_us)>=10500000);
    await page.evaluate(()=>__nir.action({type:'menu'}));
    await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
    await page.evaluate(()=>__nir.action({type:'save',slot:1}));
    await expect.poll(async()=>(await saveEnvelope(page))?.revision||0).toBeGreaterThan(1);
    const final=(await saveEnvelope(page)).snapshot;
    expect(final.handles.music).toBe(music);
    expect(final.tasks[music].state).toBe('running');
    for(const id of ['source_inc','source_dec']) expect(final.tasks[final.handles[id]].state).toBe('finished');
    expect(await page.evaluate(()=>window.fixtureAudioStarts)).toBe(starts);
    await page.evaluate(()=>__nir.action({type:'close'}));
    await page.waitForFunction(()=>__nir.state().screen==='Story');
    await recoverAudioOutput(page);
    for(const base of [700,950]) await expect.poll(()=>markerBounds(page,base)).toEqual({bounds:[base+50,100,base+81,123],count:32*24});
    expect(await page.evaluate(()=>__nir.state().error)).toBeNull();expect(errors).toEqual([]);
  });
}
