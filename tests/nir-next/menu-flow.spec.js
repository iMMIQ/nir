import {test,expect} from '@playwright/test';

test('menu rows hide by service availability and pixels, hit regions and keyboard agree after reflow',async({page})=>{
  await page.setViewportSize({width:1280,height:720});
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  for(const [port,hideY] of [[4213,220],[4214,100]]){
    await page.goto(`http://127.0.0.1:${port}/?test=1&backend=webgl2`,{waitUntil:'domcontentloaded'});
    await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
    await page.keyboard.press('Enter');
    await page.waitForFunction(()=>window.__nir.state().screen==='Story'&&!window.__nir.state().loading);
    await page.keyboard.press('Escape');
    await page.waitForFunction(()=>window.__nir.state().screen==='Menu'&&!window.__nir.state().loading);
    await expect(page.getByRole('button',{name:'Skip read',exact:true})).toHaveCount(0);
    await expect(page.getByRole('button',{name:'Resume Auto',exact:true})).toHaveCount(port===4213?1:0);
    const hide=page.getByRole('button',{name:'Hide text',exact:true});
    await expect.poll(async()=>JSON.parse(await hide.getAttribute('data-rect'))?.[1]).toBe(hideY);
    const before=await page.evaluate(()=>window.__nir.state().interaction);
    const bytes=await page.screenshot();
    const pixels=await page.evaluate(async({base64,y})=>{
      const bitmap=await createImageBitmap(new Blob([Uint8Array.from(atob(base64),c=>c.charCodeAt(0))],{type:'image/png'}));
      const canvas=document.createElement('canvas');canvas.width=1280;canvas.height=720;
      const ctx=canvas.getContext('2d');ctx.drawImage(bitmap,0,0);bitmap.close();
      return [y+40,y+100].map(yy=>Array.from(ctx.getImageData(400,yy,1,1).data));
    },{base64:bytes.toString('base64'),y:hideY});
    expect(pixels[0][2]).toBeGreaterThan(100);
    expect(pixels[1].slice(0,3)).toEqual([0,0,0]);
    await page.mouse.click(400,hideY+40);
    await page.waitForFunction(()=>window.__nir.state().interface_hidden);
    expect(await page.evaluate(()=>window.__nir.state().interaction)).toBe(before);
    await page.keyboard.press('Escape');
    await page.waitForFunction(()=>window.__nir.state().screen==='Menu'&&!window.__nir.state().interface_hidden);
    await hide.focus();await page.keyboard.press('Enter');
    await page.waitForFunction(()=>window.__nir.state().interface_hidden);
    await page.keyboard.press('Escape');
    await page.waitForFunction(()=>window.__nir.state().screen==='Menu'&&!window.__nir.state().interface_hidden);
    expect(await page.evaluate(()=>window.__nir.state().interaction)).toBe(before);
  }
  expect(errors).toEqual([]);
});
