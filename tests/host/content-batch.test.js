import { test } from 'node:test';
import assert from 'node:assert/strict';
import { acquireRequiredContentStage, ContentStagingBudget, fetchContentBatch, OwnerInbox, postContentEviction, SharedRequests, WorkPool } from '../../crates/nir-platform-web/host.js';

const turn=()=>new Promise(resolve=>setImmediate(resolve));
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b;});return {promise,resolve,reject};};
function job(hashes,priority='required'){
    const controller=new AbortController();
    return {objects:hashes.map(hash=>({hash})),priority,controller,signal:controller.signal,group:Symbol(),request:1,session:1};
}

test('batch overlaps objects, deduplicates hashes, and returns original order under the global limit',async()=>{
    const pool=new WorkPool(4),loads=new Map();let peak=0;
    const requests=new SharedRequests(hash=>{peak=Math.max(peak,pool.active);const d=deferred();loads.set(hash,d);return d.promise;});
    const first=fetchContentBatch(job(['a','b','c','a','d','e']),{pool,requests});
    const second=fetchContentBatch(job(['f','g']),{pool,requests});
    await turn();assert.equal(loads.size,4);assert.equal(pool.active,4);
    loads.get('c').resolve('C');loads.get('b').resolve('B');await turn();
    assert.equal(loads.size,6);assert.equal(pool.active,4);
    loads.get('a').resolve('A');loads.get('d').resolve('D');await turn();
    for(const [hash,d] of loads)d.resolve(hash.toUpperCase());
    assert.deepEqual(await first,['A','B','C','A','D','E']);
    assert.deepEqual(await second,['F','G']);await turn();
    assert.equal(peak,4);assert.equal(loads.size,7);assert.equal(pool.active,0);
});

test('failure cancels siblings but does not release the attempt until every worker settles',async()=>{
    const slow=deferred(),bad=deferred(),pool=new WorkPool(4);let siblingSignal,finished=false;
    const requests={get(hash,signal){if(hash==='bad')return bad.promise;siblingSignal=signal;return slow.promise;}};
    const attempt=fetchContentBatch(job(['slow','bad']),{pool,requests});
    const rejected=assert.rejects(attempt,/broken/).then(()=>{finished=true;});
    await turn();bad.reject(new Error('broken'));await turn();
    assert.equal(siblingSignal.aborted,true);assert.equal(finished,false);assert.equal(pool.active,1);
    slow.resolve('late');await rejected;await turn();assert.equal(pool.active,0);
});

test('promoted retry retains completed objects and uses a fresh attempt signal',async()=>{
    const batch=job(['good','bad'],'prefetch'),pool=new WorkPool(4),fetched=new Map(),bad=deferred();
    const counts={good:0,bad:0};
    const requests=new SharedRequests(hash=>{counts[hash]++;return hash==='good'?'GOOD':counts.bad===1?bad.promise:'RECOVERED';});
    const attempt=fetchContentBatch(batch,{pool,requests,fetched});
    const rejected=assert.rejects(attempt,/temporary/);
    await turn();bad.reject(new Error('temporary'));await rejected;
    assert.equal(batch.signal.aborted,false);assert.equal(fetched.get('good'),'GOOD');
    batch.priority='required';
    assert.deepEqual(await fetchContentBatch(batch,{pool,requests,fetched}),['GOOD','RECOVERED']);
    assert.deepEqual(counts,{good:1,bad:2});
});

test('cancelling a batch detaches its shared consumer without cancelling another batch',async()=>{
    const shared=deferred(),pool=new WorkPool(4);let upstream;
    const requests=new SharedRequests((_,signal)=>{upstream=signal;return shared.promise;});
    const one=job(['same']),two=job(['same']);
    const first=fetchContentBatch(one,{pool,requests}),second=fetchContentBatch(two,{pool,requests});
    const rejected=assert.rejects(first,/cancelled/);
    await turn();one.controller.abort(new Error('cancelled'));await rejected;
    assert.equal(upstream.aborted,false);shared.resolve('shared');
    assert.deepEqual(await second,['shared']);await turn();assert.equal(requests.jobs.size,0);
});

