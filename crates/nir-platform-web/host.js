// Bounded owner inbox. Async producers only enqueue; they never enter Rust.
export class OwnerInbox {
    constructor(capacity=256, inputLimit=128) {
        this.capacity=capacity; this.inputLimit=inputLimit; this.items=[]; this.batch=[]; this.draining=false; this.highWater=0;
    }
    push(run, kind='completion', cancel=()=>{}, group=null) {
        const limit=kind==='input'?this.inputLimit:this.capacity;
        if(this.length>=limit)return false;
        this.items.push({run,kind,cancel,group});this.highWater=Math.max(this.highWater,this.length);return true;
    }
    drain({limit=16, milliseconds=4, now=()=>performance.now(), controlsOnly=false,canRun=()=>true}={}) {
        if(this.draining)throw new Error('E_OWNER_REENTRY');
        this.draining=true;
        const priority={control:0,input:1,completion:2,resource:3};
        this.items.sort((a,b)=>priority[a.kind]-priority[b.kind]);
        const batch=this.batch=this.items.splice(0,Math.min(limit,this.items.length)),start=now();
        let consumed=0,resources=0;
        try {
            while(batch.length) {
                const item=batch[0];
                if(!canRun(item.kind)||(controlsOnly&&item.kind!=='control') || (item.kind==='resource'&&resources===1) || (consumed>0&&now()-start>=milliseconds))break;
                consumed++;batch.shift();
                if(item.kind==='resource')resources++;
                item.run();
            }
        } finally { this.items.unshift(...batch.splice(0));this.draining=false; }
        return consumed;
    }
    get length(){return this.items.length+this.batch.length;}
    get hasInput(){return this.items.some(i=>i.kind==='input');}
    get hasControl(){return this.items.some(i=>i.kind==='control');}
    cancelGroup(group){for(const queue of [this.items,this.batch])for(let i=queue.length-1;i>=0;i--)if(queue[i].group===group)queue.splice(i,1)[0].cancel();}
    clear(){for(const queue of [this.items,this.batch])for(const item of queue.splice(0))item.cancel();}
}

// Global admission across preparation generations, including uncancellable decoders.
export class WorkPool {
    constructor(limit=4, capacity=128){this.limit=limit;this.capacity=capacity;this.active=0;this.waiting=[];}
    run(work,signal){
        if(signal.aborted)return Promise.reject(signal.reason);
        if(this.waiting.length>=this.capacity)return Promise.reject(new Error('E_RESOURCE_QUEUE'));
        return new Promise((resolve,reject)=>{
            const item={work,signal,resolve,reject};
            item.abort=()=>{const index=this.waiting.indexOf(item);if(index>=0){this.waiting.splice(index,1);reject(signal.reason);}};
            signal.addEventListener('abort',item.abort,{once:true});this.waiting.push(item);this.drain();
        });
    }
    drain(){
        while(this.active<this.limit&&this.waiting.length){
            const item=this.waiting.shift();item.signal.removeEventListener('abort',item.abort);this.active++;
            Promise.resolve().then(()=>{item.signal.throwIfAborted();return item.work();}).then(item.resolve,item.reject).finally(()=>{this.active--;this.drain();});
        }
    }
}

