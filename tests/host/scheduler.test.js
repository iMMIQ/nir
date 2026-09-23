import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ContentStagingBudget, OwnerInbox, SharedRequests, WorkPool, acquireRequiredContentStage, contentBatchEnvelope, queueContentSkip } from '../../crates/nir-platform-web/host.js';

test('input overload leaves room for terminal completions',()=>{
    const q=new OwnerInbox(8,4),out=[];
    for(let n=0;n<4;n++)assert(q.push(()=>out.push('input'),'input'));
    assert.equal(q.push(()=>{},'input'),false);
    for(let n=0;n<4;n++)assert(q.push(()=>out.push('saved')));
    assert.equal(q.push(()=>{}),false);
    q.drain({limit:8,milliseconds:Infinity});assert.equal(out.filter(x=>x==='saved').length,4);
    assert.equal(q.highWater,8);
});
test('owner yields at item/time limits and never consumes reentrant production',()=>{
    const q=new OwnerInbox(),out=[];let time=0;
    q.push(()=>{out.push(1);time=10;q.push(()=>out.push(3));});q.push(()=>out.push(2));
    assert.equal(q.drain({now:()=>time,milliseconds:4}),1);assert.deepEqual(out,[1]);
    assert.equal(q.drain({limit:1}),1);assert.deepEqual(out,[1,2]);
    q.drain();assert.deepEqual(out,[1,2,3]);
});
test('inputs precede resource delivery and each turn admits one resource',()=>{
    const q=new OwnerInbox(),out=[];
    q.push(()=>out.push('image1'),'resource');q.push(()=>out.push('image2'),'resource');q.push(()=>out.push('choice'),'input');
    q.drain({milliseconds:Infinity});assert.deepEqual(out,['choice','image1']);assert.equal(q.length,1);
    q.drain();assert.deepEqual(out,['choice','image1','image2']);
});
test('recovery stops the current batch and still admits its control completion',()=>{
    const q=new OwnerInbox(),out=[];let recovering=false;
    q.push(()=>recovering=true,'control');q.push(()=>out.push('saved'));
    q.drain({canRun:kind=>!recovering||kind==='control'});assert.equal(q.length,1);assert.deepEqual(out,[]);
    q.push(()=>recovering=false,'control');q.drain({canRun:kind=>!recovering||kind==='control'});assert.deepEqual(out,['saved']);
});
test('cancelling a resource group releases bytes without dropping independent saves',()=>{
    const q=new OwnerInbox(),out=[];
    q.push(()=>out.push('stale'),'resource',()=>out.push('released'),9);q.push(()=>out.push('saved'));
    q.cancelGroup(9);q.drain();assert.deepEqual(out,['released','saved']);
});
test('shared fetch aborts only after its last consumer cancels',async()=>{
    let upstream,loads=0;
    const requests=new SharedRequests((key,signal)=>{loads++;upstream=signal;return new Promise((_,reject)=>signal.addEventListener('abort',()=>reject(signal.reason)));});
    const a=new AbortController(),b=new AbortController();
    const one=requests.get('same',a.signal),two=requests.get('same',b.signal);
    await Promise.resolve();a.abort();await assert.rejects(one);assert.equal(upstream.aborted,false);assert.equal(loads,1);
    b.abort();await assert.rejects(two);assert.equal(upstream.aborted,true);assert.equal(requests.jobs.size,0);
});
test('late old completion cannot remove a replacement fetch',async()=>{
    const resolve=[];const requests=new SharedRequests(()=>new Promise(r=>resolve.push(r)));
    const a=new AbortController(),b=new AbortController();
    const old=requests.get('same',a.signal);await Promise.resolve();a.abort();await assert.rejects(old);
    const current=requests.get('same',b.signal);await Promise.resolve();resolve[0]('obsolete');await Promise.resolve();await Promise.resolve();
    assert.equal(requests.jobs.size,1);resolve[1]('new');assert.equal(await current,'new');assert.equal(requests.jobs.size,0);
});


test('resource admission is global and cancelled decoders hold their slot until settled',async()=>{
    const pool=new WorkPool(1),a=new AbortController(),b=new AbortController(),c=new AbortController();
    let finish,started=0;
    const first=pool.run(()=>{started++;return new Promise(resolve=>finish=resolve);},a.signal);
    await Promise.resolve();
    const cancelled=pool.run(()=>started++,b.signal);b.abort();await assert.rejects(cancelled);
    const next=pool.run(()=>started++,c.signal);a.abort();
    assert.equal(started,1);assert.equal(pool.active,1);assert.equal(pool.waiting.length,1);
    finish();await first;await next;assert.equal(started,2);
});

