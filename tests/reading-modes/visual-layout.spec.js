import {test,expect} from '@playwright/test';
import {recoverAudioOutput} from '../nir-next/audio-output-helper.js';

test.use({hasTouch:true,deviceScaleFactor:2});
async function control(page,type){
  await page.waitForFunction(type=>[...document.querySelectorAll('#actions button')].some(b=>!b.disabled&&JSON.parse(b.dataset.action).type===type),type);
  return page.evaluate(type=>{
    const button=[...document.querySelectorAll('#actions button')].find(b=>!b.disabled&&JSON.parse(b.dataset.action).type===type);
    const [x,y,width,height]=JSON.parse(button.dataset.rect),canvas=document.querySelector('#stage').getBoundingClientRect();
    return {x:x+canvas.left,y:y+canvas.top,width,height};
  },type);
}
async function tap(page,type){const r=await control(page,type);await page.touchscreen.tap(r.x+r.width/2,r.y+r.height/2);}

for(const worker of ['required','main'])for(const safeArea of [false,true]){
  test(`responsive visual controls and safe-area input; ${worker}; inset=${safeArea}`,async({page},info)=>{
    const errors=[];page.on('pageerror',e=>errors.push(e.message));
    if(safeArea)await page.addInitScript(()=>document.addEventListener('DOMContentLoaded',()=>{
      for(const [edge,value] of Object.entries({left:12,right:18,top:24,bottom:30}))document.documentElement.style.setProperty(`--nir-safe-${edge}`,`${value}px`);
    }));
    await page.setViewportSize({width:390,height:844});
    await page.goto(`http://127.0.0.1:4271/?test=1&worker=${worker}&backend=webgl2`);
    await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    await tap(page,'new_game');await recoverAudioOutput(page);
    await page.waitForFunction(()=>__nir.state().dialogue?.id==='intro'&&__nir.state().dialogue.ready&&!__nir.state().loading&&!__nir.state().paused);
    const identity=await page.evaluate(()=>({session:__nir.state().session,interaction:__nir.state().interaction}));
    const dialogue=await control(page,'advance');
    expect(dialogue.height).toBeLessThan(330);
    expect(dialogue.height).toBeGreaterThanOrEqual(140);
    expect(dialogue.y+dialogue.height).toBeLessThanOrEqual(844-(safeArea?30:0));
    const canvas=await page.locator('#stage').boundingBox();
    expect(canvas).toEqual({x:safeArea?12:0,y:safeArea?24:0,width:safeArea?360:390,height:safeArea?790:844});
    await page.screenshot({path:info.outputPath('portrait.png')});
    await page.evaluate(()=>__nir.action({type:'font_size',delta:.5}));
    await page.waitForFunction(()=>__nir.state().preferences.font_scale===1.5);
    expect((await control(page,'advance')).height).toBeGreaterThanOrEqual(dialogue.height);
    await page.setViewportSize({width:844,height:390});
    await page.waitForFunction(()=>{
      const menu=[...document.querySelectorAll('#actions button')].find(b=>JSON.parse(b.dataset.action).type==='menu');
      return menu&&JSON.parse(menu.dataset.rect)[2]===115;
    });
    const menu=await control(page,'menu');
    expect(menu.x+menu.width).toBeLessThanOrEqual(844-(safeArea?18:0));
    await page.screenshot({path:info.outputPath('landscape-large-font.png')});
    // Compare actual rendered pixels while the story identity stays fixed.
    await page.mouse.move(1,1);
    const normal=(await page.screenshot({clip:menu})).toString('base64');
    await page.mouse.move(menu.x+menu.width/2,menu.y+menu.height/2);
    await expect.poll(async()=> (await page.screenshot({clip:menu})).toString('base64')).not.toBe(normal);
    const hover=(await page.screenshot({clip:menu})).toString('base64');
    await page.mouse.down();
    await expect.poll(async()=> (await page.screenshot({clip:menu})).toString('base64')).not.toBe(hover);
    await page.mouse.up();
    await page.waitForFunction(()=>__nir.state().screen==='Menu');
    await page.screenshot({path:info.outputPath('menu.png')});
    const close=await control(page,'close');
    await page.evaluate(()=>[...document.querySelectorAll('#actions button')].find(b=>JSON.parse(b.dataset.action).type==='close').focus());
    expect(await page.locator('#focus-ring').boundingBox()).toEqual(close);
    expect(await page.locator('#focus-ring').evaluate(e=>e.style.borderColor)).toBe('rgb(212, 186, 122)');
    expect(await page.evaluate(()=>({session:__nir.state().session,interaction:__nir.state().interaction}))).toEqual(identity);
    expect(errors).toEqual([]);
  });
}
