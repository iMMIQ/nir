import {test} from 'node:test';
import assert from 'node:assert/strict';
import {validateMetadataRecord,initialRuntimePreferences} from '../../crates/nir-platform-web/host.js';

test('missing metadata differs from corrupt null or incompatible shapes',()=>{
  assert.equal(validateMetadataRecord('preferences',undefined),null);
  assert.deepEqual(validateMetadataRecord('profile',undefined),[]);
  for(const value of [null,false,1,'bad',[],new Date()])assert.throws(()=>validateMetadataRecord('preferences',value),/E_PREFERENCES_RECORD/);
  for(const value of [null,false,{},[1],new Array(1)])assert.throws(()=>validateMetadataRecord('profile',value),/E_PROFILE_RECORD/);
  assert.deepEqual(validateMetadataRecord('profile',['old','new']),['old','new']);
});
test('legacy partial preferences remain readable and are negotiated by the existing defaults',()=>{
  const saved={locale:'en',font_scale:1.4,bgm_volume:.2,reduced_motion:false};
  assert.equal(validateMetadataRecord('preferences',saved),saved);
  assert.deepEqual(validateMetadataRecord('preferences',{}),{});
  const program={player:{font_scale:1,bgm_volume:.5,voice_volume:.7,sfx_volume:.3,reduced_motion:false},locale_config:{default_ui:'en',default_text:'en',ui:{en:[]},text:{en:[]}},default_locale:'en'};
  const preferences=initialRuntimePreferences(program,saved,[],false);
  assert.equal(preferences.font_scale,1.4);assert.equal(preferences.ui_locale,'en');assert.equal(preferences.bgm_volume,.2);
  assert.equal(preferences.auto_wait_voice,true);assert.equal(preferences.voice_continue,true);
});
test('unreadable preference fields and character maps are protected instead of silently normalized for storage',()=>{
  for(const value of [{font_scale:'large'},{bgm_volume:NaN},{voice_volume:Infinity},{text_speed:null},{ui_locale:1},{voice_continue:0},{auto_wait_voice:'false'},{future_field:true},{character_voices:[]},{character_voices:{aki:{volume:.2,muted:'true'}}},{character_voices:{aki:{volume:.2,muted:false,unknown:1}}}])
    assert.throws(()=>validateMetadataRecord('preferences',value),/E_PREFERENCES_RECORD/);
  const valid={ui_locale:'unsupported',font_scale:100,bgm_volume:-1,character_voices:{aki:{volume:.2,muted:false}}};
  assert.equal(validateMetadataRecord('preferences',valid),valid); // Normal range/locale negotiation still belongs to the existing runtime.
});
