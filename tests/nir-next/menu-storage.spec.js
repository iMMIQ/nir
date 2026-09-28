import {test,expect} from '@playwright/test';
async function revision(page){return page.evaluate(async()=>{
  const db=await new Promise((ok,no)=>{const r=indexedDB.open('nir-player-isolated-v1');r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});
  const rows=await new Promise((ok,no)=>{const r=db.transaction('saves').objectStore('saves').getAll();r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});db.close();
  return rows.find(r=>r.envelope.slot===1)?.envelope.revision||0;
});}
test('authored slot selection saves, confirms overwrite, cancels stale confirmation and loads',async({page})=>{
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto('http://127.0.0.1:4207/?test=1&backend=webgl2',{waitUntil:'domcontentloaded'}).catch(e=>{if(!e.message.includes('interrupted'))throw e;});
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>window.__nir.state().dialogue&&!window.__nir.state().loading);
  await page.keyboard.press('Escape');
  await page.waitForFunction(()=>window.__nir.state().screen==='Menu'&&!window.__nir.state().loading);
  const select=page.getByRole('button',{name:'Select slot 2',exact:true});
  const save=page.getByRole('button',{name:'Save selected',exact:true});
  const load=page.getByRole('button',{name:'Load selected',exact:true});
  await expect(load).toBeDisabled();
  const selectionRevision=JSON.parse(await select.getAttribute('data-action')).revision;
  await select.focus();await page.keyboard.press('Enter');
  // The slot selection changes the menu model. Saving must use the newly
  // rendered control identity, not the intentionally rejected old revision.
  await expect.poll(async()=>JSON.parse(await save.getAttribute('data-action')).revision).toBeGreaterThan(selectionRevision);
  await save.focus();await page.keyboard.press('Enter');
  await expect.poll(()=>revision(page)).toBe(1);
  await expect(load).toBeEnabled();
  await save.focus();await page.keyboard.press('Enter');
  const confirm=page.getByRole('button',{name:/^(Confirm overwrite|确认覆盖)$/});
  await expect(confirm).toBeVisible();
  await expect(save).toHaveCount(0);
  const stale=JSON.parse(await confirm.getAttribute('data-action'));
  await page.keyboard.press('Escape');
  await expect(save).toBeVisible();
  await page.evaluate(async a=>{window.__nir.action(a);for(let i=0;i<4;i++)await new Promise(requestAnimationFrame);},stale);
  expect(await revision(page)).toBe(1);
  // Exercise confirmation focus at a narrow viewport with enlarged text.
  await page.setViewportSize({width:390,height:720});
  await page.evaluate(()=>window.__nir.action({type:'font_size',delta:.5}));
  await page.waitForFunction(()=>!window.__nir.state().loading);
  await save.focus();await page.keyboard.press('Enter');
  await expect(confirm).toBeVisible();
  await confirm.focus();await page.keyboard.press('Enter');
  await expect.poll(()=>revision(page)).toBe(2);
  await expect(load).toBeEnabled();
  const session=await page.evaluate(()=>window.__nir.state().session);
  await load.focus();await page.keyboard.press('Enter');
  await page.waitForFunction(s=>window.__nir.state().session>s&&!window.__nir.state().loading,session);
  expect(await page.evaluate(()=>window.__nir.state().screen)).toBe('Story');
  expect(errors).toEqual([]);
});
