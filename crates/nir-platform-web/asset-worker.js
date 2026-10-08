// Physical work remains admitted until its reply, including cancelled decodes.
const PROTOCOL=1;let wasm=null,memory=null,timings=false,build=null,root=null,active=0,queue=[],closed=false;
const controllers=new Map(),cancelled=new Set();let fetchCount=0,decodeCount=0;
const send=(kind,id,value,transfer=[])=>postMessage({protocol:PROTOCOL,build,kind,id,value},transfer);
async function run(message){const {kind,id,value}=message;
 if(message.protocol!==PROTOCOL||!Number.isSafeInteger(id)||(build!==null&&message.build!==build))throw new Error('E_WORKER_PROTOCOL');
 if(kind==='hello'){
  if(wasm)throw new Error('E_WORKER_HELLO');build=message.build;root=new URL(value.root);
  const url=new URL(value.glueUrl);if(url.origin!==root.origin||!url.pathname.endsWith(`/objects/${value.glueDigest}.js`))throw new Error('E_WORKER_IDENTITY');
  if(!/^[0-9a-f]{64}$/.test(build)||!/^[0-9a-f]{64}$/.test(value.glueDigest))throw new Error('E_WORKER_IDENTITY');wasm=await import(url.href);memory=(await wasm.default({module_or_path:value.wasmBytes})).memory;timings=value.debug||value.diagnostics;if(timings)performance.setResourceTimingBufferSize(4096);if(value.debug)self.__nirWorker={role:'asset',stats:()=>({active,queued:queue.length,fetches:fetchCount,decodes:decodeCount})};send('reply',id,{protocol:PROTOCOL,build,role:'asset',timeOrigin:performance.timeOrigin});return;
 }
 if(cancelled.delete(id))throw new Error('E_CANCELLED');
 if(!wasm)throw new Error('E_WORKER_NOT_READY');
 if(kind==='fetch'){fetchCount++;
  const url=new URL(value.url);if(url.origin!==root.origin||!url.pathname.startsWith(root.pathname+'objects/'+value.digest+'.')||!(/^[0-9a-f]{64}$/).test(value.digest))throw new Error('E_ORIGIN');
  const controller=new AbortController();controllers.set(id,controller);
  try{const response=await fetch(url,{signal:controller.signal});if(!response.ok)throw new Error(`E_HTTP: ${response.status}`);const bytes=await response.arrayBuffer();if(bytes.byteLength!==value.bytes||wasm.hash_bytes(new Uint8Array(bytes))!==value.digest)throw new Error('E_OBJECT_DIGEST');let resource=null;if(timings){let entry=performance.getEntriesByName(url.href).at(-1);if(!entry){await new Promise(r=>setTimeout(r,0));entry=performance.getEntriesByName(url.href).at(-1);}if(entry){resource={object:value.digest};for(const key of ['startTime','duration','fetchStart','domainLookupStart','domainLookupEnd','connectStart','connectEnd','requestStart','responseStart','responseEnd','transferSize','encodedBodySize','decodedBodySize'])resource[key]=entry[key];resource.nextHopProtocol=entry.nextHopProtocol;resource.initiatorType=entry.initiatorType;}}send('reply',id,{bytes,resource,wasmMemoryBytes:memory.buffer.byteLength},[bytes]);}finally{controllers.delete(id);}return;
 }
 if(kind==='decode'){decodeCount++;const start_us=String(Math.round(performance.now()*1000));
  const [width,height,pixels]=wasm.decode_image(new Uint8Array(value.bytes));
  if(width!==value.width||height!==value.height||pixels.byteLength!==width*height*4)throw new Error('E_ASSET_DIMENSIONS');
  send('reply',id,{width,height,pixels:pixels.buffer,wasmMemoryBytes:memory.buffer.byteLength,start_us,end_us:String(Math.round(performance.now()*1000))},[pixels.buffer]);return;
 }
 throw new Error('E_WORKER_METHOD');
}
onmessage=event=>{const m=event.data;if(m.kind==='dispose'){closed=true;for(const c of controllers.values())c.abort();close();return;}if(m.kind==='cancel'){if(m.protocol===PROTOCOL&&m.build===build){if(controllers.has(m.id))controllers.get(m.id).abort();else if(queue.some(job=>job.id===m.id))cancelled.add(m.id);}return;}if(queue.length+active>=64){send('error',m.id,{message:'E_WORKER_CAPACITY: asset inbox'});return;}queue.push(m);drain();};
function drain(){while(active<4&&queue.length&&!closed){const m=queue.shift();active++;run(m).catch(e=>send('error',m.id,{message:String(e)})).finally(()=>{active--;drain();});}}
