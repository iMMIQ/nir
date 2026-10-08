import {test,expect} from '@playwright/test';
import fs from 'node:fs/promises';
import {buildRealtimePcmFixture} from './realtime-pcm.fixture.js';
import {closeScaleFixture} from '../performance/fixtures.js';
import {recoverAudioOutput} from '../nir-next/audio-output-helper.js';
import {installRealtimePcm,armRealtimePcm,readRealtimePcm,compareRealtimePcm} from './realtime-pcm.js';

const fixtures={};
test.beforeAll(async()=>{
  fixtures.whole=await buildRealtimePcmFixture({region:false,port:4275});
  fixtures.region=await buildRealtimePcmFixture({region:true,port:4276});
});
test.afterAll(async()=>{for(const fixture of Object.values(fixtures))await closeScaleFixture(fixture);});

const identity=state=>({session:state.session,interaction:state.interaction,history:state.history_count,position:state.position,variables:state.variables});
async function snapshot(page,label) {
  return page.evaluate(label=>({label,state:__nir.state(),audio:__nir.diagnostics().host_work.audio_domains,
    clock:realtimePcm.contexts[0].currentTime,starts:realtimePcm.starts.length,stops:realtimePcm.stops.length,
    wall:performance.now()}),label);
}
async function ready(page,history) {
  await page.waitForFunction(history=>__nir.state().screen==='Story'&&!__nir.state().loading&&__nir.state().dialogue?.ready&&__nir.state().history_count===history,history);
  await recoverAudioOutput(page);
}
async function trustedButton(page,name) {
  const button=page.getByRole('button',{name,exact:true});await expect(button).toBeEnabled();
  const rect=JSON.parse(await button.getAttribute('data-rect'));
  await page.mouse.click(rect[0]+rect[2]/2,rect[1]+rect[3]/2);
}

