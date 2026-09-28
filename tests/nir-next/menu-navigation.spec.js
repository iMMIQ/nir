import {test,expect} from '@playwright/test';

test('authored subpages preserve parent locals and close one page without resuming Story',async({page})=>{
  await page.setViewportSize({width:1280,height:720});
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto('http://127.0.0.1:4219/?test=1&backend=webgl2',{waitUntil:'domcontentloaded'});
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  const button=name=>page.getByRole('button',{name,exact:true});
  async function click(name){
    const rect=JSON.parse(await button(name).getAttribute('data-rect'));
    await page.mouse.click(rect[0]+rect[2]/2,rect[1]+rect[3]/2);
  }
  async function depth(n,screen){await page.waitForFunction(({n,screen})=>{const s=window.__nir.state();return s.menu_depth===n&&s.screen===screen&&!s.loading;},{n,screen});}
  await button('Open system').focus();await page.keyboard.press('Enter');
  await depth(1,'Title');
  await expect(button('Parent')).toBeEnabled();
  await expect(button('Open history')).toBeDisabled();
  const unavailable=JSON.parse(await button('Open history').getAttribute('data-action'));
  await page.evaluate(a=>window.__nir.action(a),unavailable);
  await depth(1,'Title');
  await page.keyboard.press('Escape');await depth(0,'Title');
  await expect(button('Start')).toBeEnabled();
  await click('Open system');await depth(1,'Title');
  // Right click in a title subpage also returns to its parent.
  await page.mouse.click(1000,500,{button:'right'});await depth(0,'Title');
  await click('Start');
  for(let n=1;n<=6;n++){
    await page.waitForFunction(n=>{const s=window.__nir.state();return s.history_count===n&&s.dialogue?.ready&&!s.loading;},n);
    if(n<6)await page.evaluate(()=>window.__nir.action({type:'advance'}));
  }
  await page.keyboard.press('Escape');await depth(0,'Menu');
  await expect(button('Parent')).toBeDisabled();
  await click('Select two');
  const staleParent=JSON.parse(await button('Open history').getAttribute('data-action'));
  const before=await page.evaluate(()=>{const s=window.__nir.state();return {position:s.position,tick:s.tick_us,interaction:s.interaction,variables:s.variables,history:s.history_count};});
  const selected=await page.screenshot({clip:{x:80,y:100,width:200,height:60}});
  await click('Open history');await depth(1,'Menu');
  await expect(page.locator('#nir-history-button')).toBeHidden();
  await page.waitForFunction(()=>!window.__nir.state().history_pending&&window.__nir.state().scrolls.some(v=>v.menu));
  const staleChild=JSON.parse(await button('Return to parent').getAttribute('data-action'));
  await page.mouse.move(300,250);await page.mouse.wheel(0,-100);
  await expect.poll(()=>page.evaluate(()=>{const v=window.__nir.state().scrolls.find(v=>v.menu);return v.offset<v.max;})).toBe(true);
  await page.keyboard.press('Escape');await depth(0,'Menu');
  await expect(button('Open history')).toBeEnabled();
  await expect(page.locator('#nir-history-button')).toBeVisible();
  expect((await page.screenshot({clip:{x:80,y:100,width:200,height:60}})).equals(selected)).toBe(true);
  const fresh=JSON.parse(await button('Open history').getAttribute('data-action'));
  expect(fresh.instance).toBeGreaterThan(staleParent.instance);
  await page.evaluate(a=>window.__nir.action(a),staleParent);
  await page.evaluate(a=>window.__nir.action(a),staleChild);
  await depth(0,'Menu');await expect(button('Open history')).toBeEnabled();
  const after=await page.evaluate(()=>{const s=window.__nir.state();return {position:s.position,tick:s.tick_us,interaction:s.interaction,variables:s.variables,history:s.history_count};});
  expect(after).toEqual(before);
  await click('Open history');await depth(1,'Menu');
  await button('Return to parent').focus();await page.keyboard.press('Enter');
  await depth(0,'Menu');
  expect((await page.screenshot({clip:{x:80,y:100,width:200,height:60}})).equals(selected)).toBe(true);
  await page.keyboard.press('Escape');
  await page.waitForFunction(()=>window.__nir.state().screen==='Story');
  await page.keyboard.press('Escape');await depth(0,'Menu');
  expect((await page.screenshot({clip:{x:80,y:100,width:200,height:60}})).equals(selected)).toBe(false);
  expect(errors).toEqual([]);
});
