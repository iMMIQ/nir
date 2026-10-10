import {test,expect} from '@playwright/test';
import {recoverAudioOutput} from '../nir-next/audio-output-helper.js';

async function envelope(page) {
  return page.evaluate(async()=>{
    for(const {name} of await indexedDB.databases()) {
      const db=await new Promise((resolve,reject)=>{const r=indexedDB.open(name);r.onsuccess=()=>resolve(r.result);r.onerror=()=>reject(r.error);});
      if(!db.objectStoreNames.contains('saves')) {db.close();continue;}
      const rows=await new Promise((resolve,reject)=>{const t=db.transaction('saves'),r=t.objectStore('saves').getAll();t.oncomplete=()=>resolve(r.result);t.onerror=()=>reject(t.error);});
      db.close();const row=rows.find(r=>r.envelope?.slot===1);if(row)return row.envelope;
    }
    return null;
  });
}
for(const worker of ['required','main']) {
  test(`advance wait restores input identity and click leaves movie and music running; ${worker}`,async({page})=>{
    test.skip(process.env.NIR_TEST_ADVANCE_WAIT!=='1','Run separate advance-wait fixture');
    const errors=[];page.on('pageerror',e=>errors.push(e.message));
    const requests=[];page.on('request',r=>{if(r.url().includes('/objects/'))requests.push(r.url());});
    await page.addInitScript(()=>{
      window.fixtureAudioStarts=0;const start=AudioBufferSourceNode.prototype.start;
      AudioBufferSourceNode.prototype.start=function(...args){window.fixtureAudioStarts++;return start.apply(this,args);};
    });
    await page.setViewportSize({width:1280,height:720});
    await page.goto(`http://127.0.0.1:4268/?test=1&worker=${worker}&backend=webgl2`);
    await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    await page.getByRole('button',{name:'Start',exact:true}).focus();await page.keyboard.press('Enter');
    await page.waitForFunction(()=>__nir.state().advance_wait!==null&&__nir.state().advance_wait!==undefined&&!__nir.state().loading);
    await recoverAudioOutput(page);
    const old=await page.evaluate(()=>__nir.state().interaction);
    await page.evaluate(()=>{__nir.hidden(true);__nir.action({type:'menu'});});
    await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
    await page.evaluate(()=>__nir.action({type:'save',slot:1}));
    await expect.poll(async()=>(await envelope(page))?.revision||0).toBeGreaterThan(0);
    const saved=(await envelope(page)).snapshot;
    expect(saved.waiting.advance.interaction).toBe(old);
    const movie=saved.handles.movie,music=saved.handles.music;
    expect(saved.tasks[movie].state).toBe('running');
    await page.reload();await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
    const session=await page.evaluate(()=>__nir.state().session);
    await page.evaluate(()=>__nir.action({type:'load',slot:1}));
    await page.waitForFunction(s=>__nir.state().session>s&&!__nir.state().loading&&__nir.state().paused,session);
    const current=await page.evaluate(()=>__nir.state().interaction);
    expect(current).not.toBe(old);
    const tick=await page.evaluate(()=>__nir.state().tick_us);
    await page.waitForTimeout(100);expect(await page.evaluate(()=>__nir.state().tick_us)).toBe(tick);
    await page.evaluate(()=>{__nir.hidden(false);__nir.action({type:'continue'});});
    await page.waitForFunction(()=>!__nir.state().paused);await recoverAudioOutput(page);
    await page.evaluate(token=>{const s=__nir.state();__nir.rawAction({type:'advance'},token,s.sequence+1,s.session);},old);
    await page.waitForTimeout(100);
    expect(await page.evaluate(()=>__nir.state().advance_wait)).toBe(current);
    const starts=await page.evaluate(()=>window.fixtureAudioStarts),fetched=requests.length;
    // The physical canvas click must route to the explicit wait even while
    // the old dialogue remains parked at its text pause.
    await page.locator('canvas').first().click({position:{x:900,y:300}});
    await page.waitForFunction(()=>__nir.state().advance_wait===null);
    expect(await page.evaluate(()=>__nir.state().interaction)).not.toBe(current);
    await page.waitForTimeout(250);
    expect(await page.evaluate(()=>window.fixtureAudioStarts)).toBe(starts);
    expect(requests.length).toBe(fetched);
    await page.evaluate(()=>__nir.action({type:'menu'}));
    await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading);
    await page.evaluate(()=>__nir.action({type:'save',slot:1}));
    await expect.poll(async()=>(await envelope(page))?.revision||0).toBeGreaterThan(1);
    const after=(await envelope(page)).snapshot;
    expect(after.handles.movie).toBe(movie);expect(after.tasks[movie].state).toBe('running');
    expect(after.tasks[movie].elapsed_us).not.toBe(saved.tasks[movie].elapsed_us);
    expect(after.handles.music).toBe(music);expect(after.tasks[music].state).toBe('running');
    expect(await page.evaluate(()=>__nir.state().error)).toBeNull();expect(errors).toEqual([]);
  });
}
