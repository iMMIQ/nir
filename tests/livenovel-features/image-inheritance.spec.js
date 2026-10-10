import {test,expect} from '@playwright/test';
import fs from 'node:fs';
import path from 'node:path';
import {recoverAudioOutput} from '../nir-next/audio-output-helper.js';
async function saved(page) {
  return page.evaluate(async()=>{
    for(const {name} of await indexedDB.databases()) {
      const db=await new Promise((ok,no)=>{const r=indexedDB.open(name);r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});
      if(!db.objectStoreNames.contains('saves')) {db.close();continue;}
      const rows=await new Promise((ok,no)=>{const t=db.transaction('saves'),r=t.objectStore('saves').getAll();t.oncomplete=()=>ok(r.result);t.onerror=()=>no(t.error);});
      db.close();const row=rows.find(r=>r.envelope?.slot===1);if(row)return row.envelope.snapshot;
    }
    return null;
  });
}
async function pixel(page,x,y) {
  const png=await page.screenshot();
  return page.evaluate(async({png,x,y})=>{
    const bitmap=await createImageBitmap(new Blob([Uint8Array.from(atob(png),c=>c.charCodeAt(0))],{type:'image/png'}));
    const canvas=document.createElement('canvas');canvas.width=bitmap.width;canvas.height=bitmap.height;
    const context=canvas.getContext('2d');context.drawImage(bitmap,0,0);bitmap.close();
    return Array.from(context.getImageData(x,y,1,1).data);
  },{png:png.toString('base64'),x,y});
}
function color(actual,expected) {actual.forEach((v,i)=>expect(Math.abs(v-expected[i])).toBeLessThanOrEqual(3));}
function unusedImageHash() {
  const root=path.resolve('reports/livenovel-features/project/dist/full/web');
  const channel=JSON.parse(fs.readFileSync(path.join(root,'channels/stable.json')));
  const release=JSON.parse(fs.readFileSync(path.join(root,`releases/${channel.release}.json`)));
  const executable=JSON.parse(fs.readFileSync(path.join(root,release.objects[release.program].path)));
  return executable.program.assets['inherit.unused'].object;
}
for(const worker of ['required','main'])for(const incoming of ['first','second']) {
  const geometry=process.env.NIR_TEST_IMAGE_GEOMETRY==='1';
  test(`inherited ${incoming} picture${geometry?' and rectangle':''} survives shared scene and cold transition restore; ${worker}`,async({page})=>{
    test.skip(process.env.NIR_TEST_IMAGE_INHERIT!=='1'&&!geometry,'Run separate inherited-image fixture');
    const rectangle=geometry&&incoming==='second'?[400,180,120,90]:[200,100,180,150];
    const point=[rectangle[0]+rectangle[2]/2,rectangle[1]+rectangle[3]/2];
    const rect=n=>[n.x,n.y,n.width,n.height];
    const errors=[],requests=[];page.on('pageerror',e=>errors.push(e.message));page.on('request',r=>requests.push(r.url()));
    await page.addInitScript(()=>{
      window.fixtureAudioStarts=0;const start=AudioBufferSourceNode.prototype.start;
      AudioBufferSourceNode.prototype.start=function(...args){window.fixtureAudioStarts++;return start.apply(this,args);};
    });
    await page.setViewportSize({width:1280,height:720});
    await page.goto(`http://127.0.0.1:4268/?test=1&worker=${worker}&backend=webgl2`);
    await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    await page.locator('canvas').first().click({position:{x:640,y:310}});
    await page.waitForFunction(()=>__nir.state().choice&&!__nir.state().loading);await recoverAudioOutput(page);
    const starts=await page.evaluate(()=>window.fixtureAudioStarts);
    await page.locator('canvas').first().click({position:{x:incoming==='first'?180:680,y:180}});
    await page.waitForFunction(()=>__nir.state().advance_wait!==null&&__nir.state().transition&&!__nir.state().loading);
    const token=await page.evaluate(()=>__nir.state().interaction);
    await page.evaluate(()=>{__nir.hidden(true);__nir.action({type:'menu'});});
    await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
    await page.evaluate(()=>__nir.action({type:'save',slot:1}));await expect.poll(async()=>Boolean(await saved(page))).toBe(true);
    const snapshot=await saved(page),photo=snapshot.scene.find(n=>n.id==='inherit.photo');
    expect(photo.asset).toBe(`inherit.${incoming}`);
    expect(rect(photo)).toEqual(rectangle);
    const stage=snapshot.tasks[snapshot.handles.stage];expect(stage.elapsed_us).not.toBe('5000000');
    expect(stage.source.find(n=>n.id==='inherit.photo').asset).toBe(photo.asset);
    expect(stage.target.find(n=>n.id==='inherit.photo').asset).toBe(photo.asset);
    expect(rect(stage.source.find(n=>n.id==='inherit.photo'))).toEqual(rectangle);
    expect(rect(stage.target.find(n=>n.id==='inherit.photo'))).toEqual(rectangle);
    expect(await page.evaluate(()=>window.fixtureAudioStarts)).toBe(starts);
    const music=snapshot.handles.music;
    await page.reload();await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    const session=await page.evaluate(()=>__nir.state().session);await page.evaluate(()=>__nir.action({type:'load',slot:1}));
    await page.waitForFunction(s=>__nir.state().session>s&&!__nir.state().loading&&__nir.state().paused,session);
    expect(await page.evaluate(()=>__nir.state().interaction)).not.toBe(token);
    await page.evaluate(()=>{__nir.hidden(false);__nir.action({type:'continue'});});await recoverAudioOutput(page);
    color(await pixel(page,...point),incoming==='first'?[255,0,0,255]:[0,255,0,255]);
    await page.waitForFunction(()=>__nir.state().transition===null);
    color(await pixel(page,...point),incoming==='first'?[255,0,0,255]:[0,255,0,255]);
    color(await pixel(page,680,180),[255,255,0,255]);
    await page.evaluate(()=>{__nir.hidden(true);__nir.action({type:'menu'});});await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
    await page.evaluate(()=>__nir.action({type:'save',slot:1}));
    await expect.poll(async()=>{const s=await saved(page);return s?.tasks?.[s.handles.stage]?.state;}).toBe('finished');
    expect((await saved(page)).tasks[music].state).toBe('running');
    expect((await saved(page)).handles.music).toBe(music);
    expect(rect((await saved(page)).scene.find(n=>n.id==='inherit.photo'))).toEqual(rectangle);
    const unused=unusedImageHash();expect(requests.filter(url=>url.includes(`/objects/${unused}.webp`))).toEqual([]);
    expect(await page.evaluate(()=>__nir.state().error)).toBeNull();expect(errors).toEqual([]);
  });
}
