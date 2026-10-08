// Opt-in CDP accounting for page and recursively created Worker targets.
// Network.loadingFinished is protocol-reported received bytes, not TLS/TCP
// wire traffic. Keep dataReceived's decoded/encoded body counters distinct.
// https://github.com/ChromeDevTools/devtools-protocol/blob/master/pdl/domains/Target.pdl
// https://github.com/ChromeDevTools/devtools-protocol/blob/master/pdl/domains/Network.pdl
// For an independent, version-checked transport measurement, process a
// flushed NetLog after closing the measurement browser. Exact peer matching
// excludes background traffic; SSL_* events would count decrypted data again.
// This is an opt-in diagnostic, never a dependency of the player itself.
export function summarizeNetLogSocketBytes(log,remoteAddress) {
  const types=log?.constants?.logEventTypes,phases=log?.constants?.logEventPhase,sources=log?.constants?.logSourceType;
  if(typeof remoteAddress!=='string'||!remoteAddress||!Array.isArray(log?.events))throw new TypeError('NetLog/remote address');
  for(const key of ['TCP_CONNECT','SOCKET_BYTES_RECEIVED','SOCKET_BYTES_SENT'])if(!Number.isInteger(types?.[key]))throw new Error(`NetLog event schema missing ${key}`);
  if(!Number.isInteger(phases?.PHASE_END)||!Number.isInteger(phases?.PHASE_NONE)||!Number.isInteger(sources?.SOCKET))throw new Error('NetLog phase/source schema');
  const sockets=new Map();
  for(const event of log.events)if(event.type===types.TCP_CONNECT&&event.phase===phases.PHASE_END&&event.params?.remote_address===remoteAddress){
    if(event.source?.type!==sources.SOCKET)throw new Error('NetLog TCP source is not SOCKET');
    sockets.set(event.source.id,{id:event.source.id,receivedBytes:0,sentBytes:0,receiveEvents:0,sendEvents:0});
  }
  for(const event of log.events){
    const socket=sockets.get(event.source?.id);if(!socket)continue;
    const incoming=event.type===types.SOCKET_BYTES_RECEIVED,outgoing=event.type===types.SOCKET_BYTES_SENT;if(!incoming&&!outgoing)continue;
    const count=event.params?.byte_count;
    if(event.source.type!==sources.SOCKET||event.phase!==phases.PHASE_NONE||!Number.isSafeInteger(count)||count<0)throw new Error('Invalid NetLog socket byte event');
    const key=incoming?'receivedBytes':'sentBytes';socket[key]+=count;socket[incoming?'receiveEvents':'sendEvents']++;
    if(!Number.isSafeInteger(socket[key]))throw new RangeError('NetLog socket byte overflow');
  }
  const rows=[...sockets.values()],total=key=>{const value=rows.reduce((sum,row)=>sum+row[key],0);if(!Number.isSafeInteger(value))throw new RangeError('NetLog byte overflow');return value;};
  return {remoteAddress,sockets:rows,receivedBytes:total('receivedBytes'),sentBytes:total('sentBytes'),
    scope:'SOCKET_BYTES_* for TCP sockets with the exact connected peer; HTTP headers/compressed data (and TLS records for TLS), excludes IP/TCP headers, retransmissions and QUIC. No inferred bytes for cached requests.',
    schema:'Event/source identifiers resolved from this log constants; validate again when Chromium changes.'};
}
// Pass a page CDPSession before navigation. Browser-level auto-attach only
// supports flattened transport; page-level sessions expose public nested CDP
// routing and capture the page's own network plus every descendant target.
export async function createNetworkTransferProbe(pageSession,{maxRecords=256}={}) {
  if(!Number.isSafeInteger(maxRecords)||maxRecords<0)throw new RangeError('maxRecords');
  const targets=new Map(),requests=new Map(),owners=new Map(),terminalIds=new Set(),ignoredIds=new Set(),records=[],retiredTargets=[],errors=[],setup=new Set();let closed=false,droppedRecords=0,droppedErrors=0,droppedTargets=0;
  const totals={requests:0,completed:0,failed:0,incomplete:0,redirects:0,lateObservedRequests:0,decodedBodyBytes:0,encodedBodyBytes:0,completedReceivedBytes:0,redirectReceivedBytes:0};
  const byTargetType={};
  const error=e=>{if(errors.length<32)errors.push(String(e));else droppedErrors++;};
  const keep=row=>{if(maxRecords===0){droppedRecords++;return;}if(records.length===maxRecords){records.shift();droppedRecords++;}records.push({...row});};
  const bucket=target=>byTargetType[target.type]??=Object.fromEntries(Object.keys(totals).map(k=>[k,0]));
  const add=(target,key,value=1)=>{totals[key]+=value;bucket(target)[key]+=value;};
  const remember=(set,id)=>{if(set.size===4096)set.delete(set.values().next().value);set.add(id);};
  function transfer(target,row) {
    const old=owners.get(row.requestId);if(old===target)return;
    old.requests.delete(row.requestId);target.requests.set(row.requestId,row);owners.set(row.requestId,target);
    for(const [key,n] of [['requests',1],['lateObservedRequests',row.initiationObserved?0:1],['decodedBodyBytes',row.decodedBodyBytes],['encodedBodyBytes',row.encodedBodyBytes]]){bucket(old)[key]-=n;bucket(target)[key]+=n;}
    row.targetId=target.id;row.targetType=target.type;
  }
  function begin(target,id,url,type,initiationObserved) {
    if(requests.size>=1024){error('Active request limit');remember(ignoredIds,id);return null;}
    const row={targetId:target.id,targetType:target.type,requestId:id,url,resourceType:type,initiationObserved,decodedBodyBytes:0,encodedBodyBytes:0,cached:false,serviceWorker:false};
    target.requests.set(id,row);requests.set(id,row);owners.set(id,target);add(target,'requests');if(!initiationObserved)add(target,'lateObservedRequests');return row;
  }
  function retire(target,row,status,bytes=0) {
    row.status=status;target.requests.delete(row.requestId);requests.delete(row.requestId);owners.delete(row.requestId);
    if(status!=='redirect')remember(terminalIds,row.requestId);
    add(target,status==='redirect'?'redirects':status);
    if(status==='completed')add(target,'completedReceivedBytes',bytes);
    if(status==='redirect')add(target,'redirectReceivedBytes',bytes);
    row.receivedBytes=bytes;keep(row);
  }
  function network(target,method,p) {
    if(target.protocolTrace.length<24)target.protocolTrace.push({method,requestId:p.requestId,url:p.request?.url||p.response?.url});
    if(method==='Network.requestWillBeSent') {
      const old=requests.get(p.requestId);
      if(old&&p.redirectResponse){transfer(target,old);old.httpStatus=p.redirectResponse.status;retire(target,old,'redirect',p.redirectResponse.encodedDataLength||0);}
      else if(old){if(old.url===p.request.url){transfer(target,old);return;}error(`Request identity collision: ${p.requestId}`);return;}
      if(!/^https?:/.test(p.request.url)){remember(ignoredIds,p.requestId);return;}
      terminalIds.delete(p.requestId);begin(target,p.requestId,p.request.url,p.type,true);return;
    }
    if(terminalIds.has(p.requestId)||ignoredIds.has(p.requestId))return;
    let row=requests.get(p.requestId);
    if(!row&&method==='Network.responseReceived'&&/^https?:/.test(p.response.url))row=begin(target,p.requestId,p.response.url,p.type,false);
    if(!row){if(method==='Network.loadingFinished')error(`Unattributed response completion: ${target.type}/${p.requestId}`);return;}
    // Worker bootstrap starts in its parent target and completes in the new
    // Worker session. Correlate the protocol request identity across sessions.
    transfer(target,row);
    if(method==='Network.responseReceived') {
      row.httpStatus=p.response.status;row.mimeType=p.response.mimeType;row.protocol=p.response.protocol;
      row.cached ||= !!(p.response.fromDiskCache||p.response.fromPrefetchCache);
      row.serviceWorker=!!p.response.fromServiceWorker;
    }else if(method==='Network.requestServedFromCache')row.cached=true;
    else if(method==='Network.dataReceived') {
      row.decodedBodyBytes+=p.dataLength;row.encodedBodyBytes+=p.encodedDataLength;
      add(target,'decodedBodyBytes',p.dataLength);add(target,'encodedBodyBytes',p.encodedDataLength);
    }else if(method==='Network.loadingFinished')retire(target,row,'completed',p.encodedDataLength);
    else if(method==='Network.loadingFailed'){row.cancelled=!!p.canceled;row.error=p.errorText;retire(target,row,'failed');}
  }
  function bind(parent,{sessionId,targetInfo,waitingForDebugger}) {
    const target={id:targetInfo.targetId,type:targetInfo.type,url:targetInfo.url,waitingForDebugger,protocolTrace:[],sessionId,parent,children:new Map(),requests:new Map(),pending:new Map(),sequence:0,networkEnabled:false,detached:false};
    parent.children.set(sessionId,target);targets.set(sessionId,target);
    target.send=(method,params={})=>new Promise((resolve,reject)=>{
      if(target.detached){reject(new Error('CDP target detached'));return;}
      const id=++target.sequence;
      const timer=setTimeout(()=>{target.pending.delete(id);reject(new Error(`CDP ${target.type} ${method} timeout`));},5000);
      target.pending.set(id,{resolve,reject,timer});
      parent.send('Target.sendMessageToTarget',{sessionId,message:JSON.stringify({id,method,params})}).catch(e=>{
        const pending=target.pending.get(id);if(!pending)return;target.pending.delete(id);clearTimeout(timer);reject(e);
      });
    });
    const task=(async()=>{
      try{
        await target.send('Network.enable');target.networkEnabled=true;
        await target.send('Target.setAutoAttach',{autoAttach:true,waitForDebuggerOnStart:true,flatten:false});
      }catch(e){if(!closed&&!target.detached)error(e);}
      finally{try{await target.send('Runtime.runIfWaitingForDebugger');}catch(e){if(!closed&&!target.detached)error(e);}}
    })();setup.add(task);task.finally(()=>setup.delete(task));
  }
  function detach(target) {
    if(!target||target.detached)return;target.detached=true;
    for(const child of target.children.values())detach(child);target.children.clear();
    for(const pending of target.pending.values()){clearTimeout(pending.timer);pending.reject(new Error('CDP target detached'));}target.pending.clear();
    for(const row of [...target.requests.values()]){row.error='Target detached before terminal response';retire(target,row,'incomplete');}
    targets.delete(target.sessionId);target.parent.children.delete(target.sessionId);
    const summary={id:target.id,type:target.type,url:target.url,protocolTrace:target.protocolTrace,waitingForDebugger:target.waitingForDebugger,networkEnabled:target.networkEnabled,detached:true,activeRequests:0};
    if(maxRecords===0)droppedTargets++;
    else {if(retiredTargets.length===maxRecords){retiredTargets.shift();droppedTargets++;}retiredTargets.push(summary);}
  }
  function dispatch(target,message) {
    if(message.id!==undefined){const pending=target.pending.get(message.id);if(!pending)return;target.pending.delete(message.id);clearTimeout(pending.timer);if(message.error)pending.reject(new Error(JSON.stringify(message.error)));else pending.resolve(message.result);return;}
    const p=message.params;
    if(message.method==='Target.attachedToTarget')bind(target,p);
    else if(message.method==='Target.receivedMessageFromTarget'){const child=target.children.get(p.sessionId);if(child)dispatch(child,JSON.parse(p.message));}
    else if(message.method==='Target.detachedFromTarget')detach(target.children.get(p.sessionId));
    else if(message.method?.startsWith('Network.'))network(target,message.method,p);
  }
  const {targetInfo}=await pageSession.send('Target.getTargetInfo');
  const root={send:(method,params)=>pageSession.send(method,params),id:targetInfo.targetId,type:targetInfo.type,url:targetInfo.url,protocolTrace:[],sessionId:'page-root',parent:{children:new Map()},children:new Map(),requests:new Map(),pending:new Map(),networkEnabled:false,detached:false};
  targets.set(root.sessionId,root);
  const attach=p=>bind(root,p),receive=p=>{const target=root.children.get(p.sessionId);if(target)dispatch(target,JSON.parse(p.message));},removed=p=>detach(root.children.get(p.sessionId));
  const listeners=[['Target.attachedToTarget',attach],['Target.receivedMessageFromTarget',receive],['Target.detachedFromTarget',removed],...['requestWillBeSent','responseReceived','requestServedFromCache','dataReceived','loadingFinished','loadingFailed'].map(name=>[`Network.${name}`,p=>network(root,`Network.${name}`,p)])];
  for(const [name,handler] of listeners)pageSession.on(name,handler);
  try{await pageSession.send('Network.enable');root.networkEnabled=true;await pageSession.send('Target.setAutoAttach',{autoAttach:true,waitForDebuggerOnStart:true,flatten:false});}
  catch(e){for(const [name,handler] of listeners)pageSession.off(name,handler);throw e;}
  return {
    snapshot(){return {scope:'CDP HTTP(S) page/recursive Worker response counters; no TLS/TCP overhead or retransmission measurement',limits:['Settled observed requests do not prove complete capture of every request.','Module Worker bootstrap may lack CDP completion; retain unfinished rows instead of inferring receipt.','dataReceived encoded lengths can remain zero for received payloads; these chunk counters are not total downloads.','Late-observed requests lack initiation timing even if completion is observed.'],observedRequestsSettled:requests.size===0&&totals.incomplete===0&&totals.lateObservedRequests===0&&errors.length===0&&droppedErrors===0,totals:{...totals},byTargetType:structuredClone(byTargetType),targets:[...retiredTargets,...[...targets.values()].map(t=>({id:t.id,type:t.type,url:t.url,protocolTrace:t.protocolTrace.map(r=>({...r})),waitingForDebugger:t.waitingForDebugger,networkEnabled:t.networkEnabled,detached:false,activeRequests:t.requests.size}))],activeRequests:requests.size,activeRequestDetails:[...requests.values()].slice(0,maxRecords).map(r=>({...r})),records:records.map(r=>({...r})),droppedRecords,droppedTargets,errors:[...errors],droppedErrors};},
    async close(){
      if(closed)return;closed=true;
      try{await pageSession.send('Target.setAutoAttach',{autoAttach:false,waitForDebuggerOnStart:false,flatten:false});}catch(e){error(e);}
      await Promise.allSettled([...setup]);
      for(const [name,handler] of listeners)pageSession.off(name,handler);
      for(const target of [...targets.values()])detach(target);
    },
  };
}
