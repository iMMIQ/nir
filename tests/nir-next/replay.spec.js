import {test,expect} from '@playwright/test';

const openOverlay=async page=>{
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  const button=name=>page.getByRole('button',{name,exact:true});
  await button('Start').focus();await page.keyboard.press('Enter');
  await page.waitForFunction(()=>{const s=window.__nir.state();return s.screen==='Story'&&s.dialogue?.ready&&!s.loading;});
  await page.keyboard.press('Escape');
  await page.waitForFunction(()=>{const s=window.__nir.state();return s.screen==='Menu'&&!s.loading;});
  return button;
};

// Seeds one profile key in the persistent store, exactly like a returning
// reader's unlock record, then reloads so boot hydrates it.
const seedProfile=async page=>page.evaluate(async()=>{
  const release=await (await fetch(location.pathname.replace('/index.html','.json'))).json();
  for(const item of await indexedDB.databases()) {
    const db=await new Promise(resolve=>{const r=indexedDB.open(item.name);r.onsuccess=()=>resolve(r.result);});
    if(db.objectStoreNames.contains('profile')) {
      await new Promise((resolve,reject)=>{const tx=db.transaction('profile','readwrite');tx.objectStore('profile').put(['seen'],[release.game_id,release.profile]);tx.oncomplete=resolve;tx.onerror=()=>reject(tx.error);});
    }db.close();
  }
});

test('locked replay unlocks from the profile, one double click runs the whole transaction',async({page})=>{
  await page.setViewportSize({width:1280,height:720});
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto('http://127.0.0.1:4221/?test=1&backend=webgl2',{waitUntil:'domcontentloaded'});
  let button=await openOverlay(page);
  // Locked re-check: the control is disabled, and a forged current action
  // dies at the runtime guard as well.
  await expect(button('Replay arrival')).toBeDisabled();
  await expect(button('Exit replay')).toBeDisabled();
  const forged=JSON.parse(await button('Replay arrival').getAttribute('data-action'));
  await page.evaluate(a=>window.__nir.action(a),forged);
  await page.waitForTimeout(150);
  expect(await page.evaluate(()=>window.__nir.state().replay)).toBe('inactive');
  expect(await page.evaluate(()=>window.__nir.state().screen)).toBe('Menu');
  // The unlock key arrives with the next boot.
  await seedProfile(page);
  await page.reload({waitUntil:'domcontentloaded'});
  button=await openOverlay(page);
  await expect(button('Replay arrival')).toBeEnabled();
  await expect(button('Exit replay')).toBeDisabled();
  const frozen=await page.evaluate(()=>({text:window.__nir.state().dialogue?.id,screen:window.__nir.state().screen}));
  // Double delivery of the identical click: only the first resolves.
  const click=JSON.parse(await button('Replay arrival').getAttribute('data-action'));
  await page.evaluate(a=>{window.__nir.action(a);window.__nir.action(a);},click);
  await page.waitForFunction(()=>window.__nir.state().replay==='entering');
  // The frozen page owns the screen until the candidate is prepared.
  expect(await page.evaluate(()=>window.__nir.state().screen)).toBe('Menu');
  await page.waitForFunction(()=>{const s=window.__nir.state();return s.replay==='active'&&s.screen==='Story'&&!s.loading;});
  // The replayed line differs from the frozen one.
  const live=await page.evaluate(()=>window.__nir.state().dialogue?.id);
  expect(live).not.toBe(frozen.text);
  // While live, the overlay offers only the exit.
  await page.keyboard.press('Escape');
  await page.waitForFunction(()=>{const s=window.__nir.state();return s.screen==='Menu'&&!s.loading;});
  await expect(button('Exit replay')).toBeEnabled();
  await expect(button('Replay arrival')).toBeDisabled();
  await page.keyboard.press('Escape');
  await page.waitForFunction(()=>window.__nir.state().screen==='Story');
  // Finishing the line ends the function: the outcome starts the return, and
  // the frozen session comes back on its own page.
  await page.waitForFunction(()=>window.__nir.state().dialogue?.ready);
  await page.evaluate(()=>window.__nir.action({type:'advance'}));
  await page.waitForFunction(()=>window.__nir.state().replay==='returning'||window.__nir.state().replay==='inactive');
  await page.waitForFunction(()=>window.__nir.state().replay==='inactive');
  await page.waitForFunction(()=>{const s=window.__nir.state();return s.screen==='Menu'&&!s.loading;});
  expect(await page.evaluate(()=>window.__nir.state().replay)).toBe('inactive');
  // The frozen story resumed exactly where it froze.
  const restored=await page.evaluate(()=>window.__nir.state().dialogue?.id);
  expect(restored).toBe(frozen.text);
  // Fresh authority: the control unlocks again for another transaction, and
  // the pre-freeze click can never relaunch it.
  await expect(button('Replay arrival')).toBeEnabled();
  await expect(button('Exit replay')).toBeDisabled();
  await page.evaluate(a=>window.__nir.action(a),click);
  await page.waitForTimeout(150);
  expect(await page.evaluate(()=>window.__nir.state().replay)).toBe('inactive');
  expect(errors).toEqual([]);
});

test('exit replay returns the frozen session through the overlay control',async({page})=>{
  await page.setViewportSize({width:1280,height:720});
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto('http://127.0.0.1:4221/?test=1&backend=webgl2',{waitUntil:'domcontentloaded'});
  let button=await openOverlay(page);
  await seedProfile(page);
  await page.reload({waitUntil:'domcontentloaded'});
  button=await openOverlay(page);
  const frozen=await page.evaluate(()=>window.__nir.state().dialogue?.id);
  await button('Replay arrival').focus();await page.keyboard.press('Enter');
  await page.waitForFunction(()=>{const s=window.__nir.state();return s.replay==='active'&&s.screen==='Story'&&!s.loading;});
  // Manual exit while live: the replay dies whole and the frozen session
  // restores as its own return transaction.
  await page.keyboard.press('Escape');
  await page.waitForFunction(()=>{const s=window.__nir.state();return s.screen==='Menu'&&!s.loading;});
  await expect(button('Exit replay')).toBeEnabled();
  await button('Exit replay').focus();await page.keyboard.press('Enter');
  await page.waitForFunction(()=>window.__nir.state().replay==='inactive');
  await page.waitForFunction(()=>{const s=window.__nir.state();return s.screen==='Menu'&&!s.loading;});
  const restored=await page.evaluate(()=>window.__nir.state().dialogue?.id);
  expect(restored).toBe(frozen);
  await expect(button('Exit replay')).toBeDisabled();
  expect(errors).toEqual([]);
});
