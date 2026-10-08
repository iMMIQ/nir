import {test} from 'node:test';
import assert from 'node:assert/strict';
import {openSaveDatabase,SaveDatabaseConnection} from '../../crates/nir-platform-web/host.js';

function deferred(){let resolve,reject;const promise=new Promise((ok,no)=>{resolve=ok;reject=no;});return {promise,resolve,reject};}
function database(){return {closed:0,close(){this.closed++;}};}
function openFixture(){const request={transaction:null},factory={calls:0,open(){this.calls++;return request;}};return {request,factory};}

test('denied database open rejects the original error without leaving a timer',async()=>{
  const error=new DOMException('denied','SecurityError');let calls=0;
  await assert.rejects(openSaveDatabase({open(){calls++;throw error;}}),e=>e===error);assert.equal(calls,1);
});
test('blocked open settles once; a resumed upgrade is aborted and its late connection closed',async()=>{
  const {request,factory}=openFixture(),db=database();let aborted=0,created=0;
  const p=openSaveDatabase(factory);request.onblocked();await assert.rejects(p,/E_STORAGE_BLOCKED/);
  request.transaction={abort(){aborted++;}};request.result={...db,objectStoreNames:{contains(){return false;}},createObjectStore(){created++;}};
  request.onupgradeneeded();request.onsuccess();assert.equal(aborted,1);assert.equal(created,0);assert.equal(request.result.closed,1);assert.equal(factory.calls,1);
});
test('an open with no browser callback times out; late success cannot escape cleanup',async()=>{
  const {request,factory}=openFixture(),db=database();const p=openSaveDatabase(factory,{timeoutMs:10});await assert.rejects(p,/E_STORAGE_TIMEOUT/);
  request.result=db;request.onsuccess();assert.equal(db.closed,1);assert.equal(factory.calls,1);
});
test('cancel aborts an active upgrade and suppresses late completion; pre-aborted calls never open',async()=>{
  const {request,factory}=openFixture(),controller=new AbortController(),db=database();let aborted=0;
  request.transaction={abort(){aborted++;}};const p=openSaveDatabase(factory,{signal:controller.signal});controller.abort();await assert.rejects(p,e=>e.name==='AbortError');assert.equal(aborted,1);
  request.result=db;request.onsuccess();assert.equal(db.closed,1);
  await assert.rejects(openSaveDatabase(factory,{signal:controller.signal}),e=>e.name==='AbortError');assert.equal(factory.calls,1);
});
test('concurrent storage operations share one open; only a new operation retries an open failure',async()=>{
  const first=deferred(),db=database(),error=new Error('offline');let calls=0,executed=0;
  const connection=new SaveDatabaseConnection(()=>{calls++;return calls===1?first.promise:db;});
  const a=connection.run(()=>{executed++;}),b=connection.run(()=>{executed++;});await Promise.resolve();assert.equal(calls,1);
  first.reject(error);await Promise.all([assert.rejects(a,e=>e===error),assert.rejects(b,e=>e===error)]);assert.equal(executed,0);assert.equal(calls,1);assert.equal(connection.pending,null);
  assert.equal(await connection.run(current=>{executed++;assert.equal(current,db);return 'saved';}),'saved');assert.equal(calls,2);assert.equal(executed,1);connection.close();
});
test('lost or version-changed connections reopen on the next operation without replaying uncertain work',async()=>{
  const databases=[database(),database(),database()];let calls=0,writes=0;const connection=new SaveDatabaseConnection(()=>databases[calls++]);
  await connection.run(()=>{writes++;throw new DOMException('lost','InvalidStateError');}).then(()=>assert.fail('must fail'),e=>assert.equal(e.name,'InvalidStateError'));
  assert.equal(writes,1);assert.equal(calls,1);assert.equal(databases[0].closed,1);
  await connection.run(()=>{writes++;});assert.equal(calls,2);databases[1].onversionchange();assert.equal(databases[1].closed,1);
  await connection.run(()=>{writes++;});assert.equal(calls,3);assert.equal(writes,3);connection.close();
});
test('unexpected close invalidates the handle but does not trigger an automatic open',async()=>{
  const db=database();let opens=0;const connection=new SaveDatabaseConnection(()=>{opens++;return db;});await connection.connect();db.onclose();assert.equal(opens,1);assert.equal(connection.db,null);await connection.connect();assert.equal(opens,2);connection.close();
});
test('dispose cancels pending open and prevents work; a custom late resolver is still closed',async()=>{
  const first=deferred(),db=database();let signal,work=0;const connection=new SaveDatabaseConnection(s=>{signal=s;return first.promise;});
  const p=connection.run(()=>{work++;});await Promise.resolve();connection.close();assert(signal.aborted);first.resolve(db);await assert.rejects(p,/E_STORAGE_CLOSED/);assert.equal(db.closed,1);assert.equal(work,0);await assert.rejects(connection.connect(),/E_STORAGE_CLOSED/);
});
test('dispose before the open microtask makes no factory request',async()=>{
  let opens=0;const connection=new SaveDatabaseConnection(()=>{opens++;return database();});const p=connection.connect();connection.close();await assert.rejects(p,/E_STORAGE_CLOSED/);assert.equal(opens,0);
});
