import {test,expect} from '@playwright/test';
import {recoverAudioOutput} from '../nir-next/audio-output-helper.js';

async function pixels(page) {
  const shot=await page.screenshot();
  return page.evaluate(async data=>{
    const bitmap=await createImageBitmap(new Blob([Uint8Array.from(atob(data),c=>c.charCodeAt(0))],{type:'image/png'}));
    const canvas=document.createElement('canvas');canvas.width=bitmap.width;canvas.height=bitmap.height;
    const ctx=canvas.getContext('2d');ctx.drawImage(bitmap,0,0);bitmap.close();
    return [[720,110],[720,135],[705,110],[742,110]].map(([x,y])=>Array.from(ctx.getImageData(x,y,1,1).data));
  },shot.toString('base64'));
}
for(const worker of ['required','main']) {
  test(`rotated original texture preserves UV, anisotropic scale, parent clip and cold restore; ${worker}`,async({page})=>{
    test.skip(process.env.NIR_TEST_SPRITE_TRANSFORM!=='1','Run the separate sprite-transform fixture');
    const errors=[];page.on('pageerror',e=>errors.push(e.message));
    await page.setViewportSize({width:1280,height:720});
    await page.goto(`http://127.0.0.1:4268/?test=1&worker=${worker}&backend=webgl2`);
    await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    await page.getByRole('button',{name:'Start',exact:true}).focus();await page.keyboard.press('Enter');
    await page.waitForFunction(()=>__nir.state().dialogue?.id==='intro'&&!__nir.state().loading);await recoverAudioOutput(page);
    const expected=[[255,0,255,255],[0,255,255,255],[0,0,0,255],[0,0,0,255]];
    expect(await pixels(page)).toEqual(expected);
    await page.evaluate(()=>__nir.action({type:'menu'}));
    await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
    await page.evaluate(()=>__nir.action({type:'save',slot:1}));
    await page.waitForFunction(()=>/Saved|已保存/.test(__nir.state().status));
    await page.reload();await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    const session=await page.evaluate(()=>__nir.state().session);
    await page.evaluate(()=>__nir.action({type:'load',slot:1}));
    await page.waitForFunction(s=>__nir.state().session>s&&!__nir.state().loading&&__nir.state().paused,session);
    expect(await pixels(page)).toEqual(expected);
    const tick=await page.evaluate(()=>__nir.state().tick_us);await page.waitForTimeout(100);
    expect(await page.evaluate(()=>__nir.state().tick_us)).toBe(tick);
    expect(await pixels(page)).toEqual(expected);
    expect(await page.evaluate(()=>__nir.state().error)).toBeNull();expect(errors).toEqual([]);
  });
}
