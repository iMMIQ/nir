import {test,expect} from '@playwright/test';

test('idle reading saves the advancing audio offset and restores its paused position',async({page})=>{
  const errors=[];page.on('pageerror',error=>errors.push(error.message));
  await page.addInitScript(()=>{
    window.offsetAudit=[];
    const create=AudioContext.prototype.createBufferSource;
    AudioContext.prototype.createBufferSource=function(){
      const source=create.call(this),start=source.start,context=this;
      source.start=function(when,offset=0,...rest){if(!window.offsetAudit.length){const end=performance.now()+800;while(performance.now()<end){}}window.offsetAudit.push({context,started:context.currentTime,offset,looped:source.loop});return start.call(this,when,offset,...rest);};
      return source;
    };
  });
  await page.goto('http://127.0.0.1:4199/?test=1&backend=webgl2',{waitUntil:'domcontentloaded'}).catch(error=>{
    if(!error.message.includes('interrupted'))throw error;
  });
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>window.__nir.state().dialogue&&!window.__nir.state().loading);
  await page.evaluate(()=>window.__nir.action({type:'advance'}));
  await page.waitForFunction(()=>window.__nir.state().dialogue?.ready&&!window.__nir.state().transition);
  const before=await page.evaluate(()=>Number(window.__nir.state().tick_us));
  // The audio device keeps playing while the owner thread is stalled.
  await page.evaluate(()=>{const until=performance.now()+800;while(performance.now()<until){};});
  await page.waitForTimeout(500);
  // Observe a completed owner turn, rather than assuming the next RAF ran
  // during a fixed wall-clock sleep on a busy renderer. Offset tolerance below
  // still rejects lost time even if later frames have caught up.
  await page.waitForFunction(t=>Number(window.__nir.state().tick_us)-t>900000,before);
  // Headless audio-device progress need not match wall time. Establish the
  // same device-position precondition that the restored-offset assertion uses.
  await page.waitForFunction(()=>{
    const v=window.offsetAudit.find(v=>v.looped);
    return v&&v.offset+v.context.currentTime-v.started>1;
  });
  await page.evaluate(()=>{const end=performance.now()+800;while(performance.now()<end){};window.__nir.action({type:'menu'});});
  await page.waitForFunction(()=>window.__nir.state().screen==='Menu'&&window.offsetAudit[0].context.state==='running');
  const savedOffset=await page.evaluate(()=>{
    const v=window.offsetAudit[0];return v.offset+v.context.currentTime-v.started;
  });
  await page.evaluate(()=>window.__nir.action({type:'save',slot:1}));
  await expect.poll(()=>page.evaluate(async()=>{
    for(const item of await indexedDB.databases()) {
      const db=await new Promise((resolve,reject)=>{const r=indexedDB.open(item.name);r.onsuccess=()=>resolve(r.result);r.onerror=()=>reject(r.error);});
      if(db.objectStoreNames.contains('saves')) {
        const count=await new Promise(resolve=>{const r=db.transaction('saves').objectStore('saves').count();r.onsuccess=()=>resolve(r.result);});
        db.close();if(count)return true;
      }else db.close();
    }return false;
  })).toBe(true);
  await page.waitForTimeout(200);
  const count=await page.evaluate(()=>window.offsetAudit.length);
  await page.evaluate(()=>window.__nir.action({type:'load',slot:1}));
  await page.waitForFunction(n=>window.offsetAudit.length>n&&!window.__nir.state().loading,count);
  const resumed=await page.evaluate(()=>window.offsetAudit.filter(v=>v.looped).at(-1).offset);
  expect(resumed).toBeGreaterThan(1);
  const timing=await page.evaluate(()=>({tick_us:window.__nir.state().tick_us,audio:window.offsetAudit.map(v=>({started:v.started,offset:v.offset,looped:v.looped,now:v.context.currentTime,state:v.context.state}))}));
  expect(Math.abs(resumed-savedOffset),JSON.stringify({resumed,savedOffset,timing})).toBeLessThan(.2);
  expect(errors).toEqual([]);
});
