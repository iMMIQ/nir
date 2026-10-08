import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';

for(const worker of ['main','required']) {
  test(`export failure keeps the scene and BGM then a new export downloads a valid snapshot; ${worker}`,async({page},info)=>{
    const errors=[];page.on('pageerror',e=>errors.push(e.message));
    await page.addInitScript(()=>{
      window.exportSources=[];
      const create=AudioContext.prototype.createBufferSource;
      AudioContext.prototype.createBufferSource=function(...args){
        const source=create.apply(this,args),row={source,stops:0};exportSources.push(row);
        const stop=source.stop;source.stop=function(...args){row.stops++;return stop.apply(this,args);};return source;
      };
      window.exportStory=()=>{const s=__nir.state();return {session:s.session,interaction:s.interaction,tick:s.tick_us,position:s.position,screen:s.screen};};
      window.exportLoops=()=>exportSources.filter(r=>r.source.loop).map(r=>({stops:r.stops}));
    });
    await page.goto(`http://127.0.0.1:4259/?test=1&worker=${worker}&backend=webgl2`);
    await page.waitForFunction(()=>window.__nir?.state().ready&&!__nir.state().loading);
    await page.keyboard.press('Enter');await page.waitForFunction(()=>__nir.state().dialogue?.ready&&!__nir.state().loading);
    await page.evaluate(()=>__nir.action({type:'saves'}));
    await page.waitForFunction(()=>__nir.state().screen==='Saves'&&!__nir.state().loading);
    const before=await page.evaluate(()=>({story:exportStory(),loops:exportLoops()}));expect(before.loops.length).toBeGreaterThan(0);
    await page.evaluate(()=>{
      const create=URL.createObjectURL;
      URL.createObjectURL=function(...args){URL.createObjectURL=create;throw new Error('Injected export allocation failure');};
      __nir.action({type:'export'});
    });
    await page.waitForFunction(()=>__nir.state().diagnostic?.location==='export');
    const failed=await page.evaluate(()=>({story:exportStory(),loops:exportLoops(),state:__nir.state()}));
    expect(failed.story).toEqual(before.story);expect(failed.loops).toEqual(before.loops);expect(failed.state.error).toBeNull();
    expect(failed.state.diagnostic.message).toContain('Injected export allocation failure');
    const download=page.waitForEvent('download');await page.evaluate(()=>__nir.action({type:'export'}));
    const file=await download;expect(file.suggestedFilename()).toMatch(/\.nir-save\.json$/);
    const path=info.outputPath('export.nir-save.json');await file.saveAs(path);const json=await fs.readFile(path,'utf8');
    const envelope=JSON.parse(json);expect(envelope.revision).toBe(0);
    await page.waitForFunction(()=>__nir.state().diagnostic?.location!=='export');
    const recovered=await page.evaluate(()=>({story:exportStory(),loops:exportLoops(),state:__nir.state()}));
    expect(recovered.story).toEqual(before.story);expect(recovered.loops).toEqual(before.loops);expect(errors).toEqual([]);
    // Export envelopes have revision zero, unlike persisted slot records.
    // Use the actual file importer and staged restore to prove the round trip.
    const rect=await page.evaluate(()=>{
      const node=[...document.querySelectorAll('#actions button')].find(n=>JSON.parse(n.dataset.action).type==='import');
      return node?JSON.parse(node.dataset.rect):null;
    });expect(rect).not.toBeNull();
    const choose=page.waitForEvent('filechooser');
    await page.locator('#stage').click({position:{x:rect[0]+rect[2]/2,y:rect[1]+rect[3]/2}});
    await (await choose).setFiles(path);
    await page.waitForFunction(session=>__nir.state().session>session&&!__nir.state().loading&&__nir.state().screen==='Story',before.story.session);
    const imported=await page.evaluate(()=>__nir.state());
    expect(imported.error).toBeNull();expect(imported.paused).toBe(true);
    expect(imported.tick_us).toBe(envelope.snapshot.tick_us);expect(imported.position).toBe(before.story.position);
    expect(errors).toEqual([]);
    await fs.writeFile(info.outputPath('export-recovery.json'),JSON.stringify({before,failed,recovered,imported,exportRevision:envelope.revision,
      scope:'Actual desktop browser export allocation failure, subsequent download, file picker import and staged restore/digest verification; not an external save-directory durability or hardware sound proof.'},null,2)+'\n');
  });
}
