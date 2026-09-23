import {test} from 'node:test';
import assert from 'node:assert/strict';
import {initialPreferences} from '../../crates/nir-platform-web/host.js';
const defaults={ui_locale:'zh-Hans',text_locale:'zh-Hans',font_scale:1.2,bgm_volume:.1,voice_volume:.4,sfx_volume:.2,reduced_motion:false};
const locales={default_ui:'zh-Hans',default_text:'zh-Hans',ui:{en:['latin'],'zh-Hans':['cjk']},text:{en:['latin'],'zh-Hans':['cjk']}};
test('first visit inherits work defaults and supported browser language',()=>{
    assert.deepEqual(initialPreferences(defaults,null,locales,['en-US'],false),{...defaults,ui_locale:'en',text_locale:'en'});
    assert.equal(initialPreferences(defaults,null,locales,['zh-CN'],false).text_locale,'zh-Hans');
    assert.equal(initialPreferences(defaults,null,locales,['zh-Hant-TW','en-GB'],false).ui_locale,'en');
    assert.equal(initialPreferences(defaults,null,locales,['zh-CN','en-US'],false).ui_locale,'en');
    assert.equal(initialPreferences(defaults,null,{...locales,default_ui:'en'},['zh-Hant-TW'],false).ui_locale,'en');
    assert.equal(initialPreferences(defaults,null,locales,['zh-Hans-CN'],false).ui_locale,'zh-Hans');
});
test('UI and text browser matches are negotiated independently',()=>{
    const independent={...locales,ui:{'zh-Hans':['cjk']},text:{en:['latin']},default_ui:'zh-Hans',default_text:'en'};
    const actual=initialPreferences(defaults,null,independent,['zh-Hans-CN','en-GB'],false);
    assert.equal(actual.ui_locale,'zh-Hans');
    assert.equal(actual.text_locale,'en');
});
test('system reduced motion supplements work defaults; explicit saved preferences win',()=>{
    assert.equal(initialPreferences(defaults,null,locales,[],true).reduced_motion,true);
    assert.equal(initialPreferences({...defaults,reduced_motion:true},null,locales,[],false).reduced_motion,true);
    const saved={...defaults,font_scale:1.5,bgm_volume:.8,reduced_motion:false};
    assert.deepEqual(initialPreferences(defaults,saved,locales,['en'],true),saved);
});
test('legacy one-locale preference migrates to both independent preferences',()=>{
    const saved={locale:'en',font_scale:1.4,bgm_volume:.7,voice_volume:.3,sfx_volume:.6,reduced_motion:true};
    assert.deepEqual(initialPreferences(defaults,saved,locales,['zh-CN'],false),{
        ui_locale:'en',text_locale:'en',font_scale:1.4,bgm_volume:.7,voice_volume:.3,sfx_volume:.6,reduced_motion:true,
    });
});
