import {test} from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import vm from 'node:vm';
import {WorkerClient,WORKER_PROTOCOL,SerialInputQueue,RemoteEngine} from '../../crates/nir-platform-web/host.js';
class Worker {
 messages=[];terminated=false;
 postMessage(m){this.messages.push(m);}
 terminate(){this.terminated=true;}
 reply(id,kind='reply',value=null,build='a'.repeat(64)){this.onmessage({data:{protocol:WORKER_PROTOCOL,build,id,kind,value}});}
}
test('normal admission preserves eight control slots; replies settle once',async()=>{
 const worker=new Worker(),client=new WorkerClient(worker,'a'.repeat(64),{capacity:16,timeout:0});
 const tasks=Array.from({length:8},()=>client.call('batch',[]));
 await assert.rejects(client.call('batch',[]),/E_WORKER_CAPACITY/);
 tasks.push(...Array.from({length:8},()=>client.call('gpu',null,[],{control:true})));
 await assert.rejects(client.call('gpu',null,[],{control:true}),/E_WORKER_CAPACITY/);
 assert.equal(client.highWater,16);
 for(const m of worker.messages)worker.reply(m.id);
 await Promise.all(tasks);assert.equal(client.pending.size,0);
 worker.reply(1);assert.equal(client.pending.size,0);client.close();
});
test('cancellation retains its reservation until the physical job settles',async()=>{
 const worker=new Worker(),client=new WorkerClient(worker,'a'.repeat(64),{timeout:0});
 const controller=new AbortController(),task=client.call('decode',null,[],{signal:controller.signal});
 controller.abort();assert.equal(client.pending.size,1);assert.equal(worker.messages.at(-1).kind,'cancel');
 worker.reply(1,'error',{message:'E_CANCELLED'});
 await assert.rejects(task,/E_CANCELLED/);assert.equal(client.pending.size,0);client.close();
});
test('protocol/build mismatch terminates the worker and releases all callers',async()=>{
 const worker=new Worker(),client=new WorkerClient(worker,'a'.repeat(64),{timeout:0});
 const tasks=[client.call('batch',[]),client.call('batch',[])];
 worker.reply(1,'reply',null,'b'.repeat(64));
 await Promise.all(tasks.map(p=>assert.rejects(p,/E_WORKER_PROTOCOL/)));
 assert.equal(client.pending.size,0);assert.equal(worker.terminated,true);
});
test('snapshot delivery acknowledges one update and dispose rejects outstanding work',async()=>{
 const worker=new Worker(),client=new WorkerClient(worker,'a'.repeat(64),{timeout:0});let revisions=[];
 client.onUpdate=s=>revisions.push(s.revision);worker.reply(0,'update',{revision:3});
 assert.deepEqual(revisions,[3]);assert.equal(worker.messages.at(-1).kind,'ack');
 const task=client.call('fetch',null);client.close();await assert.rejects(task,/E_WORKER_DISPOSED/);
 assert.equal(client.pending.size,0);assert.equal(worker.terminated,true);
});
test('input flood coalesces moves without crossing press/release boundaries',async()=>{
 let release;const blocked=new Promise(r=>release=r),seen=[],errors=[];
 const queue=new SerialInputQueue(e=>errors.push(e),8);
 queue.push(async()=>{await blocked;seen.push('down');});
 for(let n=0;n<10000;n++)queue.push(n=>seen.push(n),n,'move:1');
 queue.push(()=>seen.push('up'));
 queue.push(()=>seen.push('hover'),null,'move:1');
 assert.equal(queue.jobs.length,3);release();
 await new Promise(r=>setImmediate(r));
 assert.deepEqual(seen,['down',9999,'up','hover']);assert.deepEqual(errors,[]);queue.close();
});
test('lifecycle cancellation invalidates in-flight input and bounds edge events',async()=>{
 let release;const blocked=new Promise(r=>release=r),seen=[],errors=[];
 const queue=new SerialInputQueue(e=>errors.push(e.message),3);
 queue.push(async(_,valid)=>{await blocked;if(valid())seen.push('late press');});
 queue.push(()=>seen.push('queued release'));queue.push(()=>seen.push('queued press'));
 queue.push(()=>seen.push('overflow'));
 assert.deepEqual(errors,['E_INPUT_CAPACITY']);assert.equal(queue.jobs.length,0);
 release();await new Promise(r=>setImmediate(r));assert.deepEqual(seen,[]);
 queue.close();queue.push(()=>seen.push('disposed'));assert.deepEqual(seen,[]);
});
test('a delayed GPU snapshot delivers its commands without replacing a newer view',()=>{
 const client={clockOffsetUs:0};
 const snapshot=(revision,commands=[])=>({revision,commands,host:JSON.stringify({session:1,interaction:revision}),requests:[2],contentRequests:[],profile:'[]'});
 const engine=new RemoteEngine(client,snapshot(1));
 engine.apply(snapshot(3,[{type:'get_assets',request:2}]));
 engine.apply(snapshot(2,[{type:'audio_pause',domain:'story'}]));
 assert.equal(JSON.parse(engine.host_state()).interaction,3);
 assert.deepEqual(JSON.parse(engine.commands()),[{type:'get_assets',request:2},{type:'audio_pause',domain:'story'}]);
 assert.equal(engine.accepts(2),true);assert.deepEqual(JSON.parse(engine.commands()),[]);
});
test('fatal overflow preserves its code and releases the reserved caller',async()=>{
 const worker=new Worker(),client=new WorkerClient(worker,'a'.repeat(64),{timeout:0});
 const call=client.call('batch',[]);
 worker.reply(0,'fatal',{code:'E_WORKER_CAPACITY',message:'runtime inbox'});
 await assert.rejects(call,/E_WORKER_CAPACITY: runtime inbox/);
 assert.equal(worker.terminated,true);assert.equal(client.pending.size,0);
});
for(const method of ['hidden','audio_blocked'])test(`${method} reaches Runtime after ordinary RPC admission is exhausted`,async()=>{
 const worker=new Worker(),client=new WorkerClient(worker,'a'.repeat(64),{capacity:16,timeout:0});
 const normal=Array.from({length:8},()=>client.call('batch',[]));
 const snapshot=revision=>({revision,commands:[],host:JSON.stringify({session:1,interaction:0}),requests:[],contentRequests:[],profile:'[]'});
 const engine=new RemoteEngine(client,snapshot(1));engine[method](true);
 const delivered=engine.sync(),message=worker.messages.at(-1);
 assert.equal(message.value[0].method,method);assert.equal(client.pending.size,9);
 worker.reply(message.id,'reply',{snapshot:snapshot(2),results:[null]});await delivered;
 for(const message of worker.messages.slice(0,8))worker.reply(message.id);
 await Promise.all(normal);client.close();
});
test('late loss of a replaced OffscreenCanvas cannot revoke the current surface',async()=>{
 const source=await fs.readFile(new URL('../../crates/nir-platform-web/runtime-worker.js',import.meta.url),'utf8');
 const global={self:{},performance:{now:()=>0},setInterval:()=>0,onmessage:null};vm.createContext(global);
 vm.runInContext(source+`
  const old={listeners:[],addEventListener(_,fn){this.listeners.push(fn);}};
  const fresh={listeners:[],addEventListener(_,fn){this.listeners.push(fn);}};
  canvas=old;watchCanvas();canvas=fresh;watchCanvas();watchCanvas();
  old.listeners[0]({preventDefault(){}});
  self.staleLost=contextLost;
  fresh.listeners[0]({preventDefault(){}});
  self.currentLost=contextLost;self.listeners=fresh.listeners.length;
 `,global);
 assert.equal(global.self.staleLost,false);assert.equal(global.self.currentLost,true);assert.equal(global.self.listeners,1);
});
