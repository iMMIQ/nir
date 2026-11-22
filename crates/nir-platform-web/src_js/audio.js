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
        const active=!hidden&&before.session===after.session;
        const foreground=active&&!before.foreground_paused&&!after.foreground_paused;
        const story=active&&!storyBlocked&&!before.paused&&!after.paused&&before.screen==='Story';
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
        this.Context=Context;
        this.routes=new Map(['story','foreground_ui','story:voice','story:sfx'].map(key=>[key,this._createRoute()]));
        this.warn=warn;this.now=now;this.closed=false;
        for(const r of this.routes.values())this._attachRoute(r);
    }
    _createRoute() {
        return {context:new this.Context(),domainPaused:true,busPaused:false,paused:true,unlocked:null,pending:null,pendingGesture:false,resumeAt:0,outputError:false,resumeError:false,resumeAttempts:0,resumeResolved:0,resumeRejected:0,lastResumeMs:null,maxResumeMs:0};
    }
    _attachRoute(r) {
        r.context.addEventListener?.('statechange',()=>{if(!this.closed){if(r.context.state==='running')r.outputError=false;this.changed?.();}});
        r.context.addEventListener?.('error',()=>{if(!this.closed){r.outputError=true;this.changed?.();}});
        r.context.suspend().catch(this.warn);
    }
    /// Recreates a route whose AudioContext was externally closed by the OS
    /// (e.g. Safari/iOS background audio session teardown).
    recreateClosed(domain,bus='bgm') {
        if(this.closed)return null;
        const route=this.route(domain,bus);
        if(route.context.state!=='closed')return route.context;
        route.context=new this.Context();
        route.unlocked=null;route.pending=null;route.pendingGesture=false;route.pendingSuspended=false;
        route.outputError=false;route.resumeError=false;
        this._attachRoute(route);
        if(!route.paused)this.resume(route,globalThis.navigator?.userActivation?.isActive??true);
        return route.context;
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

