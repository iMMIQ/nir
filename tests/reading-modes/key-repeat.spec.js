import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import {recoverAudioOutput} from '../nir-next/audio-output-helper.js';

test.afterEach(async({page},info)=>{
  const result=await page.evaluate(()=>({state:globalThis.__nir?.state(),audit:globalThis.repeatAudit,
    focus:document.activeElement?.dataset.action})).catch(()=>null);
  await fs.writeFile(info.outputPath('terminal.json'),JSON.stringify({status:info.status,result},null,2)+'\n');
});

for(const worker of ['required','main'])for(const key of ['Enter','Space'])
test(`holding ${key} activates the focused Auto button once; ${worker}`,async({page},info)=>{
  const errors=[];page.on('pageerror',error=>errors.push(error.message));
  await page.addInitScript(()=>{
    globalThis.repeatAudit={keys:[],clicks:[]};
    window.addEventListener('keydown',event=>{
      if(event.key==='Enter'||event.key===' '){
        // Dispatch can run a microtask checkpoint between DOM listeners.
        // Inspect defaultPrevented after the Host has handled the event.
        setTimeout(()=>repeatAudit.keys.push({key:event.key,repeat:event.repeat,
          trusted:event.isTrusted,prevented:event.defaultPrevented}),0);
      }
    });
    window.addEventListener('click',event=>{
      const button=event.target.closest('#actions button');
      if(button)repeatAudit.clicks.push({trusted:event.isTrusted,action:JSON.parse(button.dataset.action)});
    },true);
  });
  await page.goto(`http://127.0.0.1:4271/?test=1&worker=${worker}&backend=webgl2`);
  await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
  const rect=await page.evaluate(()=>JSON.parse([...document.querySelectorAll('#actions button')]
    .find(button=>!button.disabled&&JSON.parse(button.dataset.action).type==='new_game').dataset.rect));
  await page.mouse.click(rect[0]+rect[2]/2,rect[1]+rect[3]/2);
  await recoverAudioOutput(page);
  await page.waitForFunction(()=>__nir.state().dialogue?.id==='intro'&&!__nir.state().paused&&!__nir.state().loading);
  const autoFocused=()=>page.evaluate(()=>document.activeElement?.dataset.action&&
    JSON.parse(document.activeElement.dataset.action).type==='toggle_auto');
  for(let count=0;count<12&&!await autoFocused();count++){
    const previous=await page.evaluate(()=>document.activeElement?.dataset.action??null);
    await page.keyboard.press('Tab');
    await page.waitForFunction(previous=>document.activeElement?.dataset.action&&
      document.activeElement.dataset.action!==previous,previous);
  }
  expect(await autoFocused()).toBe(true);
  expect(await page.evaluate(()=>__nir.state().auto)).toBe(false);
  await page.evaluate(()=>{repeatAudit.keys=[];repeatAudit.clicks=[];});
  try{
    await page.keyboard.down(key);
    if(key==='Enter'){
      // The first click rebuilds semantic controls; focus must survive that
      // rebuild while the same physical key remains held.
      await page.waitForFunction(()=>__nir.state().auto&&document.activeElement?.dataset.action&&
        JSON.parse(document.activeElement.dataset.action).type==='toggle_auto');
    }
    await page.keyboard.down(key);
  }finally{await page.keyboard.up(key);}
  await page.waitForFunction(()=>repeatAudit.keys.length===2);
  await page.waitForTimeout(150);
  const result=await page.evaluate(()=>({state:__nir.state(),audit:repeatAudit}));
  await fs.writeFile(info.outputPath('repeat.json'),JSON.stringify({worker,key,errors,result},null,2)+'\n');
  expect(errors).toEqual([]);
  expect(result.state.execution.runtime).toBe(worker==='required'?'worker':'main');
  expect(result.state.auto).toBe(true);
  expect(result.audit.clicks).toHaveLength(1);
  expect(result.audit.clicks[0]).toEqual({trusted:true,action:{type:'toggle_auto'}});
  expect(result.audit.keys.map(event=>event.repeat)).toEqual([false,true]);
  expect(result.audit.keys.every(event=>event.trusted)).toBe(true);
  expect(result.audit.keys[1].prevented).toBe(true);
});
