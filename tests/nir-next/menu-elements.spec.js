import {test,expect} from '@playwright/test';
test('authored menu interleaves text and alpha images and disabled hits block underlying actions',async({page})=>{
  await page.setViewportSize({width:1280,height:720});
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto('http://127.0.0.1:4204/?test=1&backend=webgl2',{waitUntil:'domcontentloaded'}).catch(e=>{if(!e.message.includes('interrupted'))throw e;});
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  const bytes=await page.screenshot();
  const counts=await page.evaluate(async base64=>{
    const bitmap=await createImageBitmap(new Blob([Uint8Array.from(atob(base64),c=>c.charCodeAt(0))],{type:'image/png'}));
    const canvas=document.createElement('canvas');canvas.width=1280;canvas.height=720;
    const ctx=canvas.getContext('2d');ctx.drawImage(bitmap,0,0);bitmap.close();
    return [100,450,800].map(x=>{
      const data=ctx.getImageData(x,180,280,150).data;let red=0,mixed=0;
      for(let i=0;i<data.length;i+=4){if(data[i]>180&&data[i+1]<30&&data[i+2]<30)red++;if(data[i]>60&&data[i+1]<30&&data[i+2]>60)mixed++;}
      return {red,mixed};
    });
  },bytes.toString('base64'));
  expect(counts[0].red).toBeGreaterThan(100);
  expect(counts[1].red).toBe(0);expect(counts[1].mixed).toBeGreaterThan(100);
  expect(counts[2].red).toBeGreaterThan(100);
  await page.mouse.move(500,540);
  const hovered=await page.screenshot();
  const colors=await page.evaluate(async base64=>{
    const bitmap=await createImageBitmap(new Blob([Uint8Array.from(atob(base64),c=>c.charCodeAt(0))],{type:'image/png'}));
    const canvas=document.createElement('canvas');canvas.width=1280;canvas.height=720;
    const ctx=canvas.getContext('2d');ctx.drawImage(bitmap,0,0);bitmap.close();
    return [150,500].map(x=>Array.from(ctx.getImageData(x,540,1,1).data));
  },hovered.toString('base64'));
  expect(colors[0][2]).toBeGreaterThan(100);
  expect(colors[1].slice(0,3)).toEqual([0,0,0]);
  await page.mouse.click(150,220);
  expect(await page.evaluate(()=>window.__nir.state().screen)).toBe('Title');
  await page.mouse.click(900,220);
  await page.waitForFunction(()=>window.__nir.state().screen==='Settings');
  expect(errors).toEqual([]);
});