test('promotion reorders queued objects and subsequent workers inherit required priority',async()=>{
    const pool=new WorkPool(1),gate=deferred(),order=[],signal=new AbortController().signal;
    const occupied=pool.run(()=>gate.promise,signal);await turn();
    const batch=job(['a','b','c','d','e'],'prefetch');
    const fetches=fetchContentBatch(batch,{pool,requests:{get:hash=>{order.push(hash);return hash;}}});
    const competing=pool.run(()=>order.push('other'),signal,{priority:'prefetch',group:'other'});
    batch.priority='required';assert.equal(pool.promoteGroup(batch.group),4);
    gate.resolve();await Promise.all([occupied,fetches,competing]);
    assert.deepEqual(order,['a','b','c','d','e','other']);
});

test('last content consumer holds staging ownership through uncancellable verification',async()=>{
    const verification=deferred(),pool=new WorkPool(4),batch=job(['hashing']);let upstream,settled=false;
    const requests=new SharedRequests((_,signal)=>{upstream=signal;return verification.promise;});
    const attempt=fetchContentBatch(batch,{pool,requests});
    const rejected=assert.rejects(attempt,/cancelled/).then(()=>{settled=true;});
    await turn();batch.controller.abort(new Error('cancelled'));await turn();
    assert.equal(upstream.aborted,true);assert.equal(settled,false);assert.equal(pool.active,1);
    verification.resolve(new ArrayBuffer(32));await rejected;await turn();
    assert.equal(pool.active,0);assert.equal(requests.jobs.size,0);
});

test('demand with available staging leaves speculative downloads intact',async()=>{
    const budget=new ContentStagingBudget(10),prefetch=job(['a'],'prefetch'),demand=job(['b']);
    Object.assign(prefetch,{staged:true,state:'fetching',totalBytes:4});
    assert(budget.tryReserve(prefetch.group,4));
    assert(await acquireRequiredContentStage(demand,budget,4,[prefetch,demand]));
    assert.equal(prefetch.signal.aborted,false);assert.equal(budget.used,8);
    budget.release(prefetch.group);budget.release(demand.group);assert.equal(budget.used,0);
});

test('staging eviction settles bytes and notifies Rust even if demand promotes the abandoned request',async()=>{
    const budget=new ContentStagingBudget(8),pool=new WorkPool(4),inbox=new OwnerInbox(8,4,0);
    const prefetch=job(['a'],'prefetch'),demand=job(['b']),verification=deferred();
    Object.assign(prefetch,{staged:true,state:'fetching',totalBytes:6,terminal:inbox.reserve('completion','prefetch')});
    assert(budget.tryReserve(prefetch.group,6));
    const attempt=fetchContentBatch(prefetch,{pool,requests:new SharedRequests(()=>verification.promise)});
    const rejected=assert.rejects(attempt);await turn();
    let admitted=false,skips=0;
    const admission=acquireRequiredContentStage(demand,budget,4,[prefetch,demand]).then(value=>{admitted=value;});
    await turn();assert.equal(prefetch.signal.aborted,true);assert.equal(admitted,false);assert.equal(budget.used,6);
    assert.equal(prefetch.stageEviction.reason,'staging_reclaimed');
    verification.resolve('verified');await rejected;
    prefetch.priority='required';
    const notification=postContentEviction(prefetch,{
        post:(slot,run)=>slot.post(run),skip:envelope=>{assert.equal(envelope.code,'E_PREFETCH_LIMIT');skips++;},
    });
    assert.equal(skips,0);assert.equal(budget.used,6);
    inbox.drain({milliseconds:Infinity});await notification;
    assert.equal(skips,1);assert.equal(inbox.used,0);assert.equal(admitted,false);
    budget.release(prefetch.group);await admission;
    assert.equal(admitted,true);assert.equal(budget.used,4);
    budget.release(demand.group);assert.equal(budget.used,0);
});

test('Rust cancellation suppresses an eviction completion and queued delivery is not reclaimed',async()=>{
    const inbox=new OwnerInbox(8,4,0),prefetch=job(['a'],'prefetch');
    prefetch.terminal=inbox.reserve('completion','prefetch');prefetch.cancelled=true;
    let skips=0;
    await postContentEviction(prefetch,{post:(slot,run)=>slot.post(run),skip:()=>skips++});
    inbox.drain({milliseconds:Infinity});assert.equal(skips,0);assert.equal(inbox.used,0);
    const budget=new ContentStagingBudget(8),delivering=job(['a'],'prefetch'),demand=job(['b']);
    Object.assign(delivering,{staged:true,state:'delivering',totalBytes:6});
    budget.tryReserve(delivering.group,6);
    const admission=acquireRequiredContentStage(demand,budget,4,[delivering]);
    assert.equal(delivering.signal.aborted,false);
    budget.release(delivering.group);assert(await admission);budget.release(demand.group);
});
