import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import path from 'node:path';

const deferred=()=>{let release;const promise=new Promise(ok=>release=ok);return {promise,release};};
async function voicePath(){
  const web=path.resolve('reports/nir-next/browser-menu-effects/dist/full/web');
  const {release}=JSON.parse(await fs.readFile(path.join(web,'channels/stable.json'),'utf8'));
  const manifest=JSON.parse(await fs.readFile(path.join(web,`releases/${release}.json`),'utf8'));
  const obj=async id=>JSON.parse(await fs.readFile(path.join(web,manifest.objects[id].path),'utf8'));
  const {program}=await obj(manifest.program);
  for(const id of Object.values(program.catalogs)){
    const catalog=await obj(id),voice=catalog.assets['audio.voice'];
    if(voice)return manifest.objects[voice.object].path;
  }
  throw new Error('Missing fixture voice descriptor');
}
async function activate(page,label){
  const button=page.getByRole('button',{name:label,exact:true});await expect(button).toBeEnabled();
  const rect=JSON.parse(await button.getAttribute('data-rect'));
  await page.mouse.click(rect[0]+rect[2]/2,rect[1]+rect[3]/2);
}
const audio=page=>page.evaluate(()=>({loopStarts:menuAudit.loopStarts,loopStops:menuAudit.loopStops,loops:[...menuAudit.loops]}));
for(const worker of ['required','main'])for(const scenario of ['delay','failure','cancel']){
  test(`menu preparation keeps the committed surface and music; ${scenario}, ${worker}`,async({page,context},info)=>{
    const errors=[];page.on('pageerror',e=>errors.push(e.message));
    await page.addInitScript(()=>{
      const ids=new WeakMap();let next=0;
      globalThis.menuAudit={loopStarts:0,loopStops:0,loops:new Set()};
      const proto=AudioBufferSourceNode.prototype,start=proto.start,stop=proto.stop;
      proto.start=function(...args){const result=start.apply(this,args);if(this.loop){const id=++next;ids.set(this,id);menuAudit.loopStarts++;menuAudit.loops.add(id);}return result;};
      proto.stop=function(...args){const id=ids.get(this);if(id&&menuAudit.loops.delete(id))menuAudit.loopStops++;return stop.apply(this,args);};
    });
    const target=await voicePath();let arrived=deferred(),gate=deferred(),attempts=0,fail=scenario==='failure';
    await context.route(`**/${target}`,async route=>{
      attempts++;arrived.release();await gate.promise;
      try{if(fail)await route.fulfill({status:503,body:'injected subpage MP3 failure'});else await route.continue();}
      catch(e){if(scenario!=='cancel')throw e;}
    });
    await page.goto(`http://127.0.0.1:4220/?test=1&worker=${worker}&backend=webgl2`);
    await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading&&__nir.state().menu_opacity===1);
    expect(attempts).toBe(0);const before=await audio(page);expect(before.loopStarts).toBe(1);
    await activate(page,'Open subpage');await arrived.promise;
    await expect(page.getByRole('button',{name:'Cancel page change',exact:true})).toBeEnabled();
    await expect(page.getByRole('button',{name:'Start',exact:true})).toBeDisabled();
    await expect(page.getByRole('button',{name:'Increase speed',exact:true})).toHaveCount(0);
    await page.waitForTimeout(200);expect(await audio(page)).toEqual(before);
    if(scenario==='cancel'){
      await activate(page,'Cancel page change');await expect(page.getByRole('button',{name:'Start',exact:true})).toBeEnabled();
      gate.release();await page.waitForTimeout(250);expect(await audio(page)).toEqual(before);
      arrived=deferred();gate=deferred();await activate(page,'Open subpage');await arrived.promise;gate.release();
    }else{
      gate.release();
      if(scenario==='failure'){
        await page.waitForFunction(()=>!!__nir.state().error);expect(await audio(page)).toEqual(before);
        await expect(page.getByRole('button',{name:'Start',exact:true})).toBeDisabled();
        fail=false;arrived=deferred();gate=deferred();await activate(page,'Retry');await arrived.promise;
        await expect(page.getByRole('button',{name:'Cancel page change',exact:true})).toBeEnabled();expect(await audio(page)).toEqual(before);gate.release();
      }
    }
    await page.waitForFunction(()=>!__nir.state().loading&&__nir.state().menu_opacity===1);
    await expect(page.getByRole('button',{name:'Increase speed',exact:true})).toBeEnabled();
    const committed=await audio(page);expect(committed.loopStarts).toBe(before.loopStarts+1);expect(committed.loopStops).toBe(before.loopStops+1);expect(committed.loops).toHaveLength(1);
    expect(await page.evaluate(()=>__nir.state().screen)).toBe('Title');expect(await page.evaluate(()=>__nir.state().error)).toBeNull();expect(errors).toEqual([]);
    await fs.writeFile(info.outputPath('menu-admission.json'),JSON.stringify({worker,scenario,attempts,before,committed,scope:'Neutral MP3, trusted canvas input and native source counters; no physical listening/Android proof.'},null,2)+'\n');
  });
}