for(const rate of [44100,48000])for(const worker of ['required','main'])for(const kind of ['whole','region']) {
  test(`rendered ${kind} MP3 stays sample-continuous across page, menu, stall and failed scene, ${worker}, ${rate}`,async({page},testInfo)=>{
    const fixture=fixtures[kind],gates=[],errors=[];let requests=0;
    page.on('pageerror',error=>errors.push(error.message));
    await installRealtimePcm(page,{rate});
    const pattern=`**/${fixture.imagePath}`;
    await page.context().route(pattern,async route=>{
      const attempt=++requests;await new Promise(resolve=>gates.push(resolve));
      await route.fulfill(attempt===1?{status:503,contentType:'text/plain',body:'temporary scene failure'}:
        {status:200,contentType:fixture.imageMediaType,body:fixture.imageBytes}).catch(()=>{});
    });
    const evidence={rate,worker,kind,verification:fixture.verification,observations:[],
      limitations:['rendered stereo PCM before device output, not actual DAC/speaker sound','headless Chromium 153, SwiftShader; no Android device','synthetic authored input, MP3-only release; not commercial music listening test']};
    const observe=async label=>{const row=await snapshot(page,label);evidence.observations.push(row);return row;};
    try {
      await page.goto(`${fixture.origin}/?test=1&worker=${worker}&backend=webgl2`);
      await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
      await armRealtimePcm(page);await page.keyboard.press('Enter');await ready(page,1);
      const before=await observe('first-page');expect(before.starts).toBe(1);
      expect(before.state.execution).toMatchObject({runtime:worker==='required'?'worker':'main',asset:'worker',protocol:1,fallback_reason:null});
      await page.keyboard.press('Enter');await ready(page,2);await observe('ordinary-page');
      await page.keyboard.press('Escape');await page.waitForFunction(()=>__nir.state().screen==='Menu');
      const menu=await observe('menu');await page.waitForTimeout(500);const menuLater=await observe('menu-wait');
      expect(identity(menuLater.state)).toEqual(identity(menu.state));expect(menuLater.state.tick_us).toBe(menu.state.tick_us);
      expect(menuLater.clock-menu.clock).toBeGreaterThan(.35);
      await page.keyboard.press('Escape');await ready(page,2);
      evidence.stall=await page.evaluate(()=>{
        const before={wall:performance.now(),clock:realtimePcm.contexts[0].currentTime};
        while(performance.now()-before.wall<2000){}
        return {before,after:{wall:performance.now(),clock:realtimePcm.contexts[0].currentTime}};
      });
      expect(evidence.stall.after.wall-evidence.stall.before.wall).toBeGreaterThanOrEqual(2000);
      expect(evidence.stall.after.clock-evidence.stall.before.clock).toBeGreaterThan(1.8);
      await page.keyboard.press('Enter');await expect.poll(()=>requests).toBe(1);
      await page.waitForFunction(()=>__nir.state().loading);const loading=await observe('loading');
      await page.waitForTimeout(600);const loadingLater=await observe('loading-wait');
      expect(identity(loadingLater.state)).toEqual(identity(loading.state));expect(loadingLater.state.tick_us).toBe(loading.state.tick_us);
      expect(loadingLater.clock-loading.clock).toBeGreaterThan(.4);
      gates.shift()();await page.waitForFunction(()=>__nir.state().error!==null);
      const failed=await observe('failed');await page.waitForTimeout(500);const failedLater=await observe('failed-wait');
      expect(identity(failedLater.state)).toEqual(identity(failed.state));expect(failedLater.state.tick_us).toBe(failed.state.tick_us);
      expect(failedLater.clock-failed.clock).toBeGreaterThan(.35);
      await trustedButton(page,'Retry');await expect.poll(()=>requests).toBe(2);
      await page.waitForTimeout(500);await observe('retry-wait');gates.shift()();await ready(page,3);
      await observe('scene-committed');
      const capture=await readRealtimePcm(page);
      evidence.capture={...capture,pcm:undefined,reference:undefined};
      evidence.comparison=compareRealtimePcm(capture);
      await fs.writeFile(testInfo.outputPath('rendered-pcm.f32'),Buffer.from(capture.pcm,'base64'));
      await fs.writeFile(testInfo.outputPath('decoded-reference.f32'),Buffer.from(capture.reference,'base64'));
      expect(capture.meta.rate).toBe(rate);expect(capture.meta.frames).toBe(rate*24);
      expect(capture.meta.channels).toBe(2);expect(capture.starts[0].channels).toBe(2);
      expect(capture.meta.discontinuities).toBe(0);expect(capture.starts).toHaveLength(1);
      expect(capture.stops).toHaveLength(0);expect(capture.destinationConnections).toBe(1);
      expect(capture.starts[0]).toMatchObject({offset:0,loop:true,gain:Math.fround(.3),
        loopStart:kind==='region'?.2:0});
      const expectedEnd=kind==='region'?rate*.4:capture.starts[0].frames;
      const actualEnd=capture.starts[0].loopEnd*rate;
      expect(Math.round(actualEnd)).toBe(expectedEnd);
      expect(actualEnd).toBeLessThanOrEqual(expectedEnd);
      expect(expectedEnd-actualEnd).toBeLessThanOrEqual(4*Number.EPSILON*expectedEnd);
      expect(capture.starts[0].frames).toBeGreaterThanOrEqual(rate-1);
      expect(capture.starts[0].frames).toBeLessThanOrEqual(rate);
      expect(evidence.comparison.bestAlignmentMse).toBeLessThan(1e-12);
      expect(evidence.comparison.badFrames).toBe(0);
      expect(evidence.comparison.badFramesByChannel).toEqual([0,0]);
      expect(evidence.comparison.seams).toBeGreaterThanOrEqual(kind==='region'?100:20);
      for(const row of evidence.observations) {
        const frame=Math.round(row.clock*rate);
        expect(frame).toBeGreaterThanOrEqual(evidence.comparison.firstFrame);
        expect(frame).toBeLessThan(capture.meta.first+capture.meta.frames);
      }
      expect(errors).toEqual([]);expect((await snapshot(page,'final')).state.error).toBeNull();
      evidence.requests=requests;evidence.status='passed';
    }catch(error){evidence.status='failed';evidence.error=String(error);evidence.failure=await snapshot(page,'failure').catch(()=>null);throw error;}
    finally {
      for(const release of gates)release();await page.context().unroute(pattern).catch(()=>{});
      await fs.writeFile(testInfo.outputPath('realtime-pcm.json'),JSON.stringify(evidence,null,2)+'\n');
    }
  });
}

