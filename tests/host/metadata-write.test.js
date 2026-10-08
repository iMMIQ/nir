import {test} from 'node:test';
import assert from 'node:assert/strict';
import {writePreferencesRecord,mergeProfileRecord,OwnerInbox,dispatchOwnerRequest,PersistenceWrites} from '../../crates/nir-platform-web/host.js';

const turns=async()=>{for(let i=0;i<8;i++)await Promise.resolve();};
function fixture({prior,abortThrows=false}={}) {
  const writes=[],reads=[];
  const db={transaction(kind,mode){
    assert.equal(mode,'readwrite');
    const r={result:prior},put={},tx={aborts:0,objectStore(name){assert.equal(name,kind);return {
      get(){return r;},put(value,key){writes.push({kind,value,key});return put;},
    };},abort(){this.aborts++;if(abortThrows)throw new DOMException('commit started','InvalidStateError');}};
    reads.push({r,put,tx});return tx;
  }};
  return {db,writes,reads};
}

test('metadata writes require native completion and profile merges preserve old progress',async()=>{
  const f=fixture({prior:['old']});const p=mergeProfileRecord(f.db,['game','release'],['new','old'],{timeoutMs:100});
  const {r,tx}=f.reads[0];let done=false;p.then(()=>{done=true;});r.onsuccess();await turns();assert.equal(done,false);
  assert.deepEqual(f.writes[0].value,['new','old']);f.reads[0].put.onsuccess();tx.oncomplete();await p;
  assert.equal(r.onsuccess,null);assert.equal(tx.oncomplete,null);assert.equal(tx.aborts,0);
});
test('a write blocked before its read aborts on deadline and late callbacks cannot queue puts',async()=>{
  const f=fixture();const p=writePreferencesRecord(f.db,[],{bgm_volume:.2},{timeoutMs:10});
  const {r,tx}=f.reads[0],lateRead=r.onsuccess,lateComplete=tx.oncomplete;
  await assert.rejects(p,/E_STORAGE_TIMEOUT: preferences write/);
  assert.equal(tx.aborts,1);lateRead();lateComplete();assert.deepEqual(f.writes,[]);assert.equal(tx.onabort,null);
});
test('a queued put without completion also aborts, and corruption is never overwritten',async()=>{
  const f=fixture({prior:{bgm_volume:.4}});const p=writePreferencesRecord(f.db,[],{bgm_volume:.2},{timeoutMs:10});
  f.reads[0].r.onsuccess();await assert.rejects(p,/E_STORAGE_TIMEOUT/);assert.equal(f.reads[0].tx.aborts,1);
  const bad=fixture({prior:{future_field:'keep'}});const q=writePreferencesRecord(bad.db,[],{bgm_volume:.2});
  bad.reads[0].r.onsuccess();await assert.rejects(q,/E_PREFERENCES_RECORD/);assert.deepEqual(bad.writes,[]);
});
test('failed abort after put remains pending until native completion, without duplicate notices or writes',async()=>{
  const f=fixture({abortThrows:true}),notices=[],controller=new AbortController();
  let notify;const warned=new Promise(ok=>{notify=ok;});
  const p=writePreferencesRecord(f.db,[],{bgm_volume:.2},{timeoutMs:10,signal:controller.signal,onPending:e=>{notices.push(e);notify();}});
  let done=false;p.then(()=>{done=true;});const {r,tx}=f.reads[0];r.onsuccess();await warned;
  assert.equal(done,false);assert.equal(notices.length,1);assert.equal(notices[0].code,'E_STORAGE_UNCERTAIN');
  controller.abort();await turns();assert.equal(tx.aborts,1);assert.equal(f.writes.length,1);assert.equal(done,false);
  f.reads[0].put.onsuccess();tx.oncomplete();await p;assert.equal(done,true);assert.equal(tx.onabort,null);
});
test('uncertain write can later abort; an error event alone never releases it as a failed write',async()=>{
  const f=fixture({abortThrows:true});let notify;const warned=new Promise(ok=>{notify=ok;});
  const p=mergeProfileRecord(f.db,[],['first'],{timeoutMs:10,onPending:()=>notify()});
  const {r,tx}=f.reads[0];r.onsuccess();await warned;
  const error=new DOMException('quota','QuotaExceededError');tx.error=error;tx.onerror();
  let done=false;p.catch(()=>{done=true;});await turns();assert.equal(done,false);
  tx.onabort();await assert.rejects(p,e=>e===error);assert.equal(f.writes.length,1);
});
test('invalid deadlines, keys and pre-cancelled writes never begin a transaction',async()=>{
  const f=fixture();for(const timeoutMs of [0,-1,NaN,Infinity,1.5,0x80000000])await assert.rejects(mergeProfileRecord(f.db,[],[],{timeoutMs}),RangeError);
  for(const keys of [null,[1],new Array(1)])await assert.rejects(mergeProfileRecord(f.db,[],keys),/E_PROFILE_RECORD/);
  const c=new AbortController(),reason=new Error('closed');c.abort(reason);
  await assert.rejects(writePreferencesRecord(f.db,[],{}, {signal:c.signal}),e=>e===reason);assert.equal(f.reads.length,0);
  const g=fixture();const pending=writePreferencesRecord(g.db,[],{}, {signal:new AbortController().signal,timeoutMs:100});
  g.reads[0].r.onsuccess();g.reads[0].put.onsuccess();g.reads[0].tx.oncomplete();await pending;
});
test('transaction completion with an unsuccessful put cannot acknowledge stored data',async()=>{
  const f=fixture(),p=writePreferencesRecord(f.db,[],{});const {r,put,tx}=f.reads[0];r.onsuccess();
  put.error=new DOMException('cancelled constraint error','ConstraintError');tx.onerror();tx.oncomplete();
  await assert.rejects(p,e=>e===put.error);assert.equal(tx.oncomplete,null);
});
test('cancellation after queuing a put aborts once and ignores late success handlers',async()=>{
  const f=fixture(),c=new AbortController(),reason=new Error('navigation cancelled');
  const p=writePreferencesRecord(f.db,[],{}, {signal:c.signal});const {r,put,tx}=f.reads[0];r.onsuccess();
  const latePut=put.onsuccess,lateComplete=tx.oncomplete;c.abort(reason);
  await assert.rejects(p,e=>e===reason);latePut();lateComplete();assert.equal(tx.aborts,1);assert.equal(put.onsuccess,null);
});
test('owner warning and a raced terminal reuse one reservation in order without entering the owner early',async()=>{
  const inbox=new OwnerInbox(4,2,0),slot=inbox.reserve(),events=[];
  let finish;const native=new Promise(ok=>{finish=ok;});
  const done=dispatchOwnerRequest(notify=>{notify('pending');return native;},slot,{
    post:(s,fn,terminal)=>s.post(fn,{terminal}),pending:e=>events.push(e),success:()=>events.push('stored'),failure:()=>events.push('failed'),
  });
  await turns();finish();await turns();assert.deepEqual(events,[]);assert.equal(inbox.used,1);assert.equal(inbox.length,1);
  inbox.drain();await turns();assert.deepEqual(events,['pending']);assert.equal(inbox.used,1);
  inbox.drain();await done;assert.deepEqual(events,['pending','stored']);assert.equal(inbox.used,0);assert.equal(inbox.completed,1);
});
test('an uncertain persistence lane keeps edits, rejects retry and starts only one new write after confirmation',async()=>{
  const jobs=[],events=[],writes=[];
  const q=new PersistenceWrites({request:(work,ok,no,options)=>jobs.push({work,ok,no,options}),
    writePreferences:(value,notify)=>{writes.push(value);notify(new Error('pending'));},mergeProfile:()=>{},
    stored:kind=>events.push('stored:'+kind),failed:kind=>events.push('failed:'+kind)});
  q.submit('preferences',{bgm_volume:.1});jobs[0].work(e=>jobs[0].options.pending(e));
  for(let i=0;i<200;i++)q.submit('preferences',{bgm_volume:i/200});
  assert.equal(q.retry('preferences'),false);assert.equal(jobs.length,1);assert(q.lanes.get('preferences').active);
  jobs[0].ok();assert.equal(jobs.length,2);jobs[1].work(()=>{});jobs[1].ok();
  assert.deepEqual(writes,[{bgm_volume:.1},{bgm_volume:.995}]);assert.deepEqual(events,['failed:preferences','stored:preferences']);
  assert.equal(q.retry('preferences'),false);
});
