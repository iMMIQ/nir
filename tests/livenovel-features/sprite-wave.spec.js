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
async function markerBounds(page) {
  const shot=await page.screenshot();
  return page.evaluate(async data=>{
    const bitmap=await createImageBitmap(new Blob([Uint8Array.from(atob(data),c=>c.charCodeAt(0))],{type:'image/png'}));
    const canvas=document.createElement('canvas');canvas.width=bitmap.width;canvas.height=bitmap.height;
    const ctx=canvas.getContext('2d');ctx.drawImage(bitmap,0,0);bitmap.close();
    const pixels=ctx.getImageData(0,0,canvas.width,canvas.height).data;
    const bounds=[Infinity,Infinity,0,0];let count=0;
    for(let y=40;y<200;y++)for(let x=630;x<800;x++) {
      const i=(y*canvas.width+x)*4;
      if(pixels[i]>230&&pixels[i+1]<30&&pixels[i+2]>230) {
        count++;bounds[0]=Math.min(bounds[0],x);bounds[1]=Math.min(bounds[1],y);
        bounds[2]=Math.max(bounds[2],x);bounds[3]=Math.max(bounds[3],y);
      }
    }
    return {bounds,count};
  },shot.toString('base64'));
}
function offset(task) {
  const t=Number(task.elapsed_us),step=Number(task.effect.spec.step_us),index=Math.floor(t/step);
  const ms=Math.floor((t%step)/1000),duration=step/1000;
  const p=ms===0?0:ms>=duration-1?1:index%2?
    1-Math.sin((duration-ms-1)*Math.PI/2/duration):Math.sin(ms*Math.PI/2/duration);
  const from=index===0?task.shake.base:task.shake.targets[index-1];
  return from.map((value,axis)=>Math.trunc((1-p)*value+p*task.shake.targets[index][axis]));
}
for(const worker of ['required','main']) {
  test(`sprite wave paints local offsets and survives cold restore with music; ${worker}`,async({page})=>{
    test.skip(process.env.NIR_TEST_SPRITE_WAVE!=='1','Run the separate sprite-wave fixture');
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
    const wave=saved.tasks[saved.handles.sprite_wave];
    expect(wave.shake.targets).toHaveLength(40);
    expect(wave.effect.nodes).toEqual(['moving-picture','wave-marker']);
    const displacement=offset(wave);
    expect(displacement.some(value=>value!==0)).toBe(true);
    const music=saved.handles.music;
    await page.reload();await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    const session=await page.evaluate(()=>__nir.state().session);
    await page.evaluate(()=>__nir.action({type:'load',slot:1}));
    await page.waitForFunction(s=>__nir.state().session>s&&!__nir.state().loading&&__nir.state().paused,session);
    const expected=[700+displacement[0],100+displacement[1],731+displacement[0],123+displacement[1]];
    const frozen=await markerBounds(page);
    expect(frozen).toEqual({bounds:expected,count:32*24});
    const tick=await page.evaluate(()=>__nir.state().tick_us);
    await page.waitForTimeout(100);
    expect(await page.evaluate(()=>__nir.state().tick_us)).toBe(tick);
    expect(await markerBounds(page)).toEqual(frozen);
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
    expect(final.tasks[final.handles.sprite_wave].state).toBe('finished');
    expect(await page.evaluate(()=>window.fixtureAudioStarts)).toBe(starts);
    expect(await page.evaluate(()=>__nir.state().error)).toBeNull();expect(errors).toEqual([]);
  });
}
