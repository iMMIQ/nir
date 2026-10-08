import {test,expect} from '@playwright/test';

test('image history scrollbar continuously drags, paints states and rejects stale gestures',async({page},testInfo)=>{
  await page.setViewportSize({width:1280,height:720});
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto('http://127.0.0.1:4218/?test=1&backend=webgl2',{waitUntil:'domcontentloaded'});
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  await page.keyboard.press('Enter');
  for(let count=1;count<=40;count++){
    await page.waitForFunction(n=>window.__nir.state().history_count===n&&window.__nir.state().dialogue?.ready&&!window.__nir.state().loading,count);
    if(count<40)await page.evaluate(()=>window.__nir.action({type:'advance'}));
  }
  async function settled(width=1280){await page.waitForFunction(width=>{const s=window.__nir.state();return s.screen==='Menu'&&!s.loading&&!s.history_pending&&s.history_scrollbar?.enabled&&s.history_scrollbar.rect[2]===width/40&&!s.history_error;},width);}
  async function bar(){return page.evaluate(()=>window.__nir.state().history_scrollbar);}
  async function color(x,y){
    const bytes=await page.screenshot();
    return page.evaluate(async({base64,x,y})=>{
      const bitmap=await createImageBitmap(new Blob([Uint8Array.from(atob(base64),c=>c.charCodeAt(0))],{type:'image/png'}));
      const canvas=document.createElement('canvas');canvas.width=bitmap.width;canvas.height=bitmap.height;
      const ctx=canvas.getContext('2d');ctx.drawImage(bitmap,0,0);bitmap.close();
      return [...ctx.getImageData(Math.floor(x),Math.floor(y),1,1).data].slice(0,3);
    },{base64:bytes.toString('base64'),x,y});
  }
  await page.keyboard.press('Escape');await settled();
  const initial=await bar(),x=initial.rect[0]+initial.rect[2]/2;
  const before=await page.evaluate(()=>{const s=window.__nir.state();return {position:s.position,tick:s.tick_us,interaction:s.interaction,variables:s.variables,history:s.history_count};});
  expect(initial.offset).toBe(initial.max);
  await expect(page.getByRole('button',{name:'History scroll +',exact:true})).toBeDisabled();
  await page.mouse.move(1000,600);
  try {await expect.poll(()=>color(x,initial.thumb[1]+6)).toEqual([0,80,240]);} catch(error) {
    await testInfo.attach('initial-scrollbar-state',{body:JSON.stringify({initial,color:await color(x,initial.thumb[1]+6),
      current:await page.evaluate(()=>__nir.state())},null,2),contentType:'application/json'});
    await testInfo.attach('initial-scrollbar-frame',{body:await page.screenshot(),contentType:'image/png'});throw error;
  }
  expect(await color(x,initial.increase[1]+8)).toEqual([100,100,100]);
  // Track and thumb share one semantic node: hover must still update when
  // crossing between their image parts without changing that target.
  await page.mouse.move(x,initial.track[1]+40);
  await expect.poll(()=>color(x,initial.thumb[1]+6)).toEqual([0,80,240]);
  await page.mouse.move(x,initial.thumb[1]+6);
  await expect.poll(()=>color(x,initial.thumb[1]+6)).toEqual([0,200,80]);
  await page.mouse.down();
  await expect.poll(()=>color(x,initial.thumb[1]+6)).toEqual([240,40,0]);
  expect((await bar()).offset).toBe(initial.offset);
  const middle=initial.track[1]+initial.track[3]/2;
  await page.mouse.move(x,middle-6,{steps:4});
  await expect.poll(async()=>(await bar()).offset/initial.max).toBeCloseTo(.5,4);
  const half=await bar();
  await expect.poll(()=>color(x,half.thumb[1]+6)).toEqual([240,40,0]);
  await page.screenshot({path:'reports/nir-next/history-scrollbar-half.png'});
  // Drag updates before release. Moving outside the part keeps capture and
  // clamps to the first record instead of becoming a click on release.
  await page.mouse.move(1100,10);
  await expect.poll(async()=>(await bar()).offset).toBe(0);
  await page.mouse.up();
  await expect.poll(()=>color(x,initial.decrease[1]+8)).toEqual([100,100,100]);
  await page.mouse.click(x,initial.increase[1]+8);
  await expect.poll(async()=>(await bar()).offset).toBe(45);
  await page.mouse.click(x,initial.track[1]+200);
  await expect.poll(async()=>(await bar()).offset).toBe(225);
  const current=await bar();
  const command=(v,input)=>({type:'menu_history_scroll',...v.authority,control:v.id,input});
  await page.evaluate(a=>window.__nir.action(a),command(half,{type:'position',ratio:1}));
  expect((await bar()).offset).toBe(current.offset);
  await page.evaluate(a=>window.__nir.action(a),{...command(current,{type:'position',ratio:1}),control:'unknown'});
  expect((await bar()).offset).toBe(current.offset);
  const slider=page.getByRole('slider',{name:'History scroll',exact:true});
  await expect(slider).toHaveAttribute('aria-orientation','vertical');
  // Focus and key arrive in one DOM turn, before its queued focus notice.
  await slider.evaluate(node=>{node.focus();node.dispatchEvent(new KeyboardEvent('keydown',{key:'Home',bubbles:true}));});
  await expect.poll(async()=>(await bar()).offset).toBe(0);
  await expect(slider).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await expect.poll(async()=>(await bar()).offset).toBe(45);
  await expect(slider).toBeFocused();
  await page.keyboard.press('ArrowUp');
  await expect.poll(async()=>(await bar()).offset).toBe(0);
  await page.keyboard.press('End');
  await expect.poll(async()=>(await bar()).offset).toBe(initial.max);
  const latest=await bar();
  await page.mouse.move(x,latest.thumb[1]+12);await page.mouse.down();
  await page.mouse.move(x,middle);
  await expect.poll(async()=>(await bar()).offset/initial.max).toBeCloseTo(.5,4);
  await page.locator('canvas').dispatchEvent('pointercancel');
  const canceled=await bar();
  await page.mouse.move(x,initial.track[1]);await page.mouse.up();
  expect((await bar()).offset).toBe(canceled.offset);
  // A resize reflows history and cancels the old geometry's capture.
  const start=await bar();
  await page.mouse.move(x,start.thumb[1]+12);await page.mouse.down();
  await page.setViewportSize({width:960,height:540});await settled(960);
  const resized=await bar();
  expect(resized.authority.layout).toBeGreaterThan(start.authority.layout);
  await page.mouse.move(540,400);await page.mouse.up();
  expect((await bar()).offset).toBe(resized.offset);
  await page.setViewportSize({width:1280,height:720});await settled();
  const old=await bar();
  const paused=await page.evaluate(()=>{const s=window.__nir.state();return {position:s.position,tick:s.tick_us,interaction:s.interaction,variables:s.variables,history:s.history_count};});
  expect(paused).toEqual(before);
  await page.mouse.move(x,old.thumb[1]+12);await page.mouse.down();
  await page.keyboard.press('Escape');
  await page.waitForFunction(()=>window.__nir.state().screen==='Story');
  await page.mouse.move(300,200);await page.mouse.up();
  const after=await page.evaluate(()=>{const s=window.__nir.state();return {position:s.position,tick:s.tick_us,interaction:s.interaction,variables:s.variables,history:s.history_count};});
  // The post-close release must not reveal or advance the story.
  expect(after.position).toEqual(before.position);
  expect(after.interaction).toBe(before.interaction);
  expect(after.variables).toEqual(before.variables);
  expect(after.history).toBe(before.history);
  await page.keyboard.press('Escape');await settled();
  const reopened=await bar();
  expect(reopened.authority.instance).not.toBe(old.authority.instance);
  expect(reopened.offset).toBe(reopened.max);
  await page.evaluate(a=>window.__nir.action(a),command(old,{type:'position',ratio:0}));
  expect((await bar()).offset).toBe(reopened.offset);
  const still=await page.evaluate(()=>{const s=window.__nir.state();return {position:s.position,interaction:s.interaction,variables:s.variables,history:s.history_count};});
  expect(still).toEqual({position:before.position,interaction:before.interaction,variables:before.variables,history:before.history});
  expect(errors).toEqual([]);
});
