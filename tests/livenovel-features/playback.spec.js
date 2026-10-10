import {test, expect} from '@playwright/test';
import {recoverAudioOutput} from '../nir-next/audio-output-helper.js';

async function stored(page, slot) {
  return page.evaluate(async slot => {
    for (const {name} of await indexedDB.databases()) {
      const db = await new Promise((ok,no) => {const r=indexedDB.open(name);r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});
      if (!db.objectStoreNames.contains('saves')) {db.close();continue;}
      const rows = await new Promise((ok,no) => {const t=db.transaction('saves'),r=t.objectStore('saves').getAll();t.oncomplete=()=>ok(r.result);t.onerror=()=>no(t.error);});
      db.close();
      const row=rows.find(r=>r.envelope?.slot===slot);
      if(row)return row.envelope;
    }
    return null;
  },slot);
}
async function save(page, slot) {
  const revision=(await stored(page,slot))?.revision||0;
  await page.evaluate(slot=>__nir.action({type:'save',slot}),slot);
  await expect.poll(async()=>(await stored(page,slot))?.revision||0).toBeGreaterThan(revision);
  return (await stored(page,slot)).snapshot;
}
async function pixels(page) {
  const image=await page.screenshot();
  return page.evaluate(async base64=>{
    const bitmap=await createImageBitmap(new Blob([Uint8Array.from(atob(base64),c=>c.charCodeAt(0))],{type:'image/png'}));
    const canvas=document.createElement('canvas');canvas.width=bitmap.width;canvas.height=bitmap.height;
    const context=canvas.getContext('2d');context.drawImage(bitmap,0,0);bitmap.close();
    const data=context.getImageData(0,0,canvas.width,canvas.height).data;
    let magenta=0,annotation=0;const bounds=[Infinity,Infinity,0,0];
    for(let y=0;y<canvas.height;y++)for(let x=0;x<canvas.width;x++){
      const i=(y*canvas.width+x)*4;
      if(data[i]>230&&data[i+1]<30&&data[i+2]>230){magenta++;bounds[0]=Math.min(bounds[0],x);bounds[1]=Math.min(bounds[1],y);bounds[2]=Math.max(bounds[2],x);bounds[3]=Math.max(bounds[3],y);}
      if(x>=200&&x<350&&y>=490&&y<508&&data[i]>100&&data[i+1]>100&&data[i+2]>100)annotation++;
    }
    return {magenta,annotation,bounds};
  },image.toString('base64'));
}
async function boot(page, worker) {
  await page.addInitScript(()=>{
    window.fixtureAudioStarts=0;
    const start=AudioBufferSourceNode.prototype.start;
    AudioBufferSourceNode.prototype.start=function(...args){window.fixtureAudioStarts++;return start.apply(this,args);};
  });
  await page.setViewportSize({width:1280,height:720});
  await page.goto(`http://127.0.0.1:4268/?test=1&worker=${worker}&backend=webgl2`);
  await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
  await page.getByRole('button',{name:'Start',exact:true}).focus();
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>__nir.state().dialogue?.id==='intro'&&__nir.state().dialogue.ready&&!__nir.state().loading);
  await recoverAudioOutput(page);
}

