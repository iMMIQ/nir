// Sample the scheduled envelope on its own (pause-aware) device clock.
export function envelopePosition(plan, now) {
    if(!plan || plan.owner==null)return undefined;
    const elapsed=Math.min(plan.duration,Math.max(0,Math.round((now-plan.at)*1e6)));
    return {owner:plan.owner,elapsed_us:String(plan.base+elapsed)};
}
// Revision changes refresh actions, while focus stays on the same live control.
export function focusIdentity(action) {
    if(!action)return null;
    const value=JSON.parse(action);
    if(value.type==='menu_history_voice')return JSON.stringify([value.type,value.instance,value.window,value.entry]);
    if(value.type==='menu_history_scroll'&&value.control) return JSON.stringify([value.type,value.instance,value.window,value.control,value.input.type,value.input.type==='line'?value.input.delta:null]);
    return ['menu_control','menu_value'].includes(value.type)?JSON.stringify([value.type,value.instance,value.control]):action;
}
// Place a host control around the current canvas controls. Authored menu
// navigation owns its rectangles, including the top-right return button.
export function overlayControlPosition(nodes,width,height,buttonWidth,buttonHeight) {
    const gap=8,edge=12;
    if(![width,height,buttonWidth,buttonHeight].every(Number.isFinite)||buttonWidth<=0||buttonHeight<=0)return null;
    const rects=nodes.map(n=>n.rect).filter(r=>Array.isArray(r)&&r.length===4&&r.every(Number.isFinite)&&r[2]>0&&r[3]>0);
    const right=width-edge-buttonWidth,toolbar=rects.filter(r=>r[1]<=112).slice(0,16);
    const candidates=[[right,edge],...toolbar.map(r=>[r[0]-gap-buttonWidth,edge]),...toolbar.map(r=>[right,r[1]+r[3]+gap])];
    for(const [x,y] of candidates){
        if(x<edge||y<edge||x+buttonWidth>width-edge||y+buttonHeight>height-edge)continue;
        if(rects.every(r=>x+buttonWidth+gap<=r[0]||r[0]+r[2]+gap<=x||y+buttonHeight+gap<=r[1]||r[1]+r[3]+gap<=y))return [x,y];
    }
    return null;
}
export function samePointerTarget(a,b) {
    if(!a||!b)return false;
    if(a.type==='menu_value'&&b.type==='menu_value')
        return a.instance===b.instance&&a.revision===b.revision&&a.control===b.control;
    return JSON.stringify(a)===JSON.stringify(b);
}
// These actions change reading rather than opening an independent interface.
// A gesture that started during output recovery must never become one of them,
// even if its pointerdown has already resumed the device before pointerup.
export function audioBlockedAction(action) {
    return !!action&&(['advance','continue','choose','cancel_choice','toggle_auto','toggle_skip','toggle_interface'].includes(action.type)||action.type==='hold_skip'&&action.pressed);
}
// Enabled semantic buttons have an explicit action. Preserve its native
// keyboard activation just as for a canvas click; only reading actions (or
// an unfocused reading key) spend their first gesture on output recovery.
export function audioRecoveryConsumesKey(blocked,key,focusedAction) {
    return !!blocked&&(key===' '||key==='Enter')&&(!focusedAction||audioBlockedAction(focusedAction));
}
// Both domains retain elapsed time across bounded dispatch. Story boundaries
// discard only Story time; opening a menu must not freeze foreground effects.
export class DomainElapsed {
    story=0;foreground=0;
    add(elapsed,{hidden,before,after,storyBlocked=false}) {
        const foreground=!hidden&&before.session===after.session;
        const story=foreground&&!storyBlocked&&!before.paused&&!after.paused&&before.screen==='Story';
        this.story=story?this.story+elapsed:0;
        this.foreground=foreground?this.foreground+elapsed:0;
    }
    take() {
        const story=Math.min(0xffffffff,this.story),foreground=Math.min(0xffffffff,this.foreground);
        this.story-=story;this.foreground-=foreground;
        return [story,foreground];
    }
}
// Separate device clocks: suspending Story must not freeze foreground UI media.
export class AudioDomains {
    constructor(Context, warn=console.warn, now=()=>performance.now()) {
        // Keep the original Story/BGM and foreground routes first. Separate
        // device clocks let a menu freeze voice/SFX without rebuilding music.
        this.routes=new Map(['story','foreground_ui','story:voice','story:sfx'].map(key=>[key,{context:new Context(),domainPaused:true,busPaused:false,paused:true,unlocked:null,pending:null,pendingGesture:false,resumeAt:0,outputError:false,resumeError:false,resumeAttempts:0,resumeResolved:0,resumeRejected:0,lastResumeMs:null,maxResumeMs:0}]));
        this.warn=warn;this.now=now;this.closed=false;
        for(const r of this.routes.values()) {
            r.context.addEventListener?.('statechange',()=>{if(!this.closed){if(r.context.state==='running')r.outputError=false;this.changed?.();}});
            r.context.addEventListener?.('error',()=>{if(!this.closed){r.outputError=true;this.changed?.();}});
            r.context.suspend().catch(warn);
        }
    }
    route(domain,bus='bgm') {
        if(!['story','foreground_ui'].includes(domain))throw new Error(`E_AUDIO_DOMAIN: ${domain}`);
        if(!['bgm','voice','sfx'].includes(bus))throw new Error(`E_AUDIO_BUS: ${bus}`);
        return this.routes.get(domain==='story'&&bus!=='bgm'?`story:${bus}`:domain);
    }
    context(domain,bus='bgm'){return this.route(domain,bus).context;}
    paused(domain,bus='bgm'){return this.route(domain,bus).paused;}
    unlocked(domain,bus='bgm'){return this.route(domain,bus).unlocked;}
    unlock() {
        if(this.closed)return;
        for(const route of this.routes.values()) {
            if(route.paused&&route.unlocked)continue;
            this.resume(route,globalThis.navigator?.userActivation?.isActive??true);
        }
    }
    resume(route,gesture=false) {
        if(this.closed||route.context.state==='closed')return;
        // Our own suspend() can replace the native running target before a
        // pending resume settles. Releasing that pause must submit a new target:
        // waiting for the interrupted promise first can deadlock in Chrome.
        // Otherwise only a fresh permission gesture can supersede a pending
        // policy request; repeated clicks do not accumulate native promises.
        const interrupted=route.pending&&route.pendingSuspended&&!route.paused;
        if(route.pending&&!interrupted&&(!gesture||route.pendingGesture))return;
        // An application pause does not revoke the gesture that authorized
        // this context. Keep that permission identity for the replacement.
        const permitted=gesture||!!interrupted&&route.pendingGesture;
        if(route.context.state==='running'&&!route.outputError&&route.unlocked)return;
        const started=this.now();route.resumeAttempts=Math.min(0xffffffff,route.resumeAttempts+1);
        const settled=(success,current)=>{
            const duration=Math.max(0,this.now()-started);
            const count=success?'resumeResolved':'resumeRejected';route[count]=Math.min(0xffffffff,route[count]+1);
            route.maxResumeMs=Math.max(route.maxResumeMs,duration);
            if(current)route.lastResumeMs=duration;
        };
        let pending;
        try{pending=route.context.resume();}catch(error){settled(false,true);route.resumeError=true;this.warn(error);this.changed?.();return;}
        route.unlocked=pending;route.pending=pending;route.pendingSuspended=false;route.pendingGesture=permitted;route.resumeAt=started;route.resumeError=false;route.lastResumeMs=null;
        pending.then(()=>{
            if(this.closed)return;
            settled(true,route.unlocked===pending);
            if(route.unlocked===pending){
                route.pending=null;route.resumeError=false;
                if(route.context.state==='running')route.outputError=false;
            }
            if(route.paused)route.context.suspend().catch(this.warn);
            else if(route.unlocked===pending&&route.pendingSuspended&&route.context.state==='suspended'){
                // Cover a suspension whose native completion arrived after
                // the pause was released, preserving the current target.
                this.resume(route);
            }
            this.changed?.();
        },error=>{
            if(this.closed)return;
            settled(false,route.unlocked===pending);
            if(route.unlocked!==pending)return;
            route.unlocked=null;route.pending=null;route.resumeError=true;this.warn(error);this.changed?.();
        });
    }
    apply(route) {
        const paused=route.domainPaused||route.busPaused;
        if(route.paused===paused)return;
        route.paused=paused;
        if(this.closed)return;
        // Keep the interruption identity even if the native resume promise
        // never settles after this suspend. Unpause resubmits its running target.
        if(paused){if(route.pending)route.pendingSuspended=true;route.context.suspend().catch(this.warn);}
        else if(route.unlocked)this.resume(route);
    }
    setPaused(domain,paused) {
        this.route(domain);
        const buses=domain==='story'?['bgm','voice','sfx']:['bgm'];
        for(const bus of buses){const route=this.route(domain,bus);route.domainPaused=paused;this.apply(route);}
    }
    setBusPaused(domain,bus,paused) {
        const route=this.route(domain,bus);route.busPaused=paused;this.apply(route);
    }
    // Only scheduled, audible sources demand output. Menu/background policy
    // and deliberately muted voices must not produce a recovery prompt.
    blocked(voices) {
        if(this.closed)return false;
        for(const voice of voices)if(!voice.stopped&&voice.gain.gain.value>0) {
            const route=this.route(voice.domain,voice.bus);
            if(!route.paused&&(route.outputError||route.context.state!=='running'))return true;
        }
        return false;
    }
    recoveryState(voices) {
        let pending=false,delayed=false,failed=false,closed=false,canRetry=false;
        for(const voice of voices)if(!voice.stopped&&voice.gain.gain.value>0) {
            const route=this.route(voice.domain,voice.bus);
            if(route.paused||(!route.outputError&&route.context.state==='running'))continue;
            closed ||= route.context.state==='closed';
            failed ||= route.outputError||route.resumeError||route.context.state==='closed';
            pending ||= !!route.pending;
            delayed ||= !!route.pending&&this.now()-route.resumeAt>=1500;
            // A policy resume can still need a trusted gesture. A permitted
            // in-flight resume coalesces repeated gestures, so do not offer
            // a retry button that can only rejoin that same request.
            canRetry ||= route.context.state!=='closed'&&(!route.pending||!route.pendingGesture);
        }
        return {status:failed?'failed':delayed?(canRetry?'blocked':'waiting'):pending?'pending':'blocked',canRetry:canRetry&&!closed};
    }
    recoveryStatus(voices) {return this.recoveryState(voices).status;}
    snapshot(){
        const now=this.now(),describe=route=>({state:route.context.state,paused:route.paused,
            clock_seconds:route.context.currentTime,
            base_latency_ms:Number.isFinite(route.context.baseLatency)?route.context.baseLatency*1000:null,
            resume_pending:!!route.pending,pending_gesture:!!route.pending&&route.pendingGesture,
            pending_suspended:!!route.pending&&route.pendingSuspended,
            resume_wait_ms:route.pending?Math.max(0,now-route.resumeAt):0,
            resume_error:route.resumeError,output_error:route.outputError,
            resume_attempts:route.resumeAttempts,resume_resolved:route.resumeResolved,resume_rejected:route.resumeRejected,
            last_resume_ms:route.lastResumeMs,max_resume_ms:route.maxResumeMs});
        return Object.fromEntries(['story','foreground_ui'].map(domain=>[domain,{state:this.context(domain).state,paused:this.paused(domain),buses:Object.fromEntries(['bgm','voice','sfx'].map(bus=>[bus,describe(this.route(domain,bus))]))}]));
    }
    close(){this.closed=true;for(const r of this.routes.values())r.context.close().catch(this.warn);}
}
// Author loop times resolve once to decoded sample frames. Integer arithmetic
// preserves cumulative u64 playheads, including restoration after many cycles.
export function audioLoopPlayback(region,positionUs,sampleRate,frames,authoredDurationUs) {
    if(!Number.isSafeInteger(sampleRate)||sampleRate<=0||sampleRate>0xffffffff||!Number.isSafeInteger(frames)||frames<=0)throw new Error('E_AUDIO_LOOP: decoded shape');
    const micros=value=>{
        if(typeof value!=='string'||!/^\d{1,20}$/.test(value))throw new Error('E_AUDIO_LOOP: microsecond string');
        const n=BigInt(value);if(n>0xffffffffffffffffn)throw new Error('E_AUDIO_LOOP: microsecond range');return n;
    };
    const frame=us=>(us*BigInt(sampleRate)+500000n)/1000000n;
    const startUs=micros(region.start_us),endUs=micros(region.end_us);
    const duration=authoredDurationUs===undefined?null:micros(authoredDurationUs);
    if(duration!==null&&endUs>duration)throw new Error('E_AUDIO_LOOP: authored boundaries');
    const start=frame(startUs);let end=frame(endUs);
    // Native browser resampling can lose one output frame at the asset tail.
    // Only a validated full-asset endpoint may resolve to that actual tail;
    // interior boundaries and larger truncations keep their strict contract.
    if(end===BigInt(frames)+1n&&endUs===duration)end=BigInt(frames);
    if(start>=end||end>BigInt(frames))throw new Error('E_AUDIO_LOOP: decoded boundaries');
    let position=frame(micros(positionUs));
    if(position>=end)position=start+(position-end)%(end-start);
    return {startFrame:Number(start),endFrame:Number(end),offsetFrame:Number(position)};
}

// Keep an exclusive sample endpoint inside its boundary when converting to
// Web Audio seconds. A rounded-up duration can repeat the tail and skip the
// loop head. One adjacent double fixes that error without trimming a frame.
export function audioLoopEndSeconds(frames,sampleRate) {
    if(!Number.isSafeInteger(frames)||frames<=0||frames>0xffffffff||!Number.isSafeInteger(sampleRate)||sampleRate<=0||sampleRate>0xffffffff)throw new Error('E_AUDIO_LOOP: decoded shape');
    const seconds=frames/sampleRate;
    if(seconds*sampleRate<=frames)return seconds;
    const bits=new DataView(new ArrayBuffer(8));
    bits.setFloat64(0,seconds);
    bits.setBigUint64(0,bits.getBigUint64(0)-1n);
    return bits.getFloat64(0);
}

export function audioVoiceKey(command) {
    if(command.domain!=='story'&&command.domain!=='foreground_ui')throw new Error('E_AUDIO_DOMAIN');
    if(![command.session,command.task].every(n=>Number.isInteger(n)&&n>=0&&n<=0xffffffff))throw new Error('E_AUDIO_ID');
    return `${command.domain}:${command.session}:${command.task}`;
}

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

