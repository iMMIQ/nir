import {test} from 'node:test';
import assert from 'node:assert/strict';
import {commitSaveRecord} from '../../crates/nir-platform-web/host.js';
const key=['game','profile','a'.repeat(64),0],metadata={gameId:key[0],profile:key[1],releaseDigest:key[2],slot:0};
const prior={...metadata,envelope:{revision:1}},next={revision:2};
const inspect=async json=>JSON.parse(json).revision;
function fake({abortThrows=false}={}) {
 const get={result:structuredClone(prior),error:null},put={error:null},tx={error:null,aborts:0,puts:[],abort(){this.aborts++;if(abortThrows)throw new DOMException('committing','InvalidStateError');},objectStore(){return {get:()=>get,put(value){tx.puts.push(value);return put;}};}};
 const db={reads:0,writes:0,transaction(name,mode){assert.equal(name,'saves');if(mode==='readwrite'){this.writes++;return tx;}
  this.reads++;const r={result:structuredClone(prior),error:null},readTx={abort(){},objectStore(){return {get:()=>r};}};
  queueMicrotask(()=>{r.onsuccess?.();readTx.oncomplete?.();});return readTx;}};
 return {db,tx,get,put};
}
async function begin(f,options={}) {const p=commitSaveRecord(f.db,key,next,metadata,1,inspect,{timeoutMs:100,...options});await new Promise(setImmediate);assert.equal(f.db.writes,1);return {p};}

test('save acknowledgement requires successful put and native transaction completion',async()=>{
 const f=fake(),{p}=await begin(f);let done=false;p.then(()=>done=true);
 f.get.onsuccess();assert.equal(f.tx.puts[0].envelope.revision,2);f.put.onsuccess();await Promise.resolve();assert.equal(done,false);
 f.tx.oncomplete();await p;assert.equal(f.tx.aborts,0);assert.equal(f.put.onsuccess,null);assert.equal(f.tx.oncomplete,null);
});
test('save write deadline before get aborts without put and ignores already queued callbacks',async()=>{
 const f=fake(),{p}=await begin(f,{timeoutMs:5}),get=f.get.onsuccess,complete=f.tx.oncomplete;
 await assert.rejects(p,/E_STORAGE_TIMEOUT: save write/);get();complete();assert.equal(f.tx.aborts,1);assert.deepEqual(f.tx.puts,[]);
});
test('save write deadline after put aborts once and cannot acknowledge a late success',async()=>{
 const f=fake(),{p}=await begin(f,{timeoutMs:5});f.get.onsuccess();const success=f.put.onsuccess,complete=f.tx.oncomplete;
 await assert.rejects(p,/E_STORAGE_TIMEOUT/);success();complete();assert.equal(f.tx.aborts,1);assert.equal(f.tx.puts.length,1);
});
test('uncertain save keeps original promise until actual completion, notice emitted once',async()=>{
 const notices=[],controller=new AbortController(),f=fake({abortThrows:true}),{p}=await begin(f,{timeoutMs:5,signal:controller.signal,onPending:e=>notices.push(e)});
 f.get.onsuccess();let done=false;p.then(()=>done=true,()=>done=true);await new Promise(r=>setTimeout(r,12));controller.abort();
 assert.equal(done,false);assert.equal(notices.length,1);assert.equal(notices[0].code,'E_STORAGE_UNCERTAIN');assert.equal(f.tx.aborts,1);
 f.put.onsuccess();f.tx.oncomplete();await p;assert.equal(f.tx.puts.length,1);
});
test('uncertain save whose native transaction aborts fails rather than acknowledging queued put',async()=>{
 const f=fake({abortThrows:true}),notices=[],{p}=await begin(f,{timeoutMs:5,onPending:e=>notices.push(e)});f.get.onsuccess();f.put.onsuccess();
 await new Promise(r=>setTimeout(r,12));assert.equal(notices.length,1);f.tx.onabort();await assert.rejects(p,/E_STORAGE_TIMEOUT/);
});
test('save compare-and-swap protects a replacement even with the same revision',async()=>{
 const f=fake(),{p}=await begin(f);f.get.result={...prior,envelope:{revision:1,tampered:true}};f.get.onsuccess();
 await assert.rejects(p,/E_SAVE_CONFLICT/);assert.equal(f.tx.aborts,1);assert.deepEqual(f.tx.puts,[]);
});
test('save request failure is not terminal until abort or completion and failed put cannot count as saved',async()=>{
 const f=fake(),{p}=await begin(f);f.get.onsuccess();f.put.error=new Error('write failed');let done=false;p.catch(()=>done=true);
 f.tx.onerror();await Promise.resolve();assert.equal(done,false);f.tx.oncomplete();await assert.rejects(p,/write failed/);
 const g=fake(),{p:q}=await begin(g);g.tx.oncomplete();await assert.rejects(q,/E_STORAGE_WRITE/);
});
test('save options and cancellation are checked before any write or validation',async()=>{
 const f=fake(),controller=new AbortController();controller.abort(new Error('cancelled'));
 await assert.rejects(commitSaveRecord(f.db,key,next,metadata,1,inspect,{signal:controller.signal}),/cancelled/);
 for(const timeoutMs of [0,-1,1.5,NaN,Infinity,0x80000000])await assert.rejects(commitSaveRecord(f.db,key,next,metadata,1,inspect,{timeoutMs}),RangeError);
 assert.equal(f.db.reads,0);assert.equal(f.db.writes,0);
 const g=fake(),cancel=new AbortController(),{p}=await begin(g,{signal:cancel.signal});g.get.onsuccess();cancel.abort(new Error('cancelled write'));
 await assert.rejects(p,/cancelled write/);assert.equal(g.tx.aborts,1);
});
