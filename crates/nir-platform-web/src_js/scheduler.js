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
        if(this.waiting.length>1){
            const p95ByPhase=new Map();
            const phaseP95=phase=>{
                let v=p95ByPhase.get(phase);
                if(v===undefined){v=this.recentP95(phase);p95ByPhase.set(phase,v);}
                return v;
            };
            this.waiting.sort((a,b)=>a.priority-b.priority||
                (a.deadline===null?(b.deadline===null?0:1):b.deadline===null?-1:(a.deadline-phaseP95(a.phase))-(b.deadline-phaseP95(b.phase)))||a.sequence-b.sequence);
        }
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

