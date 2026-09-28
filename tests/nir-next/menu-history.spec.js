import {test,expect} from '@playwright/test';
test('authored history pages frozen text through clipped bounded windows without moving Story',async({page})=>{
  await page.setViewportSize({width:1280,height:720});
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto('http://127.0.0.1:4208/?test=1&backend=webgl2',{waitUntil:'domcontentloaded'}).catch(e=>{if(!e.message.includes('interrupted'))throw e;});
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  await page.keyboard.press('Enter');
  for(let count=1;count<=4;count++){
    await page.waitForFunction(n=>window.__nir.state().history_count===n&&window.__nir.state().dialogue?.ready&&!window.__nir.state().loading,count);
    if(count<4)await page.evaluate(()=>window.__nir.action({type:'advance'}));
  }
  await page.keyboard.press('Escape');
  await page.waitForFunction(()=>window.__nir.state().screen==='Menu'&&!window.__nir.state().loading);
  const older=page.getByRole('button',{name:'Older',exact:true}),newer=page.getByRole('button',{name:'Newer',exact:true});
  await expect(newer).toBeDisabled();
  const before=await page.evaluate(()=>({tick:window.__nir.state().tick_us,interaction:window.__nir.state().interaction,bytes:window.__nir.state().resident_bytes}));
  const clip={x:80,y:120,width:1120,height:360};
  const latest=await page.screenshot({clip});
  const stale=JSON.parse(await older.getAttribute('data-action'));
  await older.focus();await page.keyboard.press('Enter');
  await expect(newer).toBeEnabled();
  const middle=await page.screenshot({clip});
  expect(middle.equals(latest)).toBe(false);
  await page.evaluate(async a=>{window.__nir.action(a);for(let i=0;i<4;i++)await new Promise(requestAnimationFrame);},stale);
  expect((await page.screenshot({clip})).equals(middle)).toBe(true);
  await older.focus();await page.keyboard.press('Enter');
  await expect(older).toBeDisabled();
  await newer.focus();await page.keyboard.press('Enter');
  await expect(older).toBeEnabled();
  await newer.focus();await page.keyboard.press('Enter');
  await expect(newer).toBeDisabled();
  expect((await page.screenshot({clip})).equals(latest)).toBe(true);
  const after=await page.evaluate(()=>window.__nir.state());
  expect(after.tick_us).toBe(before.tick);expect(after.interaction).toBe(before.interaction);
  expect(after.resident_bytes).toBe(before.bytes);
  expect(errors).toEqual([]);
});
