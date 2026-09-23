import {test} from 'node:test';
import assert from 'node:assert/strict';
import {initialPreferences,initialRuntimePreferences,parseRuntimeProgram,validateAssetRequest} from '../../crates/nir-platform-web/host.js';
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
test('RuntimeExecutable v2 is explicitly gated while v1 is rejected',()=>{
    const program={player:defaults,locale_config:locales,default_locale:'zh-Hans'};
    assert.equal(parseRuntimeProgram({format:2,program}),program);
    assert.throws(()=>parseRuntimeProgram({format:1,program}),/E_RUNTIME_VERSION/);
    assert.throws(()=>parseRuntimeProgram({format:2}),/E_RUNTIME_VERSION/);
});
test('saved preferences are negotiated from the small runtime root before boot preparation',()=>{
    const program={
        player:{font_scale:1.1,bgm_volume:.6,voice_volume:.7,sfx_volume:.3,reduced_motion:false,auto_delay_us:'1200000'},
        locale_config:locales,
        default_locale:'zh-Hans',
    };
    const saved={ui_locale:'en',text_locale:'en',font_scale:1.4,bgm_volume:.8,voice_volume:.2,sfx_volume:.5,reduced_motion:true};
    assert.deepEqual(initialRuntimePreferences(program,saved,['zh-CN'],false),saved);
    const initial=initialRuntimePreferences(program,null,['en-US'],false);
    assert.equal(initial.text_locale,'en');
    assert.equal(Object.hasOwn(initial,'auto_delay_us'),false);
    const damaged=initialRuntimePreferences(program,{ui_locale:'__proto__',text_locale:'constructor',font_scale:NaN,bgm_volume:'loud',voice_volume:Infinity,sfx_volume:null,reduced_motion:'yes',surprise:true},[],false);
    assert.deepEqual(damaged,{
        ui_locale:'zh-Hans',text_locale:'zh-Hans',font_scale:1.1,bgm_volume:.6,voice_volume:.7,sfx_volume:.3,reduced_motion:false,
    });
});
test('asset requests must carry exact full descriptors for every requested ID',()=>{
    const descriptors={
        'bg.station':{kind:'image',object:'a'.repeat(64),bytes:128,width:1280,height:720,duration_us:'0',decoded_bytes:3686400},
        'audio.bgm':{kind:'audio',object:'b'.repeat(64),bytes:256,width:0,height:0,duration_us:'1000',decoded_bytes:4},
    };
    assert.equal(validateAssetRequest(['bg.station','audio.bgm'],descriptors),descriptors);
    assert.throws(()=>validateAssetRequest(['bg.station'],descriptors),/E_ASSET_DESCRIPTOR/);
    assert.throws(()=>validateAssetRequest(['bg.station','bg.station'],descriptors),/E_ASSET_DESCRIPTOR/);
    assert.throws(()=>validateAssetRequest(['bg.station'],{'bg.station':{...descriptors['bg.station'],object:'bad'}}),/E_ASSET_DESCRIPTOR/);
    assert.throws(()=>validateAssetRequest(['bg.station'],{'bg.station':{...descriptors['bg.station'],kind:'video'}}),/E_ASSET_DESCRIPTOR/);
});
