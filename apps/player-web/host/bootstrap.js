// The same bytes run at the mutable root and at each immutable release entry.
const startupTrace=[];
const mark=(stage,fields={})=>startupTrace.push({stage,start_us:String(Math.round(performance.now()*1000)),end_us:String(Math.round(performance.now()*1000)),...fields});
mark('bootstrap_started');
const fixed = /(?:^|\/)releases\/([0-9a-f]{64})\/index\.html$/.exec(location.pathname);
const base = fixed ? new URL('../../', import.meta.url) : new URL('./', import.meta.url);
const rootEntry = !fixed && (location.pathname === base.pathname || location.pathname === `${base.pathname}index.html`);
const message = document.querySelector('#shell-message');
function fail(error) {document.querySelector('#shell').hidden=false;document.querySelector('#shell-title').textContent='无法启动播放器 / Unable to start';message.textContent=String(error);const b=document.querySelector('#reload');b.hidden=false;b.onclick=()=>location.reload();console.error(error);}
// LAN HTTP is not a secure context: Web Crypto may be absent. Keep verifying
// every release/object with SHA-256, using the same algorithm locally as fallback.
async function hash(bytes) {
    if(globalThis.crypto?.subtle)return Array.from(new Uint8Array(await globalThis.crypto.subtle.digest('SHA-256',bytes)),x=>x.toString(16).padStart(2,'0')).join('');
    const input=ArrayBuffer.isView(bytes)?new Uint8Array(bytes.buffer,bytes.byteOffset,bytes.byteLength):new Uint8Array(bytes);
    const k=new Uint32Array([
        0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,
        0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,
        0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,
        0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,
        0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,
        0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,
        0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,
        0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2,
    ]);
    const state=new Uint32Array([0x6a09e667,0xbb67ae85,0x3c6ef372,0xa54ff53a,0x510e527f,0x9b05688c,0x1f83d9ab,0x5be0cd19]);
    const words=new Uint32Array(64),length=input.length,padded=Math.ceil((length+9)/64)*64;
    const rotate=(x,n)=>(x>>>n)|(x<<(32-n));
    // Only the final one or two blocks need padding; do not copy large media.
    const tail=new Uint8Array(padded-length),view=new DataView(tail.buffer);
    tail[0]=0x80;view.setUint32(tail.length-8,Math.floor(length/0x20000000));view.setUint32(tail.length-4,(length*8)>>>0);
    const byte=i=>i<length?input[i]:tail[i-length];
    for(let offset=0;offset<padded;offset+=64){
        for(let i=0;i<16;i++){const j=offset+i*4;words[i]=(byte(j)<<24)|(byte(j+1)<<16)|(byte(j+2)<<8)|byte(j+3);}
        for(let i=16;i<64;i++){
            const x=words[i-15],y=words[i-2];
            words[i]=words[i-16]+(rotate(x,7)^rotate(x,18)^(x>>>3))+words[i-7]+(rotate(y,17)^rotate(y,19)^(y>>>10));
        }
        let [a,b,c,d,e,f,g,h]=state;
        for(let i=0;i<64;i++){
            const t1=(h+(rotate(e,6)^rotate(e,11)^rotate(e,25))+((e&f)^(~e&g))+k[i]+words[i])>>>0;
            const t2=((rotate(a,2)^rotate(a,13)^rotate(a,22))+((a&b)^(a&c)^(b&c)))>>>0;
            h=g;g=f;f=e;e=(d+t1)>>>0;d=c;c=b;b=a;a=(t1+t2)>>>0;
        }
        const next=[a,b,c,d,e,f,g,h];for(let i=0;i<8;i++)state[i]+=next[i];
    }
    return Array.from(state,x=>x.toString(16).padStart(8,'0')).join('');
}
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
    // The Runtime Worker instantiates verified WASM; main instantiation is fallback-only.
    await host.start({wasm,release,releaseDigest,releaseRoot:base.href,entryUrl:location.href,executable:new TextDecoder().decode(executable),fetchObject,fail,startupTrace,sha256:hash,wasmBytes});
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
