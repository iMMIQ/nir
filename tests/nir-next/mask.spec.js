import {test,expect} from '@playwright/test';

for(const [port,invert] of [[4202,false],[4203,true]])test(`alpha texture mask restores after release and respects polarity (${invert})`,async({page})=>{
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto(`http://127.0.0.1:${port}/?test=1&backend=webgl2`,{waitUntil:'domcontentloaded'}).catch(e=>{if(!e.message.includes('interrupted'))throw e;});
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>{
    const p=window.__nir.state().transition;
    const seen=window.__maskProgress??=[];
    if(seen.at(-1)!==p&&seen.length<1000)seen.push(p);
    return p>.48;
  });
  await page.evaluate(()=>window.__nir.hidden(true));
  await page.waitForFunction(()=>window.__nir.state().paused);
  const progress=await page.evaluate(()=>window.__nir.state().transition);
  expect(progress,JSON.stringify(await page.evaluate(()=>window.__maskProgress))).toBeLessThan(.52);
  expect(progress).toBeGreaterThan(.48);
  async function pixels() {
    const bytes=await page.screenshot({path:`reports/nir-next/mask-${invert}.png`});
    return page.evaluate(async base64=>{
      const image=await createImageBitmap(new Blob([Uint8Array.from(atob(base64),c=>c.charCodeAt(0))],{type:'image/png'}));
      const canvas=document.createElement('canvas');canvas.width=image.width;canvas.height=image.height;
      const ctx=canvas.getContext('2d');ctx.drawImage(image,0,0);image.close();
      return [[.25,.2],[.75,.2],[.25,.8],[.75,.8]].map(([x,y])=>Array.from(ctx.getImageData(Math.floor(x*canvas.width),Math.floor(y*canvas.height),1,1).data));
    },bytes.toString('base64'));
  }
  const before=await pixels();
  expect(before[2][0]).toBeGreaterThan(30);expect(before[2][0]).toBeLessThan(245);
  expect(before[2][2]).toBeGreaterThan(30);expect(before[2][2]).toBeLessThan(245);
  const samples=[0,1,128/255,0];
  const srgb=v=>255*(v<=.0031308?v*12.92:1.055*Math.pow(v,1/2.4)-.055);
  for(let i=0;i<4;i++) {
    const threshold=progress*1.2-.1,coordinate=invert?1-samples[i]:samples[i];
    const t=Math.max(0,Math.min(1,(coordinate-threshold+.1)/.2)),coverage=1-t*t*(3-2*t);
    expect(Math.abs(before[i][0]-srgb(1-coverage))).toBeLessThan(15);
    expect(Math.abs(before[i][2]-srgb(coverage))).toBeLessThan(15);
    expect(before[i][1]).toBeLessThan(5);
  }
  await page.evaluate(()=>window.__nir.action({type:'save',slot:2}));
  await page.waitForFunction(()=>/Saved in this browser|浏览器已保存/.test(window.__nir.state().status));
  await page.evaluate(()=>window.__nir.hidden(false));
  await page.waitForFunction(()=>window.__nir.state().transition===null);
  const session=await page.evaluate(()=>window.__nir.state().session);
  await page.evaluate(()=>window.__nir.action({type:'load',slot:2}));
  await page.waitForFunction(session=>window.__nir.state().session>session&&!window.__nir.state().loading,session);
  expect(await page.evaluate(()=>window.__nir.state().transition)).toBe(progress);
  expect(await pixels()).toEqual(before);
  expect(errors).toEqual([]);
});
