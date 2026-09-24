// Diagnostic-only ring: explicit fields, bounded storage, no narrative payloads.
export class TraceRecorder {
    constructor({capacity=4096,enabled=false,now=()=>performance.now()}={}) {
        this.capacity=Math.max(1,Math.min(4096,Math.trunc(capacity)||4096));this.enabled=enabled;this.now=now;
        this.rows=new Array(this.capacity);this.total=0;this.size=0;
    }
    record(stage,fields={}) {
        if(!this.enabled)return;
        const row={stage:String(stage).slice(0,64),at_us:String(Math.max(0,Math.round(this.now()*1000)))};
        // Never copy message/cause/action/variables/snapshot/URL or arbitrary objects.
        for(const key of ['session','device','request','host_request','task','origin_session','sequence','bytes','frames'])
            if(Number.isSafeInteger(fields[key])&&fields[key]>=0)row[key]=fields[key];
        for(const key of ['asset','object','location','cue','code','domain','operation','kind','outcome','locale'])
            if(typeof fields[key]==='string')row[key]=fields[key].slice(0,192);
        for(const key of ['start_us','end_us'])if(typeof fields[key]==='string'&&/^\d{1,20}$/.test(fields[key]))row[key]=fields[key];
        this.rows[this.total%this.capacity]=row;this.total++;this.size=Math.min(this.size+1,this.capacity);
    }
    snapshot() { return {format:1,enabled:this.enabled,capacity:this.capacity,dropped:Math.max(0,this.total-this.size),events:Array.from({length:this.size},(_,i)=>({...this.rows[(this.total-this.size+i)%this.capacity]}))}; }
}

// Detailed CPU timing is opt-in with trace diagnostics. Totals are accumulated
// per stage; turn wall time is measured separately and never summed from nested
// stages. The turn ring stays bounded even during long diagnostic sessions.
export class PerformanceRecorder {
    constructor(capacity=64) {
        this.capacity=Math.max(1,Math.min(64,Math.trunc(capacity)||64));
        this.stages=Object.create(null);this.turns=new Array(this.capacity);
        this.totalTurns=0;this.size=0;
    }
    beginTurn(startUs) { return {id:this.totalTurns+1,start_us:startUs,stages:[]}; }
    record(stage,startUs,endUs,turn) {
        if(!Number.isFinite(startUs)||!Number.isFinite(endUs)||endUs<startUs)return;
        const name=String(stage).slice(0,64),start=Math.round(startUs),end=Math.round(endUs),duration=end-start;
        let total=this.stages[name];
        if(!total)total=this.stages[name]={count:0,total_us:0,min_us:duration,max_us:duration};
        total.count++;total.total_us+=duration;total.min_us=Math.min(total.min_us,duration);total.max_us=Math.max(total.max_us,duration);
        if(turn)turn.stages.push({stage:name,start_us:start,end_us:end,duration_us:duration});
    }
    endTurn(turn,endUs) {
        if(!turn)return;
        turn.end_us=endUs;turn.total_us=Math.max(0,endUs-turn.start_us);
        turn.stages.sort((a,b)=>a.start_us-b.start_us||a.end_us-b.end_us);
        this.turns[this.totalTurns%this.capacity]=turn;
        this.totalTurns++;this.size=Math.min(this.size+1,this.capacity);
    }
    snapshot() {
        const turns=Array.from({length:this.size},(_,i)=>{
            const turn=this.turns[(this.totalTurns-this.size+i)%this.capacity];
            return {id:turn.id,start_us:turn.start_us,end_us:turn.end_us,total_us:turn.total_us,stages:turn.stages.map(row=>({...row}))};
        });
        const stages=Object.fromEntries(Object.entries(this.stages).map(([name,value])=>[name,{...value}]));
        return {enabled:true,turn_capacity:this.capacity,total_turns:this.totalTurns,dropped_turns:Math.max(0,this.totalTurns-this.size),stage_semantics:'inclusive_non_additive',stages,turns};
    }
}

// Bounded owner inbox. Async producers only enqueue; they never enter Rust.
export class OwnerInbox {
    constructor(capacity=256, inputLimit=128, controlReserve=Math.min(8,Math.floor(capacity/16)), observe=()=>{},context=()=>({})) {
        this.observe=observe;this.context=context;this.nextRequest=0;
        this.capacity=capacity; this.inputLimit=inputLimit; this.controlReserve=controlReserve;
        this.items=[]; this.batch=[]; this.draining=false; this.slots=new Set();
        this.highWater=0; this.reservedHighWater=0; this.accepted=0; this.completed=0; this.cancelled=0;
    }
    get length(){return this.items.length+this.batch.length;}
    get used(){return this.slots.size+[...this.items,...this.batch].filter(item=>!item.slot).length;}
    get hasInput(){return this.items.some(i=>i.kind==='input');}
    get hasControl(){return this.items.some(i=>i.kind==='control');}
    limit(kind){return kind==='control'?this.capacity:Math.min(this.capacity-this.controlReserve,kind==='input'?this.inputLimit:this.capacity);}
    record(){this.highWater=Math.max(this.highWater,this.used);this.reservedHighWater=Math.max(this.reservedHighWater,this.slots.size);}
    push(run, kind='completion', cancel=()=>{}, group=null) {
        if(this.used>=this.limit(kind))return false;
        this.items.push({run,kind,cancel,group});this.record();return true;
    }
    // Reserve before starting any asynchronous side effect. A queued item and its
    // reservation count once; progress can reuse the slot until the unique terminal.
    reserve(kind='completion',group=null,context={}) {
        if(this.used>=this.limit(kind))return null;
        const owner=this,host_request=++this.nextRequest,identity={...this.context(),...context};
        const emit=stage=>owner.observe(stage,{...identity,host_request,kind,...(Number.isInteger(group)?{request:group}:{})});
        const slot={kind,group,state:'pending',item:null,
            post(run,{terminal=true}={}) {
                if(slot.state!=='pending'){emit('callback_discarded');return Promise.resolve(false);}
                slot.state='queued';
                return new Promise(resolve=>{
                    const item={kind,group,slot,
                        run(){
                            slot.item=null;
                            slot.state='running';
                            try {
                                const value=run(),done=typeof terminal==='function'?terminal(value):terminal;
                                if(slot.state==='running'){
                                    if(done){slot.state='completed';owner.slots.delete(slot);owner.completed++;emit('request_completed');}
                                    else slot.state='pending';
                                }
                            } catch(e){slot.cancel();throw e;} finally {resolve(true);}
                        },
                        cancel(){resolve(false);}
                    };
                    slot.item=item;owner.items.push(item);owner.record();
                });
            },
            cancel(){
                if(!owner.slots.delete(slot))return false;
                slot.state='cancelled';owner.cancelled++;emit('request_cancelled');
                if(slot.item){owner.remove(slot.item);slot.item.cancel();slot.item=null;}
                return true;
            }
        };
        this.slots.add(slot);this.accepted++;this.record();emit('request_reserved');return slot;
    }
    remove(item){for(const q of [this.items,this.batch]){const i=q.indexOf(item);if(i>=0)q.splice(i,1);}}
    drain({limit=16,milliseconds=4,now=()=>performance.now(),controlsOnly=false,canRun=()=>true}={}) {
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
        } finally {this.items.unshift(...batch.splice(0));this.draining=false;}
        return consumed;
    }
    cancelGroup(group){
        for(const slot of [...this.slots])if(slot.group===group)slot.cancel();
        for(const queue of [this.items,this.batch])for(let i=queue.length-1;i>=0;i--)if(queue[i].group===group)queue.splice(i,1)[0].cancel();
    }
    clear(){
        for(const slot of [...this.slots])slot.cancel();
        for(const queue of [this.items,this.batch])for(const item of queue.splice(0))item.cancel();
    }
}

const CONTENT_STAGING_LIMIT = 16 * 1024 * 1024;
const CONTENT_PREFETCH_LIMIT = 2 * 1024 * 1024;

