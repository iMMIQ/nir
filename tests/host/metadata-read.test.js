import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readMetadataRecord,readStartupMetadata} from '../../crates/nir-platform-web/host.js';

function fixture() {
  const reads=[];
  const db={transaction(kind,mode){
    assert.equal(mode,'readonly');
    const request={},tx={aborts:0,objectStore(name){assert.equal(name,kind);return {get(key){return request;}};},abort(){this.aborts++;}};
    reads.push({kind,tx,request});return tx;
  }};
  return {db,reads};
}

test('metadata is published only after completion and successful reads release cancellation handlers',async()=>{
  const {db,reads}=fixture(),controller=new AbortController();
  const pending=readMetadataRecord(db,'preferences',['game','full'],{signal:controller.signal,timeoutMs:100});
  const {tx,request}=reads[0];let settled=false;pending.then(()=>{settled=true;});
  request.result={bgm_volume:.25};request.onsuccess();await Promise.resolve();assert.equal(settled,false);
  tx.oncomplete();assert.deepEqual(await pending,{bgm_volume:.25});controller.abort();assert.equal(tx.aborts,0);
  assert.equal(request.onsuccess,null);assert.equal(tx.oncomplete,null);
});
test('a stalled read times out, aborts once and ignores saved late handlers',async()=>{
  const {db,reads}=fixture();const pending=readMetadataRecord(db,'profile',[],{timeoutMs:10});
  const {tx,request}=reads[0],success=request.onsuccess,complete=tx.oncomplete;
  await assert.rejects(pending,/E_STORAGE_TIMEOUT: profile read/);assert.equal(tx.aborts,1);
  request.result=['late'];success();complete();assert.equal(tx.aborts,1);assert.equal(tx.onabort,null);
});
test('a validated value without transaction completion still times out instead of appearing durable',async()=>{
  const {db,reads}=fixture();const pending=readMetadataRecord(db,'preferences',[],{timeoutMs:10});
  const {tx,request}=reads[0];request.result={bgm_volume:.1};request.onsuccess();
  await assert.rejects(pending,/E_STORAGE_TIMEOUT/);assert.equal(tx.aborts,1);
});
test('metadata cancellation preserves its reason; pre-aborted reads never create a transaction',async()=>{
  const {db,reads}=fixture(),controller=new AbortController(),reason=new Error('navigation cancelled');
  const pending=readMetadataRecord(db,'profile',[],{signal:controller.signal});controller.abort(reason);
  await assert.rejects(pending,e=>e===reason);assert.equal(reads[0].tx.aborts,1);
  await assert.rejects(readMetadataRecord(db,'profile',[],{signal:controller.signal}),e=>e===reason);assert.equal(reads.length,1);
});
test('transaction setup and malformed records preserve original errors and cannot hang cleanup',async()=>{
  const denied=new DOMException('closed','InvalidStateError');
  await assert.rejects(readMetadataRecord({transaction(){throw denied;}},'profile',[]),e=>e===denied);
  const {db,reads}=fixture();const pending=readMetadataRecord(db,'profile',[]);
  const {tx,request}=reads[0];request.result=new Array(1);request.onsuccess();
  await assert.rejects(pending,/E_PROFILE_RECORD/);assert.equal(tx.aborts,1);
  assert.equal(request.result.length,1);assert.equal(Object.hasOwn(request.result,0),false);
});
test('startup keeps healthy preferences when only progress stalls, without writes or implicit retries',async()=>{
  const {db,reads}=fixture();const pending=readStartupMetadata(db,[],{timeoutMs:10});
  const preferences=reads.find(r=>r.kind==='preferences');preferences.request.result={font_scale:1.4};preferences.request.onsuccess();preferences.tx.oncomplete();
  const result=await pending;assert.deepEqual(result.preferences,{font_scale:1.4});assert.deepEqual(result.profile,[]);
  assert.equal(result.failures.length,1);assert.equal(result.failures[0].kind,'profile');assert.match(result.failures[0].message,/E_STORAGE_TIMEOUT/);
  assert.equal(reads.length,2);assert.equal(reads[1].tx.aborts,1);assert.equal(reads[0].tx.aborts,0);
});
test('invalid deadlines and completions without a read result are explicit failures',async()=>{
  const {db,reads}=fixture();for(const timeoutMs of [0,-1,NaN,Infinity,1.5,0x80000000])await assert.rejects(readMetadataRecord(db,'profile',[],{timeoutMs}),RangeError);
  assert.equal(reads.length,0);const pending=readMetadataRecord(db,'profile',[]);reads[0].tx.oncomplete();await assert.rejects(pending,/E_STORAGE_READ/);
});
