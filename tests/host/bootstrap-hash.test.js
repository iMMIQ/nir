import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {createHash,webcrypto} from 'node:crypto';
import {runInNewContext} from 'node:vm';
import {validateHistoryTarget} from '../../crates/nir-platform-web/host.js';

const source=readFileSync(new URL('../../apps/player-web/host/bootstrap.js',import.meta.url),'utf8');
const hashSource=source.slice(source.indexOf('async function hash('),source.indexOf('async function fetchBytes('));
const fallback=runInNewContext(`${hashSource};hash`,{crypto:undefined});
const native=runInNewContext(`${hashSource};hash`,{crypto:webcrypto});
const expected=bytes=>createHash('sha256').update(bytes).digest('hex');
test('HTTP SHA-256 fallback matches standard vectors and padding/block boundaries',async()=>{
    for(const bytes of [Buffer.alloc(0),Buffer.from('abc'),Buffer.alloc(1000000,97),...[1,55,56,63,64,65,119,120,127,128,129,4096].map(n=>Buffer.from(Array.from({length:n},(_,i)=>(i*37+11)&255)))]){
        assert.equal(await fallback(bytes),expected(bytes),`length ${bytes.length}`);
        assert.equal(await native(bytes),expected(bytes));
    }
    const backing=Uint8Array.from([1,2,3,4,5]);
    assert.equal(await fallback(backing.subarray(1,4)),expected(backing.subarray(1,4)));
    assert.equal(await fallback(backing.buffer),expected(backing));
});
test('HTTP history verification uses the injected hash and still rejects tampering',async()=>{
    const previous=globalThis.location;
    globalThis.location=new URL('http://192.0.2.1:4173/');
    try{
        const html=new TextEncoder().encode('<html>fixture</html>');
        const release=new TextEncoder().encode(JSON.stringify({format:1,game_id:'fixture',profile:'release',launch:{html:expected(html)}}));
        const options={releaseRoot:location.href,digest:expected(release),gameId:'fixture',profile:'release',sha256:fallback,subtle:undefined};
        const fetchImpl=async url=>new Response(url.pathname.endsWith('.json')?release:html);
        assert.equal((await validateHistoryTarget({...options,fetchImpl})).available,true);
        const corrupt=async url=>new Response(url.pathname.endsWith('.json')?release:'tampered');
        assert.equal((await validateHistoryTarget({...options,fetchImpl:corrupt})).status,'Release player verification failed');
    }finally{if(previous===undefined)delete globalThis.location;else globalThis.location=previous;}
});
