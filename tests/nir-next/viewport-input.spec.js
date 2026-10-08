import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import {installClosedPointerProbe} from './closed-output-pointer-probe.js';

async function setup(browser,worker) {
  const context=await browser.newContext({viewport:{width:844,height:390},deviceScaleFactor:2,hasTouch:true}),page=await context.newPage();
  await installClosedPointerProbe(page);
  await page.addInitScript(()=>{
    const NativeAudio=AudioContext;let contexts=0;
    window.viewportMusic=[];
    window.AudioContext=class extends NativeAudio {
      constructor(options={}){super({...options,sinkId:{type:'none'}});this.viewportRoute=contexts++;}
      get state(){return this.viewportRoute===2?'closed':super.state;}
    };
    const create=NativeAudio.prototype.createBufferSource;
    NativeAudio.prototype.createBufferSource=function(...args){const source=create.apply(this,args),row={source,stops:0},stop=source.stop;source.stop=function(...args){row.stops++;return stop.apply(this,args);};viewportMusic.push(row);return source;};
    const NativeWorker=Worker;
    window.Worker=class extends NativeWorker {
      constructor(url,options){super(url,options);this.runtime=options?.name==='nir-runtime';this.heldIds=new Set();}
      postMessage(message,...args){if(this.runtime&&window.viewportHoldNext&&message.kind==='batch'&&message.value.some(c=>c.method==='pointer_gesture'&&c.args[0]===0)){window.viewportHoldNext=false;this.heldIds.add(message.id);}return super.postMessage(message,...args);}
      set onmessage(handler){super.onmessage=e=>{if(e.data.kind==='reply'&&this.heldIds.delete(e.data.id)){window.viewportReplyHeld=true;setTimeout(()=>{window.viewportReplyReleased=true;handler.call(this,e);},250);}else handler.call(this,e);};}
      get onmessage(){return super.onmessage;}
    };
    window.viewportStory=()=>{const s=__nir.state();return Object.fromEntries(['session','interaction','position','tick_us','history_count','sequence'].map(k=>[k,s[k]]));};
    window.viewportLoops=()=>viewportMusic.filter(r=>r.source.loop).map(r=>({stops:r.stops}));
  });
  await page.goto(`http://127.0.0.1:4259/?test=1&worker=${worker}&backend=webgl2`);
  await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>__nir.state().screen==='Story'&&__nir.state().dialogue&&__nir.state().paused&&!__nir.state().loading);
  expect(await page.evaluate(()=>__nir.state().execution.runtime)).toBe(worker==='required'?'worker':'main');
  return {context,page};
}
async function renderedLayout(page,worker,viewport,requestFloor,verify=true) {
  await page.waitForFunction(({worker,viewport,requestFloor})=>{
    const button=[...document.querySelectorAll('#actions button')].find(b=>JSON.parse(b.dataset.action).type==='menu');
    if(!button||innerWidth!==viewport.width||innerHeight!==viewport.height)return false;
    if(worker==='main')return JSON.parse(button.dataset.rect)[2]===(viewport.width<650?111.33333587646484:115);
    // A later rotation can reuse an earlier width. Only this rotation's new
    // request can prove completion; an old portrait reply is insufficient.
    const replies=closedPointerProbe().rows.filter(r=>r.type==='rpc_reply'&&r.id>requestFloor&&r.calls.some(c=>c.method==='resize'&&c.args[0].width===viewport.width&&c.args[0].height===viewport.height));
    const last=replies.at(-1);return last&&__nir.metrics.frames>=JSON.parse(last.host).frames;
  },{worker,viewport,requestFloor});
  const proof=await page.evaluate(()=>closedPointerProbe());
  if(worker==='required'&&verify){
    const last=proof.rows.filter(r=>r.type==='rpc_reply'&&r.id>requestFloor&&r.calls.some(c=>c.method==='resize'&&c.args[0].width===viewport.width&&c.args[0].height===viewport.height)).at(-1);
    const workerMenu=last.view.nodes.find(n=>n.action.type==='menu').rect;
    expect(proof.controls.find(n=>n.action.type==='menu').rect).toEqual(workerMenu);
  }
  return proof;
}
async function requestFloor(page){return page.evaluate(()=>Math.max(0,...closedPointerProbe().rows.filter(r=>r.type==='rpc_request').map(r=>r.id)));}
async function tapMenu(page) {
  const rect=await page.evaluate(()=>JSON.parse([...document.querySelectorAll('#actions button')].find(b=>JSON.parse(b.dataset.action).type==='menu').dataset.rect));
  await page.touchscreen.tap(rect[0]+rect[2]/2,rect[1]+rect[3]/2);
  await page.waitForFunction(()=>__nir.state().screen==='Menu',null,{timeout:10000});
}
for(const worker of ['required','main']) {
  test(`rendered controls match completed viewport and trusted touch after rotation; ${worker}`,async({browser},info)=>{
    const {context,page}=await setup(browser,worker);
    try{
      const before=await page.evaluate(()=>({story:viewportStory(),music:viewportLoops()}));expect(before.music.length).toBeGreaterThan(0);
      const layouts=[];
      for(const viewport of [{width:390,height:844},{width:844,height:260},{width:390,height:844}]){
        const floor=await requestFloor(page);
        await page.setViewportSize(viewport);layouts.push(await renderedLayout(page,worker,viewport,floor));
      }
      await tapMenu(page);
      expect(await page.evaluate(()=>viewportStory())).toEqual(before.story);expect(await page.evaluate(()=>viewportLoops())).toEqual(before.music);
      await fs.writeFile(info.outputPath('viewport-touch.json'),JSON.stringify({worker,before,layouts,after:await page.evaluate(()=>closedPointerProbe())},null,2)+'\n');
    }catch(error){await fs.writeFile(info.outputPath('viewport-failure.json'),JSON.stringify(await page.evaluate(()=>closedPointerProbe()),null,2)+'\n');throw error;}finally{await context.close();}
  });
  test(`rotation cancels an unfinished trusted press before async hit testing can change its target; ${worker}`,async({browser},info)=>{
    const {context,page}=await setup(browser,worker);
    try{
      const before=await page.evaluate(()=>({story:viewportStory(),music:viewportLoops()}));
      const rect=await page.evaluate(()=>JSON.parse([...document.querySelectorAll('#actions button')].find(b=>JSON.parse(b.dataset.action).type==='menu').dataset.rect));
      const cdp=await context.newCDPSession(page);await page.evaluate(()=>{window.viewportHoldNext=true;});
      await cdp.send('Input.dispatchTouchEvent',{type:'touchStart',touchPoints:[{x:rect[0]+rect[2]/2,y:rect[1]+rect[3]/2,id:9}]});
      await page.waitForFunction(worker=>worker==='required'?window.viewportReplyHeld:closedPointerProbe().rows.some(r=>r.type==='pointerdown'&&r.trusted),worker);
      const viewport={width:390,height:844},floor=await requestFloor(page);
      await page.setViewportSize(viewport);
      await renderedLayout(page,worker,viewport,floor,false);
      await cdp.send('Input.dispatchTouchEvent',{type:'touchEnd',touchPoints:[]});
      await page.waitForFunction(worker=>{
        const d=__nir.diagnostics();return (worker!=='required'||window.viewportReplyReleased)&&d.execution.runtime_pending===0&&d.host_work.pending_owner_callbacks===0;
      },worker);
      const cancelled=await page.evaluate(()=>({probe:closedPointerProbe(),story:viewportStory(),music:viewportLoops()}));
      expect(cancelled.probe.state.screen).toBe('Story');expect(cancelled.story).toEqual(before.story);expect(cancelled.music).toEqual(before.music);
      expect(cancelled.probe.rows.some(r=>r.type==='pointerdown'&&r.trusted)).toBe(true);
      await tapMenu(page);expect(await page.evaluate(()=>viewportStory())).toEqual(before.story);expect(await page.evaluate(()=>viewportLoops())).toEqual(before.music);
      await fs.writeFile(info.outputPath('viewport-cancel.json'),JSON.stringify({worker,before,cancelled,after:await page.evaluate(()=>closedPointerProbe())},null,2)+'\n');
      await cdp.detach();
    }finally{await context.close();}
  });
}
