import {test,expect} from '@playwright/test';

async function boot(page){
  await page.goto('/?test=1');
  await page.waitForFunction(()=>window.__nir?.state().ready && !window.__nir.state().loading);
}

async function hostModule(page){
  return page.evaluate(async()=>{
    const channel=await (await fetch('/channels/stable.json')).json();
    const release=await (await fetch(`/releases/${channel.release}.json`)).json();
    const host=release.objects[release.engine.host];
    return {url:new URL(host.path,new URL('/',location.href)).href,digest:channel.release,gameId:release.game_id,profile:release.profile};
  });
}

test('save keys isolate releases while same release uses revision CAS',async({page})=>{
  await boot(page);
  const module=await hostModule(page);
  expect(['dev','release']).toContain(module.profile);
  const result=await page.evaluate(async({url,gameId,profile,digest})=>{
    const {openSaveDatabase,saveKey,commitSaveRecord,readSaveRecord,listHistoryRecords}=await import(url);
    const db=await openSaveDatabase();
    // This case isolates IDB keys and CAS; payload validation is injected.
    const inspect=async json=>JSON.parse(json).revision;
    const other='a'.repeat(64),slot=2;
    const metadata=(releaseDigest)=>({gameId,profile,releaseDigest,slot,version:'test'});
    const a=saveKey(gameId,profile,digest,slot),b=saveKey(gameId,profile,other,slot);
    const oppositeProfile=profile==='dev'?'release':'dev',devKey=saveKey(gameId,oppositeProfile,digest,slot);
    await commitSaveRecord(db,a,{revision:1,release:digest},metadata(digest),0,inspect);
    await commitSaveRecord(db,b,{revision:1,release:other},metadata(other),0,inspect);
    await commitSaveRecord(db,devKey,{revision:1,release:'opposite-profile'},
      {...metadata(digest),profile:oppositeProfile},0,inspect);
    let conflict=false;
    try{await commitSaveRecord(db,a,{revision:1,release:digest},metadata(digest),0,inspect);}catch(e){conflict=String(e).includes('E_SAVE_CONFLICT');}
    const sameRelease=await readSaveRecord(db,a),otherRelease=await readSaveRecord(db,b);
    const history=await listHistoryRecords(db,gameId,profile);
    const oppositeHistory=await listHistoryRecords(db,gameId,oppositeProfile);
    db.close();
    return {conflict,sameRelease,otherRelease,history:history.map(row=>row.key[2]),oppositeHistory:oppositeHistory.map(row=>row.key[1])};
  },module);
  expect(result.conflict).toBe(true);
  expect(result.sameRelease.envelope.release).toBe(module.digest);
  expect(result.otherRelease.envelope.release).toBe('a'.repeat(64));
  expect(result.history).toEqual(expect.arrayContaining([module.digest,'a'.repeat(64)]));
  expect(result.history).toHaveLength(2);
  expect(result.oppositeHistory).toEqual([module.profile==='dev'?'release':'dev']);
});

test('history keeps unavailable saves exportable and only opens validated releases',async({page})=>{
  await boot(page);
  const module=await hostModule(page);
  // A real Engine envelope is valid; only release availability is injected.
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>__nir.state().dialogue?.ready&&!__nir.state().loading);
  await page.evaluate(()=>__nir.action({type:'save',slot:1}));
  await page.waitForFunction(()=>__nir.state().status==='Saved');
  await page.evaluate(async({url,gameId,profile,digest})=>{
    const {openSaveDatabase,saveKey,readSaveRecord}=await import(url),db=await openSaveDatabase();
    const key=saveKey(gameId,profile,digest,1),record=await readSaveRecord(db,key);
    await new Promise((ok,no)=>{const tx=db.transaction('saves','readwrite');tx.objectStore('saves').put({...record,version:'0.9'},key);tx.oncomplete=ok;tx.onabort=()=>no(tx.error);});db.close();
  },module);
  await page.route(`**/releases/${module.digest}.json`,route=>route.fulfill({status:404,body:'Injected unavailable release'}));
  await page.evaluate(()=>window.__nir.action({type:'menu'}));
  await page.locator('#nir-history-button').click();
  const row=page.locator('#nir-history-panel tr').filter({hasText:'0.9'});
  await expect(row).toContainText('Release resources unavailable');
  await expect(row.getByRole('button',{name:'Open release'})).toBeDisabled();
  const downloadPromise=page.waitForEvent('download');
  await row.getByRole('button',{name:'Export'}).click();
  const download=await downloadPromise;
  expect(download.suggestedFilename()).toContain('slot-1');
  const stream=await download.createReadStream(),chunks=[];
  for await(const chunk of stream)chunks.push(chunk);
  expect(JSON.parse(Buffer.concat(chunks).toString()).snapshot.release).toBe(module.digest);
  await expect(page.locator('#nir-history-panel')).toBeVisible();
});

test('history target requires matching game and digest before navigation',async({page})=>{
  await boot(page);
  const module=await hostModule(page);
  const result=await page.evaluate(async({url,digest,gameId,profile})=>{
    const {validateHistoryTarget}=await import(url);
    const root=new URL('/',location.href).href;
    const valid=await validateHistoryTarget({releaseRoot:root,digest,gameId,profile});
    const differentGame=await validateHistoryTarget({releaseRoot:root,digest,gameId:'another-game',profile});
    const differentProfile=await validateHistoryTarget({releaseRoot:root,digest,gameId,profile:profile==='dev'?'release':'dev'});
    const missing=await validateHistoryTarget({releaseRoot:root,digest:'c'.repeat(64),gameId,profile});
    return {valid,differentGame,differentProfile,missing};
  },module);
  expect(result.valid).toMatchObject({available:true,url:new URL(`/releases/${module.digest}/index.html`,page.url()).href});
  expect(result.differentGame.available).toBe(false);
  expect(result.differentProfile.available).toBe(false);
  expect(result.missing.available).toBe(false);
});
