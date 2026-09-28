import {test,expect} from '@playwright/test';

test('secondary pointer preserves title and dialogue; release history is in the menu',async({page})=>{
  await page.goto('/?test=1&backend=webgl2');
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  await expect(page.locator('#nir-history-button')).toBeHidden();
  const start=await page.evaluate(()=>{
    const button=[...document.querySelectorAll('#actions button')].find(b=>JSON.parse(b.dataset.action).type==='new_game');
    button.focus();const ring=document.querySelector('#focus-ring').getBoundingClientRect();
    return {x:ring.x+ring.width/2,y:ring.y+ring.height/2};
  });
  await page.mouse.click(start.x,start.y,{button:'right'});
  expect(await page.evaluate(()=>window.__nir.state().screen)).toBe('Title');
  await page.mouse.click(start.x,start.y);
  await page.waitForFunction(()=>window.__nir.state().dialogue?.ready&&!window.__nir.state().loading);
  const before=await page.evaluate(()=>window.__nir.state().dialogue.id);
  await page.mouse.click(600,650,{button:'right'});
  await page.waitForFunction(()=>window.__nir.state().screen==='Menu');
  expect(await page.evaluate(()=>window.__nir.state().dialogue.id)).toBe(before);
  await expect(page.locator('#nir-history-button')).toBeVisible();
  await page.locator('#nir-history-button').click();
  await expect(page.locator('#nir-history-panel')).toBeVisible();
  await page.keyboard.press('Escape');
  await page.mouse.click(600,650,{button:'right'});
  await page.waitForFunction(()=>window.__nir.state().screen==='Story');
  expect(await page.evaluate(()=>window.__nir.state().dialogue.id)).toBe(before);
});
