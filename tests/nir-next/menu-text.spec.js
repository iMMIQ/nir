import {test,expect} from '@playwright/test';
test('text menu buttons paint normal, hover and disabled styles and share keyboard and pointer actions',async({page})=>{
  await page.setViewportSize({width:1280,height:720});
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto('http://127.0.0.1:4215/?test=1&backend=webgl2',{waitUntil:'domcontentloaded'});
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>window.__nir.state().dialogue&&!window.__nir.state().loading);
  await page.keyboard.press('Escape');
  await page.waitForFunction(()=>window.__nir.state().screen==='Menu'&&!window.__nir.state().loading);
  const before=await page.evaluate(()=>window.__nir.state().interaction);
  async function colors(y){
    const bytes=await page.screenshot();
    return page.evaluate(async({base64,y})=>{
      const bitmap=await createImageBitmap(new Blob([Uint8Array.from(atob(base64),c=>c.charCodeAt(0))],{type:'image/png'}));
      const canvas=document.createElement('canvas');canvas.width=1280;canvas.height=720;
      const ctx=canvas.getContext('2d');ctx.drawImage(bitmap,0,0);bitmap.close();
      const pixels=ctx.getImageData(100,y,560,80).data,counts=[0,0,0];
      for(let i=0;i<pixels.length;i+=4)for(let c=0;c<3;c++)if(pixels[i+c]>100&&pixels[i+(c+1)%3]<30&&pixels[i+(c+2)%3]<30)counts[c]++;
      return counts;
    },{base64:bytes.toString('base64'),y});
  }
  await page.mouse.move(1000,650);
  await expect.poll(async()=>(await colors(100))[0]).toBeGreaterThan(50);
  await page.mouse.move(200,140);
  await expect.poll(async()=>(await colors(100))[1]).toBeGreaterThan(50);
  await page.mouse.move(-10,-10);
  await expect.poll(async()=>(await colors(100))[0]).toBeGreaterThan(50);
  await expect(page.getByRole('button',{name:'Locked',exact:true})).toBeDisabled();
  await page.mouse.move(200,260);
  const locked=await colors(220);expect(locked[2]).toBeGreaterThan(50);expect(locked[1]).toBe(0);
  await page.mouse.click(200,260);
  expect(await page.evaluate(()=>window.__nir.state().screen)).toBe('Menu');
  const resume=page.getByRole('button',{name:'Resume',exact:true});
  await resume.focus();
  await expect.poll(async()=>(await colors(100))[1]).toBeGreaterThan(50);
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>window.__nir.state().screen==='Story');
  expect(await page.evaluate(()=>window.__nir.state().interaction)).toBe(before);
  await page.keyboard.press('Escape');
  await page.waitForFunction(()=>window.__nir.state().screen==='Menu'&&!window.__nir.state().loading);
  await page.mouse.click(200,140);
  await page.waitForFunction(()=>window.__nir.state().screen==='Story');
  expect(await page.evaluate(()=>window.__nir.state().interaction)).toBe(before);
  expect(errors).toEqual([]);
});
