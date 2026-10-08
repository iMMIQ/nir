import {test,expect} from '@playwright/test';
import {recoverAudioOutput} from './audio-output-helper.js';

// The sampled columns sit at x=.1/.9 of the viewport and y=.3 — inside the
// full-stage page background but clear of the dialogue window (y>=450), the
// story HUD (y<52) and the authored close button (x 490..790, y 320..400).
// Styled boundaries cap at 2 s (E_VIEW_EFFECTS) and the renderer submits a
// frame only a few times per second under software rasterization, so a live
// capture can neither lag nor lead the polled state by a bounded amount.
// Freezing both time domains on the crossing frame — like the window/wipe
// specs — caps the presented progress at the frozen value: the leading
// column then stays pure page for any frozen progress above .3 and the
// trailing column stays pure frame below .8.
async function columns(page,screenshot){
  return page.evaluate(async base64=>{
    const image=await createImageBitmap(new Blob([Uint8Array.from(atob(base64),c=>c.charCodeAt(0))],{type:'image/png'}));
    const canvas=document.createElement('canvas');canvas.width=image.width;canvas.height=image.height;
    const ctx=canvas.getContext('2d');ctx.drawImage(image,0,0);image.close();
    return [.1,.9].map(x=>Array.from(ctx.getImageData(Math.floor(canvas.width*x),Math.floor(canvas.height*.3),1,1).data));
  },screenshot.toString('base64'));
}
// Alpha is always 255 here; only the color channels speak.
const page_=(p)=>p[0]<25&&p[1]<25&&p[2]<25;
const story_=(p)=>p[0]>200&&p[1]<45&&p[2]<45;
// The settled frame can present a few hundred ms after the state flips —
// software rasterization submits sparingly, and the flip turn itself may not
// dirty a re-render. Sample until the expected content appears, bounded.
async function settled(page,path,predicate){
  let sampled;
  for(let attempt=0;attempt<4;attempt++){
    sampled=await columns(page,await page.screenshot({path}));
    if(sampled.every(predicate))return sampled;
    await page.waitForTimeout(350);
  }
  return sampled;
}
// Cross the bound from inside the page and freeze both time domains on that
// same frame; the frozen progress cannot drift past the crossing.
async function freezeMidFlight(page,key){
  const crossing=await page.evaluate(k=>new Promise(resolve=>{
    (function check(){
      const p=window.__nir.state()[k];
      if(p!==null&&p>.45){window.__nir.hidden(true);resolve(p);return;}
      requestAnimationFrame(check);
    })();
  }),key);
  expect(crossing).toBeLessThan(.75);
  const frozen=await page.evaluate(k=>window.__nir.state()[k],key);
  expect(frozen).toBeGreaterThan(.4);
  expect(frozen).toBeLessThan(.8);
  return frozen;
}

test('spatial page reveals composite over the frozen frame and defer the close exit',async({page})=>{
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.setViewportSize({width:1280,height:720});
  await page.goto('http://127.0.0.1:4224/?test=1&backend=webgl2',{waitUntil:'domcontentloaded'});
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  // Let the title's own enter reveal settle before leaving the page.
  await page.waitForFunction(()=>window.__nir.state().menu_transition===null);
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>window.__nir.state().screen==='Story'&&!window.__nir.state().loading);
  await recoverAudioOutput(page);
  await page.waitForFunction(()=>{const s=window.__nir.state();return s.screen==='Story'&&s.dialogue?.ready&&!s.loading;});
  await page.keyboard.press('Escape');
  await page.waitForFunction(()=>{const s=window.__nir.state();return s.screen==='Menu'&&!s.loading;});
  // The reveal wipes the page in from the left over the frozen story frame.
  await freezeMidFlight(page,'menu_transition');
  // The reveal owns visibility: no alpha ramp rides on top of the composite.
  expect(await page.evaluate(()=>window.__nir.state().menu_opacity)).toBe(1);
  const enterMid=await columns(page,await page.screenshot({path:'reports/nir-next/menu-wipe-enter.png'}));
  expect(page_(enterMid[0]),'revealed side shows the page').toBe(true);
  expect(story_(enterMid[1]),'trailing side keeps the frozen frame').toBe(true);
  await page.evaluate(()=>window.__nir.hidden(false));
  await page.waitForFunction(()=>window.__nir.state().menu_transition===null);
  const enterDone=await settled(page,'reports/nir-next/menu-wipe-enter-done.png',page_);
  expect(enterDone.every(page_),'finished reveal covers the frame').toBe(true);
  // Close reverses the composite: the sound and lock land immediately, the
  // exit itself waits for the erase to finish.
  await page.waitForFunction(()=>__nir.diagnostics().host_work.audio_domains.story.buses.bgm.state==='running'||
    document.querySelector('#nir-audio-resume')?.hidden===false);
  await recoverAudioOutput(page);
  await page.getByRole('button',{name:'Return to story',exact:true}).focus();
  await page.keyboard.press('Enter');
  expect(await page.evaluate(()=>window.__nir.state().screen)).toBe('Menu');
  await freezeMidFlight(page,'menu_transition');
  expect(await page.evaluate(()=>window.__nir.state().paused)).toBe(true);
  expect(await page.evaluate(()=>window.__nir.state().menu_opacity)).toBe(1);
  const closeMid=await columns(page,await page.screenshot({path:'reports/nir-next/menu-wipe-close.png'}));
  // The erase runs right-to-left: the page still owns the left, the frame
  // shows through on the right.
  expect(page_(closeMid[0]),'the page erases last on its leading side').toBe(true);
  expect(story_(closeMid[1]),'the frame returns where the erase has passed').toBe(true);
  await page.evaluate(()=>window.__nir.hidden(false));
  await page.waitForFunction(()=>window.__nir.state().screen==='Story');
  await recoverAudioOutput(page);
  expect(await page.evaluate(()=>window.__nir.state().paused)).toBe(false);
  const closed=await settled(page,'reports/nir-next/menu-wipe-close-end.png',story_);
  expect(closed.every(story_),'the finished close leaves the story bare').toBe(true);
  expect(errors).toEqual([]);
});
