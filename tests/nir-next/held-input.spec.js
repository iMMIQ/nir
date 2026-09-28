import {test,expect} from '@playwright/test';

test('Control advances read text while release and blur cancel the transient hold',async({page})=>{
  await page.goto('http://127.0.0.1:4199/?test=1&backend=webgl2',{waitUntil:'domcontentloaded'}).catch(error=>{
    if(!error.message.includes('interrupted'))throw error;
  });
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>window.__nir.state().dialogue&&!window.__nir.state().loading);
  const textId=await page.evaluate(()=>window.__nir.state().dialogue.id);
  // Seed the neutral fixture's revision-1 read record in the persistent profile store.
  await page.evaluate(async textId=>{
    const release=await (await fetch(location.pathname.replace('/index.html','.json'))).json();
    for(const item of await indexedDB.databases()) {
      const db=await new Promise(resolve=>{const r=indexedDB.open(item.name);r.onsuccess=()=>resolve(r.result);});
      if(db.objectStoreNames.contains('profile')) {
        await new Promise((resolve,reject)=>{const tx=db.transaction('profile','readwrite');tx.objectStore('profile').put([`read:${textId}:1`],[release.game_id,release.profile]);tx.oncomplete=resolve;tx.onerror=()=>reject(tx.error);});
      }db.close();
    }
  },textId);
  await page.reload({waitUntil:'domcontentloaded'});
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>window.__nir.state().dialogue&&!window.__nir.state().loading);
  await page.evaluate(()=>window.__nir.action({type:'advance'}));
  await page.waitForFunction(()=>window.__nir.state().dialogue?.ready);
  const token=await page.evaluate(()=>window.__nir.state().interaction);
  await page.evaluate(()=>{window.dispatchEvent(new KeyboardEvent('keydown',{key:'Control',code:'ControlLeft',ctrlKey:true}));const editor=document.createElement('div');editor.id='input-probe';editor.contentEditable='true';document.body.append(editor);editor.focus();});
  await page.keyboard.down('ControlLeft');
  await page.waitForTimeout(150);
  expect(await page.evaluate(()=>window.__nir.state().interaction)).toBe(token);
  await page.keyboard.up('ControlLeft');
  await page.evaluate(()=>document.querySelector('#input-probe').remove());
  for(const end of ['keyup','blur']) {
    await page.evaluate(end=>{
      window.dispatchEvent(new KeyboardEvent('keydown',{key:'Control',code:'ControlLeft',ctrlKey:true}));
      window.dispatchEvent(end==='blur'?new Event('blur'):new KeyboardEvent('keyup',{key:'Control',code:'ControlLeft'}));
    },end);
    await page.waitForTimeout(150);
    expect(await page.evaluate(()=>window.__nir.state().interaction)).toBe(token);
  }
  await page.keyboard.down('ControlLeft');
  await page.waitForFunction(t=>window.__nir.state().interaction!==t,token);
  await page.keyboard.up('ControlLeft');
});
