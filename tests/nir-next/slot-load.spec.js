import {test,expect} from '@playwright/test';

test('leaving the save page discards a delayed IndexedDB load completion',async({page})=>{
  await page.addInitScript(()=>{
    window.heldSlotReads=[];
    const get=IDBObjectStore.prototype.get;
    IDBObjectStore.prototype.get=function(...args){
      if(this.name==='saves'&&window.holdSlotReads)this.transaction.delayedSlotRead=true;
      return get.apply(this,args);
    };
    const descriptor=Object.getOwnPropertyDescriptor(IDBTransaction.prototype,'oncomplete');
    Object.defineProperty(IDBTransaction.prototype,'oncomplete',{
      ...descriptor,set(callback){
        descriptor.set.call(this,function(event){
          const run=()=>callback?.call(this,event);
          if(this.delayedSlotRead)window.heldSlotReads.push(run);else run();
        });
      },
    });
  });
  await page.goto('/?test=1&backend=webgl2',{waitUntil:'domcontentloaded'}).catch(e=>{if(!e.message.includes('interrupted'))throw e;});
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>window.__nir.state().dialogue&&!window.__nir.state().loading);
  await page.keyboard.press('Escape');
  await page.evaluate(()=>window.__nir.action({type:'save',slot:0}));
  await page.waitForFunction(()=>/Saved|已保存/.test(window.__nir.state().status));
  await page.evaluate(()=>window.__nir.action({type:'saves'}));
  await page.waitForFunction(()=>window.__nir.state().screen==='Saves');
  const session=await page.evaluate(()=>window.__nir.state().session);
  await page.evaluate(()=>{window.holdSlotReads=true;window.__nir.action({type:'load',slot:0});});
  await page.waitForFunction(()=>window.heldSlotReads.length===1);
  await page.keyboard.press('Escape');
  await page.waitForFunction(()=>window.__nir.state().screen==='Story');
  await page.evaluate(async()=>{
    window.holdSlotReads=false;window.heldSlotReads.splice(0).forEach(run=>run());
    for(let i=0;i<4;i++)await new Promise(requestAnimationFrame);
  });
  expect(await page.evaluate(()=>window.__nir.state().session)).toBe(session);
  expect(await page.evaluate(()=>window.__nir.state().screen)).toBe('Story');
  // A fresh request still goes through the host and commits normally.
  await page.evaluate(()=>window.__nir.action({type:'saves'}));
  await page.waitForFunction(()=>window.__nir.state().screen==='Saves');
  await page.evaluate(()=>window.__nir.action({type:'load',slot:0}));
  await page.waitForFunction(s=>window.__nir.state().session>s&&!window.__nir.state().loading,session);
});
