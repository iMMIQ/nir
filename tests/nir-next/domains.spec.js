import {test,expect} from '@playwright/test';

test('real device clocks isolate menu playback from Story and background suspension',async({page})=>{
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.addInitScript(()=>{
    window.deviceClocks=[];
    const Native=window.AudioContext;
    window.AudioContext=class extends Native {
      constructor(...args){super(...args);window.deviceClocks.push(this);}
    };
  });
  await page.goto('/?test=1&backend=webgl2',{waitUntil:'domcontentloaded'}).catch(error=>{
    if(!error.message.includes('interrupted'))throw error;
  });
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>window.__nir.state().dialogue && window.deviceClocks.every(c=>c.state==='running'));
  expect(await page.evaluate(()=>window.deviceClocks.length)).toBe(4);
  // Exercise the real foreground sample clock with an original silent buffer.
  // Declarative menu-media binding is a later phase; this checks the host route.
  await page.evaluate(()=>{
    const context=window.deviceClocks[1],source=context.createBufferSource();
    source.buffer=context.createBuffer(1,context.sampleRate,context.sampleRate);
    source.loop=true;source.connect(context.destination);source.start();
    window.foregroundProbe=source;
  });
  await page.evaluate(()=>window.__nir.action({type:'menu'}));
  await page.waitForFunction(()=>window.deviceClocks[0].state==='running'&&window.deviceClocks[1].state==='running'&&window.deviceClocks[2].state==='suspended'&&window.deviceClocks[3].state==='suspended');
  const logical=await page.evaluate(()=>({story:window.__nir.state().tick_us}));
  const menu=await page.evaluate(()=>window.deviceClocks.map(c=>c.currentTime));
  await page.waitForTimeout(250);
  const later=await page.evaluate(()=>window.deviceClocks.map(c=>c.currentTime));
  // The foreground device clock advances even when no authored UI animation
  // holds a logical foreground clock lease. Page fades are tested separately.
  expect(await page.evaluate(()=>window.__nir.state().tick_us)).toBe(logical.story);
  expect(later[0]-menu[0]).toBeGreaterThan(.1);
  expect(later[2]).toBe(menu[2]);expect(later[3]).toBe(menu[3]);
  expect(later[1]-menu[1]).toBeGreaterThan(.1);
  await page.evaluate(()=>window.__nir.hidden(true));
  await page.waitForFunction(()=>window.deviceClocks.every(c=>c.state==='suspended'));
  const hidden=await page.evaluate(()=>window.deviceClocks.map(c=>c.currentTime));
  await page.waitForTimeout(150);
  expect(await page.evaluate(()=>window.deviceClocks.map(c=>c.currentTime))).toEqual(hidden);
  await page.evaluate(()=>window.__nir.hidden(false));
  await page.waitForFunction(()=>window.deviceClocks[1].state==='running');
  expect(await page.evaluate(()=>window.deviceClocks[0].state)).toBe('running');
  expect(await page.evaluate(()=>window.deviceClocks[2].state)).toBe('suspended');
  await page.evaluate(()=>{window.foregroundProbe.stop();window.foregroundProbe.disconnect();window.__nir.action({type:'close'});});
  await page.waitForFunction(()=>window.deviceClocks.every(c=>c.state==='running'));
  expect(errors).toEqual([]);
});
