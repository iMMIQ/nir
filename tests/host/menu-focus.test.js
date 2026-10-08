import {test} from 'node:test';
import assert from 'node:assert/strict';
import {focusIdentity,samePointerTarget} from '../../crates/nir-platform-web/host.js';

test('history voice focus follows the row while pointer release rechecks playback and layout',()=>{
  const a={type:'menu_history_voice',instance:1,revision:2,window:'records',layout:3,entry:7,stop:false};
  const identity=a=>focusIdentity(JSON.stringify(a));
  assert.equal(identity(a),identity({...a,revision:3,layout:4,stop:true}));
  for(const changed of [{instance:2},{window:'other'},{entry:8}])assert.notEqual(identity(a),identity({...a,...changed}));
  for(const changed of [{revision:3},{layout:4},{stop:true}])assert.ok(!samePointerTarget(a,{...a,...changed}));
});
test('menu focus identity excludes only revision, retaining instance and control identity',()=>{
  const action=(instance,revision,control)=>JSON.stringify({type:'menu_control',instance,revision,control});
  assert.equal(focusIdentity(action(1,0,'tab')),focusIdentity(action(1,1,'tab')));
  assert.notEqual(focusIdentity(action(1,0,'tab')),focusIdentity(action(2,0,'tab')));
  assert.notEqual(focusIdentity(action(1,0,'tab')),focusIdentity(action(1,0,'slot')));
  assert.notEqual(focusIdentity('{"type":"save","slot":0}'),focusIdentity('{"type":"save","slot":1}'));
  assert.equal(focusIdentity(undefined),null);
});

test('value drag preserves revision and control identity while accepting a new value',()=>{
  const a={type:'menu_value',instance:1,revision:3,control:'volume',value:0.2};
  assert.ok(samePointerTarget(a,{...a,value:0.8}));
  assert.ok(!samePointerTarget(a,{...a,revision:4}));
  assert.ok(!samePointerTarget(a,{...a,instance:2}));
  assert.ok(!samePointerTarget(a,{...a,control:'speed'}));
  assert.ok(!samePointerTarget(a,null));
  assert.equal(focusIdentity(JSON.stringify(a)),focusIdentity(JSON.stringify({...a,revision:4,value:0.8})));
});


test('history scrollbar retains each part focus while layout changes and keeps instance authority',()=>{
  const a={type:'menu_history_scroll',instance:1,revision:3,layout:4,window:'records',control:'bar',input:{type:'position',ratio:0.5}};
  const identity=a=>focusIdentity(JSON.stringify(a));
  assert.equal(identity(a),identity({...a,revision:4,layout:5,input:{type:'position',ratio:0.8}}));
  for(const change of [{instance:2},{window:'other'},{control:'other'},{input:{type:'line',delta:-1}}])assert.notEqual(identity(a),identity({...a,...change}));
  const line={...a,input:{type:'line',delta:-1}};
  assert.equal(identity(line),identity({...line,revision:5,layout:8}));
  assert.notEqual(identity(line),identity({...line,input:{type:'line',delta:1}}));
  assert.ok(!samePointerTarget(a,{...a,layout:5}));
});
