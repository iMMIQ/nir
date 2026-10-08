import {test, expect} from '@playwright/test';
import {recoverAudioOutput} from './audio-output-helper.js';

const voices = page => page.locator('#actions button').filter({hasText:/^Replay voice$|^重播语音$/});
const stops = page => page.getByRole('button',{name:/^Stop voice$|^停止语音$/});
const snapshot = page => page.evaluate(() => {
  const s=__nir.state();return {tick:s.tick_us,interaction:s.interaction,variables:s.variables,count:s.history_count,position:s.position};
});
async function activate(page,button) {
  await recoverAudioOutput(page);
  await expect(button).toBeVisible();await button.focus();await page.keyboard.press('Enter');
}
async function audit(page) {
  await page.addInitScript(() => {
    globalThis.authoredAudio=[];
    const create=AudioContext.prototype.createBufferSource;
    AudioContext.prototype.createBufferSource=function(...args) {
      if(globalThis.rejectAudition && globalThis.__nir?.state().screen==='Menu') {
        globalThis.rejectAudition=false;throw new Error('injected authored history output failure');
      }
      const source=create.apply(this,args),start=source.start,stop=source.stop;
      const record={source,context:source.context,stops:0,ended:false};
      source.start=function(...args) {
        authoredAudio.push(record);source.addEventListener('ended',()=>{record.ended=true;});return start.apply(this,args);
      };
      source.stop=function(...args) {record.stops++;return stop.apply(this,args);};return source;
    };
  });
}
async function boot(page,port,worker) {
  await page.goto(`http://127.0.0.1:${port}/?test=1&worker=${worker}&backend=webgl2`);
  await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
  await page.keyboard.press('Enter');
  await page.waitForFunction(()=>__nir.state().screen==='Story'&&!__nir.state().loading);
  await recoverAudioOutput(page);
  for(let i=1;i<=6;i++) {
    try {
      await page.waitForFunction(i=>__nir.state().history_count===i&&__nir.state().dialogue?.ready&&!__nir.state().loading,i,{timeout:15000});
    } catch(error) {
      const proof=await page.evaluate(i=>({expected:i,state:__nir.state(),
        sources:authoredAudio.map(r=>({loop:r.source.loop,stops:r.stops,ended:r.ended,context:r.context.state,time:r.context.currentTime})),
        recovery:document.querySelector('#nir-audio-resume')?.outerHTML,hidden:document.hidden}),i);
      throw new Error(`History fixture did not reach its next reading boundary: ${JSON.stringify(proof)}; ${error}`);
    }
    if(i<6)await page.evaluate(()=>__nir.action({type:'advance'}));
  }
  await page.waitForFunction(()=>authoredAudio.filter(r=>!r.source.loop).length===6&&!authoredAudio.filter(r=>!r.source.loop)[5].ended);
  await page.keyboard.press('Escape');
  await page.waitForFunction(()=>__nir.state().screen==='Menu'&&!__nir.state().loading&&!__nir.state().history_pending&&!__nir.state().history_error);
  await expect(voices(page).first()).toBeVisible();
}
async function start(page) {
  const button=voices(page).first(),action=await button.evaluate(b=>JSON.parse(b.dataset.action));
  await activate(page,button);
  await page.waitForFunction(()=>__nir.state().history_voice&&!__nir.state().history_voice.preparing&&!__nir.state().history_voice.failed);
  await expect(stops(page)).toBeVisible();return action;
}
const sourceCount=page=>page.evaluate(()=>authoredAudio.length);
const saved=page=>page.evaluate(async()=>{
  for(const {name} of await indexedDB.databases()) {
    const db=await new Promise((ok,no)=>{const r=indexedDB.open(name);r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error);});
    if(!db.objectStoreNames.contains('saves')){db.close();continue;}
    const records=await new Promise((ok,no)=>{const tx=db.transaction('saves'),r=tx.objectStore('saves').getAll();tx.oncomplete=()=>ok(r.result);tx.onerror=()=>no(tx.error);});db.close();
    const record=records.find(r=>r.envelope?.slot===1&&r.envelope.snapshot.history.filter(h=>h.voices?.length).length===6);
    if(record)return record.envelope.snapshot;
  }return null;
});

