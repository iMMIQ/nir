export const UTF8_ENCODER = new TextEncoder();

export function parseRuntimeProgram(runtimeJson) {
    let runtime;
    try { runtime=typeof runtimeJson==='string'?JSON.parse(runtimeJson):runtimeJson; }
    catch { throw new Error('E_RUNTIME_SCHEMA'); }
    if(!runtime||runtime.format!==2||!runtime.program||typeof runtime.program!=='object')
        throw new Error('E_RUNTIME_VERSION: expected RuntimeExecutable v2');
    return runtime.program;
}

export function normalizeCharacterVoices(saved) {
    if(!saved||typeof saved!=='object'||Array.isArray(saved))return {};
    return Object.fromEntries(Object.entries(saved).filter(([id,value])=>
        id.length>0&&UTF8_ENCODER.encode(id).length<=256&&value&&typeof value==='object'&&
        typeof value.volume==='number'&&Number.isFinite(value.volume)&&typeof value.muted==='boolean')
        .sort(([a],[b])=>a<b?-1:a>b?1:0).slice(0,128)
        .map(([id,value])=>[id,{volume:Math.max(0,Math.min(1,value.volume)),muted:value.muted}]));
}
export function characterVoiceGain(preferences,character) {
    const voices=preferences?.character_voices;
    if(!character||!voices||!Object.hasOwn(voices,character))return 1;
    const value=voices[character];
    if(value?.muted===true)return 0;
    return typeof value?.volume==='number'&&Number.isFinite(value.volume)
        ?Math.max(0,Math.min(1,value.volume)):1;
}
export function initialRuntimePreferences(program,saved,languages,reducedMotion) {
    if(!program||!program.player||!program.locale_config)throw new Error('E_RUNTIME_SCHEMA');
    const defaults={
        ui_locale:program.locale_config.default_ui||program.default_locale,
        text_locale:program.locale_config.default_text||program.default_locale,
        font_scale:program.player.font_scale,
        text_speed:1,
        auto_wait_scale:1,
        auto_wait_voice:true,
        voice_continue:true,
        character_voices:{},
        bgm_volume:program.player.bgm_volume,
        voice_volume:program.player.voice_volume,
        sfx_volume:program.player.sfx_volume,
        reduced_motion:program.player.reduced_motion,
    };
    const selected=initialPreferences(defaults,saved,program.locale_config,languages,reducedMotion);
    const preferences=Object.fromEntries(['ui_locale','text_locale','text_speed','auto_wait_scale','auto_wait_voice','voice_continue','character_voices','font_scale','bgm_volume','voice_volume','sfx_volume','reduced_motion']
        .map(key=>[key,selected[key]]));
    for(const key of ['text_speed','auto_wait_scale','font_scale','bgm_volume','voice_volume','sfx_volume'])
        if(typeof preferences[key]!=='number'||!Number.isFinite(preferences[key]))preferences[key]=defaults[key];
    if(typeof preferences.reduced_motion!=='boolean')preferences.reduced_motion=defaults.reduced_motion;
    if(typeof preferences.auto_wait_voice!=='boolean')preferences.auto_wait_voice=defaults.auto_wait_voice;
    if(typeof preferences.voice_continue!=='boolean')preferences.voice_continue=defaults.voice_continue;
    preferences.character_voices=normalizeCharacterVoices(preferences.character_voices);
    return preferences;
}

export function validateAssetRequest(assets,descriptors) {
    if(!Array.isArray(assets)||!descriptors||typeof descriptors!=='object'||Array.isArray(descriptors))
        throw new Error('E_ASSET_DESCRIPTOR');
    const ids=new Set(assets);
    const keys=Object.keys(descriptors);
    if(ids.size!==assets.length||keys.length!==ids.size||keys.some(id=>!ids.has(id)))
        throw new Error('E_ASSET_DESCRIPTOR');
    for(const id of ids){
        const asset=descriptors[id];
        if(!asset||!['image','audio','font'].includes(asset.kind)||
            typeof asset.object!=='string'||!(/^[0-9a-f]{64}$/).test(asset.object)||
            !Number.isSafeInteger(asset.bytes)||asset.bytes<0||
            !Number.isSafeInteger(asset.width)||asset.width<0||
            !Number.isSafeInteger(asset.height)||asset.height<0||
            typeof asset.duration_us!=='string'||!(/^\d{1,20}$/).test(asset.duration_us)||
            !Number.isSafeInteger(asset.decoded_bytes)||asset.decoded_bytes<0)
            throw new Error(`E_ASSET_DESCRIPTOR: ${id}`);
    }
    return descriptors;
}

