import {test,expect} from '@playwright/test';

test('keyboard focus cycles controls, moves spatially and activates the selected menu action',async({page})=>{
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto('/?test=1&backend=webgl2',{waitUntil:'domcontentloaded'}).catch(e=>{if(!e.message.includes('interrupted'))throw e;});
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>window.__nir.state().dialogue&&!window.__nir.state().loading);
  await page.keyboard.press('Escape');
  await page.waitForFunction(()=>window.__nir.state().screen==='Menu');
  const focused=()=>page.evaluate(()=>document.activeElement?.dataset?.action);
  await page.keyboard.press('Tab');
  await expect.poll(focused).toBeTruthy();
  const first=await focused();
  await page.keyboard.press('Tab');
  await expect.poll(focused).not.toBe(first);
  await page.keyboard.press('Shift+Tab');
  await expect.poll(focused).toBe(first);
  await page.keyboard.press('ArrowDown');
  await expect.poll(focused).not.toBe(first);
  for(let i=0;i<24;i++) {
    const before=await focused();
    if(JSON.parse(before||'null')?.type==='settings')break;
    await page.keyboard.press('Tab');
    // Navigation applies DOM focus on the next presentation frame.
    await expect.poll(focused).not.toBe(before);
  }
  expect(JSON.parse(await focused()).type).toBe('settings');
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>window.__nir.state().screen==='Settings');
  // An editor consumes navigation before the player's shared focus route.
  await page.evaluate(()=>{const input=document.createElement('input');input.id='probe';document.body.append(input);input.focus();});
  await page.keyboard.press('ArrowDown');
  expect(await page.evaluate(()=>document.activeElement.id)).toBe('probe');
  await page.evaluate(()=>document.querySelector('#probe').remove());
  await page.keyboard.press('Escape');
  await page.waitForFunction(()=>window.__nir.state().screen==='Story');
  expect(errors).toEqual([]);
});


test('keyboard traversal scrolls narrow settings to offscreen reading controls and back',async({page})=>{
  await page.setViewportSize({width:740,height:380});
  await page.goto('/?test=1&backend=webgl2',{waitUntil:'domcontentloaded'}).catch(e=>{if(!e.message.includes('interrupted'))throw e;});
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  await page.evaluate(()=>window.__nir.action({type:'settings'}));
  await page.waitForFunction(()=>window.__nir.state().screen==='Settings');
  const focused=()=>page.evaluate(()=>document.activeElement?.dataset?.action);
  let reached=false;
  for(let i=0;i<50;i++) {
    await page.keyboard.press('Tab');
    await page.waitForFunction(()=>document.activeElement?.dataset?.action);
    // Wait for the frame that applies the requested DOM focus.
    await page.evaluate(()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r))));
    const action=JSON.parse(await focused());
    if(action.type==='text_speed'){reached=true;break;}
  }
  expect(reached).toBe(true);
  expect(await page.evaluate(()=>window.__nir.state().scrolls.find(v=>v.region==='settings').offset)).toBeGreaterThan(0);
  expect(await page.evaluate(()=>{const ids=[...document.querySelectorAll('#actions button')].map(b=>b.dataset.control);return new Set(ids).size===ids.length;})).toBe(true);
  let top=false;
  for(let i=0;i<50;i++) {
    await page.keyboard.press('Shift+Tab');
    await page.evaluate(()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r))));
    if(await page.evaluate(()=>window.__nir.state().scrolls.find(v=>v.region==='settings').offset===0)){top=true;break;}
  }
  expect(top).toBe(true);
});
