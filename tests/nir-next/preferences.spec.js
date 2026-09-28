import {test,expect} from '@playwright/test';

test('landscape settings scroll to reading preferences and persist them across reload',async({page})=>{
  const errors=[];page.on('pageerror',error=>errors.push(error.message));
  await page.setViewportSize({width:844,height:390});
  await page.goto('http://127.0.0.1:4199/?test=1&backend=webgl2',{waitUntil:'domcontentloaded'}).catch(error=>{
    if(!error.message.includes('interrupted'))throw error;
  });
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  await page.evaluate(()=>window.__nir.action({type:'settings'}));
  await page.waitForFunction(()=>window.__nir.state().screen==='Settings');
  const forward=page.locator('#actions button[data-action=\'{"delta":1,"region":"settings","type":"scroll"}\']');
  const increase=page.locator('#actions button[data-action=\'{"delta":0.25,"type":"text_speed"}\']');
  for(let i=0;i<12&&await increase.count()===0;i++) {
    await forward.evaluate(b=>b.click());
    await page.waitForTimeout(80);
  }
  await expect(increase).toHaveCount(1);
  await increase.evaluate(b=>b.click());
  await page.waitForFunction(()=>window.__nir.state().preferences.text_speed===1.25);
  const wait=page.locator('#actions button[data-action=\'{"delta":0.25,"type":"auto_wait"}\']');
  for(let i=0;i<12&&await wait.count()===0;i++) {
    await forward.evaluate(b=>b.click());
    await page.waitForTimeout(80);
  }
  await expect(wait).toHaveCount(1);
  await wait.evaluate(b=>b.click());
  await page.waitForFunction(()=>window.__nir.state().preferences.auto_wait_scale===1.25);
  await expect(page.locator('#actions button[data-action=\'{"type":"close"}\']')).toHaveCount(1);
  await page.waitForTimeout(300);
  await page.reload({waitUntil:'domcontentloaded'});
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  expect(await page.evaluate(()=>window.__nir.state().preferences.text_speed)).toBe(1.25);
  expect(await page.evaluate(()=>window.__nir.state().preferences.auto_wait_scale)).toBe(1.25);
  expect(errors).toEqual([]);
});