// Count payloads still referenced by the host. Cached and active AudioBuffers
// overlap; aliases of one buffer must not be counted twice. Browser/driver
// copies, decoder internals and detached transferred buffers are excluded.
export function mediaMemorySnapshot(bytesCache,buffers,voices,imageStaging=new Map()) {
    const encoded=new Set(bytesCache.values()),cached=new Set(buffers.values()),active=new Set();
    for(const voice of voices.values())if(voice.source?.buffer)active.add(voice.source.buffer);
    const audioBytes=items=>[...items].reduce((n,b)=>n+b.length*b.numberOfChannels*4,0);
    return {
        encoded_cache_bytes:[...encoded].reduce((n,b)=>n+b.byteLength,0),encoded_cache_objects:encoded.size,
        decoded_audio_bytes:audioBytes(new Set([...cached,...active])),decoded_audio_buffers:new Set([...cached,...active]).size,
        cached_audio_bytes:audioBytes(cached),cached_audio_assets:buffers.size,
        active_audio_bytes:audioBytes(active),active_audio_sources:voices.size,
        image_staging_bytes:[...imageStaging.values()].reduce((n,b)=>n+b.byteLength,0),
        scope:'host referenced payloads; excludes browser decoder internals, transferred buffers and physical memory'
    };
}

export function audioDecodedBudget(descriptor,sampleRate) {
    if(!Number.isSafeInteger(sampleRate)||sampleRate<=0||sampleRate>0xffffffff)
        throw Object.assign(new Error('E_AUDIO_RATE: invalid decoder rate'),{code:'E_AUDIO_RATE'});
    const frames=(BigInt(descriptor.duration_us)*BigInt(sampleRate)+999999n)/1000000n;
    const bytes=(frames+1n)*2n*4n;
    const budget=bytes>BigInt(descriptor.decoded_bytes)?bytes:BigInt(descriptor.decoded_bytes);
    if(budget>BigInt(Number.MAX_SAFE_INTEGER))throw Object.assign(new Error('E_AUDIO_MEMORY: decoded capacity overflow'),{code:'E_AUDIO_MEMORY'});
    return Number(budget);
}
export function validateDecodedAudio(descriptor,buffer,sampleRate) {
    const budget=audioDecodedBudget(descriptor,sampleRate);
    const bytes=buffer.length*buffer.numberOfChannels*4;
    if(buffer.sampleRate!==sampleRate||!Number.isSafeInteger(buffer.length)||buffer.length<=0||
        !Number.isSafeInteger(buffer.numberOfChannels)||buffer.numberOfChannels<1||buffer.numberOfChannels>2||
        !Number.isSafeInteger(bytes)||bytes>budget)
        throw Object.assign(new Error('E_AUDIO_MEMORY: decoded payload exceeds admitted capacity'),{code:'E_AUDIO_MEMORY'});
    // Capacity alone also admits truncated PCM and overlong mono decodes.
    // Match authored time before readiness so either cannot commit a scene
    // and fail later at AudioStart or end its voice prematurely.
    const nominal=(BigInt(descriptor.duration_us)*BigInt(sampleRate)+500000n)/1000000n;
    const frames=BigInt(buffer.length);
    if(frames<nominal-1n||frames>nominal+1n)
        throw Object.assign(new Error('E_AUDIO_DURATION: decoded time differs from authored duration'),{code:'E_AUDIO_DURATION'});
    return bytes;
}

// Platform adapter only. Narrative, reading policy, visual UI and layout live in Rust.

// Author defaults < browser accessibility defaults < explicitly saved player settings.
export function initialPreferences(defaults,saved,locales,languages,reducedMotion) {
    const ui=locales?.ui||{},text=locales?.text||{};
    const supports=(set,tag)=>typeof tag==='string'&&Object.hasOwn(set,tag);
    const match=(supported,fallback)=>{
        for(const tag of languages){
            if(supports(supported,tag))return tag;
            try {
                const parsed=new Intl.Locale(tag),base=parsed.language.toLowerCase();
                if(base==='en'&&supports(supported,'en'))return 'en';
                const script=(parsed.script||parsed.maximize?.().script||'').toLowerCase();
                if(base==='zh'&&script==='hans'&&supports(supported,'zh-Hans'))return 'zh-Hans';
            } catch {}
        }
        return fallback;
    };
    const defaultUi=locales?.default_ui||defaults.ui_locale||'zh-Hans';
    const defaultText=locales?.default_text||defaults.text_locale||defaultUi;
    const browserUi=match(ui,defaultUi),browserText=match(text,defaultText);
    if(saved){
        const legacy=saved.locale;
        const {locale:_oldLocale,...rest}=saved;
        const uiLocale=saved.ui_locale||(supports(ui,legacy)?legacy:browserUi);
        const textLocale=saved.text_locale||(supports(text,legacy)?legacy:browserText);
        return {...defaults,...rest,ui_locale:supports(ui,uiLocale)?uiLocale:defaultUi,text_locale:supports(text,textLocale)?textLocale:defaultText};
    }
    return {...defaults,ui_locale:browserUi,text_locale:browserText,reduced_motion:defaults.reduced_motion||reducedMotion};
}
