import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import {recoverAudioOutput} from './audio-output-helper.js';
const role='speaker.aki';
async function controls(page) {
  await page.evaluate(()=>__nir.action({type:'settings'}));
  await page.waitForFunction(()=>__nir.state().screen==='Settings'&&!__nir.state().loading);
  for(let i=0;i<35;i++) {
    const rect=await page.evaluate(role=>{
      const n=[...document.querySelectorAll('#actions button')].find(n=>{
        const a=JSON.parse(n.dataset.action);return a.type==='character_mute'&&a.character===role;
      });return n?JSON.parse(n.dataset.rect):null;
    },role);
    if(rect?.[3]>=44)return;
    await page.evaluate(()=>__nir.action({type:'scroll',region:'settings',delta:1}));
    await page.waitForTimeout(40);
  }
  throw new Error('Character mute control never fully entered the viewport');
}
async function canvasControl(page,type,delta) {
  const rect=await page.evaluate(({role,type,delta})=>{
    const n=[...document.querySelectorAll('#actions button')].find(n=>{
      const a=JSON.parse(n.dataset.action);return a.type===type&&a.character===role&&(delta===undefined||Math.abs(a.delta-delta)<1e-6);
    });return n?JSON.parse(n.dataset.rect):null;
  },{role,type,delta});
  expect(rect).not.toBeNull();expect(rect[3]).toBeGreaterThanOrEqual(44);
  await page.locator('#stage').click({position:{x:rect[0]+rect[2]/2,y:rect[1]+rect[3]/2}});
}
async function persisted(page,muted) {
  await expect.poll(()=>page.evaluate(async ({role,muted})=>{
    for(const {name} of await indexedDB.databases()) {
      const db=await new Promise((ok,no)=>{const r=indexedDB.open(name);r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});
      if(!db.objectStoreNames.contains('preferences')){db.close();continue;}
      const rows=await new Promise((ok,no)=>{const tx=db.transaction('preferences'),r=tx.objectStore('preferences').getAll();tx.oncomplete=()=>ok(r.result);tx.onerror=()=>no(tx.error);});db.close();
      if(rows.some(row=>row.character_voices?.[role]?.muted===muted))return true;
    }return false;
  },{role,muted})).toBe(true);
}
for(const worker of ['required','main']) {
  test(`character voice gains, canvas controls, persisted mute, restore and history, ${worker}`,async({page},testInfo)=>{
    await page.setViewportSize({width:390,height:844});
    const errors=[];page.on('pageerror',error=>errors.push(error.message));
    await page.addInitScript(()=>{
      window.actorContexts=[];window.actorAudio=[];const nodes=new WeakMap();
      const Native=AudioContext;window.AudioContext=class extends Native {constructor(...args){super(...args);actorContexts.push(this);}};
      const connect=AudioNode.prototype.connect;
      AudioNode.prototype.connect=function(target,...args){
        const row=nodes.get(this);
        if(row){if(this===row.source){row.envelope=target;nodes.set(target,row);}else if(this===row.envelope)row.gain=target;}
        return connect.call(this,target,...args);
      };
      const create=Native.prototype.createBufferSource;
      Native.prototype.createBufferSource=function(...args){
        const source=create.apply(this,args),row={source,context:this,stops:0,initialGain:null};actorAudio.push(row);nodes.set(source,row);
        const start=source.start,stop=source.stop;
        source.start=function(...args){row.initialGain=row.gain.gain.value;return start.apply(this,args);};
        source.stop=function(...args){row.stops++;return stop.apply(this,args);};return source;
      };
    });
    await page.goto(`http://127.0.0.1:4259/?test=1&worker=${worker}&backend=webgl2`);
    await page.waitForFunction(()=>window.__nir?.state().ready&&!__nir.state().loading);
    await page.keyboard.press('Enter');
    await page.waitForFunction(()=>__nir.state().dialogue?.ready&&!__nir.state().loading&&actorContexts.every(c=>c.state==='running'));
    const before=await page.evaluate(()=>({session:__nir.state().session,interaction:__nir.state().interaction,count:actorAudio.length}));
    await controls(page);
    await canvasControl(page,'character_volume',-.1);
    await page.waitForFunction(role=>Math.abs(__nir.state().preferences.character_voices?.[role]?.volume-.9)<1e-5,role);
    const gains=await page.evaluate(()=>actorAudio.filter(row=>row.context===actorContexts[2]).map(row=>row.gain.gain.value));
    expect(gains).toHaveLength(2);expect(gains[0]).toBeCloseTo(.72,5);expect(gains[1]).toBeCloseTo(.8,5);
    await canvasControl(page,'character_mute');await persisted(page,true);
    await fs.writeFile(testInfo.outputPath('mute-before-assert.json'),JSON.stringify(await page.evaluate(()=>({
      state:__nir.state(),sources:actorAudio.map(row=>({context:actorContexts.indexOf(row.context),stops:row.stops,initialGain:row.initialGain,gain:row.gain?.gain.value})),
      actions:[...document.querySelectorAll('#actions button')].map(n=>JSON.parse(n.dataset.action)),
      events:__nir.diagnostics().events,
    })),null,2)+'\n');
    expect(await page.evaluate(()=>actorAudio.filter(row=>row.context===actorContexts[2])[0].gain.gain.value)).toBe(0);
    expect(await page.evaluate(()=>actorAudio.length)).toBe(before.count);
    expect(await page.evaluate(()=>actorAudio.every(row=>row.stops===0))).toBe(true);
    expect(await page.evaluate(()=>__nir.state().interaction)).toBe(before.interaction);
    // Role keys and the frozen speaker name survive both locale selections.
    await page.evaluate(()=>__nir.action({type:'ui_locale',locale:'zh-Hans'}));
    await page.evaluate(()=>__nir.action({type:'text_locale',locale:'zh-Hans'}));
    await page.waitForFunction(()=>!__nir.state().locale_pending&&__nir.state().preferences.ui_locale==='zh-Hans'&&__nir.state().preferences.text_locale==='zh-Hans');
    await controls(page);
    expect(await page.evaluate(role=>__nir.state().preferences.character_voices[role],role)).toEqual({volume:expect.closeTo(.9,5),muted:true});
    expect(await page.evaluate(()=>actorAudio.filter(row=>row.context===actorContexts[2])[0].gain.gain.value)).toBe(0);
    await page.screenshot({path:testInfo.outputPath('character-portrait.png')});
    await page.setViewportSize({width:844,height:390});await controls(page);
    await page.screenshot({path:testInfo.outputPath('character-landscape.png')});
    // Cold boot must already apply mute at source.start, without an unmuted
    // first quantum, for the immediate explicit binding in this fixture.
    await page.reload();await page.waitForFunction(()=>window.__nir?.state().ready&&!__nir.state().loading);
    await page.keyboard.press('Enter');await page.waitForFunction(()=>__nir.state().dialogue?.ready&&!__nir.state().loading&&actorContexts.every(c=>c.state==='running'));
    const cold=await page.evaluate(()=>actorAudio.filter(row=>row.context===actorContexts[2]).map(row=>row.initialGain));
    expect(cold).toHaveLength(2);expect(cold[0]).toBe(0);expect(cold[1]).toBeCloseTo(.8,5);
    // A save stores identity; current global preferences win over its old mute.
    await page.evaluate(()=>__nir.action({type:'saves'}));
    await page.evaluate(()=>__nir.action({type:'save',slot:0}));
    await page.waitForFunction(()=>['Saved','已保存'].includes(__nir.state().status));
    await page.evaluate(()=>__nir.action({type:'character_mute',character:'speaker.aki',muted:false}));
    await persisted(page,false);
    const oldSession=await page.evaluate(()=>__nir.state().session);
    await page.evaluate(()=>__nir.action({type:'load',slot:0}));
    await page.waitForFunction(s=>__nir.state().session>s&&!__nir.state().loading&&__nir.state().screen==='Story',oldSession);
    // Restore intentionally freezes Story until the player chooses Continue.
    const restoredInteraction=await page.evaluate(()=>__nir.state().interaction);
    expect(await page.evaluate(()=>__nir.state().paused)).toBe(true);
    await page.keyboard.press('Enter');
    await recoverAudioOutput(page);
    expect(await page.evaluate(()=>__nir.state().interaction)).toBe(restoredInteraction);
    expect(await page.evaluate(role=>__nir.state().preferences.character_voices[role].muted,role)).toBe(false);
    const restored=await page.evaluate(()=>actorAudio.filter(row=>row.context===actorContexts[2]).slice(-2).map(row=>({initialGain:row.initialGain,gain:row.gain.gain.value})));
    expect(restored[0].initialGain).toBeCloseTo(.72,5);expect(restored[1].initialGain).toBeCloseTo(.8,5);
    await page.evaluate(()=>__nir.action({type:'character_mute',character:'speaker.aki',muted:true}));
    await page.evaluate(()=>__nir.action({type:'history'}));
    const historyBefore=await page.evaluate(()=>({tick:__nir.state().tick_us,interaction:__nir.state().interaction,position:__nir.state().position,history:__nir.state().history_count}));
    await page.evaluate(()=>__nir.action({type:'history_voice',entry:__nir.state().history_count-1}));
    await page.waitForFunction(()=>actorAudio.some(row=>row.context===actorContexts[1]&&row.initialGain!==null));
    expect(await page.evaluate(()=>actorAudio.filter(row=>row.context===actorContexts[1]).at(-1).initialGain)).toBe(0);
    await page.evaluate(()=>__nir.action({type:'character_mute',character:'speaker.aki',muted:false}));
    expect(await page.evaluate(()=>actorAudio.filter(row=>row.context===actorContexts[1]).at(-1).gain.gain.value)).toBeCloseTo(.72,5);
    expect(await page.evaluate(()=>({tick:__nir.state().tick_us,interaction:__nir.state().interaction,position:__nir.state().position,history:__nir.state().history_count}))).toEqual(historyBefore);
    const proof=await page.evaluate(()=>({state:__nir.state(),sources:actorAudio.map(row=>({context:actorContexts.indexOf(row.context),stops:row.stops,initialGain:row.initialGain,gain:row.gain.gain.value,loop:row.source.loop}))}));
    expect(errors).toEqual([]);
    const output=testInfo.outputPath('character-voice.json');await fs.writeFile(output,JSON.stringify({before,cold,restored,historyBefore,...proof},null,2)+'\n');
    await testInfo.attach('character-voice',{path:output,contentType:'application/json'});
  });
}
