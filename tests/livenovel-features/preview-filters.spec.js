import {test,expect} from '@playwright/test';
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
async function colors(page) {
  const screenshot=await page.screenshot();
  return page.evaluate(async data=>{
    const bitmap=await createImageBitmap(new Blob([Uint8Array.from(atob(data),c=>c.charCodeAt(0))],{type:'image/png'}));
    const canvas=document.createElement('canvas');canvas.width=bitmap.width;canvas.height=bitmap.height;
    const context=canvas.getContext('2d');context.drawImage(bitmap,0,0);bitmap.close();
    return [[180,180],[680,180],[420,350]].map(([x,y])=>Array.from(context.getImageData(x,y,1,1).data));
  },screenshot.toString('base64'));
}
function expectColors(actual, expected) {
  expect(actual.length).toBe(expected.length);
  // The fixture uses the real lossy WebP release path.
  actual.forEach((pixel,index)=>pixel.forEach((channel,at)=>
    expect(Math.abs(channel-expected[index][at])).toBeLessThanOrEqual(3)));
}
for(const worker of ['required','main']) {
  test(`conditional preview images and disabled hit regions survive cold restore; ${worker}`,async({page})=>{
    test.skip(process.env.NIR_TEST_PREVIEW_FILTERS!=='1','Run separate preview-filter fixture');
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
    await expect(page.getByRole('button',{name:'Stay at the station and read the letter',exact:true})).toBeDisabled();
    expectColors((await colors(page)).slice(0,2),[[255,0,255,255],[255,255,0,255]]);
    const token=await page.evaluate(()=>__nir.state().interaction);
    const starts=await page.evaluate(()=>window.fixtureAudioStarts);
    await page.mouse.move(680,180);await page.mouse.click(680,180);
    await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
    expect(await page.evaluate(()=>__nir.state().interaction)).toBe(token);
    expectColors([(await colors(page))[1]],[[255,255,0,255]]);
    expect(await page.evaluate(()=>window.fixtureAudioStarts)).toBe(starts);
    await page.evaluate(()=>{__nir.hidden(true);__nir.action({type:'menu'});});
    await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
    await page.evaluate(()=>__nir.action({type:'save',slot:1}));
    await expect.poll(async()=>Boolean(await saved(page))).toBe(true);
    const snapshot=await saved(page);
    expect(snapshot.choice.options.map(o=>[o.id,o.enabled])).toEqual([['walk',true],['stay',false]]);
    expect(snapshot.scene.find(n=>n.id==='filter.2.normal').opacity).toBe(0);
    expect(snapshot.scene.find(n=>n.id==='filter.1.disabled').opacity).toBe(1);
    await page.reload();await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    const session=await page.evaluate(()=>__nir.state().session);
    await page.evaluate(()=>__nir.action({type:'load',slot:1}));
    await page.waitForFunction(s=>__nir.state().session>s&&!__nir.state().loading&&__nir.state().paused,session);
    expect(await page.evaluate(()=>__nir.state().interaction)).not.toBe(token);
    await page.evaluate(()=>{__nir.hidden(false);__nir.action({type:'continue'});});
    await recoverAudioOutput(page);
    expectColors((await colors(page)).slice(0,2),[[255,0,255,255],[255,255,0,255]]);
    await page.locator('canvas').first().click({position:{x:180,y:180}});
    await page.waitForFunction(()=>__nir.state().outcome==='walk');
    expect(await page.evaluate(()=>__nir.state().error)).toBeNull();
    expect(errors).toEqual([]);
  });
}
