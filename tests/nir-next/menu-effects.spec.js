import {test,expect} from '@playwright/test';

test('page effects enter once per prepared page, stop with it, and defer closes behind the fade',async({page})=>{
  await page.setViewportSize({width:1280,height:720});
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.addInitScript(()=>{
    window.deviceClocks=[];
    const Native=window.AudioContext;
    window.AudioContext=class extends Native{constructor(...a){super(...a);window.deviceClocks.push(this);}};
    window.contextStarts=[0,0];window.contextStops=[0,0];
    const proto=AudioBufferSourceNode.prototype;
    const start=proto.start,stop=proto.stop;
    proto.start=function(...a){try{const i=window.deviceClocks.indexOf(this.context);if(i>=0)window.contextStarts[i]++;}catch(e){}return start.apply(this,a);};
    proto.stop=function(...a){try{const i=window.deviceClocks.indexOf(this.context);if(i>=0)window.contextStops[i]++;}catch(e){}try{return stop.apply(this,a);}catch(e){}};
  });
  await page.goto('http://127.0.0.1:4220/?test=1&backend=webgl2',{waitUntil:'domcontentloaded'});
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  const ui=()=>page.evaluate(()=>({starts:window.contextStarts[1],stops:window.contextStops[1]}));
  // The title enter fired exactly once after preparation: one sound plus one
  // looping music voice on the foreground domain, never again on later polls.
  expect(await ui()).toEqual({starts:2,stops:0});
  await page.waitForTimeout(200);
  expect(await ui()).toEqual({starts:2,stops:0});
  expect(await page.evaluate(()=>window.__nir.state().menu_opacity)).toBeLessThan(1);
  await page.waitForFunction(()=>window.__nir.state().menu_opacity===1);
  // Start leaves the title for a new session; its looping music dies with the
  // page through the session audio reset, and the story starts its own BGM.
  const button=name=>page.getByRole('button',{name,exact:true});
  await button('Start').focus();await page.keyboard.press('Enter');
  await page.waitForFunction(()=>{const s=window.__nir.state();return s.screen==='Story'&&s.dialogue?.ready&&!s.loading;});
  const story=await page.evaluate(()=>({starts:window.contextStarts[0],uiStops:window.contextStops[1]}));
  expect(story.starts).toBeGreaterThan(0);
  expect(story.uiStops).toBeGreaterThan(0);
  // The overlay's own page fires after its preparation finishes.
  await page.keyboard.press('Escape');
  await page.waitForFunction(()=>{const s=window.__nir.state();return s.screen==='Menu'&&!s.loading;});
  await page.waitForFunction(()=>window.contextStarts[1]>=4);
  // An authored menu track replaces ambience: retain and suspend Story/BGM,
  // rather than mixing two songs or destroying the original source.
  await page.waitForFunction(()=>window.deviceClocks[0].state==='suspended');
  const overlay=await ui();
  expect(overlay.starts).toBe(4);
  await page.waitForTimeout(150);
  expect(await ui()).toEqual(overlay);
  await page.waitForFunction(()=>window.__nir.state().menu_opacity===1);
  // Acceptance click feedback: exactly one sound per accepted commit, none
  // for a stale menu request.
  const stale=JSON.parse(await button('Increase speed').getAttribute('data-action'));
  await button('Increase speed').focus();await page.keyboard.press('Enter');
  await page.waitForFunction(n=>window.contextStarts[1]===n,overlay.starts+1);
  await page.evaluate(a=>window.__nir.action(a),stale);
  await page.waitForTimeout(120);
  expect(await ui()).toEqual({starts:overlay.starts+1,stops:overlay.stops});
  // Close plays its sound immediately, keeps the page and its input lock while
  // the fade runs, and commits the exit -- stopping the looping music -- only
  // when the fade completes.
  await page.keyboard.press('Escape');
  expect(await page.evaluate(()=>window.__nir.state().screen)).toBe('Menu');
  await page.waitForFunction(n=>window.contextStarts[1]===n,overlay.starts+2);
  await page.waitForFunction(()=>window.__nir.state().menu_opacity<1);
  const midFade=await ui();
  expect(midFade.stops).toBe(overlay.stops);
  await page.waitForFunction(()=>window.__nir.state().screen==='Story');
  await page.waitForFunction(()=>window.deviceClocks[0].state==='running');
  const closed=await ui();
  expect(closed.stops).toBeGreaterThan(overlay.stops);
  expect(await page.evaluate(()=>window.__nir.state().menu_opacity)).toBe(1);
  expect(errors).toEqual([]);
});
