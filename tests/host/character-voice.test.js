import {test} from 'node:test';
import assert from 'node:assert/strict';
import {normalizeCharacterVoices,characterVoiceGain} from '../../crates/nir-platform-web/host.js';

test('character mute preserves its volume and does not affect another key or unbound voice',()=>{
  const preferences={character_voices:{'speaker.aki':{volume:.4,muted:true},'speaker.other':{volume:.8,muted:false}}};
  assert.equal(characterVoiceGain(preferences,'speaker.aki'),0);
  assert.equal(characterVoiceGain(preferences,'speaker.other'),.8);
  assert.equal(characterVoiceGain(preferences,''),1);
  assert.equal(characterVoiceGain(preferences,'toString'),1);
  preferences.character_voices['speaker.aki'].muted=false;
  assert.equal(characterVoiceGain(preferences,'speaker.aki'),.4);
});
test('persisted role settings reject malformed entries and bound keys, counts and gain',()=>{
  const records=Object.fromEntries(Array.from({length:140},(_,i)=>[`speaker.${String(i).padStart(3,'0')}`,{volume:2,muted:false}]));
  records['']={volume:1,muted:false};records['很'.repeat(86)]={volume:1,muted:false};
  records.invalid={volume:NaN,muted:false};records.other={volume:.5,muted:'false'};
  const roles=normalizeCharacterVoices(records);
  assert.equal(Object.keys(roles).length,128);
  assert.deepEqual(roles['speaker.000'],{volume:1,muted:false});
  assert.equal(Object.hasOwn(roles,'speaker.139'),false);
  for(const invalid of [null,[],false,'damaged'])assert.deepEqual(normalizeCharacterVoices(invalid),{});
});
test('own JSON keys including prototype-like names remain plain data',()=>{
  const roles=normalizeCharacterVoices(JSON.parse('{"__proto__":{"volume":0.25,"muted":false}}'));
  assert.equal(Object.getPrototypeOf(roles),Object.prototype);
  assert.equal(characterVoiceGain({character_voices:roles},'__proto__'),.25);
  assert.equal(characterVoiceGain({character_voices:roles},'constructor'),1);
});