export function contentBatchEnvelope(objects, manifestObjects, {priority='required',maxBytes}={}) {
    const prefetch=priority==='prefetch';
    const limit=prefetch
        ? Math.min(CONTENT_PREFETCH_LIMIT,Number.isSafeInteger(maxBytes)&&maxBytes>=0?maxBytes:CONTENT_PREFETCH_LIMIT)
        : CONTENT_STAGING_LIMIT;
    if(!Array.isArray(objects)||objects.length===0||objects.length>128)
        return {ok:false,code:'E_CONTENT_LIMIT',reason:'object_count',totalBytes:null,maxBytes:limit,prefetch};
    let totalBytes=0;
    for(const object of objects){
        const bytes=manifestObjects?.[object?.hash]?.bytes;
        if(!Number.isSafeInteger(bytes)||bytes<1)
            return {ok:false,code:'E_CONTENT_MANIFEST',reason:'missing_object_size',totalBytes:null,maxBytes:limit,prefetch};
        totalBytes+=bytes;
        if(!Number.isSafeInteger(totalBytes))
            return {ok:false,code:'E_CONTENT_LIMIT',reason:'byte_count_overflow',totalBytes:null,maxBytes:limit,prefetch};
    }
    if(totalBytes>CONTENT_STAGING_LIMIT)
        return {ok:false,code:'E_CONTENT_LIMIT',reason:'staging_envelope',totalBytes,maxBytes:limit,prefetch};
    if(totalBytes>limit)
        return {ok:false,code:prefetch?'E_PREFETCH_LIMIT':'E_CONTENT_LIMIT',reason:prefetch?'available_budget':'batch_limit',totalBytes,maxBytes:limit,prefetch};
    return {ok:true,totalBytes,maxBytes:limit,prefetch};
}

// Accounts for encoded content buffers held across all active batches.
export class ContentStagingBudget {
    constructor(limit=CONTENT_STAGING_LIMIT,onChange=()=>{}){this.limit=limit;this.onChange=onChange;this.used=0;this.peak=0;this.reservations=new Map();this.waiting=[];this.sequence=0;}
    get available(){return Math.max(0,this.limit-this.used);}
    reserveNow(key,bytes){
        if(this.reservations.has(key))return this.reservations.get(key)===bytes;
        if(!Number.isSafeInteger(bytes)||bytes<1||bytes>this.limit||this.used+bytes>this.limit)return false;
        this.reservations.set(key,bytes);this.used+=bytes;this.peak=Math.max(this.peak,this.used);this.onChange(this);return true;
    }
    tryReserve(key,bytes){return this.waiting.length===0&&this.reserveNow(key,bytes);}
    acquire(key,bytes,signal){
        if(signal.aborted)return Promise.reject(signal.reason);
        if(this.reservations.has(key))return Promise.resolve(this.reservations.get(key)===bytes);
        if(!Number.isSafeInteger(bytes)||bytes<1||bytes>this.limit)return Promise.reject(new Error('E_CONTENT_LIMIT'));
        if(this.waiting.length===0&&this.reserveNow(key,bytes))return Promise.resolve(true);
        return new Promise((resolve,reject)=>{
            const item={key,bytes,signal,resolve,reject,sequence:this.sequence++};
            item.abort=()=>{const index=this.waiting.indexOf(item);if(index>=0){this.waiting.splice(index,1);this.onChange(this);reject(signal.reason);}};
            signal.addEventListener('abort',item.abort,{once:true});this.waiting.push(item);this.onChange(this);
        });
    }
    release(key){
        const bytes=this.reservations.get(key);if(bytes===undefined)return false;
        this.reservations.delete(key);this.used-=bytes;this.onChange(this);this.drain();return true;
    }
    drain(){
        this.waiting.sort((a,b)=>a.sequence-b.sequence);
        for(let index=0;index<this.waiting.length;){
            const item=this.waiting[index];
            if(item.signal.aborted){this.waiting.splice(index,1);item.signal.removeEventListener('abort',item.abort);this.onChange(this);item.reject(item.signal.reason);continue;}
            if(this.reserveNow(item.key,item.bytes)){
                this.waiting.splice(index,1);item.signal.removeEventListener('abort',item.abort);this.onChange(this);item.resolve(true);continue;
            }
            index++;
        }
    }
}

export async function acquireRequiredContentStage(job,budget,bytes,preparations=[]) {
    job.signal.throwIfAborted();
    const reserved=budget.tryReserve(job.group,bytes);
    if(!reserved&&Number.isSafeInteger(bytes)&&bytes>0&&bytes<=budget.limit){
        for(const other of preparations){
            if(other!==job&&other.priority==='prefetch'&&other.staged&&other.state==='fetching'&&!other.signal.aborted){
                // Only reclaim for actual pressure. Unlike CancelContent from
                // Rust, this host decision must return a terminal skip to Rust.
                other.stageEviction={code:'E_PREFETCH_LIMIT',reason:'staging_reclaimed',totalBytes:other.totalBytes,maxBytes:budget.available};
                other.controller.abort();
            }
        }
    }
    const admitted=await (reserved?true:budget.acquire(job.group,bytes,job.signal));
    // Take ownership before checking cancellation. acquire() can resolve and
    // then the caller can be cancelled before this continuation runs.
    if(admitted)job.staged=true;
    job.signal.throwIfAborted();
    return admitted;
}

// Eviction has already aborted the download. Even a subsequent promotion must
// receive this skip; Rust can then issue a fresh required request. Cancellation
// by Rust still cancels the reserved slot and suppresses late notifications.
export async function postContentEviction(job,{post,skip,isActive=()=>true}) {
    if(job.cancelled||!isActive()){job.terminal?.cancel();return;}
    await post(job.terminal,()=>{if(!job.cancelled&&isActive())skip(job.stageEviction);});
}

// A prefetch skip that has not reached the owner yet can be superseded by a
// required request for the same batch. Keep the reserved slot pending in that
// case so the promoted job can report its final result without racing a second
// slot reservation.
export async function queueContentSkip(job,envelope,{post,skip,isActive=()=>true}) {
    job.state='skip_queued';job.skipEnvelope=envelope;
    await post(job.terminal,()=>{
        if(job.signal.aborted||job.priority!=='prefetch'||!isActive())return false;
        skip(envelope);
        return true;
    },value=>value===true);
    if(job.signal.aborted||!isActive()||job.priority==='prefetch')return false;
    job.state='admitting';
    return true;
}

const WORK_PRIORITIES={required:0,near:1,speculative:2,prefetch:2,background:3};
export function workPriority(value){return Object.hasOwn(WORK_PRIORITIES,value)?(value==='prefetch'?'speculative':value):'required';}

export function awaitAbortable(promise,signal) {
    if(signal.aborted)return Promise.reject(signal.reason);
    return new Promise((resolve,reject)=>{
        const abort=()=>{signal.removeEventListener('abort',abort);reject(signal.reason);};
        signal.addEventListener('abort',abort,{once:true});
        Promise.resolve(promise).then(value=>{signal.removeEventListener('abort',abort);resolve(value);},error=>{signal.removeEventListener('abort',abort);reject(error);});
    });
}