test('required resource work jumps queued prefetch work after the active job settles',async()=>{
    const pool=new WorkPool(1),signal=new AbortController().signal,order=[];
    let finish;
    const active=pool.run(()=>new Promise(resolve=>finish=resolve),signal);
    await Promise.resolve();
    const prefetch=pool.run(()=>order.push('prefetch'),signal,{priority:'prefetch',group:'prefetch:1'});
    const required=pool.run(()=>order.push('required'),signal);
    finish();
    await Promise.all([active,prefetch,required]);
    assert.deepEqual(order,['required','prefetch']);
});

test('promoting queued prefetch work retains its place as required work',async()=>{
    const pool=new WorkPool(1),signal=new AbortController().signal,order=[];
    let finish;
    const active=pool.run(()=>new Promise(resolve=>finish=resolve),signal);
    await Promise.resolve();
    const prefetch=pool.run(()=>order.push('promoted'),signal,{priority:'prefetch',group:'content:8'});
    const unrelated=pool.run(()=>order.push('other-required'),signal);
    assert.equal(pool.promoteGroup('content:8'),1);
    finish();
    await Promise.all([active,prefetch,unrelated]);
    assert.deepEqual(order,['promoted','other-required']);
});

test('content preflight applies prefetch and global encoded-byte limits',()=>{
    const one='a'.repeat(64),large='b'.repeat(64),manifest={
        [one]:{bytes:1024*1024},
        [large]:{bytes:2*1024*1024+1},
    };
    assert.equal(contentBatchEnvelope([{hash:one}],manifest,{priority:'prefetch',maxBytes:1024*1024}).ok,true);
    assert.deepEqual(contentBatchEnvelope([{hash:large}],manifest,{priority:'prefetch',maxBytes:2*1024*1024}),{
        ok:false,code:'E_PREFETCH_LIMIT',reason:'available_budget',totalBytes:2*1024*1024+1,maxBytes:2*1024*1024,prefetch:true,
    });
    const overGlobal={...manifest,[large]:{bytes:16*1024*1024+1}};
    assert.equal(contentBatchEnvelope([{hash:large}],overGlobal).code,'E_CONTENT_LIMIT');
});

test('required staging waits for released prefetch bytes and records encoded peak',async()=>{
    const budget=new ContentStagingBudget(10),controller=new AbortController();
    assert.equal(budget.tryReserve('prefetch',7),true);
    let admitted=false;
    const demand=budget.acquire('demand',6,controller.signal).then(value=>{admitted=value;return value;});
    assert.equal(budget.tryReserve('later-prefetch',1),false);
    await Promise.resolve();assert.equal(admitted,false);assert.equal(budget.used,7);
    assert.equal(budget.release('prefetch'),true);
    assert.equal(await demand,true);assert.equal(admitted,true);assert.equal(budget.used,6);
    assert.equal(budget.peak,7);
    assert.equal(budget.release('demand'),true);assert.equal(budget.used,0);
});

test('a granted demand staging reservation is owned before a racing cancellation',async()=>{
    const budget=new ContentStagingBudget(10),controller=new AbortController();
    const job={group:'content:cancel-race',signal:controller.signal,staged:false};
    const admission=acquireRequiredContentStage(job,budget,6);
    controller.abort(new Error('cancelled after reservation grant'));
    try {
        await assert.rejects(admission,/cancelled after reservation grant/);
    } finally {
        if(job.staged)budget.release(job.group);
    }
    assert.equal(job.staged,true);assert.equal(budget.used,0);
});

test('a fetch-failure skip promoted before owner delivery reuses its terminal slot',async()=>{
    const inbox=new OwnerInbox(4,4,0),controller=new AbortController();
    const job={priority:'prefetch',state:'fetching',signal:controller.signal,terminal:inbox.reserve('completion','content:1')};
    let skipped=0,failed=0;
    const queued=queueContentSkip(job,{code:'E_PREFETCH_FAILED',reason:'fetch_failed'}, {
        post:(slot,run,terminal)=>slot.post(run,{terminal}),
        skip:()=>skipped++,
    });
    assert.equal(job.state,'skip_queued');
    job.priority='required'; // PromoteContent for the exact same request/session.
    inbox.drain({milliseconds:Infinity});
    assert.equal(await queued,true);
    assert.equal(skipped,0);assert.equal(job.terminal.state,'pending');assert.equal(inbox.used,1);
    const failedRequest=job.terminal.post(()=>failed++);
    inbox.drain({milliseconds:Infinity});assert.equal(await failedRequest,true);
    assert.equal(failed,1);assert.equal(inbox.used,0);
});


