import {test} from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import vm from 'node:vm';

const sourcePath=path.resolve('crates/nir-platform-web/asset-worker.js');

async function bootAssetWorker({fetchImpl,wasmMock}={}){
    const code=await fs.readFile(sourcePath,'utf8');
    const posted=[];
    let closed=false;
    const context={
        console,URL,Uint8Array,ArrayBuffer,Promise,Map,Set,Error,AbortController,Number,Math,
        setTimeout,clearTimeout,
        performance:{
            timeOrigin:100,
            now:()=>42,
            setResourceTimingBufferSize:()=>{},
            getEntriesByName:()=>[],
            clearResourceTimings:()=>{},
        },
        fetch:fetchImpl||(async()=>{throw new Error('unexpected fetch');}),
        postMessage:(msg,transfer)=>posted.push({msg,transfer}),
        close:()=>{closed=true;},
    };
    context.self=context;
    // Patch dynamic import() in source for unit-testing worker logic in Node/Bun VM
    const instrumented=code.replace('wasm=await import(url.href);','wasm=await __importWasm(url.href);');
    context.__importWasm=async()=>wasmMock||{
        default:async()=>({memory:{buffer:new ArrayBuffer(65536)}}),
        hash_bytes:()=> 'a'.repeat(64),
        decode_image:()=>[2,2,new Uint8Array(16)],
    };
    vm.runInNewContext(instrumented,context,{filename:'asset-worker.js'});
    const send=data=>{context.onmessage({data});};
    const nextMessage=async(predicate=()=>true)=>{
        for(let i=0;i<50;i++){
            const idx=posted.findIndex(predicate);
            if(idx>=0)return posted.splice(idx,1)[0];
            await new Promise(r=>setTimeout(r,2));
        }
        throw new Error('timed out waiting for worker message');
    };
    return {send,nextMessage,posted,isClosed:()=>closed};
}

const BUILD='1'.repeat(64);
const GLUE='2'.repeat(64);
const OBJ='a'.repeat(64);
const ROOT='https://example.test/game/';

test('asset-worker rejects invalid protocol, origin mismatch, and pre-hello calls',async()=>{
    const w=await bootAssetWorker();
    w.send({protocol:99,build:BUILD,kind:'hello',id:1,value:{root:ROOT,glueUrl:`${ROOT}objects/${GLUE}.js`,glueDigest:GLUE,wasmBytes:new ArrayBuffer(4)}});
    let reply=await w.nextMessage();
    assert.equal(reply.msg.kind,'error');
    assert.match(reply.msg.value.message,/E_WORKER_PROTOCOL/);

    w.send({protocol:1,build:BUILD,kind:'decode',id:2,value:{bytes:new ArrayBuffer(4),width:2,height:2}});
    reply=await w.nextMessage();
    assert.equal(reply.msg.kind,'error');
    assert.match(reply.msg.value.message,/E_WORKER_NOT_READY/);

    // Cross-origin glueUrl rejected with E_WORKER_IDENTITY
    w.send({protocol:1,build:BUILD,kind:'hello',id:3,value:{root:ROOT,glueUrl:`https://evil.test/objects/${GLUE}.js`,glueDigest:GLUE,wasmBytes:new ArrayBuffer(4)}});
    reply=await w.nextMessage();
    assert.equal(reply.msg.kind,'error');
    assert.match(reply.msg.value.message,/E_WORKER_IDENTITY/);
});

test('asset-worker verifies fetch digest, byte length, origin, and decode dimensions',async()=>{
    const payload=new Uint8Array([1,2,3,4]).buffer;
    const w=await bootAssetWorker({
        fetchImpl:async()=>({ok:true,status:200,arrayBuffer:async()=>payload.slice(0)}),
    });
    w.send({protocol:1,build:BUILD,kind:'hello',id:1,value:{root:ROOT,glueUrl:`${ROOT}objects/${GLUE}.js`,glueDigest:GLUE,wasmBytes:new ArrayBuffer(4)}});
    let reply=await w.nextMessage();
    assert.equal(reply.msg.kind,'reply');
    assert.equal(reply.msg.value.role,'asset');

    // Valid fetch succeeds and transfers ArrayBuffer
    w.send({protocol:1,build:BUILD,kind:'fetch',id:2,value:{url:`${ROOT}objects/${OBJ}.bin`,digest:OBJ,bytes:4}});
    reply=await w.nextMessage();
    assert.equal(reply.msg.kind,'reply');
    assert.equal(reply.msg.value.bytes.byteLength,4);

    // Mismatched byte length fails with E_OBJECT_DIGEST
    w.send({protocol:1,build:BUILD,kind:'fetch',id:3,value:{url:`${ROOT}objects/${OBJ}.bin`,digest:OBJ,bytes:99}});
    reply=await w.nextMessage();
    assert.equal(reply.msg.kind,'error');
    assert.match(reply.msg.value.message,/E_OBJECT_DIGEST/);

    // Cross-origin fetch fails with E_ORIGIN
    w.send({protocol:1,build:BUILD,kind:'fetch',id:4,value:{url:`https://evil.test/game/objects/${OBJ}.bin`,digest:OBJ,bytes:4}});
    reply=await w.nextMessage();
    assert.equal(reply.msg.kind,'error');
    assert.match(reply.msg.value.message,/E_ORIGIN/);

    // Valid decode passes; mismatched dimensions fail with E_ASSET_DIMENSIONS
    w.send({protocol:1,build:BUILD,kind:'decode',id:5,value:{bytes:payload.slice(0),width:2,height:2}});
    reply=await w.nextMessage();
    assert.equal(reply.msg.kind,'reply');
    assert.equal(reply.msg.value.width,2);
    assert.equal(reply.msg.value.pixels.byteLength,16);

    w.send({protocol:1,build:BUILD,kind:'decode',id:6,value:{bytes:payload.slice(0),width:4,height:4}});
    reply=await w.nextMessage();
    assert.equal(reply.msg.kind,'error');
    assert.match(reply.msg.value.message,/E_ASSET_DIMENSIONS/);

    w.send({kind:'dispose'});
    assert.equal(w.isClosed(),true);
});
