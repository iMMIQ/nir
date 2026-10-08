import {test} from 'node:test';
import assert from 'node:assert/strict';
import {summarizeNetLogSocketBytes} from '../performance/network-transfer.js';
const log=()=>({constants:{logEventTypes:{TCP_CONNECT:19,SOCKET_BYTES_RECEIVED:87,SOCKET_BYTES_SENT:88,SSL_SOCKET_BYTES_RECEIVED:89},logEventPhase:{PHASE_END:2,PHASE_NONE:0},logSourceType:{SOCKET:10}},events:[
  {type:19,phase:2,source:{id:60,type:10},params:{remote_address:'127.0.0.1:45405'}},
  {type:87,phase:0,source:{id:60,type:10},params:{byte_count:426}},
  {type:88,phase:0,source:{id:60,type:10},params:{byte_count:91}},
  {type:89,phase:0,source:{id:60,type:10},params:{byte_count:400}},
  {type:19,phase:2,source:{id:61,type:10},params:{remote_address:'127.0.0.1:45406'}},
  {type:87,phase:0,source:{id:61,type:10},params:{byte_count:9000}},
  {type:19,phase:2,source:{id:62,type:10},params:{remote_address:'127.0.0.1:45405'}},
  {type:87,phase:0,source:{id:62,type:10},params:{byte_count:1024}},
]});
test('socket accounting filters the exact peer, includes both connections and avoids SSL double counting',()=>{
  const result=summarizeNetLogSocketBytes(log(),'127.0.0.1:45405');
  assert.equal(result.receivedBytes,1450);assert.equal(result.sentBytes,91);assert.equal(result.sockets.length,2);
  assert.equal(summarizeNetLogSocketBytes(log(),'127.0.0.1:1').receivedBytes,0);
});
test('missing schemas, invalid events and unsafe byte counts fail rather than manufacturing a download total',()=>{
  const invalid=log();delete invalid.constants.logEventTypes.SOCKET_BYTES_RECEIVED;
  assert.throws(()=>summarizeNetLogSocketBytes(invalid,'127.0.0.1:45405'),/schema/);
  for(const count of [-1,NaN,Infinity,1.5,Number.MAX_SAFE_INTEGER+1]){
    const invalid=log();invalid.events[1].params.byte_count=count;
    assert.throws(()=>summarizeNetLogSocketBytes(invalid,'127.0.0.1:45405'),/Invalid/);
  }
  const overflow=log();overflow.events[1].params.byte_count=Number.MAX_SAFE_INTEGER;
  overflow.events.push({type:87,phase:0,source:{id:60,type:10},params:{byte_count:1}});
  assert.throws(()=>summarizeNetLogSocketBytes(overflow,'127.0.0.1:45405'),/overflow/);
});
