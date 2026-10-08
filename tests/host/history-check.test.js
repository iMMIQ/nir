import {test} from 'node:test';
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {createHash} from 'node:crypto';
import {checkSavedRelease,validateHistoryTarget,SharedRequests,WorkerClient,WORKER_PROTOCOL} from '../../crates/nir-platform-web/host.js';
const gate=()=>{let resolve;return {promise:new Promise(r=>resolve=r),resolve};};
const hash=bytes=>createHash('sha256').update(bytes).digest('hex');
const pause=ms=>new Promise(r=>setTimeout(r,ms));
const html=Buffer.from('<html>bounded history fixture</html>');
const manifest=Buffer.from(JSON.stringify({format:1,game_id:'fixture',profile:'release',launch:{html:hash(html)}}));
// Bun's node:http compatibility layer does not deliver response close while
// a reply is held. Use an actual Node TCP server for the physical assertion;
// the client and production helper still run in the selected test runtime.
async function fixture(stage,run){
 const previous=globalThis.location,arrival=gate(),release=gate(),closed=gate(),ready=gate();let responseClosed=false;
 const code=`
  const {createServer}=require('node:http');
  const [stage,rawManifest,rawHtml]=process.argv.slice(1),manifest=Buffer.from(rawManifest,'base64'),html=Buffer.from(rawHtml,'base64');let replies=[];
  const server=createServer((req,res)=>{
   const json=req.url.endsWith('.json');
   if((stage==='manifest'&&json)||(stage==='player-body'&&!json)){
    res.on('close',()=>process.send?.({kind:'closed'}));
    if(stage==='player-body'){res.writeHead(200,{'Content-Type':'text/html'});res.write(html.subarray(0,8));}
    process.send({kind:'arrived'});replies.push(()=>res.end(stage==='player-body'?html.subarray(8):manifest));return;
   }
   res.end(json?manifest:html);
  });
  process.on('message',message=>{
   if(message==='release')for(const reply of replies.splice(0))reply();
   if(message==='stop'){server.closeAllConnections();server.close(()=>process.disconnect());}
  });
  server.listen(0,'127.0.0.1',()=>process.send({kind:'ready',port:server.address().port}));
 `;
 const child=spawn('node',['-e',code,stage,manifest.toString('base64'),html.toString('base64')],{stdio:['ignore','ignore','inherit','ipc'],serialization:'json'});
 const exited=new Promise(resolve=>child.once('exit',resolve));
 child.on('message',message=>{if(message.kind==='ready')ready.resolve(message.port);if(message.kind==='arrived')arrival.resolve();if(message.kind==='closed'){responseClosed=true;closed.resolve();}});
 release.promise.then(()=>{if(child.connected)child.send('release');});
 const port=await ready.promise,site=`http://127.0.0.1:${port}/`;globalThis.location=new URL(site);
 try{await run({options:{releaseRoot:site,digest:hash(manifest),gameId:'fixture',profile:'release'},arrival,release,closed,isClosed:()=>responseClosed});}
 finally{release.resolve();if(child.connected)child.send('stop');await exited;if(previous===undefined)delete globalThis.location;else globalThis.location=previous;}
}