test('an active batch still occupies capacity and disposal settles its pending items',()=>{
    const q=new OwnerInbox(4,4),out=[];
    q.push(()=>{
        assert.equal(q.push(()=>out.push('new')),true);
        assert.equal(q.push(()=>{}),false);
        assert.equal(q.length,4);
        q.clear();
    });
    for(let n=0;n<3;n++)q.push(()=>out.push('unexpected'),'completion',()=>out.push('cancelled'));
    q.drain({milliseconds:Infinity});assert.equal(q.length,0);assert.equal(q.highWater,4);
    assert.deepEqual(out,['cancelled','cancelled','cancelled']);
});

test('accepted jobs keep a terminal slot when unrelated traffic fills the inbox',async()=>{
    const q=new OwnerInbox(8,8),out=[];
    const saved=q.reserve(),loaded=q.reserve();
    for(let n=0;n<6;n++)assert(q.push(()=>{},'input'));
    assert.equal(q.push(()=>{}),false);assert.equal(q.reserve(),null);
    const one=saved.post(()=>out.push('saved')),two=loaded.post(()=>out.push('failed'));
    assert.equal(q.used,8);q.drain({limit:8,milliseconds:Infinity});
    assert.deepEqual(out,['saved','failed']);assert.equal(await one,true);assert.equal(await two,true);
    assert.equal(q.used,0);assert.equal(q.completed,2);
});
test('reserved resource progress reuses its slot until completion or cancellation',async()=>{
    const q=new OwnerInbox(2,2),slot=q.reserve('resource',7),out=[];
    const first=slot.post(()=>out.push('chunk'),{terminal:false});q.drain();assert(await first);
    assert.equal(q.used,1);assert.equal(slot.state,'pending');
    const last=slot.post(()=>out.push('complete'));assert.equal(await slot.post(()=>out.push('duplicate')),false);
    q.cancelGroup(7);assert.equal(await last,false);assert.equal(await slot.post(()=>{}),false);
    assert.deepEqual(out,['chunk']);assert.equal(q.cancelled,1);assert.equal(q.used,0);
});
test('device control has admission room beyond ordinary job reservations',async()=>{
    const q=new OwnerInbox(16,8,2);
    for(let n=0;n<14;n++)assert(q.reserve());
    assert.equal(q.reserve(),null);const recovery=q.reserve('control');assert(recovery);
    const done=recovery.post(()=>{},{});assert(q.push(()=>{},'control'));
    q.drain({controlsOnly:true,milliseconds:Infinity});assert(await done);assert.equal(q.used,14);
    q.clear();assert.equal(q.used,0);
});
test('shutdown settles queued progress and reserved terminals exactly once',async()=>{
    const q=new OwnerInbox(),one=q.reserve(),two=q.reserve('resource');
    const pending=two.post(()=>assert.fail('disposed'),{terminal:false});
    q.clear();q.clear();assert.equal(await pending,false);assert.equal(await one.post(()=>{}),false);
    assert.equal(q.accepted,2);assert.equal(q.cancelled,2);assert.equal(q.used,0);assert.equal(q.length,0);
});
test('seeded completion, cancellation and drain interleavings preserve the ledger',async()=>{
    const q=new OwnerInbox(32,16,2),slots=[],seen=new Set();let seed=0x4e4952;
    const rng=()=>{seed^=seed<<13;seed^=seed>>>17;seed^=seed<<5;return seed>>>0;};
    for(let n=0;n<20000;n++){
        const action=rng()%5;
        if(action===0){const slot=q.reserve('completion');if(slot)slots.push(slot);}
        else if(action===1&&slots.length){const i=rng()%slots.length;slots[i].post(()=>{assert(!seen.has(i));seen.add(i);});}
        else if(action===2&&slots.length)slots[rng()%slots.length].cancel();
        else if(action===3)q.push(()=>{},'input');
        else q.drain({limit:1+rng()%8,milliseconds:Infinity});
        assert(q.used<=32);assert.equal(q.accepted,q.completed+q.cancelled+q.slots.size);
    }
    q.clear();assert.equal(q.used,0);assert.equal(q.accepted,q.completed+q.cancelled);
});
