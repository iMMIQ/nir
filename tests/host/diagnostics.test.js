import {test} from 'node:test';
import assert from 'node:assert/strict';
import {TraceRecorder,OwnerInbox} from '../../crates/nir-platform-web/host.js';

test('trace ring is bounded, ordered and excludes private payloads',()=>{
    const trace=new TraceRecorder({enabled:true,capacity:3,now:()=>1.25});
    for(let sequence=0;sequence<10;sequence++)trace.record('input',{sequence,session:2,cause:'secret',message:'private dialogue',snapshot:{name:'player'},variables:'secret',url:'file:///private',asset:'background',start_us:'1234'});
    const report=trace.snapshot();assert.equal(report.dropped,7);assert.deepEqual(report.events.map(e=>e.sequence),[7,8,9]);
    assert.equal(report.events[0].at_us,'1250');assert.equal(report.events[0].start_us,'1234');
    assert.doesNotMatch(JSON.stringify(report),/secret|private|player|snapshot|cause|variables|url/);
    report.events[0].sequence=99;assert.equal(trace.snapshot().events[0].sequence,7);
});
test('disabled trace does not read clock or retain records',()=>{
    const trace=new TraceRecorder({now:()=>{throw Error('must not be called');}});
    trace.record('event',{sequence:1});assert.equal(trace.snapshot().events.length,0);
});
test('request terminal trace identifies cancelled and duplicate callbacks',async()=>{
    const trace=new TraceRecorder({enabled:true});
    const inbox=new OwnerInbox(8,4,1,(stage,fields)=>trace.record(stage,fields));
    const first=inbox.reserve('resource',12),second=inbox.reserve('resource',13);
    first.cancel();await first.post(()=>assert.fail('late callback ran'));
    const pending=second.post(()=>{});inbox.drain();await pending;await second.post(()=>assert.fail('duplicate ran'));
    const events=trace.snapshot().events;
    assert.deepEqual(events.filter(e=>e.host_request===1).map(e=>e.stage),['request_reserved','request_cancelled','callback_discarded']);
    assert.deepEqual(events.filter(e=>e.host_request===2).map(e=>e.stage),['request_reserved','request_completed','callback_discarded']);
    assert.equal(events[0].request,12);
});
test('request observations retain origin epoch through a session replacement',async()=>{
    const trace=new TraceRecorder({enabled:true});let session=1;
    const inbox=new OwnerInbox(8,4,1,(stage,fields)=>trace.record(stage,fields),()=>({session}));
    const slot=inbox.reserve('completion','save');session=2;
    const result=slot.post(()=>{});inbox.drain();await result;
    assert.deepEqual(trace.snapshot().events.map(e=>e.session),[1,1]);
});