for(const worker of ['required','main']) {
  for(const [kind,port] of [['window',4254],['flow',4255]]) {
    test(`authored ${kind} voice is independent and scroll/page/hidden/stale controls retire it, ${worker}`,async({page})=>{
      const errors=[];page.on('pageerror',e=>errors.push(e.message));
      await page.setViewportSize({width:390,height:844});await audit(page);await boot(page,port,worker);
      const before=await snapshot(page),stale=await start(page);
      expect(stale.entry).toBe(5);
      const rect=await stops(page).evaluate(b=>JSON.parse(b.dataset.rect));expect(rect[3]).toBe(44);expect(rect[2]).toBeGreaterThanOrEqual(72);
      expect(await stops(page).evaluate(b=>document.activeElement===b)).toBe(true);
      const count=await sourceCount(page);
      await page.evaluate(a=>__nir.action(a),stale);
      expect(await sourceCount(page)).toBe(count);
      await page.waitForTimeout(150);
      expect(await page.evaluate(()=>{
        const voices=authoredAudio.filter(r=>!r.source.loop),story=voices[5],audition=voices[6],music=authoredAudio.find(r=>r.source.loop);
        return {separate:story.context!==audition.context,story:story.context.state,audition:audition.context.state,storyStops:story.stops,musicStops:music.stops,playing:music.context.currentTime>0};
      })).toEqual({separate:true,story:'suspended',audition:'running',storyStops:0,musicStops:0,playing:true});
      await page.screenshot({path:`reports/experience-authored-history-${kind}-${worker}.png`});
      if(kind==='flow') {
        const view=await page.evaluate(()=>__nir.state().scrolls.find(v=>v.menu));expect(view.max).toBeGreaterThan(0);
        await page.evaluate(a=>__nir.action(a),{type:'menu_history_scroll',...view.menu,input:{type:'position',ratio:0}});
      } else await activate(page,page.getByRole('button',{name:'Older',exact:true}));
      await page.waitForFunction(()=>__nir.state().history_voice===null);
      expect(await page.evaluate(()=>authoredAudio.filter(r=>!r.source.loop)[6].stops)).toBe(1);
      await page.evaluate(a=>__nir.action(a),stale);expect(await sourceCount(page)).toBe(count);
      await start(page);
      await activate(page,page.getByRole('button',{name:'Hide records',exact:true}));
      await page.waitForFunction(()=>__nir.state().history_voice===null);
      await expect(voices(page)).toHaveCount(0);
      expect(await page.evaluate(()=>authoredAudio.filter(r=>!r.source.loop)[7].stops)).toBe(1);
      await page.evaluate(a=>__nir.action(a),stale);expect(await sourceCount(page)).toBe(count+1);
      expect(await snapshot(page)).toEqual(before);
      if(kind==='window') {
        await activate(page,page.getByRole('button',{name:'Close history',exact:true}));
        await page.waitForFunction(()=>__nir.state().screen==='Story');await page.keyboard.press('Escape');
        const reached=[];
        for(let i=0;i<6;i++) {
          await expect.poll(()=>voices(page).first().evaluate(b=>JSON.parse(b.dataset.action).entry)).toBe(5-i);
          reached.push(await voices(page).first().evaluate(b=>JSON.parse(b.dataset.action).entry));
          if(i<5)await activate(page,page.getByRole('button',{name:'Older',exact:true}));
        }
        expect(reached).toEqual([5,4,3,2,1,0]);
      }
      await activate(page,page.getByRole('button',{name:'Close history',exact:true}));
      await page.waitForFunction(()=>__nir.state().screen==='Story');
      await recoverAudioOutput(page);
      await page.waitForFunction(()=>__nir.state().screen==='Story'&&!__nir.state().paused);
      await page.waitForFunction(()=>authoredAudio.filter(r=>!r.source.loop)[5].context.state==='running');
      expect(await page.evaluate(()=>authoredAudio.filter(r=>!r.source.loop)[5].stops)).toBe(0);
      expect(errors).toEqual([]);
    });

    test(`authored ${kind} voice retries locally and survives cold save/load, ${worker}`,async({page})=>{
      const errors=[];page.on('pageerror',e=>errors.push(e.message));
      await audit(page);await boot(page,port,worker);const before=await snapshot(page);
      await page.evaluate(()=>{globalThis.rejectAudition=true;});
      await activate(page,voices(page).first());
      await page.waitForFunction(()=>__nir.state().history_voice?.failed);
      expect(await page.evaluate(()=>({error:__nir.state().error,loading:__nir.state().loading}))).toEqual({error:null,loading:false});
      await activate(page,page.getByRole('button',{name:/^Retry voice$|^重试语音$/}));
      await page.waitForFunction(()=>__nir.state().history_voice&&!__nir.state().history_voice.failed&&!__nir.state().history_voice.preparing);
      expect(await snapshot(page)).toEqual(before);
      await page.evaluate(()=>__nir.action({type:'save',slot:1}));await expect.poll(()=>saved(page)).not.toBeNull();
      const stored=await saved(page);expect(stored.history.filter(h=>h.voices?.length)).toHaveLength(6);
      await page.reload();await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
      const session=await page.evaluate(()=>__nir.state().session);
      await page.evaluate(()=>__nir.action({type:'load',slot:1}));
      await page.waitForFunction(session=>__nir.state().session!==session&&!__nir.state().loading&&__nir.state().paused,session);
      const restored=await snapshot(page);expect({...restored,interaction:before.interaction}).toEqual(before);
      await page.evaluate(()=>__nir.action({type:'continue'}));
      await page.waitForFunction(()=>__nir.state().screen==='Story');await recoverAudioOutput(page);
      await page.waitForFunction(()=>__nir.state().screen==='Story'&&!__nir.state().paused);
      await page.keyboard.press('Escape');await expect(voices(page).first()).toBeVisible();
      const frozen=await snapshot(page);await start(page);expect(await snapshot(page)).toEqual(frozen);
      await activate(page,page.getByRole('button',{name:'Close history',exact:true}));
      await page.waitForFunction(()=>__nir.state().screen==='Story'&&__nir.state().history_voice===null);
      expect(errors).toEqual([]);
    });
  }
}
