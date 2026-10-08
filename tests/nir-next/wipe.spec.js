import {test,expect} from '@playwright/test';

test('frozen directional wipe has distinct ends, a soft edge and paused progress',async({page})=>{
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto('http://127.0.0.1:4201/?test=1&backend=webgl2',{waitUntil:'domcontentloaded'}).catch(e=>{if(!e.message.includes('interrupted'))throw e;});
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  await page.keyboard.press('Enter');
  // Waiting for a fixed (.4,.6) sampling window can be skipped entirely when
  // one slow frame advances the wipe past it between polls. Cross a lower
  // bound instead — a crossing is always observable — and pause from inside
  // the page on that same frame, so the paused progress cannot drift past
  // the range the pixel sampling below needs (the edge between the .2 and
  // .8 sample columns).
  await page.evaluate(()=>new Promise(resolve=>{
    (function check(){
      const p=window.__nir.state().transition;
      if(p!==null&&p>.45){window.__nir.hidden(true);resolve(p);return;}
      requestAnimationFrame(check);
    })();
  }));
  await page.waitForFunction(()=>window.__nir.state().paused);
  const progress=await page.evaluate(()=>window.__nir.state().transition);
  expect(progress).toBeGreaterThan(.4);
  expect(progress).toBeLessThan(.6);
  const screenshot=await page.screenshot({path:'reports/nir-next/wipe-middle.png'});
  const pixels=await page.evaluate(async base64=>{
    const image=await createImageBitmap(new Blob([Uint8Array.from(atob(base64),c=>c.charCodeAt(0))],{type:'image/png'}));
    const canvas=document.createElement('canvas');canvas.width=image.width;canvas.height=image.height;
    const ctx=canvas.getContext('2d');ctx.drawImage(image,0,0);image.close();
    return [.2,.5,.8].map(x=>Array.from(ctx.getImageData(Math.floor(canvas.width*x),Math.floor(canvas.height*.3),1,1).data));
  },screenshot.toString('base64'));
  expect(pixels[0][2]).toBeGreaterThan(240);expect(pixels[0][0]).toBeLessThan(15);
  expect(pixels[2][0]).toBeGreaterThan(240);expect(pixels[2][2]).toBeLessThan(15);
  const softness=.2,threshold=progress*(1+softness)-softness/2;
  const t=Math.max(0,Math.min(1,(.5-threshold+softness/2)/softness));
  const coverage=1-t*t*(3-2*t);
  const srgb=v=>255*(v<=.0031308?v*12.92:1.055*Math.pow(v,1/2.4)-.055);
  expect(Math.abs(pixels[1][0]-srgb(1-coverage))).toBeLessThan(15);
  expect(Math.abs(pixels[1][2]-srgb(coverage))).toBeLessThan(15);
  await page.waitForTimeout(250);
  expect(await page.evaluate(()=>window.__nir.state().transition)).toBe(progress);
  await page.evaluate(()=>window.__nir.action({type:'save',slot:2}));
  await page.waitForFunction(()=>/Saved|已保存/.test(window.__nir.state().status));
  const session=await page.evaluate(()=>window.__nir.state().session);
  await page.evaluate(()=>window.__nir.action({type:'load',slot:2}));
  await page.waitForFunction(session=>window.__nir.state().session>session&&!window.__nir.state().loading,session);
  expect(await page.evaluate(()=>window.__nir.state().transition)).toBe(progress);
  await page.evaluate(()=>{window.__nir.hidden(false);window.__nir.action({type:'continue'});});
  await page.waitForFunction(()=>window.__nir.state().transition===null);
  expect(errors).toEqual([]);
});
