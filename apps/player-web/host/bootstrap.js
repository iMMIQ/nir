// The same bytes run at the mutable root and at each immutable release entry.
const startupTrace=[];
const mark=(stage,fields={})=>startupTrace.push({stage,start_us:String(Math.round(performance.now()*1000)),end_us:String(Math.round(performance.now()*1000)),...fields});
mark('bootstrap_started');
const fixed = /(?:^|\/)releases\/([0-9a-f]{64})\/index\.html$/.exec(location.pathname);
const base = fixed ? new URL('../../', import.meta.url) : new URL('./', import.meta.url);
const rootEntry = !fixed && (location.pathname === base.pathname || location.pathname === `${base.pathname}index.html`);
const message = document.querySelector('#shell-message');
function fail(error) {document.querySelector('#shell').hidden=false;document.querySelector('#shell-title').textContent='无法启动播放器 / Unable to start';message.textContent=String(error);const b=document.querySelector('#reload');b.hidden=false;b.onclick=()=>location.reload();console.error(error);}
async function hash(bytes) {return Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',bytes)),x=>x.toString(16).padStart(2,'0')).join('');}
async function fetchBytes(path) {const url=new URL(path,base);if(url.origin!==base.origin||!url.pathname.startsWith(base.pathname))throw new Error('E_ORIGIN: object escaped release root');const r=await fetch(url);if(!r.ok)throw new Error(`E_HTTP: ${r.status} ${path}`);return r.arrayBuffer();}
try {
    if(!fixed && !rootEntry)throw new Error('E_ENTRY_PATH');
    if(rootEntry){
        const response=await fetch(new URL('channels/stable.json',base),{cache:'no-store'});
        if(!response.ok)throw new Error(`E_CHANNEL_HTTP: ${response.status}`);
        const channel=await response.json();
        if(channel.format!==1||!(/^[0-9a-f]{64}$/).test(channel.release))throw new Error('E_CHANNEL_SCHEMA');
        const target=new URL(`releases/${channel.release}/index.html`,base);target.search=location.search;target.hash=location.hash;location.replace(target);
        // Navigation ends this bootstrap; no runtime starts from the mutable URL.
        throw {redirect:true};
    }
    const releaseDigest=fixed[1];
    const raw=await fetchBytes(`releases/${releaseDigest}.json`);if(await hash(raw)!==releaseDigest)throw new Error('E_RELEASE_DIGEST');const release=JSON.parse(new TextDecoder().decode(raw));
    if(release.format!==1||!['dev','release'].includes(release.profile)||!release.engine||!release.objects||!release.launch)throw new Error('E_RELEASE_SCHEMA');
    if(!(/^[0-9a-f]{64}$/).test(release.launch.html)||!(/^[0-9a-f]{64}$/).test(release.launch.bootstrap))throw new Error('E_LAUNCH_SCHEMA');
    const launchFiles=await Promise.all(['index.html','bootstrap.js'].map(name=>fetchBytes(`releases/${releaseDigest}/${name}`)));
    if(await hash(launchFiles[0])!==release.launch.html||await hash(launchFiles[1])!==release.launch.bootstrap)throw new Error('E_LAUNCH_DIGEST');
    mark('release_verified');
    const objectUrl=(id)=>{const o=release.objects[id];if(!o||!(/^[0-9a-f]{64}$/).test(id)||!o.path.startsWith(`objects/${id}.`)||o.path.includes('..')||o.path.includes(':')||o.path.includes('\\'))throw new Error('E_OBJECT_REFERENCE');return new URL(o.path,base);};
    const fetchObject=async(id,signal,observe=()=>{})=>{
        const o=release.objects[id],url=objectUrl(id),start_us=String(Math.round(performance.now()*1000));
        const r=await fetch(url,{signal});if(!r.ok)throw Object.assign(new Error(`E_HTTP: ${r.status}`),{code:'E_HTTP'});
        observe('object_response',{object:id,start_us,end_us:String(Math.round(performance.now()*1000))});
        const bytes=await r.arrayBuffer(),downloaded_us=String(Math.round(performance.now()*1000));
        observe('object_downloaded',{object:id,bytes:bytes.byteLength,start_us,end_us:downloaded_us});
        if(bytes.byteLength!==o.bytes||await hash(bytes)!==id)throw Object.assign(new Error(`E_OBJECT_DIGEST: ${id}`),{code:'E_OBJECT_DIGEST'});
        observe('object_verified',{object:id,bytes:bytes.byteLength,start_us:downloaded_us,end_us:String(Math.round(performance.now()*1000))});
        return bytes;
    };
    // Small code objects are verified before import. Immutable URLs and same-origin policy
    // bind the subsequent browser import to the same publisher-controlled object graph.
    const controller=new AbortController(),verified=id=>fetchObject(id,controller.signal,mark);
    // Start all four downloads once the release is verified, before any import.
    const tasks=[
        verified(release.engine.js).then(()=>import(objectUrl(release.engine.js).href)),
        verified(release.engine.host).then(()=>import(objectUrl(release.engine.host).href)),
        verified(release.program).then(bytes=>{
            const root=JSON.parse(new TextDecoder().decode(bytes));
            if(root.format!==2||!root.program||typeof root.program!=='object')throw new Error('E_RUNTIME_VERSION: expected RuntimeExecutable v2');
            return bytes;
        }),
        verified(release.engine.wasm).then(bytes=>{mark('wasm_verified');return bytes;}),
    ];
    let components;
    try {components=await Promise.all(tasks);}
    catch(error){controller.abort(error);await Promise.allSettled(tasks);throw error;}
    const [wasm,host,executable,wasmBytes]=components;
    // Instantiate only the verified bytes, including when HTTP cache supplied them.
    await wasm.default({module_or_path:wasmBytes});
    mark('wasm_initialized');
    await host.start({wasm,release,releaseDigest,releaseRoot:base.href,entryUrl:location.href,executable:new TextDecoder().decode(executable),fetchObject,fail,startupTrace});
    if(release.profile==='dev'){
        try{
            const probe=await fetch(new URL('__nir_dev/status',base),{cache:'no-store'});
            if(probe.ok){
                const helper=document.createElement('script');
                helper.src=new URL('__nir_dev/client.js',base).href;
                helper.dataset.release=releaseDigest;
                helper.dataset.root=base.href;
                document.body.append(helper);
            }
        }catch{}
    }
} catch(error) {if(!error?.redirect)fail(error);}
