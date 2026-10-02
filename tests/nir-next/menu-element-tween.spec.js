import {test,expect} from '@playwright/test';

// The system page slides one 400 px image element from 1400 px right of its
// authored spot (x 80..480) across the 2 s E_VIEW_EFFECTS cap, while an
// unanimated anchor holds the authored row below. Sampling rows sit 10 px
// inside each element's top edge — clear of every label and of the authored
// close button (y 320..400). Freezing both time domains on the crossing
// frame caps the presented slide at the frozen progress, so the frozen
// button covers x=800 exactly while that progress sits in its coverage
// window (0.486..0.771 — the crossing bounds live strictly inside), and the
// settle column x=230 stays bare page until the slide is 89% done.
async function pixels(page,screenshot,spots){
  return page.evaluate(async({base64,spots})=>{
    const image=await createImageBitmap(new Blob([Uint8Array.from(atob(base64),c=>c.charCodeAt(0))],{type:'image/png'}));
    const canvas=document.createElement('canvas');canvas.width=image.width;canvas.height=image.height;
    const ctx=canvas.getContext('2d');ctx.drawImage(image,0,0);image.close();
    return spots.map(([x,y])=>Array.from(ctx.getImageData(Math.floor(canvas.width*x),Math.floor(canvas.height*y),1,1).data));
  },{base64:screenshot.toString('base64'),spots});
}
// menu.blue is [0,0,255,128] over the black page, so a covered pixel blends
// to roughly (0,0,128); alpha is always 255 here.
const blue=(p)=>p[2]>90&&p[0]<45&&p[1]<45;
const black=(p)=>p[0]<25&&p[1]<25&&p[2]<25;
const spots=[[800/1280,190/720],[230/1280,190/720],[230/1280,610/720]];
// The settled frame can present a few hundred ms after the state flips —
// software rasterization submits sparingly. Sample until every spot matches
// its predicate, bounded.
async function settled(page,path,predicates){
  let sampled;
  for(let attempt=0;attempt<4;attempt++){
    sampled=await pixels(page,await page.screenshot({path}),spots);
    if(sampled.every((p,i)=>predicates[i](p)))return sampled;
    await page.waitForTimeout(350);
  }
  return sampled;
}
// Cross the bound from inside the page and freeze both time domains on that
// same frame; the frozen progress cannot drift past the crossing.
async function freezeMidFlight(page){
  const crossing=await page.evaluate(()=>new Promise(resolve=>{
    (function check(){
      const p=window.__nir.state().menu_element_progress;
      if(p!==null&&p>.55){window.__nir.hidden(true);resolve(p);return;}
      requestAnimationFrame(check);
    })();
  }));
  expect(crossing).toBeLessThan(.70);
  const frozen=await page.evaluate(()=>window.__nir.state().menu_element_progress);
  expect(frozen).toBeGreaterThan(.5);
  expect(frozen).toBeLessThan(.75);
  return frozen;
}

test('element enter tweens ride the shared page surface and settle to authored',async({page})=>{
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.setViewportSize({width:1280,height:720});
  await page.goto('http://127.0.0.1:4225/?test=1&backend=webgl2',{waitUntil:'domcontentloaded'});
  await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>{const s=window.__nir.state();return s.screen==='Story'&&s.dialogue?.ready&&!s.loading;});
  await page.keyboard.press('Escape');
  await page.waitForFunction(()=>{const s=window.__nir.state();return s.screen==='Menu'&&!s.loading;});
  // The slide is the only motion: the page never diverts into a reveal root
  // and never alpha-fades under an enter effect.
  const frozen=await freezeMidFlight(page);
  expect(await page.evaluate(()=>window.__nir.state().menu_transition)).toBe(null);
  expect(await page.evaluate(()=>window.__nir.state().menu_opacity)).toBe(1);
  const mid=await pixels(page,await page.screenshot({path:'reports/nir-next/menu-element-enter.png'}),spots);
  expect(blue(mid[0]),`the sliding button covers its column at frozen ${frozen}`).toBe(true);
  expect(black(mid[1]),'the authored spot stays bare page mid-flight').toBe(true);
  expect(blue(mid[2]),'the unanimated anchor stays composited').toBe(true);
  await page.evaluate(()=>window.__nir.hidden(false));
  await page.waitForFunction(()=>window.__nir.state().menu_element_progress===null);
  const done=await settled(page,'reports/nir-next/menu-element-enter-done.png',[black,blue,blue]);
  expect(black(done[0]),'the mid-flight column is bare page once settled').toBe(true);
  expect(blue(done[1]),'the slide settles onto its authored spot').toBe(true);
  expect(blue(done[2]),'the anchor never moved').toBe(true);
  // Leaving the page retires the tracks; the progress surface rests at null.
  await page.getByRole('button',{name:'Return to story',exact:true}).focus();
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>window.__nir.state().screen==='Story');
  expect(await page.evaluate(()=>window.__nir.state().menu_element_progress)).toBe(null);
  expect(errors).toEqual([]);
});