// Each pool owns one bounded phase. An aborted running task keeps its place
// until the underlying operation settles, including uncancellable decoders.
// Only scheduling estimates are always on: at most eight phases, 32 numbers
// each. Detailed turn and stage traces remain opt-in above.
export class WorkPool {
    constructor(limit=4, capacity=128, now=()=>performance.now()){this.limit=limit;this.capacity=capacity;this.now=now;this.active=0;this.waiting=[];this.sequence=0;this.costs=new Map();}
    run(work,signal,{priority='required',group=null,deadline=null,phase='work'}={}){
        if(signal.aborted)return Promise.reject(signal.reason);
        if(this.waiting.length>=this.capacity)return Promise.reject(new Error('E_RESOURCE_QUEUE'));
        return new Promise((resolve,reject)=>{
            const item={work,signal,resolve,reject,priority:WORK_PRIORITIES[workPriority(priority)],group,sequence:this.sequence++,deadline:Number.isFinite(deadline)?deadline:null,phase};
            item.abort=()=>{const index=this.waiting.indexOf(item);if(index>=0){this.waiting.splice(index,1);reject(signal.reason);}};
            signal.addEventListener('abort',item.abort,{once:true});this.waiting.push(item);this.drain();
        });
    }
    promoteGroup(group,priority='required'){
        let promoted=0;
        const rank=WORK_PRIORITIES[workPriority(priority)];
        for(const item of this.waiting)if(item.group===group&&item.priority>rank){item.priority=rank;promoted++;}
        if(promoted)this.drain();
        return promoted;
    }
    recentP95(phase){
        const samples=this.costs.get(phase);
        if(!samples?.length)return 0;
        const sorted=[...samples].sort((a,b)=>a-b);
        return sorted[Math.ceil(sorted.length*.95)-1];
    }
    recordCost(phase,elapsed){
        if(!Number.isFinite(elapsed))return;
        let samples=this.costs.get(phase);
        if(!samples){if(this.costs.size===8)return;samples=[];this.costs.set(phase,samples);}
        if(samples.length===32)samples.shift();
        samples.push(Math.max(0,elapsed));
    }
    drain(){
        this.waiting.sort((a,b)=>a.priority-b.priority||
            (a.deadline===null?(b.deadline===null?0:1):b.deadline===null?-1:(a.deadline-this.recentP95(a.phase))-(b.deadline-this.recentP95(b.phase)))||a.sequence-b.sequence);
        while(this.active<this.limit&&this.waiting.length){
            const item=this.waiting.shift();item.signal.removeEventListener('abort',item.abort);this.active++;
            const start=this.now();
            Promise.resolve().then(()=>{item.signal.throwIfAborted();return item.work();}).then(item.resolve,item.reject).finally(()=>{this.recordCost(item.phase,this.now()-start);this.active--;this.drain();});
        }
    }
}

// A fetch belongs to its consumers, so cancelling one does not cancel another.
export class SharedRequests {
    constructor(load){this.load=load;this.jobs=new Map();}
    get(key,signal,{settleOnAbort=false,context=null}={}) {
        if(signal.aborted)return Promise.reject(signal.reason);
        let job=this.jobs.get(key);
        if(!job){
            const controller=new AbortController();
            job={controller,consumers:0,context};
            job.promise=Promise.resolve().then(()=>this.load(key,controller.signal,context)).finally(()=>{
                if(this.jobs.get(key)===job)this.jobs.delete(key);
            });
            this.jobs.set(key,job);
        }
        job.consumers++;
        return new Promise((resolve,reject)=>{
            let done=false;
            const finish=(fn,value,cancelled=false)=>{
                if(done)return;done=true;signal.removeEventListener('abort',abort);
                const last=--job.consumers===0;
                if(last){
                    if(this.jobs.get(key)===job)this.jobs.delete(key);
                    job.controller.abort();
                }
                // Content staging must cover even the final consumer's
                // uncancellable hash verification after fetch cancellation.
                if(cancelled&&last&&settleOnAbort){job.promise.then(()=>fn(value),()=>fn(value));return;}
                fn(value);
            };
            const abort=()=>finish(reject,signal.reason,true);
            signal.addEventListener('abort',abort,{once:true});
            job.promise.then(value=>finish(resolve,value),error=>finish(reject,error));
        });
    }
}

// One attempt owns its cancellation signal. Settle every worker before the
// caller can retry or release staging; successful objects survive a retry.
export async function fetchContentBatch(job,{pool,requests,fetched=new Map(),observe=()=>{}}) {
    const controller=new AbortController(),signal=controller.signal;
    const abort=()=>controller.abort(job.signal.reason);
    job.signal.addEventListener('abort',abort,{once:true});
    if(job.signal.aborted)abort();
    const pending=[...new Map(job.objects.map(object=>[object.hash,object])).values()]
        .filter(object=>!fetched.has(object.hash));
    let next=0,failure;
    async function worker(){
        try{
            while(next<pending.length){
                signal.throwIfAborted();
                const object=pending[next++];
                const context={request:job.request,session:job.session,object:object.hash};
                observe('module_requested',{...context,kind:job.priority});
                const data=await pool.run(()=>{
                    observe('module_fetch_started',{...context,kind:job.priority});
                    return requests.get(object.hash,signal,{settleOnAbort:true});
                },signal,{priority:job.priority,group:job.group,phase:'fetch_verify'});
                signal.throwIfAborted();
                fetched.set(object.hash,data);
            }
        }catch(error){
            if(!signal.aborted){failure=error;controller.abort(error);}
            throw error;
        }
    }
    try{
        const settled=await Promise.allSettled(Array.from({length:Math.min(4,pending.length)},worker));
        if(failure!==undefined)throw failure;
        signal.throwIfAborted();
        const rejected=settled.find(result=>result.status==='rejected');
        if(rejected)throw rejected.reason;
        return job.objects.map(object=>fetched.get(object.hash));
    }finally{job.signal.removeEventListener('abort',abort);}
}

export function parseRuntimeProgram(runtimeJson) {
    let runtime;
    try { runtime=typeof runtimeJson==='string'?JSON.parse(runtimeJson):runtimeJson; }
    catch { throw new Error('E_RUNTIME_SCHEMA'); }
    if(!runtime||runtime.format!==2||!runtime.program||typeof runtime.program!=='object')
        throw new Error('E_RUNTIME_VERSION: expected RuntimeExecutable v2');
    return runtime.program;
}

