import { test } from 'node:test';
import assert from 'node:assert/strict';
import { OwnerInbox, SharedRequests, WorkPool } from '../../crates/nir-platform-web/host.js';

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