for(const fault of ['mute','phase','right_only'])test(`PCM oracle detects real scheduled ${fault} in the player graph`,async({page},testInfo)=>{
  const fixture=fixtures.region;await installRealtimePcm(page,{rate:48000,seconds:3});
  await page.goto(`${fixture.origin}/?test=1&worker=required&backend=webgl2`);
  await page.waitForFunction(()=>globalThis.__nir?.state().ready&&!__nir.state().loading);
  await armRealtimePcm(page);await page.keyboard.press('Enter');await ready(page,1);
  const injection=await page.evaluate(fault=>{
    const at=realtimePcm.contexts[0].currentTime+.25;
    if(fault==='mute') {
      const gain=realtimePcm.finalGain.gain;
      gain.setValueAtTime(0,at);gain.setValueAtTime(realtimePcm.starts[0].gain,at+.05);
    }else if(fault==='phase'){
      // A 2% speed change for 50ms introduces approximately a 1ms phase
      // error while keeping the same live source, route and advancing clock.
      const speed=realtimePcm.firstSource.playbackRate;
      speed.setValueAtTime(1.02,at);speed.setValueAtTime(1,at+.05);
    }else {
      // Preserve both original channels, then mute only the right channel.
      // A stereo oracle must detect this without reporting a left-channel fault.
      const context=realtimePcm.contexts[0],gain=realtimePcm.finalGain;
      const split=context.createChannelSplitter(2),right=context.createGain(),merge=context.createChannelMerger(2);
      gain.connect(split);split.connect(merge,0,0);split.connect(right,1,0);right.connect(merge,0,1);
      gain.disconnect(context.destination);gain.disconnect(realtimePcm.tap);merge.connect(context.destination);
      right.gain.setValueAtTime(0,at);right.gain.setValueAtTime(1,at+.05);
    }
    return {fault,start:at,end:at+.05};
  },fault);
  const capture=await readRealtimePcm(page),comparison=compareRealtimePcm(capture);
  await fs.writeFile(testInfo.outputPath('calibration-pcm.f32'),Buffer.from(capture.pcm,'base64'));
  await fs.writeFile(testInfo.outputPath('calibration-reference.f32'),Buffer.from(capture.reference,'base64'));
  const evidence={injection,comparison,capture:{...capture,pcm:undefined,reference:undefined},verification:fixture.verification};
  try{
    expect(capture.meta.discontinuities).toBe(0);expect(capture.meta.channels).toBe(2);
    expect(capture.starts).toHaveLength(1);expect(capture.stops).toHaveLength(0);
    expect(comparison.bestAlignmentMse).toBeLessThan(1e-12);expect(comparison.badFrames).toBeGreaterThan(2000);
    expect(comparison.maxError).toBeGreaterThan(.05);
    const injectedFirst=Math.round(injection.start*48000)-capture.meta.first;
    expect(Math.abs(comparison.firstBad-injectedFirst)).toBeLessThanOrEqual(fault==='phase'?capture.meta.quantumFrames:2);
    if(fault==='right_only'){
      expect(comparison.badFramesByChannel[0]).toBe(0);expect(comparison.badFramesByChannel[1]).toBeGreaterThan(2000);
    }
    evidence.status='passed';
  }catch(error){evidence.status='failed';evidence.error=String(error);throw error;}
  finally{await fs.writeFile(testInfo.outputPath('realtime-pcm-calibration.json'),JSON.stringify(evidence,null,2)+'\n');}
});
