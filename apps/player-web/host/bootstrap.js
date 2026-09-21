// Mutable entry point. All session components come from one immutable release graph.
const startupTrace=[];
const mark=stage=>startupTrace.push({stage,start_us:String(Math.round(performance.now()*1000)),end_us:String(Math.round(performance.now()*1000))});
mark('bootstrap_started');
const base = new URL('./', import.meta.url);
const message = document.querySelector('#shell-message');
function fail(error) {document.querySelector('#shell').hidden=false;document.querySelector('#shell-title').textContent='无法启动播放器 / Unable to start';message.textContent=String(error);const b=document.querySelector('#reload');b.hidden=false;b.onclick=()=>location.reload();console.error(error);}
async function hash(bytes) {return Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',bytes)),x=>x.toString(16).padStart(2,'0')).join('');}
async function fetchBytes(path) {const url=new URL(path,base);if(url.origin!==base.origin||!url.pathname.startsWith(base.pathname))throw new Error('E_ORIGIN: object escaped release root');const r=await fetch(url);if(!r.ok)throw new Error(`E_HTTP: ${r.status} ${path}`);return r.arrayBuffer();}
try {
    if(!navigator.gpu)throw new Error('E_WEBGPU: 此首版需要启用 WebGPU 的桌面 Chromium。WebGPU-capable desktop Chromium is required.');
    const channelResponse=await fetch(new URL('channels/stable.json',base),{cache:'no-cache'});if(!channelResponse.ok)throw new Error('E_CHANNEL');
    const channel=await channelResponse.json();if(channel.format!==1||!(/^[0-9a-f]{64}$/).test(channel.release))throw new Error('E_CHANNEL_SCHEMA');
    mark('channel_ready');
    const raw=await fetchBytes(`releases/${channel.release}.json`);if(await hash(raw)!==channel.release)throw new Error('E_RELEASE_DIGEST');const release=JSON.parse(new TextDecoder().decode(raw));
    if(release.format!==1||!release.engine||!release.objects)throw new Error('E_RELEASE_SCHEMA');
    mark('release_verified');
    const objectUrl=(id)=>{const o=release.objects[id];if(!o||!(/^[0-9a-f]{64}$/).test(id)||!o.path.startsWith(`objects/${id}.`)||o.path.includes('..')||o.path.includes(':')||o.path.includes('\\'))throw new Error('E_OBJECT_REFERENCE');return new URL(o.path,base);};
    const fetchObject=async(id,signal)=>{const o=release.objects[id],url=objectUrl(id);const r=await fetch(url,{signal});if(!r.ok)throw Object.assign(new Error(`E_HTTP: ${r.status}`),{code:'E_HTTP'});const bytes=await r.arrayBuffer();if(bytes.byteLength!==o.bytes||await hash(bytes)!==id)throw Object.assign(new Error(`E_OBJECT_DIGEST: ${id}`),{code:'E_OBJECT_DIGEST'});return bytes;};
    // Small code objects are verified before import. Immutable URLs and same-origin policy
    // bind the subsequent browser import to the same publisher-controlled object graph.
    await Promise.all([fetchObject(release.engine.js),fetchObject(release.engine.host)]);
    const [wasm,host,executable]=await Promise.all([import(objectUrl(release.engine.js).href),import(objectUrl(release.engine.host).href),fetchObject(release.program)]);
    // Verify the actual WASM bytes used for instantiation, including a corrupted cache.
    const wasmBytes=await fetchObject(release.engine.wasm);
    mark('wasm_verified');
    await wasm.default({module_or_path:wasmBytes});
    mark('wasm_initialized');
    await host.start({wasm,release,releaseDigest:channel.release,executable:new TextDecoder().decode(executable),fetchObject,fail,startupTrace});
} catch(error) {fail(error);}
