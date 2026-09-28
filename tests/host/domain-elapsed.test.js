import {test} from 'node:test';
import assert from 'node:assert/strict';
import {DomainElapsed} from '../../crates/nir-platform-web/host.js';
const story={session:1,paused:false,screen:'Story'},menu={...story,paused:true,screen:'Menu'};
test('menu entry, menu frames and return preserve only foreground elapsed',()=>{
  const clock=new DomainElapsed();
  for(const [before,after] of [[story,menu],[menu,menu],[menu,story]]) {
    clock.add(800000,{hidden:false,before,after});
    assert.deepEqual(clock.take(),[0,800000]);
  }
  clock.add(50,{hidden:false,before:story,after:story});
  assert.deepEqual(clock.take(),[50,50]);
});
test('deferred input and bounded slices retain elapsed without double counting',()=>{
  const clock=new DomainElapsed();
  clock.add(0xffffffff+20,{hidden:false,before:story,after:story});
  assert.deepEqual(clock.take(),[0xffffffff,0xffffffff]);
  assert.deepEqual(clock.take(),[20,20]);
  clock.add(50,{hidden:false,before:story,after:story});
  clock.add(70,{hidden:false,before:story,after:menu});
  assert.deepEqual(clock.take(),[0,120]);
});
test('background and session replacement discard old elapsed in both domains',()=>{
  for(const boundary of [{hidden:true,before:story,after:story},{hidden:false,before:story,after:{...story,session:2}}]) {
    const clock=new DomainElapsed();
    clock.add(50,{hidden:false,before:story,after:story});
    clock.add(100,boundary);
    assert.deepEqual(clock.take(),[0,0]);
  }
});
