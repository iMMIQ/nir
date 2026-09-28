import {test,expect} from '@playwright/test';

for(const [port,paused] of [[4199,false],[4200,true]]) {
  test(`temporary hide restores without advancing and Story pause is explicit (${paused})`,async({page})=>{
    const errors=[];page.on('pageerror',e=>errors.push(e.message));
    await page.addInitScript(()=>{
      window.deviceClocks=[];const Native=window.AudioContext;
      window.AudioContext=class extends Native { constructor(...args){super(...args);window.deviceClocks.push(this);} };
    });
    await page.goto(`http://127.0.0.1:${port}/?test=1&backend=webgl2`,{waitUntil:'domcontentloaded'}).catch(e=>{if(!e.message.includes('interrupted'))throw e;});
    await page.waitForFunction(()=>window.__nir?.state().ready&&!window.__nir.state().loading);
    await page.keyboard.press('Enter');
    await page.waitForFunction(()=>window.__nir.state().dialogue&&!window.__nir.state().loading);
    await page.evaluate(()=>window.__nir.action({type:'advance'}));
    await page.waitForFunction(()=>window.__nir.state().dialogue?.ready&&window.deviceClocks[0].state==='running');
    const token=await page.evaluate(()=>window.__nir.state().interaction);
    if(paused)await page.screenshot({path:'reports/nir-next/hide-custom-ready.png'});
    await page.keyboard.press('h');
    await page.waitForFunction(paused=>window.__nir.state().interface_hidden&&window.__nir.state().paused===paused,paused);
    await page.waitForFunction(paused=>window.deviceClocks[0].state===(paused?'suspended':'running'),paused);
    await expect(page.locator('#actions button')).toHaveCount(1);
    const before=await page.evaluate(()=>window.deviceClocks[0].currentTime);
    if(paused) {
      await page.waitForTimeout(350);
      expect(await page.evaluate(()=>window.deviceClocks[0].currentTime)-before).toBeLessThan(.02);
    } else {
      // Verify continued device playback, not its throughput under CPU contention.
      await expect.poll(async()=>await page.evaluate(()=>window.deviceClocks[0].currentTime)-before).toBeGreaterThan(.2);
      expect(await page.evaluate(()=>window.deviceClocks[0].state)).toBe('running');
    }
    expect(await page.evaluate(()=>window.__nir.state().interaction)).toBe(token);
    await page.mouse.click(20,200);
    await page.waitForFunction(()=>!window.__nir.state().interface_hidden&&!window.__nir.state().paused);
    expect(await page.evaluate(()=>window.__nir.state().interaction)).toBe(token);
    await page.waitForFunction(()=>window.deviceClocks[0].state==='running');
    expect(errors).toEqual([]);
  });
}
