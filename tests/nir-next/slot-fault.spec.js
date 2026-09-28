import {test,expect} from '@playwright/test';

test('a corrupt slot reports its failure and Back restores the current story',async({page})=>{
  await page.goto('/?test=1&backend=webgl2',{waitUntil:'domcontentloaded'}).catch(e=>{if(!e.message.includes('interrupted'))throw e;});
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>window.__nir.state().dialogue&&!window.__nir.state().loading);
  await page.keyboard.press('Escape');
  await page.evaluate(()=>window.__nir.action({type:'save',slot:0}));
  await page.waitForFunction(()=>/Saved|已保存/.test(window.__nir.state().status));
  await page.evaluate(async()=>{
    const db=await new Promise((ok,no)=>{const r=indexedDB.open('nir-player-isolated-v1');r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});
    await new Promise((ok,no)=>{
      const tx=db.transaction('saves','readwrite'),cursor=tx.objectStore('saves').openCursor();
      cursor.onsuccess=()=>{if(cursor.result){const record=cursor.result.value;record.envelope.digest='invalid';cursor.result.update(record);}};
      tx.oncomplete=ok;tx.onerror=()=>no(tx.error);
    });db.close();
  });
  const before=await page.evaluate(()=>({session:window.__nir.state().session,interaction:window.__nir.state().interaction}));
  await page.evaluate(()=>window.__nir.action({type:'saves'}));
  await page.waitForFunction(()=>window.__nir.state().screen==='Saves');
  await page.evaluate(()=>window.__nir.action({type:'load',slot:0}));
  await page.waitForFunction(()=>window.__nir.state().diagnostic?.code==='E_SAVE_DIGEST');
  await page.keyboard.press('Escape');
  await page.waitForFunction(()=>window.__nir.state().screen==='Story'&&!window.__nir.state().loading);
  const after=await page.evaluate(()=>window.__nir.state());
  expect(after.session).toBe(before.session);
  expect(after.interaction).toBe(before.interaction);
  expect(after.error).toBeNull();
  expect(after.diagnostic).toBeNull();
  expect(after.paused).toBe(false);
});
