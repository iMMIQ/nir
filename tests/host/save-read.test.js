import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readSaveRecord,listSaveRecords,listHistoryRecords} from '../../crates/nir-platform-web/host.js';

function fake() {
 const request={result:undefined,error:null,onsuccess:null},tx={error:null,aborts:0,abort(){this.aborts++;},objectStore(){return {get:()=>request,openCursor:()=>request};}};
 const db={calls:0,transaction(name,mode){assert.equal(name,'saves');assert.equal(mode,'readonly');this.calls++;return tx;}};
 return {db,tx,request};
}
const range=()=>{const before=globalThis.IDBKeyRange;globalThis.IDBKeyRange={lowerBound:key=>key};return ()=>{if(before===undefined)delete globalThis.IDBKeyRange;else globalThis.IDBKeyRange=before;};};

test('save values including missing and signed zero publish only after native completion',async()=>{
 for(const value of [undefined,null,{zero:-0}]){
  const f=fake();let settled=false;const p=readSaveRecord(f.db,['key'],{timeoutMs:200}).then(v=>{settled=true;return v;});
  f.request.result=value;f.request.onsuccess();await Promise.resolve();assert.equal(settled,false);
  f.tx.oncomplete();assert.deepEqual(await p,value);assert.equal(f.request.onsuccess,null);assert.equal(f.tx.oncomplete,null);assert.equal(f.tx.aborts,0);
 }
});
test('stalled save read aborts, cleans up and ignores queued late callbacks',async()=>{
 const f=fake(),p=readSaveRecord(f.db,['key'],{timeoutMs:5});const success=f.request.onsuccess,complete=f.tx.oncomplete;
 await assert.rejects(p,/E_STORAGE_TIMEOUT: save read/);assert.equal(f.tx.aborts,1);assert.equal(f.request.onsuccess,null);assert.equal(f.tx.oncomplete,null);
 f.request.result={late:true};success();complete();assert.equal(f.tx.aborts,1);
});
test('save result without completion times out and transaction without get result cannot succeed',async()=>{
 const f=fake(),p=readSaveRecord(f.db,['key'],{timeoutMs:5});f.request.result={read:true};f.request.onsuccess();await assert.rejects(p,/E_STORAGE_TIMEOUT/);
 const g=fake(),q=readSaveRecord(g.db,['key']);g.tx.oncomplete();await assert.rejects(q,/E_STORAGE_READ/);
});
test('cancel and invalid save deadlines do not start reads or publish late results',async()=>{
 const controller=new AbortController(),f=fake(),p=readSaveRecord(f.db,['key'],{signal:controller.signal});controller.abort(new Error('cancelled'));
 await assert.rejects(p,/cancelled/);assert.equal(f.tx.aborts,1);
 const g=fake();await assert.rejects(readSaveRecord(g.db,['key'],{signal:controller.signal}),/cancelled/);assert.equal(g.db.calls,0);
 for(const timeoutMs of [0,-1,NaN,Infinity,1.5,0x80000000])await assert.rejects(readSaveRecord(g.db,['key'],{timeoutMs}),RangeError);
 assert.equal(g.db.calls,0);
});
test('native read failure and database access exception settle once without changing data',async()=>{
 const f=fake(),p=readSaveRecord(f.db,['key']);f.tx.error=new Error('read failed');f.tx.onerror();await assert.rejects(p,/read failed/);assert.equal(f.tx.aborts,1);
 await assert.rejects(readSaveRecord({transaction(){throw new Error('closed');}},['key']),/closed/);
});
test('history accumulates metadata and waits for cursor exhaustion and native completion',async()=>{
 const restore=range();try{
  const f=fake(),p=listHistoryRecords(f.db,'game','profile');let steps=0;
  f.request.result={primaryKey:['game','profile','bad',7],value:{saved_at:10,version:'second',envelope:{huge:'omitted'}},continue(){steps++;}};f.request.onsuccess();
  f.request.result={primaryKey:['game','profile'],value:{saved_at:20,version:'first'},continue(){steps++;}};f.request.onsuccess();
  f.request.result={primaryKey:['other','profile'],value:{},continue(){throw Error('outside prefix');}};f.request.onsuccess();
  f.tx.oncomplete();assert.deepEqual(await p,[{key:['game','profile'],savedAt:20,version:'first'},{key:['game','profile','bad',7],savedAt:10,version:'second'}]);assert.equal(steps,2);
 }finally{restore();}
});
test('timed out and cancelled history callbacks never continue a cursor or publish a partial list',async()=>{
 const restore=range();try{
  for(const cancel of [false,true]){
   const f=fake(),controller=new AbortController(),p=listHistoryRecords(f.db,'game','profile',{timeoutMs:5,signal:controller.signal});
   const success=f.request.onsuccess,complete=f.tx.oncomplete;let steps=0;
   f.request.result={primaryKey:['game','profile'],value:{},continue(){steps++;}};success();
   if(cancel)controller.abort(new Error('closed history'));
   await assert.rejects(p,cancel?/closed history/:/E_STORAGE_TIMEOUT: save history/);success();complete();assert.equal(steps,1);assert.equal(f.tx.aborts,1);
  }
  const f=fake(),p=listHistoryRecords(f.db,'game','profile');f.tx.oncomplete();await assert.rejects(p,/E_STORAGE_READ/);
 }finally{restore();}
});
test('slot listing reports each unreadable slot while keeping bounded inspection and order',async()=>{
 const rows=await listSaveRecords({transaction(){throw new Error('database unavailable');}},'game','profile','a'.repeat(64),()=>{throw new Error('must not inspect');});
 assert.deepEqual(rows.map(r=>r.slot),[0,1,2]);assert(rows.every(r=>r.error.includes('database unavailable')));
});
