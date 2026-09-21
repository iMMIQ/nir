import {test} from 'node:test';
import assert from 'node:assert/strict';
import {initialPreferences} from '../../crates/nir-platform-web/host.js';
const defaults={locale:'zh-Hans',font_scale:1.2,bgm_volume:.1,voice_volume:.4,sfx_volume:.2,reduced_motion:false};
test('first visit inherits work defaults and supported browser language',()=>{
    assert.deepEqual(initialPreferences(defaults,null,{en:{},'zh-Hans':{}},['en-US'],false),{...defaults,locale:'en'});
    assert.equal(initialPreferences(defaults,null,{'zh-Hans':{}},['en-US'],false).locale,'zh-Hans');
});
test('system reduced motion supplements work defaults; explicit saved preferences win',()=>{
    assert.equal(initialPreferences(defaults,null,{},[],true).reduced_motion,true);
    assert.equal(initialPreferences({...defaults,reduced_motion:true},null,{},[],false).reduced_motion,true);
    const saved={...defaults,font_scale:1.5,bgm_volume:.8,reduced_motion:false};
    assert.deepEqual(initialPreferences(defaults,saved,{},[],true),saved);
});
