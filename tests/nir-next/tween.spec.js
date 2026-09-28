import {test, expect} from '@playwright/test';

test('dialogue root animation reaches the rendered canvas and freezes under menu pause', async ({page}) => {
  const errors=[];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto('/?test=1&backend=webgl2', {waitUntil:'domcontentloaded'}).catch(error => {
    if (!error.message.includes('interrupted')) throw error;
  });
  await page.waitForFunction(() => window.__nir?.state().ready && !window.__nir.state().loading);
  await page.keyboard.press('Enter');
  await page.waitForFunction(() => {
    const s=window.__nir.state();
    return s.dialogue && !s.loading && s.dialogue_appearance.opacity < .9 && s.dialogue_appearance.opacity > .4;
  });
  // Reveal the current page once so pixel changes below are caused by the fade,
  // not the typewriter. This does not advance to another source page.
  await page.evaluate(() => window.__nir.action({type:'advance'}));
  const before = await page.screenshot();
  await page.evaluate(() => window.__nir.action({type:'menu'}));
  await page.waitForFunction(() => window.__nir.state().screen === 'Menu');
  const paused = await page.evaluate(() => window.__nir.state().dialogue_appearance);
  await page.waitForTimeout(250);
  expect(await page.evaluate(() => window.__nir.state().dialogue_appearance)).toEqual(paused);
  await page.evaluate(() => window.__nir.action({type:'close'}));
  await page.waitForFunction(() => window.__nir.state().screen === 'Story' && Math.abs(window.__nir.state().dialogue_appearance.opacity - .2) < .00001);
  const after = await page.screenshot();
  const comparison = await page.evaluate(async ([a,b]) => {
    async function pixels(base64) {
      const bytes=Uint8Array.from(atob(base64), c=>c.charCodeAt(0));
      const image=await createImageBitmap(new Blob([bytes],{type:'image/png'}));
      const canvas=document.createElement('canvas');canvas.width=image.width;canvas.height=image.height;
      const context=canvas.getContext('2d');context.drawImage(image,0,0);image.close();
      return {data:context.getImageData(0,0,canvas.width,canvas.height).data,width:canvas.width,height:canvas.height};
    }
    const x=await pixels(a),y=await pixels(b);
    let lower=0, upper=0;
    for (let row=100;row<x.height-50;row++) for(let col=100;col<x.width-100;col++) {
      const i=(row*x.width+col)*4;
      const delta=Math.abs(x.data[i]-y.data[i])+Math.abs(x.data[i+1]-y.data[i+1])+Math.abs(x.data[i+2]-y.data[i+2]);
      if(delta>15) {if(row>x.height*.65)lower++;else if(row<x.height*.4)upper++;}
    }
    return {lower,upper};
  }, [before.toString('base64'),after.toString('base64')]);
  expect(comparison.lower).toBeGreaterThan(1000);
  expect(comparison.upper).toBeLessThan(50);
  expect(await page.evaluate(() => window.__nir.state().error)).toBeFalsy();
  expect(errors).toEqual([]);
});