export function normalizeCharacterVoices(saved) {
    if(!saved||typeof saved!=='object'||Array.isArray(saved))return {};
    return Object.fromEntries(Object.entries(saved).filter(([id,value])=>
        id.length>0&&new TextEncoder().encode(id).length<=256&&value&&typeof value==='object'&&
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
export const SAVE_DATABASE='nir-player-isolated-v1';
export const saveKey=(gameId,profile,digest,slot)=>[gameId,profile,digest,slot];
export const profileKey=(gameId,profile)=>[gameId,profile];

// One in-flight write per kind, with one latest preference snapshot and the
// union of uncommitted progress. Failures wait for a new change, not a timer.
// Keep completion delivery on the owner through the supplied request function.
export class PersistenceWrites {
    constructor({request,writePreferences,mergeProfile,writeProfileValues,stored,failed}) {
        Object.assign(this,{request,writePreferences,mergeProfile,writeProfileValues,stored,failed});
        this.lanes=new Map(['preferences','profile','profile_values'].map(kind=>[kind,{active:false,dirty:false,value:null,keys:new Set(),patch:Object.create(null)}]));
        this.closed=false;
    }
    submit(kind,value) {
        if(this.closed)return;
        const lane=this.lanes.get(kind);if(!lane)throw new Error('E_PERSISTENCE_KIND');
        if(kind==='profile')for(const key of value)lane.keys.add(key);
        else if(kind==='profile_values')Object.assign(lane.patch,value);
        else lane.value=value;
        lane.dirty=true;this.pump(kind,lane);
    }
    pump(kind,lane) {
        if(this.closed||lane.active||!lane.dirty)return;
        lane.active=true;lane.dirty=false;
        const value=kind==='profile'?[...lane.keys]:kind==='profile_values'?{...lane.patch}:lane.value;
        let settled=false;
        const finish=(ok,error)=>{
            if(settled)return;settled=true;lane.active=false;
            if(this.closed)return;
            if(ok){
                if(kind==='profile')for(const key of value)lane.keys.delete(key);
                else if(kind==='profile_values'){
                    for(const key of Object.keys(value))if(lane.patch[key]===value[key])delete lane.patch[key];
                }
                else if(!lane.dirty)lane.value=null;
            }
            try {
                if(!ok)this.failed(kind,error);
                else if(!lane.dirty)this.stored(kind);
            } finally {this.pump(kind,lane);}
        };
        try {
            this.request(pending=>kind==='profile'?this.mergeProfile(value,pending):kind==='profile_values'?this.writeProfileValues(value,pending):this.writePreferences(value,pending),
                ()=>finish(true),error=>finish(false,error),{pending:error=>{
                    // A warning is not a terminal result. Keep this lane and
                    // its newest edits until the original transaction settles.
                    if(!this.closed&&!settled)this.failed(kind,error);
                }});
        }catch(error){finish(false,error);}
    }
    close() {
        this.closed=true;
        for(const lane of this.lanes.values()){lane.value=null;lane.keys.clear();lane.patch=Object.create(null);lane.dirty=false;}
    }
    retry(kind) {
        if(this.closed)return false;
        const lane=this.lanes.get(kind);if(!lane)throw new Error('E_PERSISTENCE_KIND');
        if(lane.active||(kind==='profile'?!lane.keys.size:kind==='profile_values'?!Object.keys(lane.patch).length:lane.value===null))return false;
        lane.dirty=true;this.pump(kind,lane);return true;
    }
}

// Missing records are a first visit. Incompatible originals are never reset
// by fallback negotiation or a later preference/progress write.
export function validateMetadataRecord(kind,value) {
    const invalid=()=>{throw new Error(kind==='profile'?'E_PROFILE_RECORD: unreadable progress':'E_PREFERENCES_RECORD: unreadable preferences');};
    if(value===undefined)return kind==='profile'?[]:kind==='profile_values'?{}:null;
    if(kind==='profile_values'){
        const bad=()=>{throw new Error('E_PROFILE_VALUES_RECORD: unreadable progress values');};
        if(!value||typeof value!=='object'||Array.isArray(value)||![Object.prototype,null].includes(Object.getPrototypeOf(value))||Object.keys(value).length>4096)bad();
        for(const [key,item] of Object.entries(value)){
            if(!key||new TextEncoder().encode(key).length>1024||!item||typeof item!=='object'||Array.isArray(item)||Object.keys(item).length!==2||!Object.hasOwn(item,'type')||!Object.hasOwn(item,'value'))bad();
            if(item.type==='i32'){if(!Number.isInteger(item.value)||item.value < -2147483648||item.value > 2147483647)bad();}
            else if(item.type==='bool'){if(typeof item.value!=='boolean')bad();}
            else if(item.type==='string'){if(typeof item.value!=='string'||new TextEncoder().encode(item.value).length>65536)bad();}
            else if(item.type==='f80'){if(typeof item.value!=='string'||! /^[0-9a-f]{20}$/.test(item.value))bad();const bits=BigInt('0x'+item.value),exp=Number((bits>>64n)&32767n),integer=Number((bits>>63n)&1n);if(exp===32767||(integer!==0)!==(exp!==0))bad();}
            else bad();
        }
        return value;
    }
    if(kind==='profile'){
        if(!Array.isArray(value))invalid();
        for(const key of value)if(typeof key!=='string')invalid(); // Includes sparse holes.
        return value;
    }
    if(kind!=='preferences')throw new Error('E_PERSISTENCE_KIND');
    if(!value||typeof value!=='object'||Array.isArray(value)||![Object.prototype,null].includes(Object.getPrototypeOf(value)))invalid();
    const numeric=['font_scale','text_speed','auto_wait_scale','bgm_volume','voice_volume','sfx_volume'];
    const boolean=['reduced_motion','auto_wait_voice','voice_continue'],locale=['locale','ui_locale','text_locale'];
    const known=new Set([...numeric,...boolean,...locale,'character_voices']);
    for(const key of Object.keys(value))if(!known.has(key))invalid();
    for(const key of numeric)if(Object.hasOwn(value,key)&&(typeof value[key]!=='number'||!Number.isFinite(value[key])))invalid();
    for(const key of boolean)if(Object.hasOwn(value,key)&&typeof value[key]!=='boolean')invalid();
    for(const key of locale)if(Object.hasOwn(value,key)&&typeof value[key]!=='string')invalid();
    if(Object.hasOwn(value,'character_voices')){
        const voices=value.character_voices;
        if(!voices||typeof voices!=='object'||Array.isArray(voices)||![Object.prototype,null].includes(Object.getPrototypeOf(voices))||Object.keys(voices).length>128)invalid();
        for(const [id,voice] of Object.entries(voices)){
            if(!id||new TextEncoder().encode(id).length>256||!voice||typeof voice!=='object'||Array.isArray(voice)||
                Object.keys(voice).some(key=>!['volume','muted'].includes(key))||
                typeof voice.volume!=='number'||!Number.isFinite(voice.volume)||typeof voice.muted!=='boolean')invalid();
        }
    }
    return value;
}
export function readMetadataRecord(db,kind,key,{timeoutMs=3000,signal}={}) {
    return new Promise((resolve,reject)=>{
        if(!Number.isSafeInteger(timeoutMs)||timeoutMs<=0||timeoutMs>0x7fffffff){reject(new RangeError('Invalid metadata read timeout'));return;}
        if(signal?.aborted){reject(signal.reason||new Error('E_STORAGE_ABORT: metadata read cancelled'));return;}
        let tx,r,value,received=false,settled=false;
        const finish=error=>{
            if(settled)return;
            settled=true;clearTimeout(timer);signal?.removeEventListener('abort',abort);
            if(r)r.onsuccess=null;
            if(tx){tx.oncomplete=tx.onabort=tx.onerror=null;}
            // Read results become visible only after transaction completion.
            // Abort stalled reads; never reset records or replay a write.
            if(error){try{tx?.abort();}catch{}reject(error);}else resolve(value);
        };
        const timer=setTimeout(()=>finish(new Error(`E_STORAGE_TIMEOUT: ${kind} read did not complete`)),timeoutMs);
        const abort=()=>finish(signal.reason||new Error('E_STORAGE_ABORT: metadata read cancelled'));
        signal?.addEventListener('abort',abort,{once:true});
        try{
            const storeKind=kind==='profile_values'?'profile':kind;
            tx=db.transaction(storeKind,'readonly');r=tx.objectStore(storeKind).get(kind==='profile_values'?[...key,'values']:key);
            r.onsuccess=()=>{
                if(settled)return;
                try{value=validateMetadataRecord(kind,r.result);received=true;}catch(error){finish(error);}
            };
            tx.oncomplete=()=>finish(received?null:new Error('E_STORAGE_READ: missing metadata result'));
            tx.onabort=tx.onerror=()=>finish(tx.error||r.error||new Error('E_STORAGE_ABORT'));
        }catch(error){finish(error);}
    });
}
export async function readStartupMetadata(db,key,options) {
    const failures=[];
    const [preferences,profile]=await Promise.all(['preferences','profile'].map(async kind=>{
        try{return await readMetadataRecord(db,kind,key,options);}
        catch(error){failures.push({kind,message:String(error)});return kind==='preferences'?null:[];}
    }));
    return {preferences,profile,failures};
}
export function writeMetadataRecord(db,kind,key,value,{timeoutMs=3000,signal,onPending}={}) {
    return new Promise((resolve,reject)=>{
        if(!Number.isSafeInteger(timeoutMs)||timeoutMs<=0||timeoutMs>0x7fffffff){reject(new RangeError('Invalid metadata write timeout'));return;}
        if(signal?.aborted){reject(signal.reason||new Error('E_STORAGE_ABORT: metadata write cancelled'));return;}
        try{validateMetadataRecord(kind,value);}catch(error){reject(error);return;}
        let tx,r,put,settled=false,written=false,putSucceeded=false,blocked=false,failure=null;
        const cleanup=()=>{
            clearTimeout(timer);signal?.removeEventListener('abort',abort);
            if(r)r.onsuccess=null;
            if(put)put.onsuccess=null;
            if(tx)tx.oncomplete=tx.onabort=tx.onerror=null;
        };
        const finish=error=>{
            if(settled)return;
            settled=true;cleanup();if(error)reject(error);else resolve();
        };
        const stop=error=>{
            if(settled||blocked)return;
            blocked=true;failure=error;clearTimeout(timer);signal?.removeEventListener('abort',abort);
            try{
                tx?.abort();
                // A successful abort request marks the transaction aborted
                // synchronously; late request callbacks cannot queue a put.
                finish(error);
            }catch{
                if(!written){finish(error);return;}
                // abort() can fail once commit has started. Neither a timer
                // nor an exception proves its outcome. Retain native terminal
                // handlers and the original owner slot; never replay it.
                const pending=Object.assign(new Error(`E_STORAGE_UNCERTAIN: ${kind} write is awaiting confirmation`),{code:'E_STORAGE_UNCERTAIN'});
                try{onPending?.(pending);}catch(noticeError){console.error('E_STORAGE_NOTICE',noticeError);}
            }
        };
        const timer=setTimeout(()=>stop(new Error(`E_STORAGE_TIMEOUT: ${kind} write did not complete`)),timeoutMs);
        const abort=()=>stop(signal.reason||new Error('E_STORAGE_ABORT: metadata write cancelled'));
        signal?.addEventListener('abort',abort,{once:true});
        try{
            const storeKind=kind==='profile_values'?'profile':kind,recordKey=kind==='profile_values'?[...key,'values']:key;
            tx=db.transaction(storeKind,'readwrite');const store=tx.objectStore(storeKind);r=store.get(recordKey);
            r.onsuccess=()=>{
                if(settled||blocked)return;
                try{
                    const prior=validateMetadataRecord(kind,r.result);
                    const next=kind==='profile'?[...new Set([...prior,...value])].sort():kind==='profile_values'?{...prior,...value}:value;
                    validateMetadataRecord(kind,next);
                    put=store.put(next,recordKey);written=true;
                    put.onsuccess=()=>{if(!settled)putSucceeded=true;};
                }catch(error){stop(error);}
            };
            tx.oncomplete=()=>finish(putSucceeded?null:failure||put?.error||new Error('E_STORAGE_WRITE: metadata was not written'));
            // Error events may still be cancelled by another listener. Only
            // native abort/completion decides the transaction's final result.
            tx.onerror=()=>{failure=tx.error||put?.error||r.error||new Error('E_STORAGE_WRITE');};
            tx.onabort=()=>finish(failure||tx.error||put?.error||r.error||new Error('E_STORAGE_ABORT'));
        }catch(error){stop(error);}
    });
}
export function writePreferencesRecord(db,key,value,options) {
    return writeMetadataRecord(db,'preferences',key,value,options);
}
export function mergeProfileRecord(db,key,keys,options) {
    return writeMetadataRecord(db,'profile',key,keys,options);
}

// Progress notices reuse a reserved owner slot. If native completion arrives
// before its warning is delivered, serialize both so the terminal is not lost.
export async function dispatchOwnerRequest(work,slot,{post,success,failure,pending}) {
    let notices=Promise.resolve(),accepting=true,value,error,ok=false;
    const notify=notice=>{
        if(!accepting||!pending||slot.state==='cancelled')return Promise.resolve(false);
        notices=notices.then(()=>post(slot,()=>pending(notice),false));return notices;
    };
    try{
        await Promise.resolve();
        if(slot.state==='cancelled')throw new Error('E_REQUEST_CANCELLED');
        value=await work(notify);ok=true;
    }catch(e){error=e;}
    accepting=false;await notices;
    await post(slot,()=>ok?success(value):failure(error));
}
const releaseDigestPattern=/^[0-9a-f]{64}$/;

export async function openSaveDatabase(indexedDBFactory=indexedDB,{timeoutMs=3000,signal}={}) {
    if(!Number.isSafeInteger(timeoutMs)||timeoutMs<=0||timeoutMs>0x7fffffff)throw new RangeError('Invalid storage open timeout');
    if(signal?.aborted)throw signal.reason||new Error('E_STORAGE_ABORT: database open cancelled');
    const db=await new Promise((resolve,reject)=>{
        let settled=false,r;
        const finish=(error,database)=>{
            if(settled){database?.close();return;}
            settled=true;clearTimeout(timer);signal?.removeEventListener('abort',abort);
            if(error){try{r?.transaction?.abort();}catch{}reject(error);}else resolve(database);
        };
        const timer=setTimeout(()=>finish(new Error('E_STORAGE_TIMEOUT: database did not open')),timeoutMs);
        const abort=()=>finish(signal.reason||new Error('E_STORAGE_ABORT: database open cancelled'));
        signal?.addEventListener('abort',abort,{once:true});
        try{r=indexedDBFactory.open(SAVE_DATABASE,1);}catch(error){finish(error);return;}
        r.onblocked=()=>finish(new Error('E_STORAGE_BLOCKED: database is in use'));
        r.onupgradeneeded=()=>{
            // A timed-out or disposed caller must not change a database when
            // its previously blocked request eventually resumes.
            if(settled){try{r.transaction?.abort();}catch{}return;}
            try{
                const database=r.result;
                if(!database.objectStoreNames.contains('saves'))database.createObjectStore('saves');
                for(const name of ['preferences','profile'])if(!database.objectStoreNames.contains(name))database.createObjectStore(name);
            }catch(error){finish(error);}
        };
        r.onsuccess=()=>finish(null,r.result);r.onerror=()=>finish(r.error||new Error('E_STORAGE_OPEN: database unavailable'));
    });
    db.onversionchange=()=>db.close();
    return db;
}

// Reconnect only for a new storage operation. Never replay a save or replace
// persisted data after an uncertain write result. Concurrent operations share
// the open attempt; errors remain explicit results for their original owners.
export class SaveDatabaseConnection {
    constructor(openDatabase=signal=>openSaveDatabase(indexedDB,{signal})) {
        this.openDatabase=openDatabase;this.db=null;this.pending=null;this.openController=null;this.closed=false;
    }
    connect() {
        if(this.closed)return Promise.reject(new Error('E_STORAGE_CLOSED: player disposed'));
        if(this.db)return Promise.resolve(this.db);
        if(this.pending)return this.pending;
        const controller=new AbortController();this.openController=controller;
        const pending=Promise.resolve().then(()=>{
            if(this.closed)throw new Error('E_STORAGE_CLOSED: player disposed');
            return this.openDatabase(controller.signal);
        }).then(db=>{
            if(this.closed){db.close();throw new Error('E_STORAGE_CLOSED: player disposed');}
            this.db=db;
            const invalidate=()=>{if(this.db===db)this.db=null;};
            db.onversionchange=()=>{invalidate();db.close();};db.onclose=invalidate;
            return db;
        }).finally(()=>{if(this.pending===pending){this.pending=null;this.openController=null;}});
        this.pending=pending;return pending;
    }
    async run(work) {
        const db=await this.connect();
        if(this.closed)throw new Error('E_STORAGE_CLOSED: player disposed');
        try{return await work(db);}
        catch(error){
            if(error?.name==='InvalidStateError'&&this.db===db){this.db=null;db.close();}
            throw error;
        }
    }
    close() {this.closed=true;this.openController?.abort();this.db?.close();this.db=null;}
}

function readSaveTransaction(db,operation,start,{timeoutMs=3000,signal}={}) {
    return new Promise((resolve,reject)=>{
        if(!Number.isSafeInteger(timeoutMs)||timeoutMs<=0||timeoutMs>0x7fffffff){reject(new RangeError('Invalid save read timeout'));return;}
        if(signal?.aborted){reject(signal.reason||new Error('E_STORAGE_ABORT: save read cancelled'));return;}
        let tx,value,received=false,settled=false;const requests=[];
        const finish=error=>{
            if(settled)return;
            settled=true;clearTimeout(timer);signal?.removeEventListener('abort',abort);
            for(const r of requests)r.onsuccess=null;
            if(tx)tx.oncomplete=tx.onabort=tx.onerror=null;
            if(error){try{tx?.abort();}catch{}reject(error);}else resolve(value);
        };
        const timer=setTimeout(()=>finish(new Error(`E_STORAGE_TIMEOUT: ${operation} did not complete`)),timeoutMs);
        const abort=()=>finish(signal.reason||new Error('E_STORAGE_ABORT: save read cancelled'));
        signal?.addEventListener('abort',abort,{once:true});
        const track=r=>{requests.push(r);return r;};
        const result=v=>{if(!settled){value=v;received=true;}};
        // Native events can already be queued when cancellation removes their
        // handlers. Guard saved callbacks as well, especially cursor.continue().
        const active=()=>!settled;
        try{
            tx=db.transaction('saves','readonly');
            start(tx.objectStore('saves'),track,result,active,finish);
            tx.oncomplete=()=>finish(received?null:new Error('E_STORAGE_READ: missing save result'));
            tx.onabort=tx.onerror=()=>finish(tx.error||requests.find(r=>r.error)?.error||new Error('E_STORAGE_ABORT'));
        }catch(error){finish(error);}
    });
}
export function readSaveRecord(db,key,options) {
    return readSaveTransaction(db,'save read',(store,track,result,active)=>{
        const r=track(store.get(key));r.onsuccess=()=>{if(active())result(r.result);};
    },options);
}

// Typed Rust snapshot hashing distinguishes -0.0 from 0.0. Preserve that
// number through JSON transport, including browsers without JSON.rawJSON.
export function stringifyPlayerData(value,validate) {
    const prefix='__nir_json_negative_zero_',reserved=new Set();let marker=prefix+'0__',zeros=false;
    const encode=()=>JSON.stringify(value,function(key,v){
        if(validate)validate(this,key);
        if(key.startsWith(prefix))reserved.add(key);
        if(typeof v==='string'&&v.startsWith(prefix))reserved.add(v);
        if(typeof v==='number'&&Object.is(v,-0)){zeros=true;return marker;}
        return v;
    });
    let json=encode();if(!zeros)return json;
    if(reserved.has(marker)){let n=1;while(reserved.has(prefix+n+'__'))n++;marker=prefix+n+'__';json=encode();}
    return json.replaceAll(JSON.stringify(marker),'-0.0');
}

export async function inspectSaveRecord(record,key,inspect) {
    if(record===undefined)return null;
    if(!record||typeof record!=='object'||Array.isArray(record)
        ||record.gameId!==key[0]||record.profile!==key[1]||record.releaseDigest!==key[2]||record.slot!==key[3]
        ||!record.envelope||typeof record.envelope!=='object'||Array.isArray(record.envelope))
        throw new Error('E_SAVE_IDENTITY: stored record belongs to another slot');
    if(typeof inspect!=='function')throw new Error('E_SAVE_INSPECT: validator is required');
    return await inspect(stringifyPlayerData(record.envelope),key);
}

export async function listSaveRecords(db,gameId,profile,release,inspect,options) {
    const rows=[];
    for(let slot=0;slot<3;slot++){
        const key=saveKey(gameId,profile,release,slot);
        try{
            const record=await readSaveRecord(db,key,options),revision=await inspectSaveRecord(record,key,inspect);
            if(revision!==null)rows.push({slot,revision,label:typeof record.label==='string'?record.label.slice(0,256):`#${revision}`});
        }catch(error){rows.push({slot,error:String(error).slice(0,1024)});}
    }
    return rows;
}

export function listHistoryRecords(db,gameId,profile,options) {
    return readSaveTransaction(db,'save history read',(store,track,result,active,finish)=>{
        const rows=[];
        // The entire key prefix is contiguous. A lower bound also includes
        // damaged keys with an unexpected third-key type, unlike an [] upper bound.
        const range=IDBKeyRange.lowerBound([gameId,profile]);
        const cursor=track(store.openCursor(range));
        cursor.onsuccess=()=>{
            if(!active())return;
            try{
            if(!cursor.result){result(rows.sort((a,b)=>b.savedAt-a.savedAt));return;}
            const key=cursor.result.primaryKey;
            if(!Array.isArray(key)||key[0]!==gameId||key[1]!==profile){result(rows.sort((a,b)=>b.savedAt-a.savedAt));return;}
            const record=cursor.result.value;
            const savedAt=typeof record?.saved_at==='number'&&Number.isFinite(record.saved_at)
                &&Math.abs(record.saved_at)<=8.64e15?record.saved_at:0;
            const version=typeof record?.version==='string'?record.version.slice(0,256):'Version unknown';
            rows.push({key,savedAt,version});cursor.result.continue();
            }catch(error){finish(error);}
        };
    },options);
}

export function saveHistoryExport(entry,readable) {
    if(readable)return stringifyPlayerData(entry.record.envelope);
    // Recovery exports are explicitly distinct from importable save envelopes.
    // Never manufacture metadata or discard the damaged persisted value.
    const recovery={format:'nir-save-record-recovery-v1',key:entry.key,
        record:entry.record,...(entry.record===undefined?{recordUndefined:true}:{})};
    return stringifyPlayerData(recovery,(parent,key)=>{
        const value=parent[key],type=typeof value;
        if(value===undefined&&parent===recovery&&key==='record'&&recovery.recordUndefined)return;
        if(value===null||type==='string'||type==='boolean'||(type==='number'&&Number.isFinite(value)))return;
        if(type==='object'&&(Array.isArray(value)||Object.getPrototypeOf(value)===Object.prototype||Object.getPrototypeOf(value)===null))return;
        // JSON silently drops binary, dates, maps and non-finite values. Keep
        // the original in IndexedDB and report an export failure instead.
        throw new TypeError('E_HISTORY_EXPORT: record contains non-JSON values');
    });
}

export function validHistoryKey(key) {
    return Array.isArray(key)&&key.length===4&&typeof key[0]==='string'&&typeof key[1]==='string'
        &&typeof key[2]==='string'&&releaseDigestPattern.test(key[2])&&Number.isInteger(key[3])&&key[3]>=0&&key[3]<3;
}

export async function commitSaveRecord(db,key,envelope,metadata,expectedRevision,inspect,{timeoutMs=3000,signal,onPending}={}) {
    if(!Number.isSafeInteger(timeoutMs)||timeoutMs<=0||timeoutMs>0x7fffffff)throw new RangeError('Invalid save write timeout');
    const cancelled=()=>signal?.reason||new Error('E_STORAGE_ABORT: save write cancelled');
    if(signal?.aborted)throw cancelled();
    if(!Number.isInteger(expectedRevision)||expectedRevision<0||expectedRevision>=0xffffffff)
        throw new Error('E_SAVE_REVISION: invalid expected revision');
    const incoming={...metadata,envelope};
    const revision=await inspectSaveRecord(incoming,key,inspect);
    if(revision!==expectedRevision+1)throw new Error('E_SAVE_REVISION: next revision does not match');
    const current=await readSaveRecord(db,key,{timeoutMs,signal});
    const storedRevision=await inspectSaveRecord(current,key,inspect);
    if((storedRevision??0)!==expectedRevision)throw new Error('E_SAVE_CONFLICT: another tab changed this slot. Reopen the save menu.');
    if(signal?.aborted)throw cancelled();
    const checked=stringifyPlayerData(current);
    return new Promise((resolve,reject)=>{
        let tx,r,put,settled=false,blocked=false,written=false,putSucceeded=false,failure=null;
        const finish=error=>{
            if(settled)return;
            settled=true;clearTimeout(timer);signal?.removeEventListener('abort',abort);
            if(r)r.onsuccess=null;if(put)put.onsuccess=null;
            if(tx)tx.oncomplete=tx.onabort=tx.onerror=null;
            if(error)reject(error);else resolve();
        };
        const stop=error=>{
            if(settled||blocked)return;
            blocked=true;failure=error;clearTimeout(timer);signal?.removeEventListener('abort',abort);
            try{tx?.abort();finish(error);}
            catch{
                if(!written){finish(error);return;}
                // The request may already be committing. Keep its original
                // completion and slot ownership; a timeout is not an outcome.
                const pending=Object.assign(new Error('E_STORAGE_UNCERTAIN: save is awaiting confirmation'),{code:'E_STORAGE_UNCERTAIN'});
                try{onPending?.(pending);}catch(noticeError){console.error('E_STORAGE_NOTICE',noticeError);}
            }
        };
        const timer=setTimeout(()=>stop(new Error('E_STORAGE_TIMEOUT: save write did not complete')),timeoutMs);
        const abort=()=>stop(cancelled());signal?.addEventListener('abort',abort,{once:true});
        try{
            tx=db.transaction('saves','readwrite');const store=tx.objectStore('saves');r=store.get(key);
            r.onsuccess=()=>{
                if(settled||blocked)return;
                try{
                    // Worker inspection happens before this transaction. Compare
                    // its entire checked value here without yielding the lock.
                    if(stringifyPlayerData(r.result)!==checked){stop(new Error('E_SAVE_CONFLICT: another tab changed this slot. Reopen the save menu.'));return;}
                    put=store.put({...metadata,envelope,saved_at:Date.now(),label:new Date().toLocaleString()},key);written=true;
                    put.onsuccess=()=>{if(!settled)putSucceeded=true;};
                }catch(error){stop(error);}
            };
            tx.oncomplete=()=>finish(putSucceeded?null:failure||put?.error||new Error('E_STORAGE_WRITE: save was not written'));
            tx.onerror=()=>{failure=tx.error||put?.error||r.error||new Error('E_STORAGE_WRITE');};
            tx.onabort=()=>finish(failure||tx.error||put?.error||r.error||new Error('E_STORAGE_ABORT'));
        }catch(error){stop(error);}
    });
}

// One deadline spans database admission, inspection and network verification.
// Aborting settles the UI immediately; physical Worker reservations still last
// until their reply, and synchronous WASM cannot be preempted by a JS timer.
export function checkSavedRelease(work,{signal,timeoutMs=3000}={}) {
    if(!Number.isSafeInteger(timeoutMs)||timeoutMs<=0||timeoutMs>0x7fffffff)return Promise.reject(new RangeError('Invalid saved release check timeout'));
    const controller=new AbortController(),owned=controller.signal,start=performance.now();
    const timeout=()=>Object.assign(new Error('E_STORAGE_TIMEOUT: saved release check did not complete'),{code:'E_STORAGE_TIMEOUT',operation:'release_check'});
    return new Promise((resolve,reject)=>{
        let settled=false;
        const finish=(ok,value)=>{
            if(settled)return;
            settled=true;clearTimeout(timer);signal?.removeEventListener('abort',cancel);owned.removeEventListener('abort',abort);
            if(ok&&performance.now()-start>=timeoutMs){ok=false;value=timeout();}
            controller.abort(ok?new Error('E_STORAGE_ABORT: saved release check finished'):value);
            if(ok)resolve(value);else reject(value);
        };
        const cancel=()=>controller.abort(signal.reason||new Error('E_STORAGE_ABORT: saved release check cancelled'));
        const abort=()=>finish(false,owned.reason);
        const timer=setTimeout(()=>controller.abort(timeout()),timeoutMs);
        signal?.addEventListener('abort',cancel,{once:true});owned.addEventListener('abort',abort,{once:true});
        if(signal?.aborted)cancel();
        if(!owned.aborted)Promise.resolve().then(()=>{owned.throwIfAborted();return work(owned);}).then(value=>finish(true,value),error=>finish(false,error));
    });
}

export async function validateHistoryTarget({releaseRoot,digest,gameId,profile,signal,timeoutMs=3000,fetchImpl=fetch,subtle=globalThis.crypto?.subtle,sha256=async value=>Array.from(new Uint8Array(await subtle.digest('SHA-256',value)),x=>x.toString(16).padStart(2,'0')).join('')}) {
    if(!releaseDigestPattern.test(digest))return {available:false,status:'Invalid release digest'};
    const root=new URL(releaseRoot,location.href),manifest=new URL(`releases/${digest}.json`,root),entry=new URL(`releases/${digest}/index.html`,root);
    if(root.origin!==location.origin||manifest.origin!==root.origin||entry.origin!==root.origin||!manifest.pathname.startsWith(root.pathname)||!entry.pathname.startsWith(root.pathname))
        return {available:false,status:'Release is outside this site'};
    return checkSavedRelease(async signal=>{
      try{
        const response=await fetchImpl(manifest,{cache:'no-cache',signal});
        if(!response.ok)return {available:false,status:'Release resources unavailable'};
        const bytes=await response.arrayBuffer();
        const actual=await sha256(bytes);
        signal.throwIfAborted();
        if(actual!==digest)return {available:false,status:'Release verification failed'};
        const release=JSON.parse(new TextDecoder().decode(bytes));
        if(release.format!==1||release.game_id!==gameId||release.profile!==profile)return {available:false,status:'Different game or profile'};
        if(!releaseDigestPattern.test(release.launch?.html))return {available:false,status:'Release player verification failed'};
        const entryResponse=await fetchImpl(entry,{cache:'no-cache',signal});
        if(!entryResponse.ok)return {available:false,status:'Release player unavailable'};
        const htmlDigest=await sha256(await entryResponse.arrayBuffer());
        signal.throwIfAborted();
        if(htmlDigest!==release.launch.html)return {available:false,status:'Release player verification failed'};
        return {available:true,status:'Available',url:entry.href};
      }catch(error){signal.throwIfAborted();return {available:false,status:'Release resources unavailable'};}
    },{signal,timeoutMs});
}

export async function initializeBackend({requested='auto',probe,create,replaceCanvas}) {
    if(!['auto','webgpu','webgl2'].includes(requested))throw new Error('E_RENDER_BACKEND: expected auto, webgpu or webgl2');
    let selected=requested,fallbackReason=null;
    if(selected==='auto'){
        try{selected=await probe();if(selected==='webgl2')fallbackReason='WebGPU adapter unavailable';}
        catch(error){selected='webgl2';fallbackReason=String(error);}
    }
    try{return {engine:await create(selected),fallbackReason};}
    catch(error){
        if(requested!=='auto'||selected!=='webgpu')throw error;
        fallbackReason=String(error);replaceCanvas();
        return {engine:await create('webgl2'),fallbackReason};
    }
}

export const WORKER_PROTOCOL=1;
// Coalesce only adjacent moves: a press/release is an ordering boundary.
export class SerialInputQueue {
    constructor(onError,capacity=64){this.onError=onError;this.capacity=capacity;this.jobs=[];this.running=false;this.closed=false;this.generation=0;}
    push(run,data,key=null){
        if(this.closed)return;
        const last=this.jobs.at(-1);
        if(key!==null&&last?.key===key){last.data=data;return;}
        if(this.jobs.length+(this.running?1:0)>=this.capacity){this.clear();this.onError(new Error('E_INPUT_CAPACITY'));return;}
        this.jobs.push({run,data,key});void this.drain();
    }
    clear(){this.generation++;this.jobs.length=0;}
    close(){this.closed=true;this.clear();}
    async drain(){
        if(this.running)return;this.running=true;
        try{while(this.jobs.length&&!this.closed){const job=this.jobs.shift(),generation=this.generation;try{await job.run(job.data,()=>generation===this.generation&&!this.closed);}catch(error){this.onError(error);}}}
        finally{this.running=false;}
    }
}
// A terminal RPC occupies its slot until physical settlement. Control reserves
// remain available when normal work has exhausted its admission window.
export class WorkerClient {
    constructor(worker,build,{capacity=256,timeout=30000}={}) {
        this.worker=worker;this.build=build;this.capacity=capacity;this.timeout=timeout;this.next=0;this.pending=new Map();this.closed=false;this.highWater=0;this.updates=0;this.onUpdate=null;this.onFailure=null;
        worker.onmessage=e=>this.receive(e.data);
        worker.onerror=e=>this.close(new Error(`E_WORKER_CRASH: ${e.message||'worker error'}`));
        worker.onmessageerror=()=>this.close(new Error('E_WORKER_MESSAGE'));
    }
    receive(m){
        if(this.closed)return;
        if(m.protocol!==WORKER_PROTOCOL||m.build!==this.build){this.close(new Error('E_WORKER_PROTOCOL'));return;}
        if(m.kind==='fatal'){this.close(new Error(`${m.value.code||'E_WORKER_RUNTIME'}: ${m.value.message}`));return;}
        if(m.kind==='update'){
            this.updates++;try{this.onUpdate?.(m.value);}catch(e){this.close(e);return;}
            this.worker.postMessage({protocol:WORKER_PROTOCOL,build:this.build,kind:'ack',id:0});return;
        }
        const slot=this.pending.get(m.id);if(!slot)return;
        this.pending.delete(m.id);clearTimeout(slot.timer);slot.cleanup();
        if(m.kind==='error'){const error=new Error(m.value.message);error.code=/\b(E_[A-Z_]+)\b/.exec(m.value.message)?.[1];slot.reject(error);}
        else if(m.kind==='reply')slot.resolve(m.value);
        else{slot.reject(new Error('E_WORKER_PROTOCOL'));this.close(new Error('E_WORKER_PROTOCOL'));}
    }
    call(kind,value,transfer=[],{control=false,signal=null,timeout=this.timeout}={}){
        if(this.closed)return Promise.reject(new Error('E_WORKER_CLOSED'));
        if(this.pending.size>=(control?this.capacity:this.capacity-8))return Promise.reject(new Error('E_WORKER_CAPACITY'));
        if(signal?.aborted)return Promise.reject(signal.reason||new Error('E_CANCELLED'));
        const id=++this.next;
        return new Promise((resolve,reject)=>{
            const abort=()=>{if(!this.closed)this.worker.postMessage({protocol:WORKER_PROTOCOL,build:this.build,kind:'cancel',id});};
            const timer=timeout?setTimeout(()=>this.close(new Error('E_WORKER_TIMEOUT')),timeout):null;
            const cleanup=()=>signal?.removeEventListener('abort',abort);
            this.pending.set(id,{resolve,reject,timer,cleanup});this.highWater=Math.max(this.highWater,this.pending.size);
            signal?.addEventListener('abort',abort,{once:true});
            try{this.worker.postMessage({protocol:WORKER_PROTOCOL,build:this.build,kind,id,value},transfer);}
            catch(error){this.pending.delete(id);clearTimeout(timer);cleanup();reject(error);}
        });
    }
    close(error=new Error('E_WORKER_DISPOSED')){
        if(this.closed)return;this.closed=true;this.worker.terminate();
        for(const p of this.pending.values()){clearTimeout(p.timer);p.cleanup();p.reject(error);}this.pending.clear();this.onFailure?.(error);
    }
}
export class RemoteEngine {
    constructor(client,initial){this.client=client;this.remote=true;this.calls=[];this.commandsReady=[];this.resources=new Map();this.profiles=[];this.onUpdate=null;this.inFlight=null;this.size=null;this.apply(initial);client.onUpdate=s=>{this.apply(s);this.onUpdate?.();};}
    apply(s){
        // Each message owns its commands, even when a delayed GPU completion
        // carries an older view than a subsequently received state update.
        for(const c of s.commands)if(c.type==='resource_stage'){for(const key of ['start_us','end_us'])if(typeof c[key]==='string')c[key]=String(Number(c[key])+(this.client.clockOffsetUs||0));}this.commandsReady.push(...s.commands);
        if(this.commandsReady.length>256)throw new Error('E_WORKER_CAPACITY: host commands');
        this.profiles.push(...JSON.parse(s.profile||'[]').map(row=>({...row,start_us:String(Number(row.start_us)+(this.client.clockOffsetUs||0)),end_us:String(Number(row.end_us)+(this.client.clockOffsetUs||0))}))); this.profiles=this.profiles.slice(-128);
        if(this.snapshot&&s.revision<this.snapshot.revision)return;
        this.snapshot=s;this.host=JSON.parse(s.host);this.validRequests=new Set(s.requests||[]);this.validContent=new Set(s.contentRequests||[]);
        for(const key of this.resources.keys())if(!this.validRequests.has(Number(key.split(':')[0])))this.resources.delete(key);
    }
    enqueue(method,args){if(this.calls.length>=128)throw new Error('E_WORKER_CAPACITY: calls');this.calls.push({method,args,session:this.host.session,interaction:this.host.interaction});}
    async sync(){
        if(this.inFlight)await this.inFlight;
        if(!this.calls.length)return;
        const calls=this.calls.splice(0),transfer=[];
        for(const c of calls)for(const a of c.args)if(ArrayBuffer.isView(a)&&a.buffer instanceof ArrayBuffer)transfer.push(a.buffer);
        const control=calls.some(c=>['hidden','audio_blocked','begin_recovery','simulate_device_loss'].includes(c.method)||c.method==='host_event'&&c.args[0]==='assets_cancelled');
        this.inFlight=this.client.call('batch',calls,[...new Set(transfer)],{control}).then(reply=>{
            this.apply(reply.snapshot);
            calls.forEach((c,i)=>{if(['resource','resource_decoded'].includes(c.method))this.resources.set(`${c.args[0]}:${c.args[1]}`,{done:reply.results[i],pending:false});});
        });
        try{await this.inFlight;}finally{this.inFlight=null;}
    }
    async query(method,args){await this.sync();const reply=await this.client.call('batch',[{method,args,session:this.host.session,interaction:this.host.interaction}]);this.apply(reply.snapshot);this.onUpdate?.();return reply.results[0]??(['pointer_action','control_value_action','focus_value_action','primary_action'].includes(method)?'null':undefined);}
    commands(){return stringifyPlayerData(this.commandsReady.splice(0));}
    host_state(){return this.snapshot.host;} state(){return this.snapshot.state;}
    retained_descriptors(){return this.snapshot.retained;}
    text_cache_stats(){return this.snapshot.textCache;}
    pending_events(){return this.snapshot.pending;} needs_clock(){return this.snapshot.needsClock;}
    backend(){return this.snapshot.backend;} gpu_error(){return this.snapshot.gpuError;} device_lost(){return this.snapshot.deviceLost;}
    accepts(id){return this.validRequests.has(id);} accepts_content(id){return this.validContent.has(id);}
    take_profile(){return JSON.stringify(this.profiles.splice(0));}
    begin_turn(){} continue_turn(){} tick_domains(){}
    draw(width,height,dpr){const size={width,height,dpr};if(JSON.stringify(size)!==JSON.stringify(this.size)){this.size=size;this.enqueue('resize',[size]);}return this.snapshot.view;}
    resource(request,id,bytes){return this.resourceCall('resource',request,id,[new Uint8Array(bytes)]);}
    resource_decoded(request,id,width,height,pixels){return this.resourceCall('resource_decoded',request,id,[width,height,pixels]);}
    resourceCall(method,request,id,tail){const key=`${request}:${id}`,last=this.resources.get(key);if(last?.done||!this.accepts(request))return true;if(!last?.pending){this.resources.set(key,{pending:true});this.enqueue(method,[request,id,...tail]);}return false;}
    replace_gpu(gpu){this.apply(gpu.snapshot);}
    free(){this.client.close();}
}
for(const name of ['action','host_event','content_ready','content_failed','content_skipped','resource_fault','resource_failed','audio_positions_in','audio_ended_in','audio_failed_in','hidden','audio_blocked','simulate_device_loss','begin_recovery','set_profiling','focus_control','hover','set_touch_input'])RemoteEngine.prototype[name]=function(...args){this.enqueue(name,args);};
for(const name of ['pointer_gesture','pointer_action','navigate_focus','control_value_action','focus_value_action','primary_action','hit'])RemoteEngine.prototype[name]=function(...args){return this.query(name,args);};

async function connectWorker(role,options) {
    const id=options.release.engine[role==='runtime'?'runtime_worker':'asset_worker'];
    if(!id)throw new Error('E_WORKER_UNAVAILABLE: release has no worker');
    await options.fetchObject(id);
    const url=new URL(options.release.objects[id].path,options.releaseRoot);
    const client=new WorkerClient(new Worker(url,{type:'module',name:`nir-${role}`}),options.release.engine_build);
    const bytes=options.wasmBytes.slice(0);
    try {
        const ready=await client.call('hello',{glueUrl:new URL(options.release.objects[options.release.engine.js].path,options.releaseRoot).href,glueDigest:options.release.engine.js,wasmBytes:bytes,root:options.releaseRoot,debug:new URL(location.href).searchParams.has('test'),diagnostics:new URL(location.href).searchParams.has('diagnostics')},[bytes]);
        if(ready.role!==role||ready.protocol!==WORKER_PROTOCOL||ready.build!==options.release.engine_build)throw new Error('E_WORKER_HANDSHAKE');
        client.clockOffsetUs=Math.round((ready.timeOrigin-performance.timeOrigin)*1000);
        return client;
    }catch(error){client.close();throw error;}
}

export async function start({wasm,release,releaseDigest,releaseRoot,executable,fetchObject,fail,startupTrace=[],sha256,wasmBytes}) {
    const params=new URL(location.href).searchParams;
    const trace=new TraceRecorder({enabled:params.get('trace')!=='0'&&(params.has('diagnostics')||params.has('test'))});
    const performanceStats=trace.enabled?new PerformanceRecorder(64):null;
    let traceContext={session:1,device:1};
    const observe=(stage,fields={})=>trace.record(stage,{...traceContext,...fields});
    for(const row of startupTrace)observe(row.stage,row);
    let canvas=document.querySelector('#stage');
    const shell=document.querySelector('#shell');
    const replaceCanvas=()=>{const next=canvas.cloneNode(false);canvas.replaceWith(next);canvas=next;};
    const program=parseRuntimeProgram(executable);
    const accent=program.theme.accent;
    document.querySelector('#focus-ring').style.borderColor=`rgba(${accent.slice(0,3).map(channel=>Math.round(channel*255)).join(',')},${accent[3]})`;
    const metrics={boot:performance.now(),titleMs:null,firstLineMs:null,resourceFailures:0,frames:0,audioStarts:0,deviceRecoveries:0,peakResidentBytes:0,startInputMs:null,firstLineAfterStartMs:null,contentStagingBytes:0,peakContentStagingBytes:0,contentStagingBudgetBytes:CONTENT_STAGING_LIMIT,contentStagingReservations:0,contentStagingWaiters:0};
    const syncContentStagingMetrics=budget=>{
        metrics.contentStagingBytes=budget.used;
        metrics.peakContentStagingBytes=budget.peak;
        metrics.contentStagingBudgetBytes=budget.limit;
        metrics.contentStagingReservations=budget.reservations.size;
        metrics.contentStagingWaiters=budget.waiting.length;
    };
    let viewportOffset={left:0,top:0,right:0,bottom:0};
    const refreshSafeArea=()=>{
        const style=getComputedStyle(document.documentElement);
        for(const edge of ['left','top','right','bottom'])viewportOffset[edge]=Math.max(0,parseFloat(style.getPropertyValue(`--nir-safe-${edge}`))||0);
    };
    refreshSafeArea();
    const size=()=>{const dpr=Math.min(devicePixelRatio||1,2);const width=Math.max(1,innerWidth-viewportOffset.left-viewportOffset.right),height=Math.max(1,innerHeight-viewportOffset.top-viewportOffset.bottom);return {width,height,dpr};};
    let {width,height,dpr}=size();canvas.width=Math.round(width*dpr);canvas.height=Math.round(height*dpr);
    if(!['dev','release'].includes(release.profile)||!releaseDigestPattern.test(releaseDigest))throw new Error('E_RELEASE_IDENTITY');
    const sharedKey=profileKey(release.game_id,release.profile);
    const slotKey=slot=>saveKey(release.game_id,release.profile,releaseDigest,slot);
    const storage=new SaveDatabaseConnection();
    const startup=await storage.run(db=>readStartupMetadata(db,sharedKey)).catch(error=>({
        preferences:null,profile:[],failures:['preferences','profile'].map(kind=>({kind,message:String(error)})),
    }));
    if(program.requires?.includes('story.profile-value.v1')){
        try{startup.profileValues=await storage.run(db=>readMetadataRecord(db,'profile_values',sharedKey));}
        catch(error){startup.profileValues={};startup.failures.push({kind:'profile_values',message:String(error)});}
    }
    const metadataReadFailures=new Map(startup.failures.map(f=>[f.kind,f.message])),metadataWriteFailures=new Set(),metadataPendingWrites=new Set();
    let preferences=initialRuntimePreferences(program,startup.preferences,navigator.languages||[],matchMedia('(prefers-reduced-motion: reduce)').matches);
    observe('preferences_loaded');
    const createStart=String(Math.round(performance.now()*1000));
    const workerTimings={capacity:4096,rows:[],dropped:0,incomplete:false};
    const threading={runtime:'main',asset:'inline',protocol:WORKER_PROTOCOL,fallback_reason:null};
    const workerOptions={release,releaseRoot,fetchObject,wasmBytes};
    let runtimeClient=null,assetClient=null;
    if(typeof Worker==='function'&&release.engine.asset_worker&&params.get('assets')!=='main'){
        try{assetClient=await connectWorker('asset',workerOptions);threading.asset='worker';}
        catch(error){if(params.get('worker')==='required')throw error;threading.asset_fallback_reason=String(error);}
    }
    if(assetClient){
        fetchObject=async(id,signal,observer=()=>{})=>{
            const descriptor=release.objects[id];if(!descriptor)throw new Error('E_OBJECT_REFERENCE');
            const reply=await assetClient.call('fetch',{url:new URL(descriptor.path,releaseRoot).href,digest:id,bytes:descriptor.bytes},[],{signal,timeout:0});
            const bytes=reply.bytes;threading.asset_wasm_memory_bytes=reply.wasmMemoryBytes;
            if(trace.enabled){if(reply.resource){const row={...reply.resource};for(const key of ['startTime','fetchStart','domainLookupStart','domainLookupEnd','connectStart','connectEnd','requestStart','responseStart','responseEnd'])if(row[key])row[key]+=assetClient.clockOffsetUs/1000;workerTimings.rows.push(row);if(workerTimings.rows.length>workerTimings.capacity){workerTimings.rows.shift();workerTimings.dropped++;}}else workerTimings.incomplete=true;}
            observer('object_verified',{object:id,bytes:bytes.byteLength});return bytes;
        };
    }
    const originalWasm=wasm;
    const AudioContext=window.AudioContext||window.webkitAudioContext;
    let audioDomains=null,audio=null;
    function decoderSampleRate() {
        if(!audioDomains){audioDomains=new AudioDomains(AudioContext);audio=audioDomains.context('story');}
        const rate=audio.sampleRate;
        if(!Number.isSafeInteger(rate)||rate<=0||rate>0xffffffff)throw new Error('E_AUDIO_RATE: invalid decoder rate');
        return rate;
    }
    let initialized;
    const createRuntime=async(backend)=>{
        if(!runtimeClient||runtimeClient.closed)runtimeClient=await connectWorker('runtime',workerOptions);
        const offscreen=canvas.transferControlToOffscreen();
        try{
            const reply=await runtimeClient.call('create',{canvas:offscreen,size:{width,height,dpr},executable,release:releaseDigest,title:release.title,preferences:JSON.stringify(preferences),backend,audio_sample_rate:decoderSampleRate(),profiling:trace.enabled},[offscreen]);
            const remote=new RemoteEngine(runtimeClient,reply.snapshot);remote.size={width,height,dpr};threading.runtime='worker';return remote;
        }catch(error){runtimeClient.close();throw error;}
    };
    const workersAvailable=typeof Worker==='function'&&typeof OffscreenCanvas==='function'&&typeof canvas.transferControlToOffscreen==='function'&&release.engine.runtime_worker;
    try{
        if(params.get('worker')!=='main'&&workersAvailable){
            try{
                runtimeClient=await connectWorker('runtime',workerOptions);
                initialized=await initializeBackend({requested:params.get('backend')||'auto',probe:()=>runtimeClient.call('probe',null),create:createRuntime,replaceCanvas});
            }catch(error){runtimeClient?.close();if(params.get('worker')==='required')throw error;threading.fallback_reason=String(error);replaceCanvas();}
        }else if(params.get('worker')==='required')throw new Error('E_WORKER_UNAVAILABLE');
        if(!initialized){
            await originalWasm.default({module_or_path:wasmBytes});
            initialized=await initializeBackend({requested:params.get('backend')||'auto',probe:()=>originalWasm.probe_backend(),create:backend=>originalWasm.Engine.create(executable,releaseDigest,release.title,'stage',JSON.stringify(preferences),backend,decoderSampleRate()),replaceCanvas});
        }
    }catch(error){audioDomains?.close();runtimeClient?.close();assetClient?.close();storage.close();throw error;}
    const {engine,fallbackReason}=initialized;
    if(engine.remote){
        wasm={create_gpu:async(_id,backend)=>{
            await engine.sync();let transfer=[],replacement;
            if(backend==='webgl2'){replacement=canvas.transferControlToOffscreen();transfer=[replacement];}
            const reply=await runtimeClient.call('gpu',{backend,canvas:replacement},transfer,{control:true});
            return {snapshot:reply.snapshot,free(){}};
        }};
    }
    const activeBackend=engine.backend();
    engine.set_profiling(trace.enabled);
    // Apply metadata before exposing input or submitting boot presentation.
    engine.host_event('profile',stringifyPlayerData(startup.profile));
    if(startup.profileValues)engine.host_event('profile_values',stringifyPlayerData(startup.profileValues));
    for(const failure of startup.failures)engine.host_event('persistence_read_failed',stringifyPlayerData(failure));
    if(engine.remote)await engine.sync();
    preferences=JSON.parse(engine.state()).preferences;
    observe('wasm_initialized');
    observe('engine_created',{start_us:createStart,end_us:String(Math.round(performance.now()*1000))});
    document.title=release.title;
    const historyStyle=document.createElement('style');
    historyStyle.textContent=`#nir-history-button,#nir-dev-banner{position:fixed;z-index:4;font:14px system-ui,sans-serif}#nir-history-button{top:12px;right:12px;max-width:45vw;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;padding:8px 12px;background:#d4ba7a;color:#10252b;border:0;border-radius:4px;cursor:pointer}#nir-dev-banner{top:12px;left:12px;padding:7px 10px;background:#9d6823;color:#fff;border-radius:4px;pointer-events:none}#nir-history-panel{position:fixed;z-index:6;inset:0;background:#071b20ed;color:#f2f2e9;overflow:auto;padding:clamp(18px,4vw,48px);font:15px system-ui,sans-serif}#nir-history-panel[hidden]{display:none}#nir-history-panel .nir-history-inner{max-width:900px;margin:auto}#nir-history-panel h2{font-size:24px;font-weight:500}#nir-history-panel button{margin:3px;padding:8px 12px;background:#d4ba7a;color:#10252b;border:0;border-radius:3px;cursor:pointer}#nir-history-panel button:disabled{opacity:.45;cursor:default}#nir-history-panel table{width:100%;border-collapse:collapse}#nir-history-panel th,#nir-history-panel td{text-align:left;border-bottom:1px solid #5d7474;padding:10px 6px;vertical-align:top}#nir-history-panel code{overflow-wrap:anywhere}#nir-history-panel .nir-history-status{min-width:115px}`;
    document.head.append(historyStyle);
    const historyButton=document.createElement('button');historyButton.id='nir-history-button';historyButton.type='button';historyButton.hidden=true;historyButton.textContent='发行存档 / Save history';historyButton.onclick=()=>{if(state().screen==='Story')void action({type:'menu'});historyPanel.hidden=false;document.querySelector('#actions').inert=true;historyClose.focus();void refreshHistory();};document.body.append(historyButton);
    let devBanner=null;
    if(release.profile==='dev'){devBanner=document.createElement('div');devBanner.id='nir-dev-banner';devBanner.textContent='Development build · saves are isolated';document.body.append(devBanner);}
    const historyPanel=document.createElement('section');historyPanel.id='nir-history-panel';historyPanel.hidden=true;historyPanel.setAttribute('role','dialog');historyPanel.setAttribute('aria-modal','true');historyPanel.setAttribute('aria-label','Save history');
    const historyInner=document.createElement('div');historyInner.className='nir-history-inner';
    const historyHeading=document.createElement('h2');historyHeading.textContent='发行存档 / Save history';historyInner.append(historyHeading);
    const historyClose=document.createElement('button');historyClose.type='button';historyClose.textContent='关闭 / Close';historyClose.onclick=()=>{historyGeneration++;historyReadController?.abort();historyPanel.hidden=true;document.querySelector('#actions').inert=false;historyButton.focus();};historyInner.append(historyClose);
    const historyRefresh=document.createElement('button');historyRefresh.type='button';historyRefresh.id='nir-history-refresh';historyRefresh.style.minHeight='44px';historyRefresh.textContent='刷新 / Refresh';historyRefresh.onclick=()=>void refreshHistory();historyInner.append(historyRefresh);
    const historyMessage=document.createElement('p');historyMessage.setAttribute('role','status');historyInner.append(historyMessage);
    const historyTable=document.createElement('table'),historyBody=document.createElement('tbody');historyTable.innerHTML='<thead><tr><th>Release / version</th><th>Slot</th><th>Saved</th><th>Status</th><th>Actions</th></tr></thead>';historyTable.append(historyBody);historyInner.append(historyTable);historyPanel.append(historyInner);document.body.append(historyPanel);
    historyPanel.addEventListener('keydown',e=>{e.stopPropagation();if(e.key==='Escape'){e.preventDefault();historyClose.click();}else if(e.key==='Tab'){const buttons=[...historyPanel.querySelectorAll('button:not(:disabled)')];const first=buttons[0],last=buttons.at(-1);if(e.shiftKey&&document.activeElement===first){e.preventDefault();last.focus();}else if(!e.shiftKey&&document.activeElement===last){e.preventDefault();first.focus();}}});
    const historyReadPool=new WorkPool(2);
    async function inspectHistoryEntry(entry,signal){
      return historyReadPool.run(async()=>{
        const record=await storage.run(db=>readSaveRecord(db,entry.key,{signal}));
        signal?.throwIfAborted();
        let error=null;
        try{
            if(!validHistoryKey(entry.key))throw new Error('E_SAVE_IDENTITY: invalid stored key');
            if(await inspectSaveRecord(record,entry.key,(json,key)=>inspectSlot(json,key,{signal,timeout:0}))===null)throw new Error('E_SAVE_MISSING');
        }catch(e){signal?.throwIfAborted();if(/\bE_WORKER_/.test(String(e)))throw e;error=String(e).slice(0,1024);}
        signal?.throwIfAborted();
        return {record,error};
      },signal,{priority:'required',phase:'saved_release'});
    }
    async function downloadSaveRecord(entry){
        const generation=historyGeneration,signal=historyReadController?.signal;
        const key=entry.key,digest=typeof key[2]==='string'&&releaseDigestPattern.test(key[2])?key[2].slice(0,12):'unknown';
        const slot=Number.isInteger(key[3])?key[3]:'unknown';
        try{
            // Re-read and inspect at the click. Never export an earlier checked
            // copy after another tab replaces the persisted record.
            const {record,error}=await checkSavedRelease(signal=>inspectHistoryEntry(entry,signal),{signal}),readable=!error;
            if(generation!==historyGeneration||disposed)return;
            const json=saveHistoryExport({key,record},readable);
            const url=URL.createObjectURL(new Blob([json],{type:'application/json'}));
            const a=document.createElement('a');a.href=url;a.download=`${release.game_id}-${digest}-slot-${slot}.${readable?'nir-save':'nir-save-record'}.json`;a.click();setTimeout(()=>URL.revokeObjectURL(url),1000);
        }catch(error){if(generation===historyGeneration&&!disposed)historyMessage.textContent=historyCheckError(error,'Unable to export this record');}
    }
    function historyCheckError(error,prefix){return error?.operation==='release_check'?'检查超时，请刷新 / Check timed out; refresh to retry.':`${prefix}: ${error}`;}
    let historyGeneration=0,historyReadController=null;
    async function refreshHistory(){
        const generation=++historyGeneration;historyReadController?.abort();
        const controller=new AbortController();historyReadController=controller;
        const signal=controller.signal;historyRefresh.disabled=true;
        historyMessage.textContent='Checking saved releases…';historyBody.replaceChildren();
        const statuses=[];
        try{
          await checkSavedRelease(async checkSignal=>{
            const entries=await storage.run(db=>listHistoryRecords(db,release.game_id,release.profile,{signal:checkSignal}));
            if(generation!==historyGeneration||disposed)return;
            if(!entries.length){historyMessage.textContent='No saves for this game and profile.';return;}
            historyMessage.textContent=`${entries.length} saved slot${entries.length===1?'':'s'}`;
            const verifiedTargets=new Map(),checks=[];
            const targets=new SharedRequests(async(digest,signal)=>{
                if(verifiedTargets.has(digest))return verifiedTargets.get(digest);
                const target=await validateHistoryTarget({releaseRoot,digest,gameId:release.game_id,profile:release.profile,sha256,signal});
                if(target.available)verifiedTargets.set(digest,target);
                return target;
            });
            for(const entry of entries){
                const {key,savedAt,version}=entry;
                const row=document.createElement('tr');
                const identity=document.createElement('td');const digest=document.createElement('code');digest.textContent=typeof key[2]==='string'?key[2].slice(0,256):'Unknown release';
                identity.append(document.createTextNode(`${version} · `),digest);
                const slot=document.createElement('td');slot.textContent=Number.isInteger(key[3])&&key[3]>=0&&key[3]<3?String(key[3]+1):'Unknown';
                const date=document.createElement('td');date.textContent=savedAt?new Date(savedAt).toLocaleString():'Unknown';
                const status=document.createElement('td');status.className='nir-history-status';status.textContent='Checking…';
                statuses.push(status);
                const actions=document.createElement('td');const exportButton=document.createElement('button');exportButton.type='button';exportButton.textContent='Export';exportButton.disabled=true;actions.append(exportButton);
                const openButton=document.createElement('button');openButton.type='button';openButton.textContent='Open release';openButton.disabled=true;actions.append(openButton);
                row.append(identity,slot,date,status,actions);historyBody.append(row);
                checks.push(async()=>{
                    try{
                      await checkSavedRelease(async rowSignal=>{
                        const {error}=await inspectHistoryEntry(entry,rowSignal);
                        if(generation!==historyGeneration||disposed)return;
                        exportButton.textContent=error?'Export record':'Export';exportButton.disabled=false;
                        exportButton.onclick=async()=>{
                            if(generation!==historyGeneration||disposed)return;
                            exportButton.disabled=true;
                            try{await downloadSaveRecord(entry);}
                            finally{if(generation===historyGeneration&&!disposed)exportButton.disabled=false;}
                        };
                        if(error){status.textContent=`Unreadable save: ${error}`;return;}
                        const target=await targets.get(key[2],rowSignal);
                        if(generation!==historyGeneration||disposed)return;
                        status.textContent=target.status;
                        if(target.available){
                            openButton.disabled=false;openButton.onclick=async()=>{
                                openButton.disabled=true;
                                try{
                                    const latest=await checkSavedRelease(signal=>inspectHistoryEntry(entry,signal),{signal});
                                    if(generation!==historyGeneration||disposed)return;
                                    if(latest.error){status.textContent=`Unreadable save: ${latest.error}`;exportButton.textContent='Export record';return;}
                                    const targetUrl=new URL(target.url);targetUrl.search=location.search;location.assign(targetUrl.href);
                                }catch(error){if(generation===historyGeneration&&!disposed){status.textContent=historyCheckError(error,'Unable to check save');openButton.disabled=false;}}
                            };
                        }
                      },{signal:checkSignal});
                    }catch(error){if(generation===historyGeneration&&!disposed)status.textContent=historyCheckError(error,'Unable to check save');}
                });
            }
            // Only lightweight metadata/DOM is retained for the whole list.
            // Large snapshot reads and Worker inspection have two consumers.
            let next=0;
            const checkNext=async()=>{while(next<checks.length&&!checkSignal.aborted&&generation===historyGeneration&&!disposed)await checks[next++]();};
            await Promise.all([checkNext(),checkNext()]);
          },{signal,timeoutMs:10000});
        }catch(error){if(generation===historyGeneration&&!disposed){historyMessage.textContent=historyCheckError(error,'Unable to read save history');for(const status of statuses)if(status.textContent==='Checking…')status.textContent=historyCheckError(error,'Unable to check save');}}
        finally{if(generation===historyGeneration&&!disposed)historyRefresh.disabled=false;}
    }
    const buffers=new Map(),voices=new Map(),bytesCache=new Map(),imageStaging=new Map(),assetDescriptors=new Map(),requests=new SharedRequests((id,signal)=>fetchObject(id,signal,observe)),preparations=new Map(),contentPreparations=new Map(),contentStaging=new ContentStagingBudget(CONTENT_STAGING_LIMIT,syncContentStagingMetrics);
    const mediaPeaks={encoded_cache_bytes:0,decoded_audio_bytes:0,image_staging_bytes:0};
    function mediaMemory() {
        const current=mediaMemorySnapshot(bytesCache,buffers,voices,imageStaging);
        if(trace.enabled)for(const key of Object.keys(mediaPeaks))mediaPeaks[key]=Math.max(mediaPeaks[key],current[key]);
        return {...current,peaks:trace.enabled?{...mediaPeaks}:null};
    }
    let raf=0,lastTime=null,sequence=0,disposed=false,recovering=false,pendingFocusId=null;
    const inbox=new OwnerInbox(256,128,8,observe,()=>traceContext),resourcePool=new WorkPool(4),decodePool=new WorkPool(2),uploadPool=new WorkPool(1);
    const decodeRequests=new SharedRequests((id,signal,source)=>decodePool.run(()=>audio.decodeAudioData(source.bytes.slice(0)),signal,{priority:source.priority,group:source.group,deadline:source.deadline,phase:'audio_decode'}));
    let ownerTimer=null,pendingElapsed=new DomainElapsed(),wakeRequestedAt=null,pendingWakeWaitStartUs=null,pendingWakeWaitEndUs=null,cachedHostState=null;
    function invalidateHostState(){cachedHostState=null;}
    function mutateEngine(run) {try{return run();}finally{invalidateHostState();}}
    function hostEvent(kind,value) {mutateEngine(()=>engine.host_event(kind,typeof value==='string'?value:stringifyPlayerData(value)));}
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
            if(!inbox.push(()=>{try{fn();if(engine.remote)pendingDeliveries.push(resolve);else resolve(true);}catch(e){reportHostFailure(e,'dispatch');resolve(false);}},kind,()=>resolve(false),group)){
                fail('E_EVENT_QUEUE: host inbox admission limit');resolve(false);dispose();return;
            }
            wake();
        });
    }
    function post(slot,fn,terminal=true){
        if(disposed){slot.cancel();return Promise.resolve(false);}
        const done=slot.post(()=>{try{return fn();}catch(e){if(slot.kind==='control')throw e;reportHostFailure(e,'completion');return true;}},{terminal});wake();return done;
    }
    function request(work,success,failure,{kind='completion',group=null,replace=false,pending}={}){
        if(replace)inbox.cancelGroup(group);
        const slot=inbox.reserve(kind,group);
        if(!slot){failure(new Error('E_REQUEST_CAPACITY: no terminal slot'));return null;}
        void dispatchOwnerRequest(work,slot,{post,success,failure,pending});
        return slot;
    }
    let touchInput=matchMedia('(pointer: coarse)').matches;
    engine.set_touch_input(touchInput);
    if(engine.remote){engine.onUpdate=()=>{invalidateHostState();wake();};runtimeClient.onFailure=error=>{if(!disposed){fail(error);dispose();}};}
    if(assetClient)assetClient.onFailure=error=>{if(!disposed){fail(error);dispose();}};
    let metadataRetryPending=false,metadataRetryController=null;
    const metadataPanel=document.createElement('div');metadataPanel.id='nir-storage-recovery';metadataPanel.hidden=true;
    Object.assign(metadataPanel.style,{position:'fixed',bottom:'12px',right:'12px',zIndex:'999',padding:'10px',borderRadius:'8px',background:'#182535',color:'#fff',font:'14px sans-serif',maxWidth:'min(420px, calc(100vw - 24px))',boxSizing:'border-box'});
    const metadataStatus=document.createElement('span');metadataStatus.id='nir-storage-status';metadataStatus.setAttribute('role','status');metadataStatus.setAttribute('aria-live','polite');
    const metadataRetryButton=document.createElement('button');metadataRetryButton.id='nir-storage-retry';metadataRetryButton.type='button';
    Object.assign(metadataRetryButton.style,{marginLeft:'8px',padding:'8px 12px',minHeight:'44px',borderRadius:'6px',font:'14px sans-serif'});
    metadataPanel.append(metadataStatus,metadataRetryButton);document.body.append(metadataPanel);
    metadataPanel.addEventListener('keydown',e=>e.stopPropagation());
    const recoverableMetadataKinds=()=>[...new Set([...metadataReadFailures.keys(),...metadataWriteFailures])].filter(kind=>!metadataPendingWrites.has(kind));
    metadataRetryButton.onclick=e=>{
        e.stopPropagation();if(disposed||metadataRetryPending||!recoverableMetadataKinds().length)return;
        metadataRetryPending=true;updateMetadataRecovery();
        void deliver(retryMetadata,'control').then(accepted=>{if(!accepted&&!disposed){metadataRetryPending=false;updateMetadataRecovery();}});
    };
    function updateMetadataRecovery() {
        if(disposed)return;
        const chinese=state().locale?.startsWith('zh'),unread=metadataReadFailures.size>0,unsaved=metadataWriteFailures.size>0;
        // Keep the dialogue area clear. Recovery stays available on the title
        // and menu pages, including a request begun before returning to Story.
        metadataPanel.hidden=state().screen==='Story'||(!unread&&!unsaved&&!metadataRetryPending);
        const message=metadataRetryPending?(chinese?'正在恢复设置与已读进度…':'Restoring settings and read progress…'):
            metadataPendingWrites.size?(chinese?'仍在确认设置或已读进度的保存结果，当前阅读已保留。':'Still confirming saved settings or read progress. Your current reading is kept.'):
            unread?(chinese?'设置或已读进度尚未恢复，当前阅读已保留。':'Some settings or read progress could not be restored. Your current reading is kept.'):
            (chinese?'设置或已读进度尚未保存，当前阅读已保留。':'Some settings or read progress could not be saved. Your current reading is kept.');
        if(metadataStatus.textContent!==message)metadataStatus.textContent=message;
        const label=chinese?'重试':'Retry';if(metadataRetryButton.textContent!==label)metadataRetryButton.textContent=label;metadataRetryButton.disabled=metadataRetryPending||!recoverableMetadataKinds().length;
        if(metadataPanel.hidden&&metadataPanel.contains(document.activeElement))document.activeElement.blur();
    }
    function retryMetadata() {
        if(disposed)return;
        const kinds=recoverableMetadataKinds();
        const controller=new AbortController();metadataRetryController=controller;
        const finish=()=>{if(metadataRetryController===controller){metadataRetryController=null;metadataRetryPending=false;}updateMetadataRecovery();};
        const failed=(kind,error)=>{metadataReadFailures.set(kind,String(error));hostEvent('persistence_read_failed',{kind,message:String(error)});};
        request(()=>storage.run(db=>Promise.all(kinds.map(async kind=>{
            try{return {kind,value:await readMetadataRecord(db,kind,sharedKey,{signal:controller.signal})};}
            catch(error){return {kind,error};}
        }))),rows=>{
            try{for(const row of rows){
                if(row.error){failed(row.kind,row.error);continue;}
                metadataReadFailures.delete(row.kind);
                if(row.kind==='preferences')hostEvent('preferences_recovered',initialRuntimePreferences(program,row.value,navigator.languages||[],matchMedia('(prefers-reduced-motion: reduce)').matches));
                else if(row.kind==='profile_values'){hostEvent('profile_values_recovered',row.value);persistence.retry('profile_values');}
                else {hostEvent('profile_recovered',row.value);persistence.retry('profile');}
            }}finally{finish();}
        },error=>{try{for(const kind of kinds)failed(kind,error);}finally{finish();}},{group:'metadata-retry'});
    }
    let audioBlocked=false;
    const audioRecoveryPanel=document.createElement('div');audioRecoveryPanel.id='nir-audio-recovery';audioRecoveryPanel.hidden=true;
    Object.assign(audioRecoveryPanel.style,{position:'fixed',left:'50%',top:'16px',transform:'translateX(-50%)',zIndex:'1000',padding:'12px',borderRadius:'8px',background:'#182535',color:'#fff',border:'1px solid #b6c8dd',font:'16px sans-serif',width:'min(480px, calc(100vw - 32px))',boxSizing:'border-box',overflowY:'auto',overscrollBehavior:'contain'});
    const audioResumeButton=document.createElement('button');
    audioResumeButton.id='nir-audio-resume';audioResumeButton.type='button';audioResumeButton.hidden=true;
    Object.assign(audioResumeButton.style,{padding:'12px 18px',minHeight:'44px',borderRadius:'8px',background:'#182535',color:'#fff',border:'1px solid #b6c8dd',font:'16px sans-serif'});
    audioResumeButton.onclick=e=>{e.stopPropagation();unlock();};
    const audioRecoveryStatus=document.createElement('p');audioRecoveryStatus.id='nir-audio-recovery-status';audioRecoveryStatus.setAttribute('role','status');audioRecoveryStatus.style.margin='8px 0';
    const audioRecoverySaves=document.createElement('button');audioRecoverySaves.id='nir-audio-recovery-saves';audioRecoverySaves.type='button';audioRecoverySaves.hidden=true;
    Object.assign(audioRecoverySaves.style,{padding:'12px 18px',minHeight:'44px',borderRadius:'8px',background:'#182535',color:'#fff',border:'1px solid #b6c8dd',font:'16px sans-serif'});
    audioRecoverySaves.onclick=e=>{e.stopPropagation();if(!disposed&&state().screen==='Story'&&!state().loading)action({type:'saves'},state());};
    audioRecoveryPanel.append(audioResumeButton,audioRecoveryStatus,audioRecoverySaves);document.body.append(audioRecoveryPanel);
    function checkAudioOutput() {
        if(disposed)return;
        const blocked=audioDomains.blocked(voices.values());
        if(audioBlocked!==blocked) {
            audioBlocked=blocked;
            mutateEngine(()=>engine.audio_blocked(blocked));
            observe(blocked?'audio_output_wait':'audio_output_ready');
        }
        audioRecoveryPanel.hidden=audioResumeButton.hidden=!blocked||document.hidden;
        const chinese=state().locale?.startsWith('zh'),recovery=audioDomains.recoveryState(voices.values()),status=recovery.status;
        audioResumeButton.disabled=!recovery.canRetry;
        const label=recovery.canRetry?(chinese?'点击恢复声音':'Tap to resume sound'):
            status==='failed'?(chinese?'声音不可用':'Sound unavailable'):(chinese?'正在恢复声音…':'Resuming sound…');
        if(audioResumeButton.textContent!==label)audioResumeButton.textContent=label;
        const message=status==='failed'?(recovery.canRetry?
            (chinese?'声音尚未恢复。可重试；若仍无声，请先保存进度，再重新打开页面。':'Sound has not resumed. Try again; if it stays unavailable, save your progress before reopening this page.'):
            (chinese?'声音不可用。请先保存进度，再重新打开页面。':'Sound is unavailable. Save your progress before reopening this page.')):
            status==='waiting'?(chinese?'仍在等待声音恢复，阅读位置保持不变。你可以先保存进度。':'Still waiting for sound. Your reading position is held. You can save your progress.'):
            status==='pending'?(chinese?'正在恢复声音，阅读位置保持不变。':'Resuming sound. Your reading position is held.'):
            (chinese?'点击恢复声音后继续阅读，本次点击不会翻页。':'Resume sound to continue. This tap will keep your place.');
        if(audioRecoveryStatus.textContent!==message)audioRecoveryStatus.textContent=message;
        audioRecoverySaves.hidden=!['failed','waiting'].includes(status)||state().screen!=='Story';audioRecoverySaves.disabled=state().loading;
        const saveLabel=chinese?'打开存档':'Open saves';if(audioRecoverySaves.textContent!==saveLabel)audioRecoverySaves.textContent=saveLabel;
    }
    // Coalesce state changes from all four contexts into one owner wake.
    audioDomains.changed=wake;
    function unlock() { audioDomains.unlock(); }
    function stopVoice(id) {
        const v=voices.get(id);if(!v)return;v.stopped=true;v.slot.cancel();
        try{v.source?.stop();}catch{}
        // stop() awaits a render quantum. A suspended context may not run
        // another one until the next user gesture, so release the retired
        // source's PCM and callback without resuming the paused audio graph.
        if(v.source){v.source.onended=null;v.source.buffer=null;v.source.disconnect();}
        v.gain?.disconnect();v.envelope?.disconnect();voices.delete(id);
    }
    function playVoice(c) {
        const key=audioVoiceKey(c),context=audioDomains.context(c.domain,c.bus);
        stopVoice(key);
        const failed=e=>mutateEngine(()=>engine.audio_failed_in(c.domain,c.task,c.session,String(e)));
        const slot=inbox.reserve('completion',`audio:${key}`,{session:c.session,task:c.task});
        if(!slot){failed('E_REQUEST_CAPACITY');return;}
        const buffer=buffers.get(c.asset);
        if(!buffer){post(slot,()=>failed('E_AUDIO_BUFFER'));return;}
        let v;
        try {
            if(c.loop_region&&!c.looped)throw new Error('E_AUDIO_LOOP: region on finite voice');
            const loop=c.loop_region?audioLoopPlayback(c.loop_region,c.position_us,buffer.sampleRate,buffer.length,assetDescriptors.get(c.asset)?.duration_us):null;
            const source=context.createBufferSource(),gain=context.createGain(),envelope=context.createGain();source.buffer=buffer;source.loop=c.looped;envelope.gain.value=c.envelope??1;
            if(loop)source.loopStart=loop.startFrame/buffer.sampleRate;
            if(c.looped)source.loopEnd=audioLoopEndSeconds(loop?loop.endFrame:buffer.length,buffer.sampleRate);
            gain.gain.value=(c.gain??1)*(preferences[`${c.bus}_volume`]??.5)*(c.bus==='voice'?characterVoiceGain(preferences,c.character):1);source.connect(envelope).connect(gain).connect(context.destination);
            v={domain:c.domain,session:c.session,task:c.task,position:Number(c.position_us)/1e6,context,source,gain,envelope,slot,eventGain:c.gain??1,bus:c.bus,character:c.character??'',asset:c.asset,stopped:false};voices.set(key,v);
            let offset=loop?loop.offsetFrame/buffer.sampleRate:Number(c.position_us)/1e6;
            if(!loop){if(c.looped)offset%=buffer.duration;else offset=Math.min(offset,Math.max(0,buffer.duration-.001));}
            source.onended=()=>{if(!v.stopped&&!c.looped)post(slot,()=>{
                if(voices.get(key)!==v||v.stopped)return;
                voices.delete(key);source.onended=null;source.buffer=null;source.disconnect();gain.disconnect();envelope.disconnect();
                mutateEngine(()=>engine.audio_ended_in(c.domain,c.task,c.session));
            });};
            source.start(0,offset);v.started=context.currentTime;metrics.audioStarts++;
        } catch(e){
            if(v){v.stopped=true;v.source.onended=null;v.source.buffer=null;v.source.disconnect();v.gain.disconnect();v.envelope.disconnect();voices.delete(key);}
            post(slot,()=>failed(e));
        }
    }
    function sampleAudioPositions() {
        const groups=new Map();
        for(const v of voices.values()) {
            if(v.domain!=='story'||v.stopped||v.started===undefined)continue;
            let position=v.position+Math.max(0,v.context.currentTime-v.started);
            if(!v.source.loop)position=Math.min(position,v.source.buffer.duration);
            const positions=groups.get(v.session)||[];
            positions.push({task:v.task,position_us:String(Math.round(position*1e6)),envelope:envelopePosition(v.envelopePlan,v.context.currentTime)});
            groups.set(v.session,positions);
        }
        for(const [session,positions] of groups)mutateEngine(()=>engine.audio_positions_in('story',session,JSON.stringify(positions)));
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
        bytesCache.set(a.object,bytes);if(trace.enabled)mediaMemory();return bytes;
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
                        signal.throwIfAborted();validateDecodedAudio(descriptor,buffer,audio.sampleRate);
                        buffers.set(id,buffer);if(trace.enabled)mediaMemory();
                    }
                    observe('audio_decode_ready',context);
                    // Resource readiness is independent of device unlock.
                    // Sources may be scheduled on a suspended context; a user
                    // gesture unlocks output separately. Never hold a page's
                    // resource lease waiting for resume() to settle.
                }
                let decoded=null;
                if(descriptor.kind==='image'&&assetClient){
                    node.stage=stage='image_decode';
                    decoded=await decodePool.run(()=>{const copy=bytes.slice(0);return assetClient.call('decode',{bytes:copy,width:descriptor.width,height:descriptor.height},[copy],{signal,timeout:0});},signal,{priority:job.priority,group:c.request,deadline:job.deadline,phase:'image_decode'});
                    imageStaging.set(`${c.request}:${id}`,decoded.pixels);if(trace.enabled)mediaMemory();
                    threading.asset_wasm_memory_bytes=decoded.wasmMemoryBytes;signal.throwIfAborted();observe('decode_allocate',{...context,bytes:decoded.pixels.byteLength,start_us:String(Number(decoded.start_us)+(assetClient.clockOffsetUs||0)),end_us:String(Number(decoded.end_us)+(assetClient.clockOffsetUs||0))});observe('image_decode_ready',context);
                }
                node.stage=stage='decode_upload';
                let complete=false;
                while(!complete&&!signal.aborted&&!disposed&&slot.state==='pending'){
                    // Re-admit each bounded owner step. A synchronous PNG
                    // decode remains atomic inside its one owner callback.
                    await uploadPool.run(()=>post(slot,()=>{
                        if(signal.aborted)return true;
                        try {complete=mutateEngine(()=>{if(decoded){const pixels=decoded.pixels?new Uint8Array(decoded.pixels):new Uint8Array();const done=engine.resource_decoded(c.request,id,decoded.width,decoded.height,pixels);decoded.pixels=null;return done;}return engine.resource(c.request,id,new Uint8Array(bytes));});if(complete){node.stage='ready';observe('ordered_use_ready',context);}return complete;}
                        catch(e){metrics.resourceFailures++;failed(e,id,stage,'E_RESOURCE_DECODE_UPLOAD');return true;}
                    },done=>done),signal,{priority:job.priority,group:c.request,deadline:job.deadline,phase:'decode_upload'});
                }
            }catch(e){
                node.stage='failed';
                if(!signal.aborted&&!disposed){metrics.resourceFailures++;await post(slot,()=>failed(e,id,stage,typeof e?.code==='string'?e.code:stage==='fetch'?'E_RESOURCE_FETCH':'E_AUDIO_DECODE'));}
            } finally {imageStaging.delete(`${c.request}:${id}`);if(signal.aborted||disposed)slot.cancel();}
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
        return request(()=>storage.run(db=>listSaveRecords(db,release.game_id,release.profile,releaseDigest,inspectSlot)),
            rows=>hostEvent('slots',rows),e=>hostEvent('load_failed',String(e)),{group:'slots',replace:true});
    }
    function inspectSlot(json,key,options) {
        const value={json,slot:key[3],release:key[2],game:key[0]};
        return runtimeClient?runtimeClient.call('inspect-save',value,[],options):originalWasm.inspect_save_slot(json,value.slot,value.release,value.game);
    }
    function save(c) {
        const context={request:c.job,session:state().session,operation:'save'};observe('storage_started',context);
        const metadata={gameId:release.game_id,profile:release.profile,releaseDigest,slot:c.slot,version:release.version||'',title:release.title||''};
        return request(onPending=>storage.run(db=>commitSaveRecord(db,slotKey(c.slot),c.envelope,metadata,c.expected_revision,inspectSlot,{onPending})),()=>{observe('storage_committed',context);hostEvent('saved',{job:c.job,slot:c.slot,revision:c.envelope.revision});if(historyPanel&&!historyPanel.hidden)void refreshHistory();},
            e=>{const code=e?.name==='QuotaExceededError'?'E_STORAGE_QUOTA':String(e).includes('E_SAVE_CONFLICT')?'E_SAVE_CONFLICT':'E_STORAGE';observe('diagnostic',{...context,domain:'storage',code});hostEvent('save_failed',{job:c.job,code,message:String(e)});},{group:`save:${c.job}`,pending:error=>{observe('storage_pending',context);hostEvent('save_pending',{job:c.job,message:String(error)});}});
    }
    function load(command) {
        const session=state().session;
        return request(()=>storage.run(async db=>{const key=slotKey(command.slot),s=await readSaveRecord(db,key);if(s===undefined)throw new Error('E_SAVE_MISSING');await inspectSaveRecord(s,key,inspectSlot);return s.envelope;}),
            value=>{if(state().session===session)hostEvent('slot_loaded',{job:command.job,envelope:value});},
            e=>{if(state().session===session)hostEvent('slot_load_failed',{job:command.job,message:String(e)});},{group:'load',replace:true});
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
    const persistence=new PersistenceWrites({request,
        writePreferences:(value,onPending)=>{
            // Temporary defaults are not a recovered disk snapshot. Hold user
            // edits until a successful read can merge the untouched fields.
            if(metadataReadFailures.has('preferences'))throw new Error('E_PREFERENCES_UNREAD: restore preferences before saving changes');
            return storage.run(db=>writePreferencesRecord(db,sharedKey,value,{onPending}));
        },
        mergeProfile:(keys,onPending)=>storage.run(db=>mergeProfileRecord(db,sharedKey,keys,{onPending})),
        writeProfileValues:(values,onPending)=>storage.run(db=>writeMetadataRecord(db,'profile_values',sharedKey,values,{onPending})),
        stored:kind=>{metadataPendingWrites.delete(kind);metadataWriteFailures.delete(kind);hostEvent('persistence_stored',{kind});updateMetadataRecovery();},
        failed:(kind,error)=>{if(error?.code==='E_STORAGE_UNCERTAIN')metadataPendingWrites.add(kind);else metadataPendingWrites.delete(kind);metadataWriteFailures.add(kind);hostEvent('persistence_failed',{kind,message:String(error)});updateMetadataRecovery();},
    });
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
            case 'audio_start':if(c.session===state().session)playVoice(c);break;
            case 'audio_character':{
                const voice=voices.get(audioVoiceKey(c));
                if(voice){voice.character=c.character;voice.gain.gain.value=voice.eventGain*(preferences[`${voice.bus}_volume`]??.5)*(voice.bus==='voice'?characterVoiceGain(preferences,voice.character):1);}
                break;
            }
            case 'audio_envelope':{
                const v=voices.get(audioVoiceKey(c));if(!v)break;
                const now=v.context.currentTime,param=v.envelope.gain;
                param.cancelScheduledValues(now);param.setValueAtTime(c.from,now);
                const duration=Number(c.duration_us)/1e6;
                v.envelopePlan={owner:c.owner,base:Number(c.elapsed_us),at:now,duration:Number(c.duration_us)};
                if(duration>0)param.linearRampToValueAtTime(c.to,now+duration);
                else param.setValueAtTime(c.to,now);
                break;
            }
            case 'audio_stop':stopVoice(audioVoiceKey(c));break;
            case 'audio_reset':audioDomains.route(c.domain);for(const [id,v] of [...voices])if(v.domain===c.domain)stopVoice(id);break;
            case 'audio_pause':audioDomains.setPaused(c.domain,c.paused);break;
            case 'audio_bus_pause':audioDomains.setBusPaused(c.domain,c.bus,c.paused);break;
            case 'save':save(c);break;case 'load':load(c);break;case 'list_saves':listSaves();break;
            case 'apply_preferences':preferences=c.preferences;for(const v of voices.values())v.gain.gain.value=v.eventGain*(preferences[`${v.bus}_volume`]??.5)*(v.bus==='voice'?characterVoiceGain(preferences,v.character):1);break;
            case 'persist_preferences':preferences=c.preferences;for(const v of voices.values())v.gain.gain.value=v.eventGain*(preferences[`${v.bus}_volume`]??.5)*(v.bus==='voice'?characterVoiceGain(preferences,v.character):1);persistence.submit('preferences',preferences);break;
            case 'persist_profile':persistence.submit('profile',c.keys);break;
            case 'persist_profile_values':persistence.submit('profile_values',c.values);break;
            case 'export':{
                let url;
                try {
                    url=URL.createObjectURL(new Blob([c.json],{type:'application/json'}));
                    const a=document.createElement('a');a.href=url;a.download=`${release.game_id}.nir-save.json`;a.click();
                }catch(error){hostEvent('export_failed',{job:c.job,message:String(error)});break;}
                finally {if(url)setTimeout(()=>URL.revokeObjectURL(url),1000);}
                // Browser accepted the download; its external file write or
                // cancellation is not observable from the page.
                hostEvent('exported',{job:c.job});break;
            }
            case 'import':importSave();break;
            case 'trace':if(testMode){traces.push({event:c.event,at:c.at});if(traces.length>4096)traces.splice(0,traces.length-4096);}break;
            default:throw new Error(`E_HOST_PROTOCOL: ${c.type}`);
        }
    }}
    function state(){return cachedHostState||(cachedHostState=JSON.parse(engine.host_state()));}
    function debugState(){return {...JSON.parse(engine.state()),execution:{...threading}};}
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
        if(audioBlocked&&audioBlockedAction(a)){unlock();return Promise.resolve(false);}
        // A delayed query keeps the reading eligibility of its original input.
        if(context.loading&&(['advance','continue','choose','cancel_choice'].includes(a.type)||a.type==='hold_skip'&&a.pressed))return Promise.resolve(false);
        observe('input_received',{sequence:sequence+1,session:context.session});
        unlock();if(a.type==='new_game'&&metrics.startInputMs===null)metrics.startInputMs=performance.now();
        sequence=Math.max(sequence+1,state().sequence+1);const seq=sequence;
        return deliver(()=>{sampleAudioPositions();if(a.type==='title'||a.type==='new_game')inbox.cancelGroup('load');mutateEngine(()=>engine.action(JSON.stringify(a),context.interaction,seq,context.session));},'input');
    }
    let semanticSignature='',announcement='',announcementLocale='';
    function semantics(view) {
        // Semantic rectangles use the same CSS pixels as the canvas controls,
        // including after rotation and at high device pixel ratios. Keep the
        // recovery card below compact controls in the first two toolbar rows;
        // constrain its height so long explanations remain scrollable.
        if(!audioRecoveryPanel.hidden){
            let top=16;
            for(const n of view.nodes)if(n.rect[1]<=112&&n.rect[3]<=64)
                top=Math.max(top,n.rect[1]+n.rect[3]+8);
            audioRecoveryPanel.style.top=`${top}px`;
            audioRecoveryPanel.style.maxHeight=`${Math.max(0,height-top-16)}px`;
        }
        historyButton.hidden=state().screen!=='Menu'||state().menu_depth>0;
        if(!historyButton.hidden){
            const size=historyButton.getBoundingClientRect();
            const position=overlayControlPosition(view.nodes,width,height,size.width,size.height);
            if(position){historyButton.style.right=`${width-position[0]-size.width}px`;historyButton.style.top=`${position[1]}px`;}
            else historyButton.hidden=true;
        }
        document.documentElement.lang=view.locale||'zh-Hans';const s=state();const signature=JSON.stringify([view.nodes,view.locale,view.announcement_locale,s.interaction,s.session]);
        if(signature!==semanticSignature){semanticSignature=signature;const nav=document.querySelector('#actions'),focused=focusIdentity(document.activeElement?.dataset?.action);nav.replaceChildren();for(const n of view.nodes){const b=document.createElement('button');b.textContent=n.label;b.setAttribute('aria-label',n.label);b.lang=n.locale||view.locale||'zh-Hans';b.disabled=!n.enabled;if(['range','scrollbar'].includes(n.value?.type)){b.setAttribute('role','slider');b.setAttribute('aria-valuemin',n.value.min??0);b.setAttribute('aria-valuemax',n.value.max);b.setAttribute('aria-valuenow',n.value.value);if(n.value.type==='scrollbar')b.setAttribute('aria-orientation','vertical');}else if(n.value?.type==='toggle'){b.setAttribute('role','switch');b.setAttribute('aria-checked',String(n.value.checked));}b.dataset.action=JSON.stringify(n.action);b.dataset.control=String(n.id);b.dataset.rect=JSON.stringify(n.rect);const context={interaction:s.interaction,session:s.session};b.onclick=()=>action(n.action,{...context,loading:state().loading});b.onfocus=()=>{deliver(()=>{mutateEngine(()=>engine.focus_control(n.id));mutateEngine(()=>engine.hover(n.rect[0]+n.rect[2]/2,n.rect[1]+n.rect[3]/2));},'input');const ring=document.querySelector('#focus-ring');Object.assign(ring.style,{display:'block',left:`${n.rect[0]+viewportOffset.left}px`,top:`${n.rect[1]+viewportOffset.top}px`,width:`${n.rect[2]}px`,height:`${n.rect[3]}px`});};b.onblur=()=>{document.querySelector('#focus-ring').style.display='none';deliver(()=>mutateEngine(()=>engine.focus_control(undefined)),'input');};nav.append(b);if(focusIdentity(b.dataset.action)===focused)b.focus({preventScroll:true});}}
        if(pendingFocusId!==null){const target=pendingFocusId;pendingFocusId=null;if(target.session===s.session&&target.interaction===s.interaction&&target.screen===s.screen)document.querySelector(`#actions button[data-control="${target.id}"]`)?.focus({preventScroll:true});}
        const spokenLocale=view.announcement_locale||view.locale||'zh-Hans';if(view.announcement&&(view.announcement!==announcement||spokenLocale!==announcementLocale)){announcement=view.announcement;announcementLocale=spokenLocale;const live=document.querySelector('#announcement');live.lang=spokenLocale;live.textContent=announcement;}
    }
    let frameRunning=false,frameAgain=false,lastAudioSample=0,lastSubmittedFrames=0,lastSubmittedDevice=null;
    const pendingDeliveries=[];
    async function frame(now){if(frameRunning){frameAgain=true;return;}frameRunning=true;try{await runFrame(now);}finally{frameRunning=false;for(const resolve of pendingDeliveries.splice(0))resolve(!disposed);if(frameAgain&&!disposed){frameAgain=false;wake();}}}
    async function runFrame(now) {
        if(disposed)return;
        const perfTurn=performanceStats?performanceStats.beginTurn(Math.round(now*1000)):null;
        if(perfTurn&&pendingWakeWaitStartUs!==null){performanceStats.record('wake_wait',pendingWakeWaitStartUs,pendingWakeWaitEndUs,perfTurn);pendingWakeWaitStartUs=pendingWakeWaitEndUs=null;}
        const eventStartUs=perfTurn?Math.round(performance.now()*1000):0;
        try {
            mutateEngine(()=>engine.begin_turn());
            checkDevice();if(disposed){finishPerformanceTurn(perfTurn);return;}
            const outputWasBlocked=audioBlocked;checkAudioOutput();
            const before=state();traceContext={session:before.session,device:before.device};
            const elapsed=lastTime===null?0:Math.max(0,Math.round((now-lastTime)*1000));lastTime=now;
            inbox.drain({canRun:kind=>!disposed&&(!recovering||kind==='control')&&(kind==='control'||engine.pending_events()<112)});await engine.sync?.();invalidateHostState();if(disposed){finishPerformanceTurn(perfTurn);return;}flush();checkAudioOutput();updateMetadataRecovery();await engine.sync?.();invalidateHostState();
            if(recovering){if(inbox.hasControl)wake();if(perfTurn)performanceStats.record('event_handling',eventStartUs,Math.round(performance.now()*1000),perfTurn);finishPerformanceTurn(perfTurn);return;}
            const after=state();
            // checkAudioOutput can release the barrier before `before` is
            // captured. Discard the preceding blocked interval even then;
            // foreground UI keeps its independent elapsed time.
            pendingElapsed.add(elapsed,{hidden:document.hidden,before,after,storyBlocked:outputWasBlocked||audioBlocked});
            if(!inbox.hasInput){const [story,foreground]=pendingElapsed.take();if(!engine.remote||now-lastAudioSample>=100){sampleAudioPositions();lastAudioSample=now;}mutateEngine(()=>engine.tick_domains(story,foreground));}
            mutateEngine(()=>engine.continue_turn());await engine.sync?.();invalidateHostState();flush();
            if(perfTurn)performanceStats.record('event_handling',eventStartUs,Math.round(performance.now()*1000),perfTurn);
            const current=size();if(current.width!==width||current.height!==height||current.dpr!==dpr){({width,height,dpr}=current);if(!engine.remote){canvas.width=Math.round(width*dpr);canvas.height=Math.round(height*dpr);}}
            let view=JSON.parse(await mutateEngine(()=>engine.draw(width,height,dpr)));await engine.sync?.();invalidateHostState();flush();prune();checkAudioOutput();await engine.sync?.();invalidateHostState();
            // Remote draw queues resize and initially returns the previous
            // snapshot. Publish rectangles from the completed owner turn,
            // matching the canvas and its pointer hit tests after rotation.
            if(engine.remote)view=JSON.parse(engine.snapshot.view);
            const semanticStartUs=perfTurn?Math.round(performance.now()*1000):0;semantics(view);
            if(perfTurn)performanceStats.record('semantics',semanticStartUs,Math.round(performance.now()*1000),perfTurn);
            const s=state();
            if(s.frames>0&&(s.frames!==lastSubmittedFrames||s.device!==lastSubmittedDevice)){observe('render_submitted',{session:s.session,device:s.device,frames:s.frames});lastSubmittedFrames=s.frames;lastSubmittedDevice=s.device;}
            metrics.frames=s.frames;metrics.peakResidentBytes=Math.max(metrics.peakResidentBytes,s.resident_bytes);syncContentStagingMetrics(contentStaging);
            metrics.maxTurnUploadBytes=Math.max(metrics.maxTurnUploadBytes||0,s.turn_upload_bytes);metrics.uploadSteps=s.upload_steps;
            metrics.activeRequests=inbox.slots.size;metrics.requestHighWater=inbox.reservedHighWater;metrics.acceptedRequests=inbox.accepted;metrics.completedRequests=inbox.completed;metrics.cancelledRequests=inbox.cancelled;
            metrics.inboxHighWater=inbox.highWater;metrics.maxTurnWork=Math.max(metrics.maxTurnWork||0,s.turn_work);
            if(view.ready){shell.hidden=true;if(metrics.titleMs===null)metrics.titleMs=performance.now()-metrics.boot;if(s.has_dialogue&&metrics.firstLineMs===null){metrics.firstLineMs=performance.now()-metrics.boot;metrics.firstLineAfterStartMs=performance.now()-metrics.startInputMs;metrics.navigationToFirstLineMs=performance.now();observe('first_line_submitted');}}
            if(inbox.length||(!engine.remote&&engine.pending_events()))wake();
            else if(!engine.remote&&engine.needs_clock()&&!document.hidden)schedule();
            finishPerformanceTurn(perfTurn);
        }catch(e){
            try{finishPerformanceTurn(perfTurn);}catch{}
            if(!disposed){fail(e);dispose();}
        }
    }
    function schedule() {if(disposed||recovering)return;if(!raf)raf=requestAnimationFrame(()=>{raf=0;wake();});}
    const setInputMode=touch=>{if(touchInput===touch)return;touchInput=touch;mutateEngine(()=>engine.set_touch_input(touch));schedule();};
    let down=null,barPointer=null;
    // The host retains capture until release even if a page change cancels the
    // shared gesture, so that release cannot become a click in the new page.
    const barGesture=async(phase,x,y)=>{try{const consumed=await mutateEngine(()=>engine.pointer_gesture(phase,x,y,0));schedule();return consumed;}catch(error){if(!disposed)reportHostFailure(error,'gesture');return false;}};
    const cancelPointer=()=>{
        down=null;pointerQueue.clear();
        if(barPointer===null)barGesture(3,0,0);
        if(barPointer!==null){const id=barPointer;barPointer=null;barGesture(3,0,0);if(canvas.hasPointerCapture(id))canvas.releasePointerCapture(id);}
    };
    const scrollAction=(view,delta,page=false)=>view.menu
        ? {type:'menu_history_scroll',...view.menu,input:{type:page?'page':'step',delta}}
        : {type:'scroll',region:view.region,delta};
    const scrollAt=(x,y)=>state().scrolls.find(v=>x>=v.rect[0]&&x<=v.rect[0]+v.rect[2]&&y>=v.rect[1]&&y<=v.rect[1]+v.rect[3]);
    const downAsync=async(e,valid)=>{if(e.button!==0&&e.button!==2)return;pendingFocusId=null;if(document.activeElement?.closest('#actions'))document.activeElement.blur();mutateEngine(()=>engine.focus_control(undefined));unlock();if(e.button===0&&barPointer===null&&await barGesture(0,e.clientX,e.clientY)){if(!valid())return;barPointer=e.pointerId;try{canvas.setPointerCapture(e.pointerId);}catch{}down=null;return;}if(!valid())return;const context=e.context,target=JSON.parse(await engine.pointer_action(e.clientX,e.clientY,e.button));if(!valid())return;down={button:e.button,action:target,context,x:e.clientX,y:e.clientY,scroll:e.button===0?scrollAt(e.clientX,e.clientY):null};};
    const upAsync=async(e,valid)=>{
        if(e.button===0&&barPointer!==e.pointerId)await barGesture(2,e.clientX,e.clientY);
        if(barPointer===e.pointerId&&e.button===0){barGesture(2,e.clientX,e.clientY);const id=barPointer;barPointer=null;if(canvas.hasPointerCapture(id))canvas.releasePointerCapture(id);return;}
        if(!down||e.button!==down.button)return;
        const current=state();
        if(current.session!==down.context.session||current.interaction!==down.context.interaction||current.screen!==down.context.screen){down=null;return;}
        const dy=e.clientY-down.y;
        if(down.scroll&&Math.abs(dy)>30&&Math.abs(e.clientX-down.x)<80){action(scrollAction(down.scroll,dy<0?1:-1),down.context);}
        else if((down.action?.type==='menu_value'&&typeof down.action.value==='number')||Math.hypot(e.clientX-down.x,dy)<20){const hit=JSON.parse(await engine.pointer_action(e.clientX,e.clientY,e.button));if(valid()&&down&&samePointerTarget(down.action,hit))action(hit,{...down.context,loading:down.context.loading||e.context.loading});}
        down=null;
    };
    let hoverTarget;
    const moveAsync=async(e,valid)=>{if(e.pointerType==='mouse')setInputMode(false);if(barPointer===e.pointerId){barGesture(1,e.clientX,e.clientY);return;}const action=await engine.hit(e.clientX,e.clientY);if(!valid())return;const s=state(),target=JSON.stringify([s.session,s.interaction,s.screen,action]);if(target===hoverTarget&&!state().history_scrollbar)return;hoverTarget=target;deliver(()=>mutateEngine(()=>engine.hover(e.clientX,e.clientY)),'input');};
    const pointerQueue=new SerialInputQueue(error=>reportHostFailure(error,'pointer'));
    const pointerEvent=(fn,e)=>{const s=state(),context={session:s.session,interaction:s.interaction,screen:s.screen,loading:s.loading};const data={button:e.button,pointerId:e.pointerId,clientX:e.clientX-viewportOffset.left,clientY:e.clientY-viewportOffset.top,context,pointerType:e.pointerType};pointerQueue.push(fn,data,fn===moveAsync?`move:${e.pointerId}`:null);};
    const onDown=e=>{const blocked=audioBlocked;unlock();if(blocked)cancelPointer();pointerEvent(async(data,valid)=>{
        setInputMode(data.pointerType==='touch');
        await downAsync(data,valid);
        if(blocked&&down&&audioBlockedAction(down.action))down=null;
    },e);};
    const onUp=e=>pointerEvent(upAsync,e);
    const onMove=e=>pointerEvent(moveAsync,e);
    const onLeave=()=>pointerQueue.push(()=>{if(barPointer!==null)return;down=null;hoverTarget=undefined;barGesture(3,0,0);deliver(()=>mutateEngine(()=>engine.hover(-1,-1)),'input');},null);
    const onWheel=(e)=>{const view=scrollAt(e.clientX-viewportOffset.left,e.clientY-viewportOffset.top);if(view&&e.deltaY){e.preventDefault();action(scrollAction(view,e.deltaY>0?1:-1));}};
    const heldControls=new Set();
    const releaseHeld=()=>{if(!heldControls.size)return;heldControls.clear();action({type:'hold_skip',pressed:false});};
    const onEditingFocus=e=>{if(e.target?.isContentEditable||e.target?.matches?.('input,textarea,select'))releaseHeld();};
    const onKeyUp=e=>{if(e.key==='Control'){if(!heldControls.delete(e.code))return;if(!heldControls.size)action({type:'hold_skip',pressed:false});}};
    const handleKey=async(e)=>{
        setInputMode(false);
        if(audioBlocked&&(e.key===' '||e.key==='Enter')){
            if(e.repeat){e.preventDefault();return;}
            if(audioRecoveryPanel.contains(document.activeElement))return;
            const focused=document.activeElement;
            const focusedAction=focused?.tagName==='BUTTON'&&!focused.disabled&&focused.closest('#actions')
                ?JSON.parse(focused.dataset.action):null;
            if(audioRecoveryConsumesKey(audioBlocked,e.key,focusedAction)){e.preventDefault();unlock();return;}
        }
        if(!historyPanel.hidden){if(e.key==='Escape'){e.preventDefault();historyClose.click();}return;}
        if(e.isComposing||document.activeElement?.isContentEditable||document.activeElement?.matches('input,textarea,select'))return;
        if(e.key==='Control'&&!e.repeat&&!e.isComposing&&!e.metaKey&&!e.altKey){
            if(document.activeElement?.isContentEditable||document.activeElement?.matches('input,textarea,select'))return;
            heldControls.add(e.code);action({type:'hold_skip',pressed:true});return;
        }
        // Repeated reader keys must also suppress a focused button's native
        // activation; ignoring the event still lets Enter click it again.
        if(e.repeat&&(e.key===' '||e.key==='Enter')&&document.activeElement?.closest('#actions')){e.preventDefault();return;}
        if(e.isComposing||e.repeat||e.ctrlKey||e.metaKey||e.altKey)return;
        if(e.key==='PageUp'||e.key==='PageDown'){
            const s=state(),view=s.scrolls.find(v=>v.region==='choices')||s.scrolls[0];
            if(view){e.preventDefault();action(scrollAction(view,e.key==='PageDown'?1:-1,true),s);}return;
        }
        if(e.key==='Escape'){e.preventDefault();const s=state();action({type:s.choice_cancellable?'cancel_choice':s.menu_depth>0||['Menu','Settings','Saves','History'].includes(s.screen)?'close':'menu'});return;}
        // Let browser chrome and host-owned controls remain reachable at the
        // boundary; only movement within the player uses its semantic order.
        if(e.key==='Tab'&&document.activeElement?.closest('#actions')) {
            const buttons=[...document.querySelectorAll('#actions button:not(:disabled)')];
            const rect=JSON.parse(document.activeElement.dataset.rect),x=rect[0]+rect[2]/2,y=rect[1]+rect[3]/2;
            const canFollow=state().scrolls.some(v=>x>=v.rect[0]&&x<=v.rect[0]+v.rect[2]&&y>=v.rect[1]&&y<=v.rect[1]+v.rect[3]&&(e.shiftKey?v.offset>0:v.offset<v.max));
            if(!canFollow&&document.activeElement===(e.shiftKey?buttons[0]:buttons.at(-1)))return;
        }
        const valueDirection={ArrowLeft:0,ArrowDown:5,ArrowRight:1,ArrowUp:4,Home:2,End:3}[e.key];
        if(valueDirection!==undefined&&document.activeElement?.closest('#actions')){
            const focused=document.activeElement;
            e.preventDefault();const a=JSON.parse(await engine.control_value_action(Number(focused.dataset.control),focused.dataset.action,valueDirection));
            if(a){e.preventDefault();action(a,state());return;}
        }
        const direction={Tab:e.shiftKey?0:1,ArrowLeft:2,ArrowRight:3,ArrowUp:4,ArrowDown:5}[e.key];
        if(direction!==undefined && (!document.activeElement?.matches('button')||document.activeElement.closest('#actions'))) {
            e.preventDefault();const id=await mutateEngine(()=>engine.navigate_focus(direction));
            if(id!==undefined){e.preventDefault();const s=state();pendingFocusId={id,session:s.session,interaction:s.interaction,screen:s.screen};schedule();}return;
        }
        if(document.activeElement?.tagName==='BUTTON')return;
        if(e.key.toLowerCase()==='h'){e.preventDefault();action({type:'toggle_interface'});return;}
        if(e.key===' '||e.key==='Enter'){e.preventDefault();unlock();const s=state(),a=JSON.parse(await engine.primary_action());if(a)action(a,s);}

    };
    const onKey=e=>{void handleKey(e).catch(error=>reportHostFailure(error,'keyboard'));};
    const onVisibility=()=>{const hidden=document.hidden;if(hidden){releaseHeld();cancelPointer();}deliver(()=>mutateEngine(()=>engine.hidden(hidden)),'control');};
    const onResize=()=>{cancelPointer();refreshSafeArea();hoverTarget=undefined;schedule();};
    let glContextLost=false;
    const onCancel=()=>cancelPointer();
    // Touch releases implicit capture immediately on pointerup. Process that
    // notification after the queued up; it must not cancel a pending tap.
    const onLostCapture=e=>{const id=e.pointerId;pointerQueue.push(()=>{if(barPointer===id)cancelPointer();},null);};
    const onBlur=()=>{releaseHeld();cancelPointer();};
    const onGlLost=e=>{e.preventDefault();glContextLost=true;cancelPointer();deliver(checkDevice,'control');};
    const onContextMenu=e=>e.preventDefault();
    function bindCanvas(){canvas.addEventListener('contextmenu',onContextMenu);canvas.addEventListener('pointermove',onMove);canvas.addEventListener('pointerleave',onLeave);canvas.addEventListener('wheel',onWheel,{passive:false});canvas.addEventListener('pointerdown',onDown);canvas.addEventListener('pointerup',onUp);canvas.addEventListener('pointercancel',onCancel);canvas.addEventListener('lostpointercapture',onLostCapture);canvas.addEventListener('webglcontextlost',onGlLost);}
    function unbindCanvas(){canvas.removeEventListener('contextmenu',onContextMenu);canvas.removeEventListener('pointermove',onMove);canvas.removeEventListener('pointerleave',onLeave);canvas.removeEventListener('wheel',onWheel);canvas.removeEventListener('pointerdown',onDown);canvas.removeEventListener('pointerup',onUp);canvas.removeEventListener('pointercancel',onCancel);canvas.removeEventListener('lostpointercapture',onLostCapture);canvas.removeEventListener('webglcontextlost',onGlLost);}
    bindCanvas();window.addEventListener('keydown',onKey);window.addEventListener('keyup',onKeyUp);window.addEventListener('blur',onBlur);document.addEventListener('focusin',onEditingFocus);document.addEventListener('visibilitychange',onVisibility);window.addEventListener('resize',onResize);
    function checkDevice(){
        if(disposed||recovering)return;
        const validation=engine.gpu_error();if(validation&&!glContextLost){observe('diagnostic',{domain:'render',code:'E_GPU_VALIDATION',operation:'render'});fail(`E_GPU_VALIDATION: ${validation}`);dispose();return;}
        if(!glContextLost&&!engine.device_lost())return;
        cancelPointer();recovering=true;observe('device_loss_detected');metrics.deviceRecoveries++;mutateEngine(()=>engine.begin_recovery());
        const recovery=inbox.reserve('control','device');
        if(!recovery){fail('E_REQUEST_CAPACITY: device recovery');dispose();return;}
        const backend=activeBackend;
        if(backend==='webgl2'){unbindCanvas();replaceCanvas();bindCanvas();glContextLost=false;}
        wasm.create_gpu('stage',backend).then(gpu=>{
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
        return {format:1,worker_resource_timing:workerTimings,execution:{...threading,actual_adapters:(engine.snapshot?.adapters||[]).map(row=>({...row,atMs:row.atMs+(runtimeClient?.clockOffsetUs||0)/1000})),runtime_pending:runtimeClient?.pending.size||0,runtime_high_water:runtimeClient?.highWater||0,asset_pending:assetClient?.pending.size||0,asset_high_water:assetClient?.highWater||0},release:releaseDigest,engine:release.engine.wasm,backend:activeBackend,fallback_reason:fallbackReason,...trace.snapshot(),performance,resource_memory:{media:mediaMemory(),renderer:state().renderer_memory},content_staging:{encoded_bytes:contentStaging.used,peak_encoded_bytes:contentStaging.peak,budget_encoded_bytes:contentStaging.limit,reservations:contentStaging.reservations.size,waiting_demands:contentStaging.waiting.length},host_work:{story_clock:state().story_clock,resource_pool_active:resourcePool.active,resource_pool_waiting:resourcePool.waiting.length,decode_pool_active:decodePool.active,decode_pool_waiting:decodePool.waiting.length,upload_pool_active:uploadPool.active,upload_pool_waiting:uploadPool.waiting.length,shared_fetches:requests.jobs.size,content_jobs:contentPreparations.size,media_jobs:preparations.size,request_slots:inbox.slots.size,pending_owner_callbacks:inbox.length,audio_state:audio.state,audio_paused:audioDomains.paused('story'),audio_domains:audioDomains.snapshot(),pending_media:[...preparations].slice(0,128).map(([request,job])=>({request,session:job.session,priority:job.priority,aborted:job.signal.aborted,stages:[...job.nodes.values()].slice(0,128).map(node=>({asset:node.asset,stage:node.stage}))})),pending_content:[...contentPreparations.values()].slice(0,128).map(job=>({request:job.request,session:job.session,priority:job.priority,state:job.state,staged:job.staged,aborted:job.signal.aborted}))},measurement:{clock:'performance.now; navigation origin',stage_timing:'inclusive, non-additive intervals',gpu_time:'unmeasured',physical_memory:'unmeasured'}};
    };
    if(trace.enabled)window.nirDiagnostics={snapshot:diagnostics,download(){const url=URL.createObjectURL(new Blob([JSON.stringify(diagnostics(),null,2)],{type:'application/json'}));const a=document.createElement('a');a.href=url;a.download='nir-diagnostics.json';a.click();setTimeout(()=>URL.revokeObjectURL(url),1000);}};
    if(testMode)window.__nir={state:debugState,action,metrics,traces,diagnostics,inspectSave:inspectSlot,resourceTimings:()=>[...performance.getEntriesByType('resource').map(e=>e.toJSON()),...workerTimings.rows.map(row=>({...row,name:new URL(release.objects[row.object].path,releaseRoot).href}))],actualAdapters:()=>[...(window.__nirActualAdapters||[]),...(diagnostics().execution.actual_adapters||[])],needsClock:()=>engine.needs_clock(),rawAction:(a,token,seq,epoch)=>deliver(()=>mutateEngine(()=>engine.action(JSON.stringify(a),token,seq,epoch)),'input'),loseDevice:()=>engine.simulate_device_loss(),hidden:(v)=>deliver(()=>mutateEngine(()=>engine.hidden(v)))};
    function dispose(){if(disposed)return;cancelPointer();pointerQueue.close();disposed=true;metadataRetryController?.abort();metadataPanel.remove();historyGeneration++;historyReadController?.abort();audioRecoveryPanel.remove();historyPanel.remove();historyButton.remove();historyStyle.remove();devBanner?.remove();clearTimeout(ownerTimer);persistence.close();inbox.clear();for(const request of [...contentPreparations.keys()])cancelContent(request);for(const request of [...preparations.keys()])cancelPreparation(request);cancelAnimationFrame(raf);clearInterval(poll);for(const id of [...voices.keys()])stopVoice(id);audioDomains.close();assetClient?.close();storage.close();unbindCanvas();window.removeEventListener('keydown',onKey);window.removeEventListener('keyup',onKeyUp);window.removeEventListener('blur',onBlur);document.removeEventListener('focusin',onEditingFocus);window.removeEventListener('resize',onResize);document.removeEventListener('visibilitychange',onVisibility);engine.free();}
    window.addEventListener('pagehide',e=>{if(e.persisted){cancelPointer();deliver(()=>mutateEngine(()=>engine.hidden(true)));}else{dispose();}});
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
