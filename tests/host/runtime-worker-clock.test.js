import {test} from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import vm from 'node:vm';

const source=await fs.readFile(new URL('../../crates/nir-platform-web/runtime-worker.js',import.meta.url),'utf8');
function runtime(initial={},active=true){
  let now=0;
  const state={session:1,interaction:1,paused:false,foreground_paused:false,screen:'Story',...initial},ticks=[],timers=[];
  const engine={
    host_state:()=>JSON.stringify(state),state:()=>JSON.stringify(state),
    begin_turn(){},continue_turn(){},tick_domains(...values){ticks.push(values);},
    action(next){Object.assign(state,next);},hidden(value){state.paused=value||state.screen==='Menu';state.foreground_paused=value;},
    commands:()=> '[]',draw:()=>'{"nodes":[]}',needs_clock:()=>state.clockDemand!==false,pending_events:()=>0,
    device_lost:()=>state.deviceLost===true,backend:()=> 'webgl2',gpu_error:()=>null,
    retained_descriptors:()=> '[]',take_profile:()=> '[]',text_cache_stats:()=> '{}',
  };
  const context={self:{engine},performance:{now:()=>now},setInterval:()=>0,setTimeout:fn=>{timers.push(fn);return timers.length;},clearTimeout(){},postMessage(){},onmessage:null};
  vm.createContext(context);
  vm.runInContext(source+`
    engine=self.engine;wasm={};canvas={width:1,height:1};size={width:1,height:1,dpr:1};last=0;clockActive=${active};outstanding=true;
    self.frame=frame;self.dispatch=dispatch;self.clock=()=>({storyElapsed,foregroundElapsed,clockActive});
  `,context);
  return {
    ticks,timers,state,clock:context.self.clock,
    frame(at){now=at;context.self.frame();},
    async action(at,next){now=at;await context.self.dispatch({protocol:1,id:1,kind:'batch',value:[{method:'action',args:[next]}]});},
    async hidden(at,value){now=at;await context.self.dispatch({protocol:1,id:1,kind:'batch',value:[{method:'hidden',args:[value]}]});},
  };
}
test('Worker foreground clock keeps a long active interval in Story and Menu',()=>{
  for(const initial of [{},{paused:true,screen:'Menu'}]){
    const owner=runtime(initial);owner.frame(900);
    assert.deepEqual(owner.ticks,[[initial.paused?0:900000,900000]]);
  }
});
test('Worker dispatches u32 slices without losing or double counting either remainder',()=>{
  const owner=runtime({clockDemand:false}),elapsed=0xffffffff+12500;
  owner.frame(elapsed/1000);
  assert.deepEqual(owner.ticks,[[0xffffffff,0xffffffff]]);
  assert.equal(owner.timers.length,1,'a remainder must schedule another turn even without new clock demand');
  owner.frame(elapsed/1000);owner.frame(elapsed/1000);
  assert.deepEqual(owner.ticks,[[0xffffffff,0xffffffff],[12500,12500],[0,0]]);
});
test('Worker menu boundaries discard Story time and retain elapsed foreground time',async()=>{
  const owner=runtime();
  await owner.action(500,{paused:true,screen:'Menu'});owner.frame(516);
  assert.deepEqual(owner.ticks,[[0,516000]]);
  await owner.action(524,{paused:false,screen:'Story'});owner.frame(540);owner.frame(556);
  assert.deepEqual(owner.ticks,[[0,516000],[16000,24000],[16000,16000]]);
});
test('Worker background and foreground pause release discard the suspended interval',async()=>{
  const owner=runtime({paused:true,screen:'Menu'});
  await owner.hidden(100,true);owner.frame(30000);
  await owner.hidden(30100,false);owner.frame(30116);
  assert.deepEqual(owner.ticks,[[0,16000]]);
  await owner.action(30200,{foreground_paused:true});owner.frame(60200);
  await owner.action(60300,{foreground_paused:false});owner.frame(60316);
  assert.deepEqual(owner.ticks,[[0,16000],[0,0],[0,16000]]);
});
test('Worker session replacement drops both old u32 remainders',async()=>{
  const owner=runtime(),at=(0xffffffff+12500)/1000;
  owner.frame(at);await owner.action(at,{session:2});owner.frame(at+16);
  assert.deepEqual(owner.ticks,[[0xffffffff,0xffffffff],[16000,16000]]);
});
test('Worker foreground-only pause leaves Story time independent',async()=>{
  const owner=runtime();
  await owner.action(100,{foreground_paused:true});owner.frame(116);
  await owner.action(200,{foreground_paused:false});owner.frame(216);
  assert.deepEqual(owner.ticks,[[116000,0],[100000,16000]]);
});
test('Worker starts a new menu clock after idle without spending the idle interval',async()=>{
  const owner=runtime({clockDemand:false},false);
  await owner.action(60000,{paused:true,screen:'Menu',clockDemand:true});owner.frame(60016);
  assert.deepEqual(owner.ticks,[[0,16000]]);
});
test('Worker menu boundary preserves a foreground remainder and drops the Story remainder',async()=>{
  const owner=runtime(),at=(0xffffffff+12500)/1000;
  owner.frame(at);await owner.action(at+50,{paused:true,screen:'Menu'});owner.frame(at+66);
  assert.deepEqual(owner.ticks,[[0xffffffff,0xffffffff],[0,78500]]);
});
test('Worker device loss clears both retained remainders before GPU restoration',()=>{
  const owner=runtime(),at=(0xffffffff+12500)/1000;
  owner.frame(at);owner.state.deviceLost=true;owner.frame(at+16);
  const clock=owner.clock();
  assert.equal(clock.storyElapsed,0);assert.equal(clock.foregroundElapsed,0);assert.equal(clock.clockActive,false);
});
