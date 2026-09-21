// Platform adapter only. Narrative, reading policy, visual UI and layout live in Rust.
export async function start({wasm,release,releaseDigest,executable,fetchObject,fail}) {
    const canvas=document.querySelector('#stage'), shell=document.querySelector('#shell');
    const metrics={boot:performance.now(),titleMs:null,firstLineMs:null,resourceFailures:0,frames:0,audioStarts:0,deviceRecoveries:0,peakResidentBytes:0,startInputMs:null,firstLineAfterStartMs:null};
    const size=()=>{const dpr=Math.min(devicePixelRatio||1,2);const width=innerWidth,height=innerHeight;return {width,height,dpr};};
    let {width,height,dpr}=size();canvas.width=Math.round(width*dpr);canvas.height=Math.round(height*dpr);
    const engine=await wasm.Engine.create(executable,releaseDigest,release.title,'stage');
    const program=JSON.parse(executable).program;
    document.title=release.title;
    const AudioContext=window.AudioContext||window.webkitAudioContext;
    const audio=new AudioContext();let unlocked=null,audioPaused=true;
    const buffers=new Map(),voices=new Map(),bytesCache=new Map(),inflight=new Map(),decodeJobs=new Map();
    let preferences={bgm_volume:.3,voice_volume:.8,sfx_volume:.5}, raf=0,lastTime=null,sequence=0,disposed=false,recovering=false;
    const pendingCallbacks=[];
    const namespace=release.game_id+(location.hostname==='localhost'||location.hostname==='127.0.0.1'?':dev':'');
    const db=await new Promise((resolve,reject)=>{const r=indexedDB.open('nir-player-v1',1);r.onupgradeneeded=()=>{for(const store of ['saves','preferences','profile'])if(!r.result.objectStoreNames.contains(store))r.result.createObjectStore(store);};r.onsuccess=()=>resolve(r.result);r.onerror=()=>reject(r.error);});
    db.onversionchange=()=>db.close();
    const read=(store,key)=>new Promise((resolve,reject)=>{const tx=db.transaction(store,'readonly');const r=tx.objectStore(store).get(key);let value;r.onsuccess=()=>{value=r.result;};tx.oncomplete=()=>resolve(value);tx.onabort=tx.onerror=()=>reject(tx.error||r.error);});
    const write=(store,key,value)=>new Promise((resolve,reject)=>{const tx=db.transaction(store,'readwrite');tx.objectStore(store).put(value,key);tx.oncomplete=resolve;tx.onabort=tx.onerror=()=>reject(tx.error);});
    function hostEvent(kind,value) {engine.host_event(kind,typeof value==='string'?value:JSON.stringify(value));flush();schedule();}
    function deliver(fn) {if(disposed)return;if(recovering){pendingCallbacks.push(fn);return;}try{syncTime();fn();flush();schedule();}catch(e){console.error(e);try{hostEvent('load_failed',String(e));}catch(fatal){fail(fatal);}}}
    function unlock() {if(audio.state!=='running'){unlocked=audio.resume();unlocked.catch(e=>console.warn('Audio unlock failed',e));}else{unlocked=Promise.resolve();}}
    function stopVoice(id) {const v=voices.get(id);if(!v)return;v.stopped=true;try{v.source?.stop();}catch{}v.source?.disconnect();v.gain?.disconnect();voices.delete(id);}
    function playVoice(c) {
        stopVoice(c.task);const buffer=buffers.get(c.asset);if(!buffer){deliver(()=>engine.audio_failed(c.task,c.session,'E_AUDIO_BUFFER'));return;}
        const source=audio.createBufferSource(),gain=audio.createGain();source.buffer=buffer;source.loop=c.looped;gain.gain.value=preferences[`${c.bus}_volume`]??.5;source.connect(gain).connect(audio.destination);
        const v={source,gain,bus:c.bus,asset:c.asset,stopped:false};voices.set(c.task,v);let offset=Number(c.position_us)/1e6;if(c.looped)offset%=buffer.duration;else offset=Math.min(offset,Math.max(0,buffer.duration-.001));
        source.onended=()=>{if(!v.stopped&&!c.looped){voices.delete(c.task);deliver(()=>engine.audio_ended(c.task,c.session));}};
        source.start(0,offset);metrics.audioStarts++;
    }
    async function asset(id) {const a=program.assets[id];if(!a)throw new Error(`E_ASSET: ${id}`);if(bytesCache.has(a.object))return bytesCache.get(a.object);if(inflight.has(a.object))return inflight.get(a.object);const job=fetchObject(a.object).then(b=>{bytesCache.set(a.object,b);return b;}).finally(()=>inflight.delete(a.object));inflight.set(a.object,job);return job;}
    function prune() {
        if(disposed||recovering)return;
        const keep=new Set(JSON.parse(engine.retained()));for(const v of voices.values())keep.add(v.asset);
        const objects=new Set([...keep].map(id=>program.assets[id]?.object));
        for(const id of buffers.keys())if(!keep.has(id))buffers.delete(id);
        for(const id of bytesCache.keys())if(!objects.has(id))bytesCache.delete(id);
    }
    async function prepare(c) {
        // Small bounded groups; no speculative jobs can occupy a required-resource slot.
        let next=0;async function worker(){while(next<c.assets.length&&!disposed){if(!recovering&&!engine.accepts(c.request))break;const id=c.assets[next++];try{const bytes=await asset(id);if(!recovering&&!engine.accepts(c.request)){prune();continue;}if(program.assets[id].kind==='audio'){
                    if(!buffers.has(id)){if(!decodeJobs.has(id))decodeJobs.set(id,audio.decodeAudioData(bytes.slice(0)).finally(()=>decodeJobs.delete(id)));buffers.set(id,await decodeJobs.get(id));}
                    if(unlocked)await unlocked;
                    if(audio.state!=='running'&&!audioPaused)throw new Error('E_AUDIO_LOCKED: activate sound with a user gesture');
                }
                deliver(()=>engine.resource(c.request,id,new Uint8Array(bytes)));prune();
            }catch(e){metrics.resourceFailures++;deliver(()=>engine.resource_failed(c.request,String(e)));}}}
        await Promise.all(Array.from({length:Math.min(4,c.assets.length)},worker));
    }
    async function listSaves() {const rows=[];for(let slot=0;slot<3;slot++){const s=await read('saves',`${namespace}:${slot}`);if(s)rows.push({slot,revision:s.revision,label:s.label||`#${s.revision}`});}deliver(()=>hostEvent('slots',rows));}
    async function save(c) {
        try{await new Promise((resolve,reject)=>{const tx=db.transaction('saves','readwrite'),store=tx.objectStore('saves'),r=store.get(`${namespace}:${c.slot}`);let conflict=false;
            r.onsuccess=()=>{const current=r.result;if((current?.revision||0)!==c.expected_revision){conflict=true;tx.abort();return;}const record={...c.envelope,label:new Date().toLocaleString(),saved_at:Date.now()};store.put(record,`${namespace}:${c.slot}`);};
            tx.oncomplete=resolve;tx.onabort=()=>reject(new Error(conflict?'E_SAVE_CONFLICT: another tab changed this slot. Reopen the save menu.':`E_STORAGE: ${tx.error}`));tx.onerror=()=>{};
        });deliver(()=>hostEvent('saved',{job:c.job,slot:c.slot,revision:c.envelope.revision}));}
        catch(e){deliver(()=>hostEvent('save_failed',{job:c.job,message:String(e)}));}
    }
    const envelope=(record)=>{const {label,saved_at,...e}=record;return e;};
    async function load(slot) {try{const s=await read('saves',`${namespace}:${slot}`);if(!s)throw new Error('E_SAVE_MISSING');deliver(()=>hostEvent('loaded',envelope(s)));}catch(e){deliver(()=>hostEvent('load_failed',String(e)));}}
    async function mergeProfile(keys) {await new Promise((resolve,reject)=>{const tx=db.transaction('profile','readwrite'),store=tx.objectStore('profile'),r=store.get(namespace);r.onsuccess=()=>store.put([...new Set([...(r.result||[]),...keys])].sort(),namespace);tx.oncomplete=resolve;tx.onabort=tx.onerror=()=>reject(tx.error);});}
    function flush() {if(disposed)return;for(const c of JSON.parse(engine.commands())){
        switch(c.type){
            case 'get_assets':prepare(c);break;
            case 'audio_start':playVoice(c);break;
            case 'audio_stop':stopVoice(c.task);break;
            case 'audio_reset':for(const id of [...voices.keys()])stopVoice(id);break;
            case 'audio_pause':audioPaused=c.paused;if(c.paused){audio.suspend().catch(()=>{});}else if(unlocked){audio.resume().catch(e=>console.warn(e));}break;
            case 'save':save(c);break;case 'load':load(c.slot);break;case 'list_saves':listSaves().catch(e=>deliver(()=>hostEvent('load_failed',String(e))));break;
            case 'persist_preferences':preferences=c.preferences;for(const v of voices.values())v.gain.gain.value=preferences[`${v.bus}_volume`]??.5;write('preferences',namespace,preferences).catch(e=>deliver(()=>hostEvent('load_failed',`E_PREFERENCES: ${e}`)));break;
            case 'persist_profile':mergeProfile(c.keys).catch(e=>deliver(()=>hostEvent('load_failed',`E_PROFILE: ${e}`)));break;
            case 'export':{const url=URL.createObjectURL(new Blob([c.json],{type:'application/json'}));const a=document.createElement('a');a.href=url;a.download=`${release.game_id}.nir-save.json`;a.click();setTimeout(()=>URL.revokeObjectURL(url),1000);break;}
            case 'import':{const input=document.createElement('input');input.type='file';input.accept='.json,application/json';input.onchange=async()=>{try{const f=input.files[0];if(!f)return;if(f.size>16*1024*1024)throw new Error('E_SAVE_LIMIT');const data=await f.text();deliver(()=>hostEvent('loaded',data));}catch(e){deliver(()=>hostEvent('load_failed',String(e)));}};input.click();break;}
            case 'trace':if(testMode)traces.push({event:c.event,at:c.at});break;
            default:throw new Error(`E_HOST_PROTOCOL: ${c.type}`);
        }
    }}
    function state(){return JSON.parse(engine.state());}
    function action(a,context=state()) {if(disposed||recovering)return;unlock();if(a.type==='new_game'&&metrics.startInputMs===null)metrics.startInputMs=performance.now();if(a.type!=='choose')syncTime();sequence=Math.max(sequence+1,state().sequence+1);try{engine.action(JSON.stringify(a),context.interaction,sequence,context.session);flush();schedule();}catch(e){console.error(e);}}
    function syncTime(now=performance.now(),includeHidden=false) {
        const s=state();if(lastTime!==null&&(!document.hidden||includeHidden)&&!s.paused&&s.screen==='Story')engine.tick(Math.min(4294967295,Math.max(0,Math.round((now-lastTime)*1000))));lastTime=now;
    }
    let semanticSignature='',announcement='';
    function semantics(view) {
        document.documentElement.lang=view.locale||'zh-Hans';const s=state();const signature=JSON.stringify([view.nodes,s.interaction,s.session]);
        if(signature!==semanticSignature){semanticSignature=signature;const nav=document.querySelector('#actions'),focused=document.activeElement?.dataset?.action;nav.replaceChildren();for(const n of view.nodes){const b=document.createElement('button');b.textContent=n.label;b.disabled=!n.enabled;b.dataset.action=JSON.stringify(n.action);const context={interaction:s.interaction,session:s.session};b.onclick=()=>action(n.action,context);b.onfocus=()=>{const ring=document.querySelector('#focus-ring');Object.assign(ring.style,{display:'block',left:`${n.rect[0]}px`,top:`${n.rect[1]}px`,width:`${n.rect[2]}px`,height:`${n.rect[3]}px`});};b.onblur=()=>document.querySelector('#focus-ring').style.display='none';nav.append(b);if(b.dataset.action===focused)b.focus({preventScroll:true});}}
        if(view.announcement&&view.announcement!==announcement){announcement=view.announcement;document.querySelector('#announcement').textContent=announcement;}
    }
    function frame(now) {raf=0;if(disposed||recovering)return;try{
        syncTime(now);flush();
        const current=size();if(current.width!==width||current.height!==height||current.dpr!==dpr){({width,height,dpr}=current);canvas.width=Math.round(width*dpr);canvas.height=Math.round(height*dpr);}
        const view=JSON.parse(engine.draw(width,height,dpr));flush();prune();semantics(view);const s=state();metrics.frames=s.frames;metrics.peakResidentBytes=Math.max(metrics.peakResidentBytes,s.resident_bytes);
        if(view.ready){shell.hidden=true;if(metrics.titleMs===null)metrics.titleMs=performance.now()-metrics.boot;if(s.dialogue&&metrics.firstLineMs===null){metrics.firstLineMs=performance.now()-metrics.boot;metrics.firstLineAfterStartMs=performance.now()-metrics.startInputMs;}}
        if(engine.needs_clock()&&!document.hidden)schedule();
    }catch(e){fail(e);}}
    function schedule() {if(disposed||recovering)return;if(!raf)raf=requestAnimationFrame(frame);}
    let down=null;
    const onDown=(e)=>{unlock();down={action:JSON.parse(engine.hit(e.clientX,e.clientY)),context:state(),x:e.clientX,y:e.clientY};};
    const onUp=(e)=>{if(down&&Math.hypot(down.x-e.clientX,down.y-e.clientY)<20){const hit=JSON.parse(engine.hit(e.clientX,e.clientY));if(down.action&&JSON.stringify(down.action)===JSON.stringify(hit))action(down.action,down.context);}down=null;};
    const onKey=(e)=>{if(e.isComposing||e.repeat||e.ctrlKey||e.metaKey||e.altKey)return;if(e.key==='Escape'){e.preventDefault();const s=state();action({type:['Menu','Settings','Saves','History'].includes(s.screen)?'close':'menu'});return;}if(document.activeElement?.tagName==='BUTTON')return;if(e.key===' '||e.key==='Enter'){e.preventDefault();const s=state();action({type:s.screen==='Title'?'new_game':s.paused?'continue':'advance'});}else if(e.key==='ArrowDown'||e.key==='ArrowUp'){e.preventDefault();document.querySelector('#actions button:not([disabled])')?.focus();}};
    const onVisibility=()=>{syncTime(performance.now(),true);deliver(()=>engine.hidden(document.hidden));};
    const onResize=()=>schedule();
    canvas.addEventListener('pointerdown',onDown);canvas.addEventListener('pointerup',onUp);canvas.addEventListener('pointercancel',()=>down=null);window.addEventListener('keydown',onKey);document.addEventListener('visibilitychange',onVisibility);window.addEventListener('resize',onResize);
    const poll=setInterval(async()=>{if(disposed||recovering)return;const validation=engine.gpu_error();if(validation){fail(`E_GPU_VALIDATION: ${validation}`);clearInterval(poll);return;}if(!engine.device_lost())return;recovering=true;metrics.deviceRecoveries++;try{syncTime();engine.begin_recovery();flush();const gpu=await wasm.create_gpu('stage');engine.replace_gpu(gpu);recovering=false;for(const fn of pendingCallbacks.splice(0))fn();flush();schedule();}catch(e){recovering=false;fail(`E_DEVICE_RECOVERY: ${e}`);}},500);
    const testMode=new URL(location.href).searchParams.has('test'),traces=[];
    if(testMode)window.__nir={state,action,metrics,traces,rawAction:(a,token,seq,epoch)=>deliver(()=>engine.action(JSON.stringify(a),token,seq,epoch)),loseDevice:()=>engine.simulate_device_loss(),hidden:(v)=>deliver(()=>engine.hidden(v))};
    const savedPreferences=await read('preferences',namespace),profile=await read('profile',namespace);
    if(savedPreferences){preferences=savedPreferences;hostEvent('preferences',preferences);}else{const langs=navigator.languages||[];const locale=langs.some(l=>l==='zh'||l==='zh-CN'||l==='zh-SG'||l==='zh-Hans')?'zh-Hans':langs.some(l=>l==='en'||l.startsWith('en-'))?'en':program.default_locale;hostEvent('preferences',{locale,font_scale:1,bgm_volume:.3,voice_volume:.8,sfx_volume:.5,reduced_motion:matchMedia('(prefers-reduced-motion: reduce)').matches});}
    if(profile)hostEvent('profile',profile);
    flush();schedule();
    function dispose(){if(disposed)return;disposed=true;cancelAnimationFrame(raf);clearInterval(poll);for(const id of [...voices.keys()])stopVoice(id);audio.close();db.close();canvas.removeEventListener('pointerdown',onDown);canvas.removeEventListener('pointerup',onUp);window.removeEventListener('keydown',onKey);window.removeEventListener('resize',onResize);document.removeEventListener('visibilitychange',onVisibility);engine.free();}
    window.addEventListener('pagehide',e=>{if(e.persisted){deliver(()=>engine.hidden(true));}else{dispose();}});
    window.addEventListener('pageshow',e=>{if(e.persisted){deliver(()=>engine.hidden(false));}});
}
