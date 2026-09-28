import {test,expect} from '@playwright/test';

test('single text shadow reaches the rendered canvas and follows interface visibility',async({page})=>{
  await page.goto('http://127.0.0.1:4200/?test=1&backend=webgl2',{waitUntil:'domcontentloaded'}).catch(e=>{if(!e.message.includes('interrupted'))throw e;});
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>window.__nir.state().dialogue&&!window.__nir.state().loading);
  await page.evaluate(()=>window.__nir.action({type:'advance'}));
  await page.waitForFunction(()=>window.__nir.state().dialogue?.ready);
  async function shadowPixels() {
    const bytes=await page.screenshot();
    return page.evaluate(async base64=>{
      const bitmap=await createImageBitmap(new Blob([Uint8Array.from(atob(base64),c=>c.charCodeAt(0))],{type:'image/png'}));
      const canvas=document.createElement('canvas');canvas.width=bitmap.width;canvas.height=bitmap.height;
      const context=canvas.getContext('2d');context.drawImage(bitmap,0,0);bitmap.close();
      const data=context.getImageData(0,0,canvas.width,canvas.height).data;
      let count=0;
      for(let y=Math.floor(canvas.height*.55);y<canvas.height;y++)for(let x=0;x<canvas.width;x++) {
        const i=(y*canvas.width+x)*4;
        if(data[i]>140&&data[i+2]>140&&data[i+1]<80)count++;
      }
      return count;
    },bytes.toString('base64'));
  }
  expect(await shadowPixels()).toBeGreaterThan(20);
  const token=await page.evaluate(()=>window.__nir.state().interaction);
  await page.keyboard.press('h');
  await page.waitForFunction(()=>window.__nir.state().interface_hidden);
  expect(await shadowPixels()).toBe(0);
  await page.keyboard.press('h');
  await page.waitForFunction(()=>!window.__nir.state().interface_hidden);
  expect(await shadowPixels()).toBeGreaterThan(20);
  expect(await page.evaluate(()=>window.__nir.state().interaction)).toBe(token);
});