for(const worker of ['required','main']) {
  test(`sprite motion continues across overlay and cold restore; ${worker}`,async({page})=>{
    await boot(page,worker);
    const snapshot=await save(page,2);
    const motion=Object.values(snapshot.tasks).find(task=>task.id===snapshot.handles.motion);
    expect(motion.state).toBe('running');expect(motion.elapsed_us).not.toBe('0');
    expect(motion.scene_generation).toBe(snapshot.scene_generation);
    const scene=snapshot.scene.find(node=>node.id==='moving-picture');
    expect(scene.y).toBeLessThan(500);expect(scene.preserve_pose).toEqual(['y']);
    await page.reload();
    await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    const session=await page.evaluate(()=>__nir.state().session);
    await page.evaluate(()=>__nir.action({type:'load',slot:2}));
    await page.waitForFunction(s=>__nir.state().session>s&&!__nir.state().loading&&__nir.state().paused,session);
    expect(await page.evaluate(()=>__nir.state().tick_us)).toBe(snapshot.tick_us);
    const restored=await save(page,1), restoredMotion=restored.tasks[motion.id];
    expect(restoredMotion.elapsed_us).toBe(motion.elapsed_us);
    expect(restored.handles.motion).toBe(snapshot.handles.motion);
    expect(restored.handles.music).toBe(snapshot.handles.music);
    expect(restored.scene).toEqual(snapshot.scene);
    const transition=Object.values(snapshot.tasks).find(task=>task.id===snapshot.handles.overlay);
    expect(await page.evaluate(()=>__nir.state().transition)).toBeCloseTo(Number(transition.elapsed_us)/10000000,6);
    const screenshot=await page.screenshot();
    const y=await page.evaluate(async encoded=>{
      const bitmap=await createImageBitmap(new Blob([Uint8Array.from(atob(encoded),c=>c.charCodeAt(0))],{type:'image/png'}));
      const canvas=document.createElement('canvas');canvas.width=bitmap.width;canvas.height=bitmap.height;
      const context=canvas.getContext('2d');context.drawImage(bitmap,0,0);bitmap.close();
      const column=context.getImageData(416,0,1,canvas.height).data;
      for(let y=0;y<canvas.height;y++){const i=y*4;if(column[i]<5&&column[i+1]>250&&column[i+2]>250)return y;}
      return null;
    },screenshot.toString('base64'));
    const expected=motion.captured+(motion.effect.to-motion.captured)*Number(motion.elapsed_us)/30000000;
    expect(y).not.toBeNull();expect(Math.abs(y-expected)).toBeLessThanOrEqual(1);
    expect(await page.evaluate(()=>__nir.diagnostics().host_work.audio_paused)).toBe(true);
    const starts=await page.evaluate(()=>window.fixtureAudioStarts);
    await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
    expect(await page.evaluate(()=>window.fixtureAudioStarts)).toBe(starts);
    expect(await page.evaluate(()=>__nir.state().error)).toBeNull();
  });
  test(`bitmap atlas captures paint and survive restore; ${worker}`,async({page})=>{
    await boot(page,worker);
    expect(await page.evaluate(()=>__nir.state().variables.hud_number)).toEqual({type:'i32',value:34});
    async function colors(){
      const screenshot=await page.screenshot();
      return page.evaluate(async encoded=>{
        const bitmap=await createImageBitmap(new Blob([Uint8Array.from(atob(encoded),c=>c.charCodeAt(0))],{type:'image/png'}));
        const canvas=document.createElement('canvas');canvas.width=bitmap.width;canvas.height=bitmap.height;
        const context=canvas.getContext('2d');context.drawImage(bitmap,0,0);bitmap.close();
        return [108,124].map(x=>Array.from(context.getImageData(x,112,1,1).data));
      },screenshot.toString('base64'));
    }
    const before=await colors(), expected=[[255,40,40,255],[40,255,40,255]];
    before.forEach((rgba,index)=>rgba.forEach((value,channel)=>expect(Math.abs(value-expected[index][channel])).toBeLessThanOrEqual(1)));
    const snapshot=await save(page,1);
    expect(snapshot.scene.every(n=>!n.bitmap_text)).toBe(true);
    expect(snapshot.scene.filter(n=>n.parent==='digit-hud').map(n=>n.clip)).toEqual([[16,0,16,24],[32,0,16,24]]);
    const session=await page.evaluate(()=>__nir.state().session);
    await page.evaluate(()=>__nir.action({type:'load',slot:1}));
    await page.waitForFunction(s=>__nir.state().session>s&&!__nir.state().loading&&__nir.state().paused,session);
    expect(await colors()).toEqual(before);
    expect(await page.evaluate(()=>__nir.state().error)).toBeNull();
  });
  test(`ruby, inline picture and paragraph pause paint and restore; ${worker}`,async({page},info)=>{
    const errors=[];page.on('pageerror',e=>errors.push(e.message));await boot(page,worker);
    const before=await pixels(page);
    expect(before.magenta).toBeGreaterThan(500);
    expect(before.bounds[0]).toBeGreaterThanOrEqual(200);expect(before.bounds[2]).toBeLessThan(1080);
    expect(before.bounds[1]).toBeGreaterThanOrEqual(490);expect(before.bounds[3]).toBeLessThan(610);
    expect(before.annotation).toBeGreaterThan(10);
    const saved=await save(page,1),dialogue=Object.values(saved.tasks).find(t=>t.dialogue?.text_id==='intro').dialogue;
    expect(saved.audio_paused).toEqual(['bgm']);
    expect(dialogue.spans[0].ruby).toBe('reading');expect(dialogue.spans[1].image.asset).toBe('fixture.inline');
    expect(dialogue.spans[2].pause).toBe(true);expect(dialogue.span).toBe(2);
    const interaction=await page.evaluate(()=>__nir.state().interaction);
    await page.keyboard.press('Space');await page.waitForFunction(()=>__nir.state().dialogue.visible.endsWith(' continues.'));
    expect(await page.evaluate(()=>__nir.state().interaction)).toBe(interaction);
    const session=await page.evaluate(()=>__nir.state().session);
    await page.evaluate(()=>__nir.action({type:'load',slot:1}));
    await page.waitForFunction(s=>__nir.state().session>s&&!__nir.state().loading&&__nir.state().paused,session);
    const restored=await pixels(page);expect(restored.magenta).toBe(before.magenta);expect(restored.bounds).toEqual(before.bounds);
    await info.attach('restored-pixels',{body:JSON.stringify({before,restored}),contentType:'application/json'});
    expect(errors).toEqual([]);expect(await page.evaluate(()=>__nir.state().error)).toBeNull();
  });
  test(`extended numeric bits survive WASM save and restore; ${worker}`,async({page})=>{
    await boot(page,worker);
    expect(await page.evaluate(()=>__nir.state().variables.extended_delta)).toEqual({type:'f80',value:'3fc08000000000000000'});
    const snapshot=await save(page,1);
    expect(snapshot.variables.extended).toEqual({type:'f80',value:'3fff8000000000000001'});
    const session=await page.evaluate(()=>__nir.state().session);
    await page.evaluate(()=>__nir.action({type:'load',slot:1}));
    await page.waitForFunction(s=>__nir.state().session>s&&!__nir.state().loading&&__nir.state().paused,session);
    expect(await page.evaluate(()=>__nir.state().variables.extended_delta)).toEqual(snapshot.variables.extended_delta);
    expect(await page.evaluate(()=>__nir.state().error)).toBeNull();
  });
  test(`profile facts survive new game and old saves; ${worker}`,async({page})=>{
    await boot(page,worker);
    const initial=await save(page,1);expect(initial.variables.profile_flag.value).toBe(0);
    await page.keyboard.press('Space');await page.waitForFunction(()=>__nir.state().dialogue.visible.endsWith(' continues.'));
    await page.keyboard.press('Space');await page.waitForFunction(()=>__nir.state().dialogue?.id==='arrival'&&__nir.state().dialogue.ready&&!__nir.state().loading);
    await page.keyboard.press('Space');await page.waitForFunction(()=>__nir.state().choice&&!__nir.state().loading);
    await recoverAudioOutput(page);
    await page.getByRole('button',{name:'Walk home together along the river',exact:true}).focus();
    await page.keyboard.press('Enter');await page.waitForFunction(()=>__nir.state().outcome==='walk');
    await page.evaluate(()=>__nir.action({type:'title'}));
    await page.waitForFunction(()=>__nir.state().screen==='Title'&&!__nir.state().loading);
    await page.evaluate(()=>__nir.action({type:'new_game'}));
    await page.waitForFunction(()=>__nir.state().dialogue?.id==='intro'&&__nir.state().dialogue.ready&&!__nir.state().loading);
    expect(await page.evaluate(()=>__nir.state().variables.profile_flag.value)).toBe(1);
    const session=await page.evaluate(()=>__nir.state().session);
    await page.evaluate(()=>__nir.action({type:'load',slot:1}));
    await page.waitForFunction(s=>__nir.state().session>s&&!__nir.state().loading&&__nir.state().paused,session);
    expect(await page.evaluate(()=>__nir.state().variables.profile_flag.value)).toBe(0);
    await page.evaluate(()=>__nir.action({type:'continue'}));await recoverAudioOutput(page);
    await page.keyboard.press('Space');await page.waitForFunction(()=>__nir.state().dialogue.visible.endsWith(' continues.'));
    await page.keyboard.press('Space');await page.waitForFunction(()=>__nir.state().dialogue?.id==='arrival'&&__nir.state().dialogue.ready&&!__nir.state().loading);
    await page.keyboard.press('Space');await page.waitForFunction(()=>__nir.state().choice&&!__nir.state().loading);
    expect(await page.evaluate(()=>__nir.state().variables.profile_flag.value)).toBe(1);
    expect(await page.evaluate(()=>__nir.state().error)).toBeNull();
  });
  test(`mutable progress survives reload and old save, then resets; ${worker}`,async({page})=>{
    await boot(page,worker);await save(page,1);
    const choose=async(label)=>{
      await page.keyboard.press('Space');await page.waitForFunction(()=>__nir.state().dialogue.visible.endsWith(' continues.'));
      await page.keyboard.press('Space');await page.waitForFunction(()=>__nir.state().dialogue?.id==='arrival'&&__nir.state().dialogue.ready&&!__nir.state().loading);
      await page.keyboard.press('Space');await page.waitForFunction(()=>__nir.state().choice&&!__nir.state().loading);
      await recoverAudioOutput(page);
      await page.getByRole('button',{name:label,exact:true}).focus();await page.keyboard.press('Enter');
      await page.waitForFunction(()=>__nir.state().outcome&&!__nir.state().loading);
    };
    const progress=()=>page.evaluate(async()=>{
      const db=await new Promise((ok,no)=>{const r=indexedDB.open('nir-player-isolated-v1');r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});
      const rows=await new Promise((ok,no)=>{const t=db.transaction('profile'),r=t.objectStore('profile').getAll();t.oncomplete=()=>ok(r.result);t.onerror=()=>no(t.error);});db.close();
      return rows.find(r=>r?.['fixture.mutable'])?.['fixture.mutable']?.value;
    });
    await choose('Walk home together along the river');await expect.poll(progress).toBe(1);
    await page.reload();await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    await page.getByRole('button',{name:'Start',exact:true}).focus();await page.keyboard.press('Enter');
    await page.waitForFunction(()=>__nir.state().dialogue?.id==='intro'&&__nir.state().dialogue.ready&&!__nir.state().loading);
    expect(await page.evaluate(()=>__nir.state().variables.profile_value.value)).toBe(1);
    const session=await page.evaluate(()=>__nir.state().session);await page.evaluate(()=>__nir.action({type:'load',slot:1}));
    await page.waitForFunction(s=>__nir.state().session>s&&!__nir.state().loading&&__nir.state().paused,session);
    expect(await page.evaluate(()=>__nir.state().variables.profile_value.value)).toBe(0);
    await page.evaluate(()=>__nir.action({type:'continue'}));await recoverAudioOutput(page);
    await choose('Stay at the station and read the letter');await expect.poll(progress).toBe(0);
    await page.evaluate(()=>__nir.action({type:'title'}));await page.waitForFunction(()=>__nir.state().screen==='Title'&&!__nir.state().loading);
    await page.evaluate(()=>__nir.action({type:'new_game'}));await page.waitForFunction(()=>__nir.state().dialogue?.id==='intro'&&__nir.state().dialogue.ready&&!__nir.state().loading);
    expect(await page.evaluate(()=>__nir.state().variables.profile_value.value)).toBe(0);
    expect(await page.evaluate(()=>__nir.state().error)).toBeNull();
  });
  test(`authored menu lock, audio bus pause and positioned picture choice; ${worker}`,async({page})=>{
    const errors=[];page.on('pageerror',e=>errors.push(e.message));await boot(page,worker);
    const first=await save(page,1),music=Object.values(first.tasks).find(t=>t.effect.type==='audio'&&t.effect.bus==='bgm');
    expect(first.audio_paused).toEqual(['bgm']);
    const starts=await page.evaluate(()=>window.fixtureAudioStarts);
    await expect.poll(()=>page.evaluate(()=>__nir.diagnostics().host_work.audio_domains.story.buses.bgm.state)).toBe('suspended');
    const clock=await page.evaluate(()=>__nir.diagnostics().host_work.audio_domains.story.buses.bgm.clock_seconds);
    await page.waitForTimeout(150);
    expect(await page.evaluate(()=>__nir.diagnostics().host_work.audio_domains.story.buses.bgm.clock_seconds)).toBe(clock);
    await page.keyboard.press('Space');await page.waitForFunction(()=>__nir.state().dialogue.visible.endsWith(' continues.'));
    await page.keyboard.press('Space');await page.waitForFunction(()=>__nir.state().dialogue?.id==='arrival'&&!__nir.state().loading);
    const interaction=await page.evaluate(()=>__nir.state().interaction);
    for(const action of [{type:'menu'},{type:'history'},{type:'save',slot:2}])await page.evaluate(a=>__nir.action(a),action);
    await page.waitForTimeout(150);
    expect(await page.evaluate(()=>__nir.state().screen)).toBe('Story');expect(await stored(page,2)).toBeNull();
    expect(await page.evaluate(()=>__nir.state().interaction)).toBe(interaction);
    await page.keyboard.press('Space');await page.waitForFunction(()=>__nir.state().choice&&!__nir.state().loading);
    const resumed=await save(page,1);
    await recoverAudioOutput(page);
    await expect.poll(()=>page.evaluate(()=>__nir.diagnostics().host_work.audio_domains.story.buses.bgm.state)).toBe('running');
    await expect.poll(()=>page.evaluate(()=>__nir.diagnostics().host_work.audio_domains.story.buses.bgm.clock_seconds)).toBeGreaterThan(clock);
    expect(await page.evaluate(()=>window.fixtureAudioStarts)).toBe(starts);
    expect(resumed.audio_paused||[]).toEqual([]);
    expect(resumed.tasks[music.id].state).toBe('running');expect(resumed.tasks[music.id].effect.asset).toBe(music.effect.asset);
    const buttons=await page.locator('#actions button').evaluateAll(bs=>bs.filter(b=>JSON.parse(b.dataset.action).type==='choose').map(b=>({rect:JSON.parse(b.dataset.rect),action:JSON.parse(b.dataset.action)})));
    expect(buttons.map(b=>b.rect)).toEqual([[120,120,200,150],[620,120,200,150]]);
    await page.mouse.move(130,130);await page.waitForTimeout(100);
    await page.mouse.click(130,130);await page.waitForFunction(()=>__nir.state().outcome==='walk');
    expect(errors).toEqual([]);expect(await page.evaluate(()=>__nir.state().error)).toBeNull();
  });
}
