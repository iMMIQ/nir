import {test,expect} from '@playwright/test';

test('story exports reflow menu controls after VM changes and saved context restores their visibility',async({page})=>{
  await page.setViewportSize({width:1280,height:720});
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto('http://127.0.0.1:4217/?test=1&backend=webgl2',{waitUntil:'domcontentloaded'});
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>window.__nir.state().choice&&!window.__nir.state().loading);
  await page.evaluate(()=>window.__nir.action({type:'save',slot:0}));
  await expect.poll(()=>page.evaluate(async()=>{
    const db=await new Promise((ok,no)=>{const r=indexedDB.open('nir-player-isolated-v1');r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});
    const rows=await new Promise((ok,no)=>{const r=db.transaction('saves').objectStore('saves').getAll();r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});db.close();
    return rows.find(r=>r.envelope.slot===0)?.envelope.revision||0;
  })).toBe(1);
  async function openMenu(){
    await page.keyboard.press('Escape');
    await page.waitForFunction(()=>window.__nir.state().screen==='Menu'&&!window.__nir.state().loading);
  }
  await openMenu();
  const initial=page.getByRole('button',{name:'Initial only',exact:true});
  const resume=page.getByRole('button',{name:'Resume',exact:true});
  await expect(initial).toHaveCount(1);
  await expect.poll(async()=>JSON.parse(await resume.getAttribute('data-rect'))[1]).toBe(200);
  const stale=JSON.parse(await initial.getAttribute('data-action'));
  await resume.focus();await page.keyboard.press('Enter');
  await page.waitForFunction(()=>window.__nir.state().screen==='Story');
  await page.evaluate(()=>window.__nir.action({type:'choose',option:'walk'}));
  await page.waitForFunction(()=>window.__nir.state().variables.affection.value===1&&!window.__nir.state().loading);
  await openMenu();
  await expect(initial).toHaveCount(0);
  await expect.poll(async()=>JSON.parse(await resume.getAttribute('data-rect'))[1]).toBe(100);
  await page.evaluate(action=>window.__nir.action(action),stale);
  expect(await page.evaluate(()=>window.__nir.state().screen)).toBe('Menu');
  // The same post-layout rectangle used by semantic controls is clickable.
  await page.mouse.click(400,140);
  await page.waitForFunction(()=>window.__nir.state().screen==='Story');
  await page.evaluate(()=>window.__nir.action({type:'load',slot:0}));
  await page.waitForFunction(()=>window.__nir.state().choice&&window.__nir.state().variables.affection.value===0&&!window.__nir.state().loading);
  await openMenu();
  await expect(initial).toHaveCount(1);
  await expect.poll(async()=>JSON.parse(await resume.getAttribute('data-rect'))[1]).toBe(200);
  expect(errors).toEqual([]);
});
