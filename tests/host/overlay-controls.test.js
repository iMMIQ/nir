import {test} from 'node:test';
import assert from 'node:assert/strict';
import {overlayControlPosition} from '../../crates/nir-platform-web/host.js';

test('save history never covers authored top-right navigation',()=>{
  const nodes=[{rect:[1176,12,92,36]}];
  const p=overlayControlPosition(nodes,1280,720,220,34);
  assert.deepEqual(p,[948,12]);
  assert.ok(p[0]+220+8<=1176);
});
test('compact menus can move the host control below occupied toolbar rows',()=>{
  const p=overlayControlPosition([{rect:[12,12,144,44]},{rect:[216,12,92,36]}],320,720,144,34);
  assert.ok(p);assert.ok(p[1]>=64);
});
test('an entirely occupied viewport does not replace a canvas control',()=>{
  assert.equal(overlayControlPosition([{rect:[0,0,320,120]}],320,120,144,34),null);
});
test('invalid geometry cannot position a host control outside the viewport',()=>{
  for(const width of [NaN,Infinity,0,20])assert.equal(overlayControlPosition([],width,720,144,34),null);
});
