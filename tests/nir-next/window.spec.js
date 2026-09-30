import {test,expect} from '@playwright/test';

test('window wipe reveal erases from the leading edge and paused progress survives a save',async({page})=>{
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto('http://127.0.0.1:4216/?test=1&backend=webgl2',{waitUntil:'domcontentloaded'}).catch(e=>{if(!e.message.includes('interrupted'))throw e;});
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  await page.keyboard.press('Enter');
  // Cross a lower bound on the reveal progress and pause from inside the
  // page on that same frame, so the frozen progress stays inside the band
  // the pixel sampling below needs (the edge between the .2 and .8 columns).
  await page.evaluate(()=>new Promise(resolve=>{
    (function check(){
      const p=window.__nir.state().window;
      if(p!==null&&p>.45){window.__nir.hidden(true);resolve(p);return;}
      requestAnimationFrame(check);
    })();
  }));
  await page.waitForFunction(()=>window.__nir.state().paused);
  const progress=await page.evaluate(()=>window.__nir.state().window);
  expect(progress).toBeGreaterThan(.4);
  expect(progress).toBeLessThan(.6);
  const screenshot=await page.screenshot({path:'reports/nir-next/window-middle.png'});
  const pixels=await page.evaluate(async base64=>{
    // The fixture pins the window box to [20,450,1240,220] on its 1280x720
    // stage; sample just inside the box bottom — below every text run and
    // above the hint that sits outside the box — while the wipe edge itself
    // spans the full surface, so its columns use canvas fractions.
    const w=window.innerWidth,h=window.innerHeight;
    const scale=Math.min(w/1280,h/720);
    const ox=(w-1280*scale)/2,oy=(h-720*scale)/2;
    const y=Math.floor(oy+670*scale-8);
    const image=await createImageBitmap(new Blob([Uint8Array.from(atob(base64),c=>c.charCodeAt(0))],{type:'image/png'}));
    const canvas=document.createElement('canvas');canvas.width=image.width;canvas.height=image.height;
    const ctx=canvas.getContext('2d');ctx.drawImage(image,0,0);image.close();
    const row=canvas.height/h;
    return [.2,.5,.8].map(x=>{
      const px=Math.floor(canvas.width*x),py=Math.floor(y*row);
      return Array.from(ctx.getImageData(px,py,1,1).data);
    });
  },screenshot.toString('base64'));
  // Left of the edge the hiding reveal has erased the window: solid scene.
  expect(pixels[0][0]).toBeGreaterThan(240);expect(pixels[0][1]).toBeLessThan(15);
  // Right of the edge the window is still fully present: opaque green box.
  expect(pixels[2][0]).toBeLessThan(15);expect(pixels[2][1]).toBeGreaterThan(240);
  const softness=.2,threshold=progress*(1+softness)-softness/2;
  const t=Math.max(0,Math.min(1,(.5-threshold+softness/2)/softness));
  const coverage=1-t*t*(3-2*t);
  const srgb=v=>255*(v<=.0031308?v*12.92:1.055*Math.pow(v,1/2.4)-.055);
  expect(Math.abs(pixels[1][0]-srgb(coverage))).toBeLessThan(15);
  expect(Math.abs(pixels[1][1]-srgb(1-coverage))).toBeLessThan(15);
  await page.waitForTimeout(250);
  expect(await page.evaluate(()=>window.__nir.state().window)).toBe(progress);
  await page.evaluate(()=>window.__nir.action({type:'save',slot:2}));
  await page.waitForFunction(()=>/Saved in this browser|浏览器已保存/.test(window.__nir.state().status));
  const session=await page.evaluate(()=>window.__nir.state().session);
  await page.evaluate(()=>window.__nir.action({type:'load',slot:2}));
  await page.waitForFunction(session=>window.__nir.state().session>session&&!window.__nir.state().loading,session);
  expect(await page.evaluate(()=>window.__nir.state().window)).toBe(progress);
  await page.evaluate(()=>{window.__nir.hidden(false);window.__nir.action({type:'continue'});});
  await page.waitForFunction(()=>window.__nir.state().window===null);
  expect(errors).toEqual([]);
});
