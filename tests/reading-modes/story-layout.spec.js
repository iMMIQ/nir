import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import {recoverAudioOutput} from '../nir-next/audio-output-helper.js';
import {installClosedPointerProbe} from '../nir-next/closed-output-pointer-probe.js';

test.use({hasTouch:true,deviceScaleFactor:2});
const identity=s=>({session:s.session,interaction:s.interaction,position:s.position,history:s.history_count});
const overlap=(a,b)=>a[0]<b[0]+b[2]-.01&&b[0]<a[0]+a[2]-.01&&a[1]<b[1]+b[3]-.01&&b[1]<a[1]+a[3]-.01;
const controls=page=>page.evaluate(()=>[...document.querySelectorAll('#actions button')].map(b=>({
  action:JSON.parse(b.dataset.action),rect:JSON.parse(b.dataset.rect),disabled:b.disabled,
})));
async function tap(page,type){
  await page.waitForFunction(type=>[...document.querySelectorAll('#actions button')].some(b=>!b.disabled&&JSON.parse(b.dataset.action).type===type),type);
  const rect=(await controls(page)).find(b=>!b.disabled&&b.action.type===type).rect;
  await page.touchscreen.tap(rect[0]+rect[2]/2,rect[1]+rect[3]/2);
}
for(const worker of ['required','main'])test(`compact reading keeps touch controls outside its text after rotation; ${worker}`,async({page},info)=>{
  const errors=[],layouts=[];page.on('pageerror',e=>errors.push(e.message));
  await installClosedPointerProbe(page);
  try{
    await page.goto(`http://127.0.0.1:4275/?test=1&worker=${worker}&backend=webgl2`);
    await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    await tap(page,'new_game');await recoverAudioOutput(page);
    await page.waitForFunction(()=>__nir.state().dialogue?.id==='letter'&&!__nir.state().loading&&!__nir.state().paused);
    await page.keyboard.press('Space');
    // The authored Sfx Gate and natural text reveal may finish without another
    // reader input. Keep the final waiting line while checking its viewport.
    await page.waitForFunction(()=>__nir.state().dialogue?.id==='letter'&&__nir.state().dialogue.ready&&!__nir.state().loading&&!__nir.state().paused);
    const before=await page.evaluate(()=>__nir.state());
    expect(before.execution.runtime).toBe(worker==='required'?'worker':'main');
    for(const fontScale of [1,1.5]){
      if(fontScale!==1)await page.evaluate(()=>__nir.action({type:'font_size',delta:.5}));
      await page.waitForFunction(scale=>Math.abs(__nir.state().preferences.font_scale-scale)<.001,fontScale);
      for(const viewport of [{width:240,height:240},{width:844,height:260},{width:390,height:844},{width:240,height:240}]){
        const previous=page.viewportSize(),changed=previous.width!==viewport.width||previous.height!==viewport.height;
        const floor=await page.evaluate(()=>Math.max(0,...closedPointerProbe().rows.filter(r=>r.type==='rpc_request').map(r=>r.id)));
        await page.setViewportSize(viewport);
        await page.waitForFunction(({worker,viewport,floor,changed})=>{
          const menu=[...document.querySelectorAll('#actions button')].find(b=>JSON.parse(b.dataset.action).type==='menu');
          if(!menu||innerWidth!==viewport.width||innerHeight!==viewport.height)return false;
          const rect=JSON.parse(menu.dataset.rect),margin=viewport.width<650?20:48;
          const columns=viewport.width<650?3:5,w=Math.min(115,(viewport.width-2*margin-(columns-1)*8)/columns);
          if(Math.abs(rect[2]-w)>.01||rect[3]!==44)return false;
          if(worker==='main'||!changed)return true;
          const reply=closedPointerProbe().rows.filter(r=>r.type==='rpc_reply'&&r.id>floor&&r.calls.some(c=>c.method==='resize'&&c.args[0].width===viewport.width&&c.args[0].height===viewport.height)).at(-1);
          return reply&&__nir.metrics.frames>=JSON.parse(reply.host).frames;
        },{worker,viewport,floor,changed});
        const state=await page.evaluate(()=>__nir.state()),nodes=await controls(page);
        expect(identity(state)).toEqual(identity(before));
        const toolbar=nodes.filter(n=>['menu','history','toggle_auto','toggle_skip','toggle_interface'].includes(n.action.type));
        expect(toolbar).toHaveLength(5);
        const body=state.scrolls.find(v=>v.region==='dialogue');
        const scroll=nodes.filter(n=>n.action.type==='scroll'&&n.action.region==='dialogue');
        for(const node of [...toolbar,...scroll]){
          const [x,y,w,h]=node.rect;
          expect(w).toBeGreaterThanOrEqual(44);expect(h).toBeGreaterThanOrEqual(44);
          expect(x).toBeGreaterThanOrEqual(0);expect(y).toBeGreaterThanOrEqual(0);
          expect(x+w).toBeLessThanOrEqual(viewport.width+.01);expect(y+h).toBeLessThanOrEqual(viewport.height+.01);
          if(body)expect(overlap(node.rect,body.rect)).toBe(false);
        }
        if(viewport.width===240){expect(body).toBeDefined();expect(scroll).toHaveLength(2);}
        if(body&&scroll.some(n=>n.action.delta===-1&&!n.disabled)){
          const up=scroll.find(n=>n.action.delta===-1&&!n.disabled),offset=body.offset;
          await page.touchscreen.tap(up.rect[0]+up.rect[2]/2,up.rect[1]+up.rect[3]/2);
          await page.waitForFunction(offset=>__nir.state().scrolls.find(v=>v.region==='dialogue')?.offset<offset,offset);
          expect(identity(await page.evaluate(()=>__nir.state()))).toEqual(identity(before));
        }
        layouts.push({fontScale,viewport,state,nodes});
      }
    }
    await page.screenshot({path:info.outputPath('240-reader.png')});
    await tap(page,'menu');await page.waitForFunction(()=>__nir.state().screen==='Menu');
    expect(identity(await page.evaluate(()=>__nir.state()))).toEqual(identity(before));
    await tap(page,'close');await page.waitForFunction(()=>__nir.state().screen==='Story');
    expect(identity(await page.evaluate(()=>__nir.state()))).toEqual(identity(before));
    expect(errors).toEqual([]);
  }finally{
    await fs.writeFile(info.outputPath('layouts.json'),JSON.stringify({errors,layouts,final:await page.evaluate(()=>globalThis.__nir?.state()).catch(()=>null),
      probe:await page.evaluate(()=>globalThis.closedPointerProbe?.()).catch(()=>null),
      scope:'Current SDK Worker/Main, actual compiled MP3 author Gate, CSS semantic rectangles and trusted touch after viewport changes; desktop emulation, not Android or pixel/DAC fidelity proof.'},null,2)+'\n');
  }
});
