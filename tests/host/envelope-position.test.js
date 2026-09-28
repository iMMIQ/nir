import {test} from 'node:test';
import assert from 'node:assert/strict';
import {envelopePosition} from '../../crates/nir-platform-web/host.js';
test('device envelope checkpoint follows paused device time and resumes from restored progress',()=>{
  const plan={owner:12,base:250000,at:2,duration:750000};
  assert.deepEqual(envelopePosition(plan,2),{owner:12,elapsed_us:'250000'});
  assert.deepEqual(envelopePosition(plan,2.5),{owner:12,elapsed_us:'750000'});
  assert.deepEqual(envelopePosition(plan,2.5),envelopePosition(plan,2.5));
  assert.deepEqual(envelopePosition(plan,20),{owner:12,elapsed_us:'1000000'});
  assert.deepEqual(envelopePosition(plan,1),{owner:12,elapsed_us:'250000'});
  assert.equal(envelopePosition({...plan,owner:null},2),undefined);
  assert.equal(envelopePosition(undefined,2),undefined);
});
