import { stringifyPlayerData } from './storage.js';

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
        const calls=this.calls.splice(0),transfer=new Set();
        const collectTransfer=val=>{
            if(ArrayBuffer.isView(val)&&val.buffer instanceof ArrayBuffer)transfer.add(val.buffer);
            else if(val instanceof ArrayBuffer)transfer.add(val);
            else if(Array.isArray(val))for(const item of val)collectTransfer(item);
        };
        for(const c of calls)for(const a of c.args)collectTransfer(a);
        const control=calls.some(c=>['hidden','audio_blocked','begin_recovery','simulate_device_loss'].includes(c.method)||c.method==='host_event'&&c.args[0]==='assets_cancelled');
        this.inFlight=this.client.call('batch',calls,[...transfer],{control}).then(reply=>{
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
for(const name of ['action','host_event','content_ready','content_failed','content_skipped','resource_fault','resource_failed','audio_positions_in','audio_ended_in','audio_failed_in','hidden','audio_blocked','simulate_device_loss','begin_recovery','set_profiling','set_tracing','focus_control','hover','set_touch_input'])RemoteEngine.prototype[name]=function(...args){this.enqueue(name,args);};
for(const name of ['pointer_gesture','pointer_action','navigate_focus','control_value_action','focus_value_action','primary_action','hit'])RemoteEngine.prototype[name]=function(...args){return this.query(name,args);};

export function isTestHookAllowed(release,loc=globalThis.location) {
    if(release?.profile==='dev')return true;
    if(globalThis.__NIR_ALLOW_TEST_HOOKS__===true)return true;
    const host=String(loc?.hostname||'').toLowerCase();
    return host==='127.0.0.1'||host==='localhost'||host==='::1'||host==='[::1]';
}

export async function connectWorker(role,options) {
    const id=options.release.engine[role==='runtime'?'runtime_worker':'asset_worker'];
    if(!id)throw new Error('E_WORKER_UNAVAILABLE: release has no worker');
    await options.fetchObject(id);
    const url=new URL(options.release.objects[id].path,options.releaseRoot);
    const client=new WorkerClient(new Worker(url,{type:'module',name:`nir-${role}`}),options.release.engine_build);
    const bytes=options.wasmBytes.slice(0);
    try {
        const ready=await client.call('hello',{glueUrl:new URL(options.release.objects[options.release.engine.js].path,options.releaseRoot).href,glueDigest:options.release.engine.js,wasmBytes:bytes,root:options.releaseRoot,debug:Boolean(options.testMode),diagnostics:new URL(location.href).searchParams.has('diagnostics')},[bytes]);
        if(ready.role!==role||ready.protocol!==WORKER_PROTOCOL||ready.build!==options.release.engine_build)throw new Error('E_WORKER_HANDSHAKE');
        client.clockOffsetUs=Math.round((ready.timeOrigin-performance.timeOrigin)*1000);
        return client;
    }catch(error){client.close();throw error;}
}

