import {test,expect} from '@playwright/test';

test('continuous history prepares, scrolls with guarded input, reflows and releases on close',async({page})=>{
  await page.setViewportSize({width:1280,height:720});
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto('http://127.0.0.1:4218/?test=1&backend=webgl2',{waitUntil:'domcontentloaded'});
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  await page.keyboard.press('Enter');
  for(let count=1;count<=40;count++){
    await page.waitForFunction(n=>window.__nir.state().history_count===n&&window.__nir.state().dialogue?.ready&&!window.__nir.state().loading,count);
    if(count<40)await page.evaluate(()=>window.__nir.action({type:'advance'}));
  }
  async function settled(){
    await page.waitForFunction(()=>{
      const s=window.__nir.state();return s.screen==='Menu'&&!s.loading&&!s.history_pending&&s.scrolls.some(v=>v.menu)&&!s.history_error;
    });
  }
  async function view(){return page.evaluate(()=>window.__nir.state().scrolls.find(v=>v.menu));}
  const command=(v,input)=>({type:'menu_history_scroll',...v.menu,input});
  await page.keyboard.press('Escape');await settled();
  const initial=await view();
  expect(initial.offset).toBe(initial.max);
  expect(initial.max).toBeGreaterThan(initial.rect[3]);
  const before=await page.evaluate(()=>{const s=window.__nir.state();return {position:s.position,tick:s.tick_us,interaction:s.interaction,variables:s.variables,history:s.history_count};});
  const newest=await page.screenshot({clip:{x:80,y:120,width:600,height:360}});
  await page.mouse.move(300,260);await page.mouse.wheel(0,-100);
  await expect.poll(async()=>(await view()).offset).toBe(initial.offset-initial.step);
  const older=await page.screenshot({clip:{x:80,y:120,width:600,height:360}});
  expect(older.equals(newest)).toBe(false);
  const stale=command(initial,{type:'position',ratio:0});
  await page.evaluate(a=>window.__nir.action(a),stale);
  expect((await view()).offset).toBe(initial.offset-initial.step);
  await page.keyboard.press('PageUp');
  await expect.poll(async()=>(await view()).offset).toBe(initial.offset-initial.step-180);
  const current=await view();
  await page.evaluate(a=>window.__nir.action(a),command(current,{type:'position',ratio:-1}));
  expect((await view()).offset).toBe(current.offset);
  await page.evaluate(()=>window.__nir.action({type:'font_size',delta:0.25}));
  await settled();
  const resized=await view();
  expect(resized.menu.revision).toBeGreaterThan(current.menu.revision);
  expect(resized.menu.layout).toBeGreaterThan(current.menu.layout);
  expect(resized.offset).toBeLessThan(resized.max);
  await page.evaluate(a=>window.__nir.action(a),command(current,{type:'position',ratio:0}));
  expect((await view()).offset).toBe(resized.offset);
  // Pointer outside the clipped viewport does not scroll it.
  await page.mouse.move(1000,300);await page.mouse.wheel(0,-100);
  expect((await view()).offset).toBe(resized.offset);
  const after=await page.evaluate(()=>{const s=window.__nir.state();return {position:s.position,tick:s.tick_us,interaction:s.interaction,variables:s.variables,history:s.history_count};});
  expect(after).toEqual(before);
  await page.keyboard.press('Escape');
  await page.waitForFunction(()=>window.__nir.state().screen==='Story');
  expect(await page.evaluate(()=>window.__nir.state().scrolls.some(v=>v.menu))).toBe(false);
  await page.keyboard.press('Escape');await settled();
  const reopened=await view();
  expect(reopened.menu.instance).not.toBe(resized.menu.instance);
  expect(reopened.offset).toBe(reopened.max);
  await page.evaluate(a=>window.__nir.action(a),command(resized,{type:'position',ratio:0}));
  expect((await view()).offset).toBe(reopened.offset);
  expect(errors).toEqual([]);
});