export function initialRuntimePreferences(program,saved,languages,reducedMotion) {
    if(!program||!program.player||!program.locale_config)throw new Error('E_RUNTIME_SCHEMA');
    const defaults={
        ui_locale:program.locale_config.default_ui||program.default_locale,
        text_locale:program.locale_config.default_text||program.default_locale,
        font_scale:program.player.font_scale,
        bgm_volume:program.player.bgm_volume,
        voice_volume:program.player.voice_volume,
        sfx_volume:program.player.sfx_volume,
        reduced_motion:program.player.reduced_motion,
    };
    const selected=initialPreferences(defaults,saved,program.locale_config,languages,reducedMotion);
    const preferences=Object.fromEntries(['ui_locale','text_locale','font_scale','bgm_volume','voice_volume','sfx_volume','reduced_motion']
        .map(key=>[key,selected[key]]));
    for(const key of ['font_scale','bgm_volume','voice_volume','sfx_volume'])
        if(typeof preferences[key]!=='number'||!Number.isFinite(preferences[key]))preferences[key]=defaults[key];
    if(typeof preferences.reduced_motion!=='boolean')preferences.reduced_motion=defaults.reduced_motion;
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

// Platform adapter only. Narrative, reading policy, visual UI and layout live in Rust.
export async function start({wasm,release,releaseDigest,executable,fetchObject,fail,startupTrace=[]}) {
    const params=new URL(location.href).searchParams;
    const trace=new TraceRecorder({enabled:params.get('trace')!=='0'&&(params.has('diagnostics')||params.has('test'))});
    const performanceStats=trace.enabled?new PerformanceRecorder(64):null;
    let traceContext={session:1,device:1};
    const observe=(stage,fields={})=>trace.record(stage,{...traceContext,...fields});
    for(const row of startupTrace)observe(row.stage,row);
    const canvas=document.querySelector('#stage'), shell=document.querySelector('#shell');
    const program=parseRuntimeProgram(executable);
    const metrics={boot:performance.now(),titleMs:null,firstLineMs:null,resourceFailures:0,frames:0,audioStarts:0,deviceRecoveries:0,peakResidentBytes:0,startInputMs:null,firstLineAfterStartMs:null,contentStagingBytes:0,peakContentStagingBytes:0,contentStagingBudgetBytes:CONTENT_STAGING_LIMIT,contentStagingReservations:0,contentStagingWaiters:0};
    const syncContentStagingMetrics=budget=>{
        metrics.contentStagingBytes=budget.used;
        metrics.peakContentStagingBytes=budget.peak;
        metrics.contentStagingBudgetBytes=budget.limit;
        metrics.contentStagingReservations=budget.reservations.size;
        metrics.contentStagingWaiters=budget.waiting.length;
    };
    const size=()=>{const dpr=Math.min(devicePixelRatio||1,2);const width=innerWidth,height=innerHeight;return {width,height,dpr};};
    let {width,height,dpr}=size();canvas.width=Math.round(width*dpr);canvas.height=Math.round(height*dpr);
    const namespace=release.game_id+(location.hostname==='localhost'||location.hostname==='127.0.0.1'?':dev':'');
    const db=await new Promise((resolve,reject)=>{const r=indexedDB.open('nir-player-v1',1);r.onupgradeneeded=()=>{for(const store of ['saves','preferences','profile'])if(!r.result.objectStoreNames.contains(store))r.result.createObjectStore(store);};r.onsuccess=()=>resolve(r.result);r.onerror=()=>reject(r.error);});
    db.onversionchange=()=>db.close();
    const read=(store,key)=>new Promise((resolve,reject)=>{const tx=db.transaction(store,'readonly');const r=tx.objectStore(store).get(key);let value;r.onsuccess=()=>{value=r.result;};tx.oncomplete=()=>resolve(value);tx.onabort=tx.onerror=()=>reject(tx.error||r.error);});
    const write=(store,key,value)=>new Promise((resolve,reject)=>{const tx=db.transaction(store,'readwrite');tx.objectStore(store).put(value,key);tx.oncomplete=resolve;tx.onabort=tx.onerror=()=>reject(tx.error);});
    const savedPreferences=await read('preferences',namespace);
    let preferences=initialRuntimePreferences(program,savedPreferences,navigator.languages||[],matchMedia('(prefers-reduced-motion: reduce)').matches);
    observe('preferences_loaded');
    const createStart=String(Math.round(performance.now()*1000));
    const engine=await wasm.Engine.create(executable,releaseDigest,release.title,'stage',JSON.stringify(preferences));
    engine.set_profiling(trace.enabled);
    preferences=JSON.parse(engine.state()).preferences;
    observe('engine_created',{start_us:createStart,end_us:String(Math.round(performance.now()*1000))});
    document.title=release.title;
    const AudioContext=window.AudioContext||window.webkitAudioContext;
    const audio=new AudioContext();let unlocked=null,audioPaused=true;
    const buffers=new Map(),voices=new Map(),bytesCache=new Map(),assetDescriptors=new Map(),requests=new SharedRequests((id,signal)=>fetchObject(id,signal,observe)),preparations=new Map(),contentPreparations=new Map(),contentStaging=new ContentStagingBudget(CONTENT_STAGING_LIMIT,syncContentStagingMetrics);
    let raf=0,lastTime=null,sequence=0,disposed=false,recovering=false;
    const inbox=new OwnerInbox(256,128,8,observe,()=>traceContext),resourcePool=new WorkPool(4),decodePool=new WorkPool(2),uploadPool=new WorkPool(1);
    const decodeRequests=new SharedRequests((id,signal,source)=>decodePool.run(()=>audio.decodeAudioData(source.bytes.slice(0)),signal,{priority:source.priority,group:source.group,deadline:source.deadline,phase:'audio_decode'}));
    let ownerTimer=null,pendingElapsed=0,wakeRequestedAt=null,pendingWakeWaitStartUs=null,pendingWakeWaitEndUs=null,cachedHostState=null;
    function invalidateHostState(){cachedHostState=null;}
    function mutateEngine(run) {try{return run();}finally{invalidateHostState();}}
    function hostEvent(kind,value) {mutateEngine(()=>engine.host_event(kind,typeof value==='string'?value:JSON.stringify(value)));}
    function reportHostFailure(error,operation) {
        observe('diagnostic',{domain:'host',code:'E_HOST',operation});
        // A thrown WASM call may still own its mutable Engine borrow until this
        // callback returns. Report the failure in a later owner turn.
        queueMicrotask(()=>{if(!disposed)deliver(()=>hostEvent('host_failed',String(error)),'control');});
    }
    function wake() {
        if(disposed||ownerTimer!==null)return;
        if(performanceStats)wakeRequestedAt=performance.now();
        ownerTimer=setTimeout(()=>{
            ownerTimer=null;const now=performance.now();
            if(performanceStats){pendingWakeWaitStartUs=Math.round(wakeRequestedAt*1000);pendingWakeWaitEndUs=Math.round(now*1000);wakeRequestedAt=null;}
            frame(now);
        },0);
    }
    function deliver(fn,kind='completion',group=null) {
        if(disposed)return Promise.resolve(false);
        return new Promise(resolve=>{
            if(!inbox.push(()=>{try{fn();resolve(true);}catch(e){reportHostFailure(e,'dispatch');resolve(false);}},kind,()=>resolve(false),group)){
                fail('E_EVENT_QUEUE: host inbox admission limit');resolve(false);dispose();return;
            }
            wake();
        });
    }
    function post(slot,fn,terminal=true){
        if(disposed){slot.cancel();return Promise.resolve(false);}
        const done=slot.post(()=>{try{return fn();}catch(e){if(slot.kind==='control')throw e;reportHostFailure(e,'completion');return true;}},{terminal});wake();return done;
    }
    function request(work,success,failure,{kind='completion',group=null,replace=false}={}){
        if(replace)inbox.cancelGroup(group);
        const slot=inbox.reserve(kind,group);
        if(!slot){failure(new Error('E_REQUEST_CAPACITY: no terminal slot'));return null;}
        Promise.resolve().then(()=>{
            if(slot.state==='cancelled')throw new Error('E_REQUEST_CANCELLED');
            return work();
        }).then(value=>post(slot,()=>success(value)),error=>post(slot,()=>failure(error)));
        return slot;
    }
    function unlock() {if(audio.state!=='running'){unlocked=audio.resume();unlocked.catch(e=>console.warn('Audio unlock failed',e));}else{unlocked=Promise.resolve();}}
    function stopVoice(id) {
        const v=voices.get(id);if(!v)return;v.stopped=true;v.slot.cancel();
        try{v.source?.stop();}catch{}v.source?.disconnect();v.gain?.disconnect();voices.delete(id);
    }
    function playVoice(c) {
        stopVoice(c.task);
        const failed=e=>mutateEngine(()=>engine.audio_failed(c.task,c.session,String(e)));
        const slot=inbox.reserve('completion',`audio:${c.session}:${c.task}`,{session:c.session,task:c.task});
        if(!slot){failed('E_REQUEST_CAPACITY');return;}
        const buffer=buffers.get(c.asset);
        if(!buffer){post(slot,()=>failed('E_AUDIO_BUFFER'));return;}
        let v;
        try {
            const source=audio.createBufferSource(),gain=audio.createGain();source.buffer=buffer;source.loop=c.looped;
            gain.gain.value=preferences[`${c.bus}_volume`]??.5;source.connect(gain).connect(audio.destination);
            v={source,gain,slot,bus:c.bus,asset:c.asset,stopped:false};voices.set(c.task,v);
            let offset=Number(c.position_us)/1e6;if(c.looped)offset%=buffer.duration;else offset=Math.min(offset,Math.max(0,buffer.duration-.001));
            source.onended=()=>{if(!v.stopped&&!c.looped)post(slot,()=>{
                if(voices.get(c.task)===v){voices.delete(c.task);source.disconnect();gain.disconnect();}
                mutateEngine(()=>engine.audio_ended(c.task,c.session));
            });};
            source.start(0,offset);metrics.audioStarts++;
        } catch(e){
            if(v){v.stopped=true;v.source.disconnect();v.gain.disconnect();voices.delete(c.task);}
            post(slot,()=>failed(e));
        }
    }
    async function asset(id,a,signal,context) {
        if(!a)throw new Error(`E_ASSET_DESCRIPTOR: ${id}`);
        signal.throwIfAborted();
        if(bytesCache.has(a.object)){
            const bytes=bytesCache.get(a.object);
            if(bytes.byteLength!==a.bytes)throw Object.assign(new Error(`E_ASSET_SIZE: ${id}`),{code:'E_ASSET_SIZE'});
            observe('bytes_cache_hit',context);return bytes;
        }
        observe('fetch_started',context);
        const bytes=await requests.get(a.object,signal,{settleOnAbort:true});signal.throwIfAborted();
        if(bytes.byteLength!==a.bytes)throw Object.assign(new Error(`E_ASSET_SIZE: ${id}`),{code:'E_ASSET_SIZE'});
        observe('fetch_verified',{...context,bytes:bytes.byteLength});
        bytesCache.set(a.object,bytes);return bytes;
    }
    function cancelPreparation(request) {
        inbox.cancelGroup(request);
        const job=preparations.get(request);
        const acknowledge=()=>deliver(()=>{prune(true);hostEvent('assets_cancelled',{request});},'control');
        if(job){
            if(job.cancelAckQueued)return;
            job.cancelAckQueued=true;job.controller.abort();
            // The final consumer of an uncancellable decoder waits for its
            // actual settlement before Rust may release the retired budget.
            job.done.then(acknowledge);
        }else acknowledge();
    }
    function promotePreparation(request,session) {
        const job=preparations.get(request);
        if(!job||job.session!==session||job.signal.aborted)return false;
        job.priority='required';
        resourcePool.promoteGroup(request);decodePool.promoteGroup(request);uploadPool.promoteGroup(request);
        return true;
    }
    function prune(force=false) {
        if(disposed||(recovering&&!force))return;
        const retained=JSON.parse(engine.retained_descriptors());
        for(const [id,descriptor] of Object.entries(retained))assetDescriptors.set(id,descriptor);
        const keep=new Set(Object.keys(retained));for(const v of voices.values())keep.add(v.asset);
        const objects=new Set([...keep].map(id=>assetDescriptors.get(id)?.object).filter(Boolean));
        for(const id of buffers.keys())if(!keep.has(id))buffers.delete(id);
        for(const id of assetDescriptors.keys())if(!keep.has(id))assetDescriptors.delete(id);
        for(const id of bytesCache.keys())if(!objects.has(id))bytesCache.delete(id);
    }
    async function prepare(c) {
        let descriptors;
        try { descriptors=validateAssetRequest(c.assets,c.descriptors); }
        catch(error) { mutateEngine(()=>engine.resource_failed(c.request,String(error)));return; }
        for(const [id,descriptor] of Object.entries(descriptors))assetDescriptors.set(id,descriptor);
        const terminal=inbox.reserve('completion',c.request,{session:c.session,device:c.device});
        if(!terminal){mutateEngine(()=>engine.resource_failed(c.request,'E_REQUEST_CAPACITY'));return;}
        const controller=new AbortController(),signal=controller.signal;
        let resolveDone;
        const done=new Promise(resolve=>resolveDone=resolve);
        const job={controller,signal,session:c.session,priority:workPriority(c.priority),deadline:Number.isFinite(c.deadline_ms)?c.deadline_ms:null,nodes:new Map(),done,resolveDone,cancelAckQueued:false};
        preparations.set(c.request,job);
        const failed=(message,id='',stage='admission',code='E_PREPARE')=>{
            observe('resource_failed',{request:c.request,session:c.session,device:c.device,asset:id,code,operation:stage,domain:'prepare'});
            mutateEngine(()=>engine.resource_fault(c.request,id,code,stage,String(message)));controller.abort();
        };
        let next=0;
        async function worker(){while(next<c.assets.length&&!disposed&&!signal.aborted){
            const id=c.assets[next++],slot=inbox.reserve('resource',c.request,{session:c.session,device:c.device});
            if(!slot){await post(terminal,()=>failed('E_REQUEST_CAPACITY'));return;}
            const descriptor=descriptors[id];
            // The awaited stages are the node's dependencies: verified bytes
            // precede audio decode, then owner upload, then readiness.
            const node={asset:id,stage:'fetch_verify'};job.nodes.set(id,node);
            let stage='fetch';const context={request:c.request,session:c.session,device:c.device,asset:id,object:descriptor?.object};
            observe('resource_queued',context);
            try {
                const bytes=await resourcePool.run(async()=>{
                    observe('resource_admitted',context);
                    return asset(id,descriptor,signal,context);
                },signal,{priority:job.priority,group:c.request,deadline:job.deadline,phase:'fetch_verify'});
                signal.throwIfAborted();
                if(descriptor.kind==='audio'){
                    node.stage=stage='audio_decode';observe('audio_decode_started',context);
                    if(!buffers.has(id)){
                        const shared=decodeRequests.jobs.get(id);
                        if(shared)decodePool.promoteGroup(shared.context.group,job.priority);
                        const buffer=await decodeRequests.get(id,signal,{settleOnAbort:true,context:{bytes,priority:job.priority,group:c.request,deadline:job.deadline}});
                        signal.throwIfAborted();buffers.set(id,buffer);
                    }
                    observe('audio_decode_ready',context);
                    // Decoding is valid while the context is suspended. Only
                    // required playback needs an unlocked context, and a
                    // cancelled request must not wait for resume to settle.
                    if(job.priority==='required'&&unlocked&&!audioPaused)await awaitAbortable(unlocked,signal);
                    signal.throwIfAborted();
                    if(job.priority==='required'&&audio.state!=='running'&&!audioPaused)throw new Error('E_AUDIO_LOCKED: activate sound with a user gesture');
                }
                node.stage=stage='decode_upload';
                let complete=false;
                while(!complete&&!signal.aborted&&!disposed&&slot.state==='pending'){
                    // Re-admit each bounded owner step. A synchronous PNG
                    // decode remains atomic inside its one owner callback.
                    await uploadPool.run(()=>post(slot,()=>{
                        if(signal.aborted)return true;
                        try {complete=mutateEngine(()=>engine.resource(c.request,id,new Uint8Array(bytes)));if(complete){node.stage='ready';observe('ordered_use_ready',context);}return complete;}
                        catch(e){metrics.resourceFailures++;failed(e,id,stage,'E_RESOURCE_DECODE_UPLOAD');return true;}
                    },done=>done),signal,{priority:job.priority,group:c.request,deadline:job.deadline,phase:'decode_upload'});
                }
            }catch(e){
                node.stage='failed';
                if(!signal.aborted&&!disposed){metrics.resourceFailures++;await post(slot,()=>failed(e,id,stage,typeof e?.code==='string'?e.code:stage==='fetch'?'E_RESOURCE_FETCH':'E_AUDIO_DECODE'));}
            } finally {if(signal.aborted||disposed)slot.cancel();}
        }}
        try {
            await Promise.all(Array.from({length:Math.min(4,c.assets.length)},worker));
            if(!signal.aborted&&!disposed)await post(terminal,()=>{});
            else terminal.cancel();
        } finally {if(preparations.get(c.request)===job)preparations.delete(c.request);job.resolveDone();}
    }
    function cancelContent(request) {
        const job=contentPreparations.get(request);
        inbox.cancelGroup(`content:${request}`);
        job?.controller.abort();
        if(job)job.cancelled=true;
    }
    function contentDetail(envelope) {
        return JSON.stringify({reason:envelope.reason,total_bytes:envelope.totalBytes,max_bytes:envelope.maxBytes});
    }
    function postContentSkip(job,envelope) {
        return queueContentSkip(job,envelope,{
            post,
            isActive:()=>!disposed,
            skip:skipped=>skipContent(job,skipped),
        });
    }
    function skipContent(job,skipped) {
        observe('module_skipped',{request:job.request,session:job.session,code:skipped.code,bytes:skipped.totalBytes??0});
        mutateEngine(()=>engine.content_skipped(job.request,skipped.code,contentDetail(skipped)));
    }
    function promoteContent(request,session) {
        const job=contentPreparations.get(request);
        if(!job||job.signal.aborted||job.session!==session)return false;
        job.priority='required';job.maxBytes=null;
        resourcePool.promoteGroup(job.group);
        // A queued skip checks this priority when its owner callback runs. It
        // then leaves the reservation pending for this request's result.
        return true;
    }
    async function admitContentStage(job,totalBytes) {
        if(job.priority==='prefetch')return contentStaging.tryReserve(job.group,totalBytes);
        return acquireRequiredContentStage(job,contentStaging,totalBytes,contentPreparations.values());
    }
    async function runContent(job) {
        const {request,session,group,signal}=job;
        const bytes=[],fetched=new Map();job.bytes=bytes;
        try {
            let envelope=contentBatchEnvelope(job.objects,release.objects,{priority:job.priority,maxBytes:job.maxBytes});
            if(!envelope.ok){
                if(job.priority==='prefetch'){
                    if(!await postContentSkip(job,envelope))return;
                    envelope=contentBatchEnvelope(job.objects,release.objects,{priority:'required'});
                    if(!envelope.ok)throw new Error(envelope.code);
                }else throw new Error(envelope.code);
            }
            job.totalBytes=envelope.totalBytes;
            let staged=await admitContentStage(job,envelope.totalBytes);
            if(!staged&&job.priority==='prefetch'){
                const skipped={ok:false,code:'E_PREFETCH_LIMIT',reason:'staging_capacity',totalBytes:envelope.totalBytes,maxBytes:contentStaging.available,prefetch:true};
                if(!await postContentSkip(job,skipped))return;
                staged=await admitContentStage(job,envelope.totalBytes);
            }
            // Promotion can race the synchronous prefetch admission result.
            // Re-enter the required waiter path instead of treating that stale
            // speculative denial as a terminal capacity error.
            if(!staged&&job.priority==='required')staged=await admitContentStage(job,envelope.totalBytes);
            if(!staged)throw new Error('E_CONTENT_STAGE_LIMIT');
            job.staged=true;signal.throwIfAborted();
            for(;;){
                bytes.length=0;
                try{
                    job.state='fetching';
                    bytes.push(...await fetchContentBatch(job,{pool:resourcePool,requests,fetched,observe}));
                    signal.throwIfAborted();
                    job.state='delivering';
                    await post(job.terminal,()=>{
                        if(!signal.aborted&&engine.accepts_content(request)){
                            mutateEngine(()=>engine.content_ready(request,bytes.map(b=>new Uint8Array(b))));
                            observe('module_delivered',{request,session,kind:job.priority,bytes:job.totalBytes});
                        }
                    });
                    break;
                }catch(error){
                    if(signal.aborted||disposed||job.priority!=='prefetch')throw error;
                    const skipped={ok:false,code:'E_PREFETCH_FAILED',reason:'fetch_failed',totalBytes:job.totalBytes,maxBytes:job.maxBytes,prefetch:true};
                    if(!await postContentSkip(job,skipped))return;
                    // A same-batch demand may promote while the skip completion
                    // is queued. Retry the failed object under the same request.
                    if(job.priority!=='required')return;
                }
            }
        }catch(error){
            if(job.stageEviction)await postContentEviction(job,{post,isActive:()=>!disposed,skip:skipped=>skipContent(job,skipped)});
            else if(!signal.aborted&&!disposed)await post(job.terminal,()=>{
                observe('module_failed',{request,session,code:'E_MODULE_PREPARE'});
                mutateEngine(()=>engine.content_failed(request,String(error)));
            });
            else job.terminal?.cancel();
        }finally{
            bytes.length=0;fetched.clear();job.bytes=null;
            if(job.staged)contentStaging.release(group);
            if(contentPreparations.get(request)===job)contentPreparations.delete(request);
        }
    }
    function prepareContent(c) {
        const group=`content:${c.request}`;
        const terminal=inbox.reserve('completion',group,{session:c.session});
        if(!terminal){mutateEngine(()=>engine.content_failed(c.request,'E_REQUEST_CAPACITY'));return;}
        const controller=new AbortController();
        const priority=c.priority==='prefetch'?'prefetch':'required';
        const job={request:c.request,session:c.session,objects:c.objects,group,priority,maxBytes:priority==='prefetch'?c.max_bytes:null,controller,signal:controller.signal,terminal,state:'starting',staged:false,cancelled:false};
        contentPreparations.set(c.request,job);
        void runContent(job);
    }
    function listSaves() {
        return request(async()=>{
            const rows=[];for(let slot=0;slot<3;slot++){const s=await read('saves',`${namespace}:${slot}`);if(s)rows.push({slot,revision:s.revision,label:s.label||`#${s.revision}`});}return rows;
        },rows=>hostEvent('slots',rows),e=>hostEvent('load_failed',String(e)),{group:'slots',replace:true});
    }
    function save(c) {
        const context={request:c.job,session:state().session,operation:'save'};observe('storage_started',context);
        return request(()=>new Promise((resolve,reject)=>{
            const tx=db.transaction('saves','readwrite'),store=tx.objectStore('saves'),r=store.get(`${namespace}:${c.slot}`);let conflict=false,writeError=null;
            r.onsuccess=()=>{try{const current=r.result;if((current?.revision||0)!==c.expected_revision){conflict=true;tx.abort();return;}const record={...c.envelope,label:new Date().toLocaleString(),saved_at:Date.now()};store.put(record,`${namespace}:${c.slot}`);}catch(e){writeError=e;tx.abort();}};
            tx.oncomplete=resolve;tx.onabort=()=>reject(writeError||new Error(conflict?'E_SAVE_CONFLICT: another tab changed this slot. Reopen the save menu.':`E_STORAGE: ${tx.error}`));tx.onerror=()=>{};
        }),()=>{observe('storage_committed',context);hostEvent('saved',{job:c.job,slot:c.slot,revision:c.envelope.revision});},
            e=>{const code=e?.name==='QuotaExceededError'?'E_STORAGE_QUOTA':String(e).includes('E_SAVE_CONFLICT')?'E_SAVE_CONFLICT':'E_STORAGE';observe('diagnostic',{...context,domain:'storage',code});hostEvent('save_failed',{job:c.job,code,message:String(e)});},{group:`save:${c.job}`});
    }
    const envelope=(record)=>{const {label,saved_at,...e}=record;return e;};
    function load(slot) {
        const session=state().session;
        return request(async()=>{const s=await read('saves',`${namespace}:${slot}`);if(!s)throw new Error('E_SAVE_MISSING');return envelope(s);},
            value=>{if(state().session===session)hostEvent('loaded',value);},
            e=>{if(state().session===session)hostEvent('load_failed',String(e));},{group:'load',replace:true});
    }
    function importSave(){
        inbox.cancelGroup('load');const slot=inbox.reserve('completion','load'),session=state().session;
        if(!slot){hostEvent('load_failed','E_REQUEST_CAPACITY');return;}
        const input=document.createElement('input');input.type='file';input.accept='.json,application/json';
        input.oncancel=()=>post(slot,()=>{});
        input.onchange=async()=>{
            if(slot.state==='cancelled')return;
            try{
                const f=input.files[0];if(!f){post(slot,()=>{});return;}
                if(f.size>16*1024*1024)throw new Error('E_SAVE_LIMIT');
                const data=await f.text();post(slot,()=>{if(state().session===session)hostEvent('loaded',data);});
            }catch(e){post(slot,()=>{if(state().session===session)hostEvent('load_failed',String(e));});}
        };
        try{input.click();}catch(e){post(slot,()=>hostEvent('load_failed',String(e)));}
    }
    async function mergeProfile(keys) {await new Promise((resolve,reject)=>{const tx=db.transaction('profile','readwrite'),store=tx.objectStore('profile'),r=store.get(namespace);r.onsuccess=()=>store.put([...new Set([...(r.result||[]),...keys])].sort(),namespace);tx.oncomplete=resolve;tx.onabort=tx.onerror=()=>reject(tx.error);});}
    function flush() {if(disposed)return;
        if(trace.enabled){const current=state();traceContext={session:current.session,device:current.device,locale:current.locale};}
        for(const c of JSON.parse(engine.commands())){
        switch(c.type){
            case 'observation':observe(c.stage,c);break;
            case 'resource_stage':observe(c.stage,c);break;
            case 'diagnostic':{const d=c.diagnostic;observe('diagnostic',{code:d.code,location:d.location,...d.details,asset:d.details?.references?.[0]});break;}
            case 'get_content':prepareContent(c);break;
            case 'cancel_content':cancelContent(c.request);break;
            case 'promote_content':promoteContent(c.request,c.session);break;
            case 'get_assets':prepare(c);break;
            case 'promote_assets':promotePreparation(c.request,c.session);break;
            case 'cancel_assets':cancelPreparation(c.request);break;
            case 'audio_start':playVoice(c);break;
            case 'audio_stop':stopVoice(c.task);break;
            case 'audio_reset':for(const id of [...voices.keys()])stopVoice(id);break;
            case 'audio_pause':audioPaused=c.paused;if(c.paused){audio.suspend().catch(()=>{});}else if(unlocked){audio.resume().catch(e=>console.warn(e));}break;
            case 'save':save(c);break;case 'load':load(c.slot);break;case 'list_saves':listSaves();break;
            case 'apply_preferences':preferences=c.preferences;for(const v of voices.values())v.gain.gain.value=preferences[`${v.bus}_volume`];break;
            case 'persist_preferences':preferences=c.preferences;for(const v of voices.values())v.gain.gain.value=preferences[`${v.bus}_volume`]??.5;{const value=preferences;request(()=>write('preferences',namespace,value),()=>{},e=>hostEvent('load_failed',`E_PREFERENCES: ${e}`));}break;
            case 'persist_profile':request(()=>mergeProfile(c.keys),()=>{},e=>hostEvent('load_failed',`E_PROFILE: ${e}`));break;
            case 'export':{const url=URL.createObjectURL(new Blob([c.json],{type:'application/json'}));const a=document.createElement('a');a.href=url;a.download=`${release.game_id}.nir-save.json`;a.click();setTimeout(()=>URL.revokeObjectURL(url),1000);break;}
            case 'import':importSave();break;
            case 'trace':if(testMode){traces.push({event:c.event,at:c.at});if(traces.length>4096)traces.splice(0,traces.length-4096);}break;
            default:throw new Error(`E_HOST_PROTOCOL: ${c.type}`);
        }
    }}
    function state(){return cachedHostState||(cachedHostState=JSON.parse(engine.host_state()));}
    function debugState(){return JSON.parse(engine.state());}
    function finishPerformanceTurn(turn) {
        if(!turn)return;
        try {
            if(!disposed){
                const rows=JSON.parse(engine.take_profile());
                for(const row of rows)performanceStats.record(row.stage,Number(row.start_us),Number(row.end_us),turn);
            }
        } finally {performanceStats.endTurn(turn,Math.round(performance.now()*1000));}
    }
    function action(a,context=state()) {
        if(disposed||recovering)return Promise.resolve(false);
        observe('input_received',{sequence:sequence+1,session:context.session});
        unlock();if(a.type==='new_game'&&metrics.startInputMs===null)metrics.startInputMs=performance.now();
        sequence=Math.max(sequence+1,state().sequence+1);const seq=sequence;
        return deliver(()=>{if(a.type==='title'||a.type==='new_game')inbox.cancelGroup('load');mutateEngine(()=>engine.action(JSON.stringify(a),context.interaction,seq,context.session));},'input');
    }
    let semanticSignature='',announcement='',announcementLocale='';
    function semantics(view) {
        document.documentElement.lang=view.locale||'zh-Hans';const s=state();const signature=JSON.stringify([view.nodes,view.locale,view.announcement_locale,s.interaction,s.session]);
        if(signature!==semanticSignature){semanticSignature=signature;const nav=document.querySelector('#actions'),focused=document.activeElement?.dataset?.action;nav.replaceChildren();for(const n of view.nodes){const b=document.createElement('button');b.textContent=n.label;b.lang=n.locale||view.locale||'zh-Hans';b.disabled=!n.enabled;b.dataset.action=JSON.stringify(n.action);const context={interaction:s.interaction,session:s.session};b.onclick=()=>action(n.action,context);b.onfocus=()=>{const ring=document.querySelector('#focus-ring');Object.assign(ring.style,{display:'block',left:`${n.rect[0]}px`,top:`${n.rect[1]}px`,width:`${n.rect[2]}px`,height:`${n.rect[3]}px`});};b.onblur=()=>document.querySelector('#focus-ring').style.display='none';nav.append(b);if(b.dataset.action===focused)b.focus({preventScroll:true});}}
        const spokenLocale=view.announcement_locale||view.locale||'zh-Hans';if(view.announcement&&(view.announcement!==announcement||spokenLocale!==announcementLocale)){announcement=view.announcement;announcementLocale=spokenLocale;const live=document.querySelector('#announcement');live.lang=spokenLocale;live.textContent=announcement;}
    }
    function frame(now) {
        if(disposed)return;
        const perfTurn=performanceStats?performanceStats.beginTurn(Math.round(now*1000)):null;
        if(perfTurn&&pendingWakeWaitStartUs!==null){performanceStats.record('wake_wait',pendingWakeWaitStartUs,pendingWakeWaitEndUs,perfTurn);pendingWakeWaitStartUs=pendingWakeWaitEndUs=null;}
        const eventStartUs=perfTurn?Math.round(performance.now()*1000):0;
        try {
            mutateEngine(()=>engine.begin_turn());
            checkDevice();if(disposed){finishPerformanceTurn(perfTurn);return;}
            const before=state();traceContext={session:before.session,device:before.device};
            const elapsed=lastTime===null?0:Math.min(250000,Math.max(0,Math.round((now-lastTime)*1000)));lastTime=now;
            inbox.drain({canRun:kind=>!disposed&&(!recovering||kind==='control')&&(kind==='control'||engine.pending_events()<112)});if(disposed){finishPerformanceTurn(perfTurn);return;}flush();
            if(recovering){if(inbox.hasControl)wake();if(perfTurn)performanceStats.record('event_handling',eventStartUs,Math.round(performance.now()*1000),perfTurn);finishPerformanceTurn(perfTurn);return;}
            const after=state();
            if(!document.hidden&&!before.paused&&before.screen==='Story'&&!after.paused&&before.session===after.session){
                pendingElapsed=Math.min(250000,pendingElapsed+elapsed);
                if(!inbox.hasInput){mutateEngine(()=>engine.tick(pendingElapsed));pendingElapsed=0;}
            }else{pendingElapsed=0;}
            mutateEngine(()=>engine.continue_turn());flush();
            if(perfTurn)performanceStats.record('event_handling',eventStartUs,Math.round(performance.now()*1000),perfTurn);
            const current=size();if(current.width!==width||current.height!==height||current.dpr!==dpr){({width,height,dpr}=current);canvas.width=Math.round(width*dpr);canvas.height=Math.round(height*dpr);}
            const submitBefore=state().frames;
            const view=JSON.parse(mutateEngine(()=>engine.draw(width,height,dpr)));flush();prune();
            const semanticStartUs=perfTurn?Math.round(performance.now()*1000):0;semantics(view);
            if(perfTurn)performanceStats.record('semantics',semanticStartUs,Math.round(performance.now()*1000),perfTurn);
            const s=state();
            if(s.frames!==submitBefore)observe('render_submitted',{session:s.session,device:s.device,frames:s.frames});
            metrics.frames=s.frames;metrics.peakResidentBytes=Math.max(metrics.peakResidentBytes,s.resident_bytes);syncContentStagingMetrics(contentStaging);
            metrics.maxTurnUploadBytes=Math.max(metrics.maxTurnUploadBytes||0,s.turn_upload_bytes);metrics.uploadSteps=s.upload_steps;
            metrics.activeRequests=inbox.slots.size;metrics.requestHighWater=inbox.reservedHighWater;metrics.acceptedRequests=inbox.accepted;metrics.completedRequests=inbox.completed;metrics.cancelledRequests=inbox.cancelled;
            metrics.inboxHighWater=inbox.highWater;metrics.maxTurnWork=Math.max(metrics.maxTurnWork||0,s.turn_work);
            if(view.ready){shell.hidden=true;if(metrics.titleMs===null)metrics.titleMs=performance.now()-metrics.boot;if(s.has_dialogue&&metrics.firstLineMs===null){metrics.firstLineMs=performance.now()-metrics.boot;metrics.firstLineAfterStartMs=performance.now()-metrics.startInputMs;metrics.navigationToFirstLineMs=performance.now();observe('first_line_submitted');}}
            if(inbox.length||engine.pending_events())wake();
            else if(engine.needs_clock()&&!document.hidden)schedule();
            finishPerformanceTurn(perfTurn);
        }catch(e){
            try{finishPerformanceTurn(perfTurn);}catch{}
            fail(e);dispose();
        }
    }
    function schedule() {if(disposed||recovering)return;if(!raf)raf=requestAnimationFrame(()=>{raf=0;wake();});}
    let down=null;
    const scrollAt=(x,y)=>state().scrolls.find(v=>x>=v.rect[0]&&x<=v.rect[0]+v.rect[2]&&y>=v.rect[1]&&y<=v.rect[1]+v.rect[3]);
    const onDown=(e)=>{unlock();down={action:JSON.parse(engine.hit(e.clientX,e.clientY)),context:state(),x:e.clientX,y:e.clientY,scroll:scrollAt(e.clientX,e.clientY)};};
    const onUp=(e)=>{
        if(!down)return;
        const dy=e.clientY-down.y;
        if(down.scroll&&Math.abs(dy)>30&&Math.abs(e.clientX-down.x)<80){action({type:'scroll',region:down.scroll.region,delta:dy<0?1:-1},down.context);}
        else if(Math.hypot(e.clientX-down.x,dy)<20){const hit=JSON.parse(engine.hit(e.clientX,e.clientY));if(down.action&&JSON.stringify(down.action)===JSON.stringify(hit))action(down.action,down.context);}
        down=null;
    };
    const onWheel=(e)=>{const view=scrollAt(e.clientX,e.clientY);if(view&&e.deltaY){e.preventDefault();action({type:'scroll',region:view.region,delta:e.deltaY>0?1:-1});}};
    const onKey=(e)=>{
        if(e.isComposing||e.repeat||e.ctrlKey||e.metaKey||e.altKey)return;
        if(e.key==='PageUp'||e.key==='PageDown'){
            const s=state(),view=s.scrolls.find(v=>v.region==='choices')||s.scrolls[0];
            if(view){e.preventDefault();action({type:'scroll',region:view.region,delta:e.key==='PageDown'?1:-1},s);}return;
        }
        if(e.key==='Escape'){e.preventDefault();const s=state();action({type:['Menu','Settings','Saves','History'].includes(s.screen)?'close':'menu'});return;}
        if(document.activeElement?.tagName==='BUTTON')return;
        if(e.key===' '||e.key==='Enter'){e.preventDefault();const s=state();action({type:s.screen==='Title'?'new_game':s.paused?'continue':'advance'});}
        else if(e.key==='ArrowDown'||e.key==='ArrowUp'){e.preventDefault();document.querySelector('#actions button:not([disabled])')?.focus();}
    };
    const onVisibility=()=>{const hidden=document.hidden;deliver(()=>mutateEngine(()=>engine.hidden(hidden)),'control');};
    const onResize=()=>schedule();
    canvas.addEventListener('wheel',onWheel,{passive:false});canvas.addEventListener('pointerdown',onDown);canvas.addEventListener('pointerup',onUp);canvas.addEventListener('pointercancel',()=>down=null);window.addEventListener('keydown',onKey);document.addEventListener('visibilitychange',onVisibility);window.addEventListener('resize',onResize);
    function checkDevice(){
        if(disposed||recovering)return;
        const validation=engine.gpu_error();if(validation){observe('diagnostic',{domain:'render',code:'E_GPU_VALIDATION',operation:'render'});fail(`E_GPU_VALIDATION: ${validation}`);dispose();return;}
        if(!engine.device_lost())return;
        recovering=true;observe('device_loss_detected');metrics.deviceRecoveries++;mutateEngine(()=>engine.begin_recovery());
        const recovery=inbox.reserve('control','device');
        if(!recovery){fail('E_REQUEST_CAPACITY: device recovery');dispose();return;}
        wasm.create_gpu('stage').then(gpu=>{
            if(disposed||recovery.state==='cancelled'){gpu.free();return;}
            post(recovery,()=>{mutateEngine(()=>engine.replace_gpu(gpu));recovering=false;lastTime=performance.now();});
        },e=>post(recovery,()=>{fail(`E_DEVICE_RECOVERY: ${e}`);dispose();}));
    }
    const poll=setInterval(()=>{if(!disposed&&!recovering)deliver(checkDevice,'control');},500);
    const testMode=new URL(location.href).searchParams.has('test'),traces=[];
    const disabledPerformance={enabled:false,turn_capacity:64,total_turns:0,dropped_turns:0,stage_semantics:'inclusive_non_additive',stages:{},turns:[]};
    const diagnostics=()=>{
        const performance=performanceStats?performanceStats.snapshot():{...disabledPerformance};
        if(performanceStats&&!disposed)performance.text_cache=JSON.parse(engine.text_cache_stats());
        return {format:1,release:releaseDigest,engine:release.engine.wasm,...trace.snapshot(),performance,content_staging:{encoded_bytes:contentStaging.used,peak_encoded_bytes:contentStaging.peak,budget_encoded_bytes:contentStaging.limit,reservations:contentStaging.reservations.size,waiting_demands:contentStaging.waiting.length},host_work:{resource_pool_active:resourcePool.active,resource_pool_waiting:resourcePool.waiting.length,decode_pool_active:decodePool.active,decode_pool_waiting:decodePool.waiting.length,upload_pool_active:uploadPool.active,upload_pool_waiting:uploadPool.waiting.length,shared_fetches:requests.jobs.size,content_jobs:contentPreparations.size,media_jobs:preparations.size,request_slots:inbox.slots.size,pending_owner_callbacks:inbox.length,audio_state:audio.state,audio_paused:audioPaused,pending_media:[...preparations].slice(0,128).map(([request,job])=>({request,session:job.session,priority:job.priority,aborted:job.signal.aborted,stages:[...job.nodes.values()].slice(0,128).map(node=>({asset:node.asset,stage:node.stage}))})),pending_content:[...contentPreparations.values()].slice(0,128).map(job=>({request:job.request,session:job.session,priority:job.priority,state:job.state,staged:job.staged,aborted:job.signal.aborted}))},measurement:{clock:'performance.now; navigation origin',stage_timing:'inclusive, non-additive intervals',gpu_time:'unmeasured',physical_memory:'unmeasured'}};
    };
    if(trace.enabled)window.nirDiagnostics={snapshot:diagnostics,download(){const url=URL.createObjectURL(new Blob([JSON.stringify(diagnostics(),null,2)],{type:'application/json'}));const a=document.createElement('a');a.href=url;a.download='nir-diagnostics.json';a.click();setTimeout(()=>URL.revokeObjectURL(url),1000);}};
    if(testMode)window.__nir={state:debugState,action,metrics,traces,diagnostics,needsClock:()=>engine.needs_clock(),rawAction:(a,token,seq,epoch)=>deliver(()=>mutateEngine(()=>engine.action(JSON.stringify(a),token,seq,epoch)),'input'),loseDevice:()=>engine.simulate_device_loss(),hidden:(v)=>deliver(()=>mutateEngine(()=>engine.hidden(v)))};
    request(()=>read('profile',namespace),profile=>{if(profile)hostEvent('profile',profile);},e=>hostEvent('load_failed',`E_STORAGE_BOOT: ${e}`),{group:'boot'});
    function dispose(){if(disposed)return;disposed=true;clearTimeout(ownerTimer);inbox.clear();for(const request of [...contentPreparations.keys()])cancelContent(request);for(const request of [...preparations.keys()])cancelPreparation(request);cancelAnimationFrame(raf);clearInterval(poll);for(const id of [...voices.keys()])stopVoice(id);audio.close();db.close();canvas.removeEventListener('wheel',onWheel);canvas.removeEventListener('pointerdown',onDown);canvas.removeEventListener('pointerup',onUp);window.removeEventListener('keydown',onKey);window.removeEventListener('resize',onResize);document.removeEventListener('visibilitychange',onVisibility);engine.free();}
    window.addEventListener('pagehide',e=>{if(e.persisted){deliver(()=>mutateEngine(()=>engine.hidden(true)));}else{dispose();}});
    window.addEventListener('pageshow',e=>{if(e.persisted){deliver(()=>mutateEngine(()=>engine.hidden(false)));}});
}

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
                if(base==='zh'&&parsed.script?.toLowerCase()==='hans'&&supports(supported,'zh-Hans'))return 'zh-Hans';
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