test('one read/inspection/network budget cancels a later phase without admitting a retry',async()=>{
 const first=gate(),second=gate(),started=gate();let signals=[],admissions=0;
 const check=checkSavedRelease(async signal=>{admissions++;signals.push(signal);first.resolve();await started.promise;signals.push(signal);second.resolve();await new Promise(()=>{});},{timeoutMs:100});
 await first.promise;await pause(30);started.resolve();await second.promise;
 await assert.rejects(check,/E_STORAGE_TIMEOUT/);assert.equal(admissions,1);assert.equal(signals[0],signals[1]);assert(signals[0].aborted);
});
test('finished and rejected attempts clean their caller listener; pre-cancel and invalid budgets admit no work',async()=>{
 const parent=new AbortController();let listeners=0,calls=0;
 const add=parent.signal.addEventListener.bind(parent.signal),remove=parent.signal.removeEventListener.bind(parent.signal);
 parent.signal.addEventListener=(...args)=>{listeners++;return add(...args);};parent.signal.removeEventListener=(...args)=>{listeners--;return remove(...args);};
 assert.equal(await checkSavedRelease(()=>42,{signal:parent.signal}),42);assert.equal(listeners,0);
 for(const error of [new Error('inspection failure'),null,undefined])await assert.rejects(checkSavedRelease(()=>Promise.reject(error),{signal:parent.signal}),e=>e===error);
 assert.equal(listeners,0);parent.abort(new Error('panel closed'));
 await assert.rejects(checkSavedRelease(()=>calls++,{signal:parent.signal}),/panel closed/);assert.equal(listeners,0);
 for(const timeoutMs of [0,-1,NaN,Infinity,1.5,0x80000000])await assert.rejects(checkSavedRelease(()=>calls++,{timeoutMs}),RangeError);
 assert.equal(calls,0);
});
test('synchronous validation returning beyond the deadline cannot claim success',async()=>{
 await assert.rejects(checkSavedRelease(()=>{const until=performance.now()+12;while(performance.now()<until){}return 'Available';},{timeoutMs:5}),/E_STORAGE_TIMEOUT/);
});
test('timed out inspection keeps the physical Worker reservation and does not terminate playback',async()=>{
 const worker={messages:[],terminated:false,postMessage(m){this.messages.push(m);},terminate(){this.terminated=true;}};
 const client=new WorkerClient(worker,'a'.repeat(64),{timeout:0});
 await assert.rejects(checkSavedRelease(signal=>client.call('inspect-save',{json:'fixture'},[],{signal,timeout:0}),{timeoutMs:10}),/E_STORAGE_TIMEOUT/);
 assert.equal(client.pending.size,1);assert.equal(worker.terminated,false);assert.equal(worker.messages.at(-1).kind,'cancel');
 const reply=(id,value)=>worker.onmessage({data:{protocol:WORKER_PROTOCOL,build:'a'.repeat(64),kind:'reply',id,value}});
 const reading=client.call('batch',[]);reply(worker.messages.at(-1).id,'reading continues');assert.equal(await reading,'reading continues');
 reply(1,17);await Promise.resolve();assert.equal(client.pending.size,0);assert.equal(worker.terminated,false);client.close();
});
for(const stage of ['manifest','player-body'])for(const cancelled of [false,true])test(`actual HTTP ${stage} aborts on ${cancelled?'panel close':'deadline'}`,async()=>{
 await fixture(stage,async({options,arrival,release,closed,isClosed})=>{
  const controller=new AbortController();const check=validateHistoryTarget({...options,signal:controller.signal,timeoutMs:cancelled?3000:100});
  const failure=assert.rejects(check,cancelled?/panel closed/:/E_STORAGE_TIMEOUT/);
  await arrival.promise;if(cancelled)controller.abort(new Error('panel closed'));await failure;
  await Promise.race([closed.promise,pause(1000).then(()=>{throw new Error('actual HTTP response did not close');})]);assert(isClosed());release.resolve();
 });
});
test('cancelling one shared target consumer does not abort the other; last consumer closes actual HTTP',async()=>{
 await fixture('manifest',async({options,arrival,closed,isClosed})=>{
  let fetches=0;const shared=new SharedRequests((_id,signal)=>{fetches++;return validateHistoryTarget({...options,signal});});
  const a=new AbortController(),b=new AbortController();const first=shared.get('release',a.signal),second=shared.get('release',b.signal);
  const firstFailure=assert.rejects(first,/first closed/),secondFailure=assert.rejects(second,/last closed/);
  await arrival.promise;a.abort(new Error('first closed'));await firstFailure;assert(!isClosed());assert.equal(fetches,1);
  b.abort(new Error('last closed'));await secondFailure;await closed.promise;assert(isClosed());assert.equal(shared.jobs.size,0);
 });
});
test('late uncancellable hash cannot issue another fetch or claim an available target',async()=>{
 const previous=globalThis.location;globalThis.location=new URL('https://fixture.invalid/');const started=gate(),release=gate();let fetches=0;
 try{
  const check=validateHistoryTarget({releaseRoot:location.href,digest:hash(manifest),gameId:'fixture',profile:'release',timeoutMs:20,
   fetchImpl:async()=>{fetches++;return new Response(manifest);},sha256:async bytes=>{started.resolve();await release.promise;return hash(bytes);}});
  const failure=assert.rejects(check,/E_STORAGE_TIMEOUT/);await started.promise;await failure;release.resolve();await pause(10);assert.equal(fetches,1);
 }finally{release.resolve();if(previous===undefined)delete globalThis.location;else globalThis.location=previous;}
});
