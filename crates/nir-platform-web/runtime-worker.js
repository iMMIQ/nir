// One WASM/GPU owner. No DOM, Web Audio or shared WASM memory.
const PROTOCOL=1, MAX_CALLS=128, MAX_COMMANDS=248;
let build=null,wasm=null,engine=null,canvas=null,view='{"nodes":[],"ready":false}',size={width:1,height:1,dpr:1},debug=false;
const knownRequests=new Set(),knownContent=new Set(),adapters=[];
let contextLost=false,watchedCanvas=null;
function watchCanvas(){if(watchedCanvas===canvas)return;const owner=watchedCanvas=canvas;owner.addEventListener('webglcontextlost',e=>{e.preventDefault();if(owner!==canvas)return;contextLost=true;dirty=true;publish();});}
let last=performance.now(),hidden=false,scheduled=null,clockActive=false,closed=false,outstanding=false,dirty=false,commands=[],revision=0;
const UI=new Set(['focus_control','navigate_focus','hover','set_touch_input','pointer_gesture','pointer_action','hit','control_value_action','primary_action','focus_value_action']);
const ALLOWED=new Set([...UI,'action','host_event','content_ready','content_failed','content_skipped','resource','resource_decoded','resource_fault','resource_failed','audio_positions_in','audio_ended_in','audio_failed_in','hidden','audio_blocked','simulate_device_loss','begin_recovery','set_profiling','resize']);
const stamp=()=>performance.now();
function collect(){const next=JSON.parse(engine.commands());for(const c of next){if(c.type==='get_assets')knownRequests.add(c.request);if(c.type==='get_content')knownContent.add(c.request);}commands.push(...next);if(commands.length>MAX_COMMANDS)throw new Error('E_WORKER_CAPACITY: output commands');dirty=true;}
function snapshot(initial=false){for(const id of knownRequests)if(!engine.accepts_resource(id))knownRequests.delete(id);for(const id of knownContent)if(!engine.accepts_content(id))knownContent.delete(id);const host=engine.host_state();return {revision:++revision,requests:[...knownRequests],contentRequests:[...knownContent],host,state:debug||initial?engine.state():host,view,commands:commands.splice(0),retained:engine.retained_descriptors(),pending:engine.pending_events(),needsClock:engine.needs_clock(),backend:engine.backend(),gpuError:engine.gpu_error(),deviceLost:engine.device_lost()||contextLost,adapters,profile:engine.take_profile(),textCache:engine.text_cache_stats()};}
function send(kind,id,value,transfer=[]){postMessage({protocol:PROTOCOL,build,kind,id,value},transfer);}
function publish(){if(!engine||outstanding||!dirty||closed)return;outstanding=true;dirty=false;send('update',0,snapshot());}
function draw(){if(engine.device_lost()||contextLost){dirty=true;return;}const w=Math.round(size.width*size.dpr),h=Math.round(size.height*size.dpr);if(canvas.width!==w)canvas.width=w;if(canvas.height!==h)canvas.height=h;view=engine.draw(size.width,size.height,size.dpr);collect();}
function schedule(){if(closed||!engine||scheduled!==null)return;if(engine.needs_clock()||engine.pending_events()){if(!clockActive)last=stamp();clockActive=true;scheduled=setTimeout(frame,16);}else clockActive=false;}
function frame(){scheduled=null;if(closed||!engine)return;try {if(engine.device_lost()||contextLost){clockActive=false;dirty=true;publish();return;}const now=stamp(),before=JSON.parse(engine.host_state());engine.begin_turn();const elapsed=Math.max(0,Math.round((now-last)*1000));last=now;if(!hidden)engine.tick_domains(before.paused||before.screen!=='Story'?0:Math.min(elapsed,0xffffffff),Math.min(elapsed,250000));engine.continue_turn();collect();draw();publish();schedule();}catch(error){send('fatal',0,{code:'E_WORKER_RUNTIME',message:String(error)});closed=true;}}
function objectUrl(raw,digest,root){const url=new URL(raw);if(url.origin!==new URL(root).origin||!/^[0-9a-f]{64}$/.test(digest)||!url.pathname.endsWith(`/objects/${digest}.js`))throw new Error('E_WORKER_IDENTITY');return url.href;}
async function dispatch(message){
 const {kind,id,value}=message;
 if(message.protocol!==PROTOCOL||(!Number.isSafeInteger(id))||(build!==null&&message.build!==build))throw new Error('E_WORKER_PROTOCOL');
 if(kind==='hello'){
  if(wasm)throw new Error('E_WORKER_HELLO');build=message.build;if(!/^[0-9a-f]{64}$/.test(build))throw new Error('E_WORKER_IDENTITY');
  wasm=await import(objectUrl(value.glueUrl,value.glueDigest,value.root));await wasm.default({module_or_path:value.wasmBytes});debug=value.debug===true;
  if((debug||value.diagnostics)&&navigator.gpu){
   const gpu=navigator.gpu,request=gpu.requestAdapter.bind(gpu);
   const wrapped=async options=>{
    const adapter=await request(options),row={atMs:performance.now(),adapterAvailable:!!adapter,options:{powerPreference:options?.powerPreference,forceFallbackAdapter:options?.forceFallbackAdapter},vendor:'',architecture:'',device:'',description:'',fallback:null};
    try{const info=adapter?.info;for(const key of ['vendor','architecture','device','description'])if(typeof info?.[key]==='string')row[key]=info[key].slice(0,256);if(typeof info?.isFallbackAdapter==='boolean')row.fallback=info.isFallbackAdapter;}catch{}
    adapters.push(row);if(adapters.length>16)adapters.shift();return adapter;
   };
   // Observability must not prevent startup on a browser with a sealed GPU API.
   try{Object.defineProperty(gpu,'requestAdapter',{configurable:true,value:wrapped});}catch{}
  }
  send('reply',id,{protocol:PROTOCOL,build,role:'runtime',timeOrigin:performance.timeOrigin,offscreen:typeof OffscreenCanvas==='function'});return;
 }
 if(kind==='ack'){outstanding=false;publish();return;}
 if(kind==='dispose'){closed=true;clearTimeout(scheduled);engine?.free();engine=null;close();return;}
 if(!wasm)throw new Error('E_WORKER_NOT_READY');
 if(kind==='probe'){send('reply',id,await wasm.probe_backend());return;}
 if(kind==='inspect-save'){
  if(typeof value?.json!=='string'||typeof value.release!=='string'||typeof value.game!=='string'||!Number.isInteger(value.slot)||value.slot<0||value.slot>2)throw new Error('E_WORKER_PROTOCOL');
  // Read-only auxiliary validation: no Player pump, draw, or clock mutation.
  send('reply',id,wasm.inspect_save_slot(value.json,value.slot,value.release,value.game));return;
 }
 if(kind==='create'){
  if(engine)throw new Error('E_WORKER_OWNER');canvas=value.canvas;size=value.size;
  if(!(canvas instanceof OffscreenCanvas))throw new Error('E_CANVAS');
  watchCanvas();
  engine=await wasm.Engine.create(value.executable,value.release,value.title,canvas,value.preferences,value.backend,value.audio_sample_rate);engine.set_profiling(value.profiling);last=stamp();
  // Actually configure/draw the requested backend before Ready, not just an API check.
  engine.begin_turn();engine.continue_turn();collect();draw();dirty=false;
  if(debug)self.__nirWorker={role:'runtime',loseContext:()=>{const extension=canvas.getContext('webgl2').getExtension('WEBGL_lose_context');if(!extension)throw new Error('E_CONTEXT_LOSS_UNAVAILABLE');extension.loseContext();},state:()=>JSON.parse(engine.state()),queues:()=>({inbox:queue.length,outbound:commands.length,unacknowledged:outstanding?1:0})};
  send('reply',id,{snapshot:snapshot(true),role:'runtime',protocol:PROTOCOL,build});schedule();return;
 }
 if(kind==='gpu'){
  clearTimeout(scheduled);scheduled=null;
  const replacement=value.canvas||canvas;const gpu=await wasm.create_gpu(replacement,value.backend);canvas=replacement;engine.replace_gpu(gpu);contextLost=false;watchCanvas();last=stamp();
  engine.begin_turn();engine.continue_turn();collect();draw();send('reply',id,{snapshot:snapshot()});schedule();return;
 }
 if(kind!=='batch'||!Array.isArray(value)||value.length>MAX_CALLS)throw new Error('E_WORKER_PROTOCOL');
 const before=JSON.parse(engine.host_state());engine.begin_turn();const results=[];
 for(const call of value){
  if(!ALLOWED.has(call.method)||!Array.isArray(call.args))throw new Error('E_WORKER_METHOD');
  const current=JSON.parse(engine.host_state());
  if(UI.has(call.method)&&(call.session!==current.session||call.interaction!==current.interaction)){results.push(null);continue;}
  if(call.method==='resize'){size=call.args[0];results.push(null);continue;}
  if(call.method==='simulate_device_loss')contextLost=true;
  if(call.method==='hidden'){hidden=call.args[0];last=stamp();}
  results.push(engine[call.method](...call.args));
 }
 // Input is consumed before any subsequent clock catch-up.
 engine.continue_turn();collect();draw();dirty=false;
 const after=JSON.parse(engine.host_state());if(before.session!==after.session||before.paused!==after.paused||before.foreground_paused!==after.foreground_paused||before.screen!==after.screen)last=stamp();
 send('reply',id,{results,snapshot:snapshot()});schedule();
}
let queue=[],busy=false;
onmessage=event=>{if(closed)return;if(queue.length>=256){send('fatal',0,{code:'E_WORKER_CAPACITY',message:'runtime inbox'});closed=true;return;}queue.push(event.data);void drain();};
async function drain(){if(busy)return;busy=true;try {while(queue.length&&!closed){const message=queue.shift();try{await dispatch(message);}catch(error){send('error',message.id,{message:String(error)});}}}finally{busy=false;}}

setInterval(()=>{if(engine&&!closed&&(engine.device_lost()||contextLost)){dirty=true;publish();}},250);
