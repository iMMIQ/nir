import {chromium} from '@playwright/test';
import {summarizeNetLogSocketBytes} from '../tests/performance/network-transfer.js';
import http from 'node:http';
import {gzipSync} from 'node:zlib';
import fs from 'node:fs/promises';
import path from 'node:path';
import assert from 'node:assert/strict';
// Run with Node: the calibration requires real HTTP socket byte counters.
// CHROMIUM optionally selects an installed browser; artifacts are never overwritten.
const output=process.env.NIR_NETWORK_REPORT||'reports/network-transfer-calibration.json';assert.equal(await fs.stat(output).catch(()=>null),null);
await fs.mkdir(path.dirname(output),{recursive:true});
const files={
 '/':Buffer.from(`<link rel="icon" href="data:,"><script>globalThis.result=null;Promise.all([fetch('/page.bin').then(r=>r.arrayBuffer()).then(b=>b.byteLength),new Promise(ok=>{const w=new Worker('/worker.js',{name:'parent',type:'module'});w.onmessage=e=>ok(e.data);})]).then(result=>globalThis.result=result)</script>`),
 '/worker.js':Buffer.from(`Promise.all([fetch('/worker.bin').then(r=>r.arrayBuffer()).then(b=>b.byteLength),fetch('/gzip.bin').then(r=>r.arrayBuffer()).then(b=>b.byteLength),new Promise(ok=>{const w=new Worker('/nested.js',{type:'module'});w.onmessage=e=>ok(e.data);})]).then(r=>postMessage(r))`),
 '/nested.js':Buffer.from(`fetch('/nested.bin').then(r=>r.arrayBuffer()).then(b=>postMessage(b.byteLength))`),
 '/page.bin':Buffer.alloc(4111,1),'/worker.bin':Buffer.alloc(8193,2),'/nested.bin':Buffer.alloc(16385,3),'/gzip.bin':gzipSync(Buffer.alloc(32768,4)),
 '/redirected.bin':Buffer.alloc(73,5),'/cached.bin':Buffer.alloc(257,6),
};
const report={status:'running',server:[],limits:['Controlled HTTP loopback with page, Worker and nested Worker; socket payload lengths are not cellular wire bytes.','Synthetic byte-counter calibration, not game latency, GPU allocations or physical memory budgets.']};
let browser,session;const sockets=[];let pageReceivedBytes=0;
const server=http.createServer((req,res)=>{
 const p=new URL(req.url,'http://local').pathname;
 if(p==='/redirect'){res.writeHead(302,{Location:'/redirected.bin','Content-Length':0});res.end();report.server.push({path:p,bodyBytes:0});return;}
 if(p==='/abort.bin') {res.writeHead(200,{'Content-Type':'application/octet-stream','Content-Length':65536,'Cache-Control':'no-store'});res.write(Buffer.alloc(1024,9));const timer=setTimeout(()=>res.end(Buffer.alloc(64512,9)),1000);res.on('close',()=>clearTimeout(timer));report.server.push({path:p,bodyBytes:65536,partial:true});return;}
 const body=files[p];if(!body){res.writeHead(404);res.end();report.server.push({path:p,bodyBytes:0,status:404});return;}
 const headers={'Content-Type':p==='/'?'text/html':p.endsWith('.js')?'text/javascript':'application/octet-stream','Content-Length':body.length,'Cache-Control':p==='/cached.bin'?'public,max-age=3600':'no-store'};if(p==='/gzip.bin')headers['Content-Encoding']='gzip';
 res.writeHead(200,headers);res.end(body);report.server.push({path:p,bodyBytes:body.length});
});
server.on('connection',socket=>sockets.push(socket));await new Promise(ok=>server.listen(0,'127.0.0.1',ok));report.peer=`127.0.0.1:${server.address().port}`;report.netlog=output+'.netlog.json';assert.equal(await fs.stat(report.netlog).catch(()=>null),null);
try {
 browser=await chromium.launch({executablePath:process.env.CHROMIUM||undefined,headless:true,args:['--no-proxy-server','--log-net-log='+report.netlog]});report.browserVersion=browser.version();
 const context=await browser.newContext(),page=await context.newPage();session=await context.newCDPSession(page);await session.send('Network.enable');session.on('Network.loadingFinished',e=>pageReceivedBytes+=e.encodedDataLength);
 await page.goto(`http://${report.peer}/`);await page.waitForFunction(()=>globalThis.result!==null,null,{timeout:15000});assert.deepEqual(await page.evaluate(()=>result),[4111,[8193,32768,16385]]);
 await page.evaluate(async()=>{await fetch('/redirect').then(r=>r.arrayBuffer());await fetch('/cached.bin').then(r=>r.arrayBuffer());await fetch('/cached.bin').then(r=>r.arrayBuffer());const abort=new AbortController();const r=await fetch('/abort.bin',{signal:abort.signal});await r.body.getReader().read();abort.abort();});await page.waitForTimeout(300);
 await browser.close();browser=null;
 report.socketMeasurement=summarizeNetLogSocketBytes(JSON.parse(await fs.readFile(report.netlog,'utf8')),report.peer);
 report.serverSocketBytesWritten=sockets.reduce((sum,socket)=>sum+socket.bytesWritten,0);report.pageCdpReceivedBytes=pageReceivedBytes;
 assert(sockets.length>0,'HTTP connection events and socket counters are required; run using Node');assert.equal(report.server.filter(r=>r.path==='/cached.bin').length,1);assert.equal(report.server.filter(r=>r.path==='/worker.js').length,1);assert.equal(report.server.filter(r=>r.path==='/nested.js').length,1);
 assert.equal(report.socketMeasurement.receivedBytes,report.serverSocketBytesWritten,'NetLog must match actual HTTP socket writes including both module bootstraps, headers, gzip, redirect and partial aborted body');
 assert(report.socketMeasurement.receivedBytes>pageReceivedBytes+8193+16385);
 report.status='passed-full-peer-http-socket-calibration';
}catch(e){report.status='failed';report.error=String(e);throw e;}
finally{await browser?.close();await new Promise(ok=>server.close(ok));report.finished=new Date().toISOString();report.runtime=process.version;report.pid=process.pid;await fs.writeFile(output,JSON.stringify(report,null,2)+'\n');}
console.log(JSON.stringify({status:report.status,received:report.socketMeasurement?.receivedBytes,serverSocketWrites:report.serverSocketBytesWritten,pageOnlyCdp:report.pageCdpReceivedBytes}));