// A fetch belongs to its consumers, so cancelling one does not cancel another.
export class SharedRequests {
    constructor(load){this.load=load;this.jobs=new Map();}
    get(key,signal) {
        if(signal.aborted)return Promise.reject(signal.reason);
        let job=this.jobs.get(key);
        if(!job){
            const controller=new AbortController();
            job={controller,consumers:0};
            job.promise=Promise.resolve().then(()=>this.load(key,controller.signal)).finally(()=>{
                if(this.jobs.get(key)===job)this.jobs.delete(key);
            });
            this.jobs.set(key,job);
        }
        job.consumers++;
        return new Promise((resolve,reject)=>{
            let done=false;
            const finish=(fn,value)=>{
                if(done)return;done=true;signal.removeEventListener('abort',abort);
                if(--job.consumers===0){
                    if(this.jobs.get(key)===job)this.jobs.delete(key);
                    job.controller.abort();
                }
                fn(value);
            };
            const abort=()=>finish(reject,signal.reason);
            signal.addEventListener('abort',abort,{once:true});
            job.promise.then(value=>finish(resolve,value),error=>finish(reject,error));
        });
    }
}

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
    const buffers=new Map(),voices=new Map(),bytesCache=new Map(),requests=new SharedRequests(fetchObject),decodeJobs=new Map(),preparations=new Map();
    let preferences={bgm_volume:.3,voice_volume:.8,sfx_volume:.5}, raf=0,lastTime=null,sequence=0,disposed=false,recovering=false;
    const inbox=new OwnerInbox(),resourcePool=new WorkPool();
    let ownerTimer=null,pendingElapsed=0;
    const namespace=release.game_id+(location.hostname==='localhost'||location.hostname==='127.0.0.1'?':dev':'');
    const db=await new Promise((resolve,reject)=>{const r=indexedDB.open('nir-player-v1',1);r.onupgradeneeded=()=>{for(const store of ['saves','preferences','profile'])if(!r.result.objectStoreNames.contains(store))r.result.createObjectStore(store);};r.onsuccess=()=>resolve(r.result);r.onerror=()=>reject(r.error);});
    db.onversionchange=()=>db.close();
    const read=(store,key)=>new Promise((resolve,reject)=>{const tx=db.transaction(store,'readonly');const r=tx.objectStore(store).get(key);let value;r.onsuccess=()=>{value=r.result;};tx.oncomplete=()=>resolve(value);tx.onabort=tx.onerror=()=>reject(tx.error||r.error);});
    const write=(store,key,value)=>new Promise((resolve,reject)=>{const tx=db.transaction(store,'readwrite');tx.objectStore(store).put(value,key);tx.oncomplete=resolve;tx.onabort=tx.onerror=()=>reject(tx.error);});
    function hostEvent(kind,value) {engine.host_event(kind,typeof value==='string'?value:JSON.stringify(value));}
    function wake() {if(disposed||ownerTimer!==null)return;ownerTimer=setTimeout(()=>{ownerTimer=null;frame(performance.now());},0);}
    function deliver(fn,kind='completion',group=null) {
        if(disposed)return Promise.resolve(false);
        return new Promise(resolve=>{
            if(!inbox.push(()=>{try{fn();resolve(true);}catch(e){console.error(e);hostEvent('load_failed',String(e));resolve(false);}},kind,()=>resolve(false),group)){
                fail('E_EVENT_QUEUE: host inbox admission limit');resolve(false);dispose();return;
            }
            wake();
        });
    }
    function unlock() {if(audio.state!=='running'){unlocked=audio.resume();unlocked.catch(e=>console.warn('Audio unlock failed',e));}else{unlocked=Promise.resolve();}}
    function stopVoice(id) {const v=voices.get(id);if(!v)return;v.stopped=true;try{v.source?.stop();}catch{}v.source?.disconnect();v.gain?.disconnect();voices.delete(id);}
    function playVoice(c) {
        stopVoice(c.task);const buffer=buffers.get(c.asset);if(!buffer){deliver(()=>engine.audio_failed(c.task,c.session,'E_AUDIO_BUFFER'));return;}
        const source=audio.createBufferSource(),gain=audio.createGain();source.buffer=buffer;source.loop=c.looped;gain.gain.value=preferences[`${c.bus}_volume`]??.5;source.connect(gain).connect(audio.destination);
        const v={source,gain,bus:c.bus,asset:c.asset,stopped:false};voices.set(c.task,v);let offset=Number(c.position_us)/1e6;if(c.looped)offset%=buffer.duration;else offset=Math.min(offset,Math.max(0,buffer.duration-.001));
        source.onended=()=>{if(!v.stopped&&!c.looped){voices.delete(c.task);deliver(()=>engine.audio_ended(c.task,c.session));}};
        source.start(0,offset);metrics.audioStarts++;
    }
    async function asset(id,signal) {
        const a=program.assets[id];if(!a)throw new Error(`E_ASSET: ${id}`);
        signal.throwIfAborted();
        if(bytesCache.has(a.object))return bytesCache.get(a.object);
        const bytes=await requests.get(a.object,signal);signal.throwIfAborted();
        bytesCache.set(a.object,bytes);return bytes;
    }
    function cancelPreparation(request) {inbox.cancelGroup(request);const controller=preparations.get(request);controller?.abort();preparations.delete(request);}
    function prune() {
        if(disposed||recovering)return;
        const keep=new Set(JSON.parse(engine.retained()));for(const v of voices.values())keep.add(v.asset);
        const objects=new Set([...keep].map(id=>program.assets[id]?.object));
        for(const id of buffers.keys())if(!keep.has(id))buffers.delete(id);
        for(const id of bytesCache.keys())if(!objects.has(id))bytesCache.delete(id);
    }
    async function prepare(c) {
        const controller=new AbortController(),signal=controller.signal;
        preparations.set(c.request,controller);
        let next=0;
        async function worker(){while(next<c.assets.length&&!disposed&&!signal.aborted){
            const id=c.assets[next++];
            try {
                await resourcePool.run(async()=>{
                const bytes=await asset(id,signal);signal.throwIfAborted();
                if(program.assets[id].kind==='audio'){
                    if(!buffers.has(id)){
                        if(!decodeJobs.has(id))decodeJobs.set(id,audio.decodeAudioData(bytes.slice(0)).finally(()=>decodeJobs.delete(id)));
                        const buffer=await decodeJobs.get(id);signal.throwIfAborted();buffers.set(id,buffer);
                    }
                    if(unlocked)await unlocked;
                    signal.throwIfAborted();
                    if(audio.state!=='running'&&!audioPaused)throw new Error('E_AUDIO_LOCKED: activate sound with a user gesture');
                }
                // Backpressure: keep the worker occupied until its owner consumes the bytes.
                let complete=false;
                while(!complete&&!signal.aborted&&!disposed){
                    await deliver(()=>{if(!signal.aborted){try{complete=engine.resource(c.request,id,new Uint8Array(bytes));}catch(e){engine.resource_failed(c.request,String(e));controller.abort();}}},'resource',c.request);
                }
                },signal);
            }catch(e){
                if(signal.aborted||disposed)return;
                metrics.resourceFailures++;
                await deliver(()=>engine.resource_failed(c.request,String(e)));
                controller.abort();return;
            }
        }}
        try {await Promise.all(Array.from({length:Math.min(4,c.assets.length)},worker));}
        finally {if(preparations.get(c.request)===controller)preparations.delete(c.request);}
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
            case 'cancel_assets':cancelPreparation(c.request);break;
            case 'audio_start':playVoice(c);break;
            case 'audio_stop':stopVoice(c.task);break;
            case 'audio_reset':for(const id of [...voices.keys()])stopVoice(id);break;
            case 'audio_pause':audioPaused=c.paused;if(c.paused){audio.suspend().catch(()=>{});}else if(unlocked){audio.resume().catch(e=>console.warn(e));}break;
            case 'save':save(c);break;case 'load':load(c.slot);break;case 'list_saves':listSaves().catch(e=>deliver(()=>hostEvent('load_failed',String(e))));break;
            case 'persist_preferences':preferences=c.preferences;for(const v of voices.values())v.gain.gain.value=preferences[`${v.bus}_volume`]??.5;write('preferences',namespace,preferences).catch(e=>deliver(()=>hostEvent('load_failed',`E_PREFERENCES: ${e}`)));break;
            case 'persist_profile':mergeProfile(c.keys).catch(e=>deliver(()=>hostEvent('load_failed',`E_PROFILE: ${e}`)));break;
            case 'export':{const url=URL.createObjectURL(new Blob([c.json],{type:'application/json'}));const a=document.createElement('a');a.href=url;a.download=`${release.game_id}.nir-save.json`;a.click();setTimeout(()=>URL.revokeObjectURL(url),1000);break;}
            case 'import':{const input=document.createElement('input');input.type='file';input.accept='.json,application/json';input.onchange=async()=>{try{const f=input.files[0];if(!f)return;if(f.size>16*1024*1024)throw new Error('E_SAVE_LIMIT');const data=await f.text();deliver(()=>hostEvent('loaded',data));}catch(e){deliver(()=>hostEvent('load_failed',String(e)));}};input.click();break;}
            case 'trace':if(testMode){traces.push({event:c.event,at:c.at});if(traces.length>4096)traces.splice(0,traces.length-4096);}break;
            default:throw new Error(`E_HOST_PROTOCOL: ${c.type}`);
        }
    }}
    function state(){return JSON.parse(engine.state());}
    function action(a,context=state()) {
        if(disposed||recovering)return Promise.resolve(false);
        unlock();if(a.type==='new_game'&&metrics.startInputMs===null)metrics.startInputMs=performance.now();
        sequence=Math.max(sequence+1,state().sequence+1);const seq=sequence;
        return deliver(()=>engine.action(JSON.stringify(a),context.interaction,seq,context.session),'input');
    }
    let semanticSignature='',announcement='';
    function semantics(view) {
        document.documentElement.lang=view.locale||'zh-Hans';const s=state();const signature=JSON.stringify([view.nodes,s.interaction,s.session]);
        if(signature!==semanticSignature){semanticSignature=signature;const nav=document.querySelector('#actions'),focused=document.activeElement?.dataset?.action;nav.replaceChildren();for(const n of view.nodes){const b=document.createElement('button');b.textContent=n.label;b.disabled=!n.enabled;b.dataset.action=JSON.stringify(n.action);const context={interaction:s.interaction,session:s.session};b.onclick=()=>action(n.action,context);b.onfocus=()=>{const ring=document.querySelector('#focus-ring');Object.assign(ring.style,{display:'block',left:`${n.rect[0]}px`,top:`${n.rect[1]}px`,width:`${n.rect[2]}px`,height:`${n.rect[3]}px`});};b.onblur=()=>document.querySelector('#focus-ring').style.display='none';nav.append(b);if(b.dataset.action===focused)b.focus({preventScroll:true});}}
        if(view.announcement&&view.announcement!==announcement){announcement=view.announcement;document.querySelector('#announcement').textContent=announcement;}
    }
    function frame(now) {
        if(disposed)return;
        try {
            engine.begin_turn();
            const before=state(),elapsed=lastTime===null?0:Math.min(250000,Math.max(0,Math.round((now-lastTime)*1000)));lastTime=now;
            inbox.drain({canRun:kind=>!disposed&&(!recovering||kind==='control')});if(disposed)return;flush();
            if(recovering){if(inbox.hasControl)wake();return;}
            const after=state();
            if(!document.hidden&&!before.paused&&before.screen==='Story'&&!after.paused&&before.session===after.session){
                pendingElapsed=Math.min(250000,pendingElapsed+elapsed);
                if(!inbox.hasInput){engine.tick(pendingElapsed);pendingElapsed=0;}
            }else{pendingElapsed=0;}
            engine.continue_turn();flush();
            const current=size();if(current.width!==width||current.height!==height||current.dpr!==dpr){({width,height,dpr}=current);canvas.width=Math.round(width*dpr);canvas.height=Math.round(height*dpr);}
            const view=JSON.parse(engine.draw(width,height,dpr));flush();prune();semantics(view);const s=state();
            metrics.frames=s.frames;metrics.peakResidentBytes=Math.max(metrics.peakResidentBytes,s.resident_bytes);
            metrics.maxTurnUploadBytes=Math.max(metrics.maxTurnUploadBytes||0,s.turn_upload_bytes);metrics.uploadSteps=s.upload_steps;
            metrics.inboxHighWater=inbox.highWater;metrics.maxTurnWork=Math.max(metrics.maxTurnWork||0,s.turn_work);
            if(view.ready){shell.hidden=true;if(metrics.titleMs===null)metrics.titleMs=performance.now()-metrics.boot;if(s.dialogue&&metrics.firstLineMs===null){metrics.firstLineMs=performance.now()-metrics.boot;metrics.firstLineAfterStartMs=performance.now()-metrics.startInputMs;}}
            if(inbox.length||engine.pending_events())wake();
            else if(engine.needs_clock()&&!document.hidden)schedule();
        }catch(e){fail(e);dispose();}
    }
    function schedule() {if(disposed||recovering)return;if(!raf)raf=requestAnimationFrame(()=>{raf=0;wake();});}
    let down=null;
    const onDown=(e)=>{unlock();down={action:JSON.parse(engine.hit(e.clientX,e.clientY)),context:state(),x:e.clientX,y:e.clientY};};
    const onUp=(e)=>{if(down&&Math.hypot(down.x-e.clientX,down.y-e.clientY)<20){const hit=JSON.parse(engine.hit(e.clientX,e.clientY));if(down.action&&JSON.stringify(down.action)===JSON.stringify(hit))action(down.action,down.context);}down=null;};
    const onKey=(e)=>{if(e.isComposing||e.repeat||e.ctrlKey||e.metaKey||e.altKey)return;if(e.key==='Escape'){e.preventDefault();const s=state();action({type:['Menu','Settings','Saves','History'].includes(s.screen)?'close':'menu'});return;}if(document.activeElement?.tagName==='BUTTON')return;if(e.key===' '||e.key==='Enter'){e.preventDefault();const s=state();action({type:s.screen==='Title'?'new_game':s.paused?'continue':'advance'});}else if(e.key==='ArrowDown'||e.key==='ArrowUp'){e.preventDefault();document.querySelector('#actions button:not([disabled])')?.focus();}};
    const onVisibility=()=>{const hidden=document.hidden;deliver(()=>engine.hidden(hidden),'control');};
    const onResize=()=>schedule();
    canvas.addEventListener('pointerdown',onDown);canvas.addEventListener('pointerup',onUp);canvas.addEventListener('pointercancel',()=>down=null);window.addEventListener('keydown',onKey);document.addEventListener('visibilitychange',onVisibility);window.addEventListener('resize',onResize);
    const poll=setInterval(()=>{
        if(disposed||recovering)return;
        const validation=engine.gpu_error();if(validation){fail(`E_GPU_VALIDATION: ${validation}`);dispose();return;}
        if(!engine.device_lost())return;
        deliver(()=>{
            if(recovering)return;
            recovering=true;metrics.deviceRecoveries++;engine.begin_recovery();
            wasm.create_gpu('stage').then(gpu=>{
                if(disposed){gpu.free();return;}
                deliver(()=>{engine.replace_gpu(gpu);recovering=false;lastTime=performance.now();},'control');
            },e=>{if(!disposed){fail(`E_DEVICE_RECOVERY: ${e}`);dispose();}});
        },'control');
    },500);
    const testMode=new URL(location.href).searchParams.has('test'),traces=[];
    if(testMode)window.__nir={state,action,metrics,traces,rawAction:(a,token,seq,epoch)=>deliver(()=>engine.action(JSON.stringify(a),token,seq,epoch),'input'),loseDevice:()=>engine.simulate_device_loss(),hidden:(v)=>deliver(()=>engine.hidden(v))};
    const savedPreferences=await read('preferences',namespace),profile=await read('profile',namespace);
    if(savedPreferences){preferences=savedPreferences;deliver(()=>hostEvent('preferences',preferences));}else{const langs=navigator.languages||[];const locale=langs.some(l=>l==='zh'||l==='zh-CN'||l==='zh-SG'||l==='zh-Hans')?'zh-Hans':langs.some(l=>l==='en'||l.startsWith('en-'))?'en':program.default_locale;deliver(()=>hostEvent('preferences',{locale,font_scale:1,bgm_volume:.3,voice_volume:.8,sfx_volume:.5,reduced_motion:matchMedia('(prefers-reduced-motion: reduce)').matches}));}
    if(profile)deliver(()=>hostEvent('profile',profile));
    wake();
    function dispose(){if(disposed)return;disposed=true;clearTimeout(ownerTimer);inbox.clear();for(const request of [...preparations.keys()])cancelPreparation(request);cancelAnimationFrame(raf);clearInterval(poll);for(const id of [...voices.keys()])stopVoice(id);audio.close();db.close();canvas.removeEventListener('pointerdown',onDown);canvas.removeEventListener('pointerup',onUp);window.removeEventListener('keydown',onKey);window.removeEventListener('resize',onResize);document.removeEventListener('visibilitychange',onVisibility);engine.free();}
    window.addEventListener('pagehide',e=>{if(e.persisted){deliver(()=>engine.hidden(true));}else{dispose();}});
    window.addEventListener('pageshow',e=>{if(e.persisted){deliver(()=>engine.hidden(false));}});
}
