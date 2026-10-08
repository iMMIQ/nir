import {test} from 'node:test';
import assert from 'node:assert/strict';
import {audioLoopPlayback,audioLoopEndSeconds} from '../../crates/nir-platform-web/host.js';

const region={start_us:'200000',end_us:'600000'};
test('loop playheads traverse intro once and restore inside the repeating body',()=>{
  for(const [us,frame] of [['0',0],['100000',1],['600000',2],['900000',5],['1400000',2]]){
    assert.deepEqual(audioLoopPlayback(region,us,10,8),{startFrame:2,endFrame:6,offsetFrame:frame});
  }
  // Maximum u64 playhead stays exact; floating-point modulo loses its phase.
  assert.equal(audioLoopPlayback(region,'18446744073709551615',10,8).offsetFrame,4);
  assert.deepEqual(audioLoopPlayback({start_us:'1999',end_us:'9999'},'20000',1000,12),
    {startFrame:2,endFrame:10,offsetFrame:4});
});

test('invalid decoded intervals fail rather than silently using a whole-buffer loop',()=>{
  for(const value of [
    {start_us:'600000',end_us:'600000'},
    {start_us:'700000',end_us:'600000'},
    {start_us:'0',end_us:'900000'},
    {start_us:'200001',end_us:'200002'},
    {start_us:'-1',end_us:'600000'},
    {start_us:'200000',end_us:'18446744073709551616'},
  ])assert.throws(()=>audioLoopPlayback(value,'0',10,8),/E_AUDIO_LOOP/);
  assert.throws(()=>audioLoopPlayback(region,200000,10,8),/E_AUDIO_LOOP/);
  assert.throws(()=>audioLoopPlayback(region,'0',0,8),/E_AUDIO_LOOP/);
  assert.throws(()=>audioLoopPlayback(region,'0',10,0),/E_AUDIO_LOOP/);
});

test('a full authored endpoint accepts one lost resampling frame and keeps intro/body phase',()=>{
  const tail={start_us:'200000',end_us:'800000'};
  assert.deepEqual(audioLoopPlayback(tail,'0',44100,35279,'800000'),{startFrame:8820,endFrame:35279,offsetFrame:0});
  assert.deepEqual(audioLoopPlayback(tail,'800000',44100,35279,'800000'),{startFrame:8820,endFrame:35279,offsetFrame:8821});
  assert.throws(()=>audioLoopPlayback(tail,'0',44100,35279),/E_AUDIO_LOOP/);
  assert.throws(()=>audioLoopPlayback(tail,'0',44100,35279,'900000'),/E_AUDIO_LOOP/);
  assert.throws(()=>audioLoopPlayback(tail,'0',44100,35278,'800000'),/E_AUDIO_LOOP/);
  assert.throws(()=>audioLoopPlayback({start_us:'800000',end_us:'800001'},'0',44100,35279,'800000'),/E_AUDIO_LOOP/);
  assert.throws(()=>audioLoopPlayback({start_us:'799990',end_us:'800000'},'0',44100,35279,'800000'),/E_AUDIO_LOOP/);
});

test('exclusive loop seconds preserve the final frame at native and decoded rates',()=>{
  let seed=0x61c88647;
  for(const rate of [1,8000,22050,44100,48000,96000,192000,0xffffffff]) {
    const frames=[1,7,13,3139545,0xffffffff];
    for(let i=0;i<2000;i++){seed=(Math.imul(seed,1664525)+1013904223)>>>0;frames.push(seed||1);}
    for(const count of frames){
      const seconds=audioLoopEndSeconds(count,rate),end=seconds*rate;
      assert(end<=count,`endpoint crossed frame ${count} at ${rate}Hz`);
      assert(count-end<=4*Number.EPSILON*count,`endpoint trimmed frame ${count} at ${rate}Hz`);
      assert.equal(Math.round(end),count);
    }
  }
  for(const [frames,rate] of [[0,48000],[-1,48000],[1.5,48000],[0x100000000,48000],[1,0],[1,NaN],[1,1.5],[1,0x100000000]])
    assert.throws(()=>audioLoopEndSeconds(frames,rate),/E_AUDIO_LOOP/);
});
