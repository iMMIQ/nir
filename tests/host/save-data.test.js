import {test} from 'node:test';
import assert from 'node:assert/strict';
import {stringifyPlayerData,inspectSaveRecord,RemoteEngine,validHistoryKey,saveHistoryExport} from '../../crates/nir-platform-web/host.js';

test('snapshot transport preserves negative zero and user text or keys resembling markers',()=>{
    const value={zero:-0,normal:0,text:'__nir_json_negative_zero_0__',
        '__nir_json_negative_zero_1__':['__nir_json_negative_zero_2__',-0]};
    const decoded=JSON.parse(stringifyPlayerData(value));
    assert.deepEqual(decoded,value);
    assert(Object.is(decoded.zero,-0));
    assert(Object.is(decoded.__nir_json_negative_zero_1__[1],-0));
    assert.equal(stringifyPlayerData(undefined),undefined);
    assert.equal(stringifyPlayerData({one:1,text:'unchanged'}),JSON.stringify({one:1,text:'unchanged'}));
});

test('Worker command transport retains the actual snapshot number before persistence',()=>{
    const snapshot={revision:1,commands:[{type:'save',envelope:{snapshot:{scene:[{x:-0}]}}}],
        host:JSON.stringify({session:1,interaction:1}),requests:[],contentRequests:[],profile:'[]'};
    const remote=new RemoteEngine({clockOffsetUs:0},snapshot);
    const command=JSON.parse(remote.commands())[0];
    assert(Object.is(command.envelope.snapshot.scene[0].x,-0));
});

test('missing and unreadable records differ, and identity failures never reach the payload validator',async()=>{
    const key=['game','release','a'.repeat(64),0];let calls=0;
    const inspect=async()=>{calls++;return 1;};
    assert.equal(await inspectSaveRecord(undefined,key,inspect),null);
    for(const bad of [null,false,{},[],{gameId:'another',profile:key[1],releaseDigest:key[2],slot:0}])
        await assert.rejects(inspectSaveRecord(bad,key,inspect),/E_SAVE_IDENTITY/);
    assert.equal(calls,0);
    const metadata={gameId:key[0],profile:key[1],releaseDigest:key[2],slot:0};
    for(const envelope of [undefined,null,false,[],1,'bad'])
        await assert.rejects(inspectSaveRecord({...metadata,envelope},key,inspect),/E_SAVE_IDENTITY/);
    assert.equal(calls,0);
    await assert.rejects(inspectSaveRecord({...metadata,envelope:{}},key),/E_SAVE_INSPECT/);
});


test('history keys are validated without trusting record metadata',()=>{
    const key=['game','release','a'.repeat(64),0];assert(validHistoryKey(key));
    for(const bad of [null,[],key.slice(0,3),[...key,1],[...key.slice(0,3),7],['game','release',['bad'],0]])
        assert.equal(validHistoryKey(bad),false);
});

test('recovery exports preserve raw damaged values and differ from save envelopes',()=>{
    const key=['game','release','a'.repeat(64),0];
    for(const record of [null,42,{unexpected:'preserve',zero:-0},undefined]){
        const recovered=JSON.parse(saveHistoryExport({key,record},false));
        assert.equal(recovered.format,'nir-save-record-recovery-v1');assert.deepEqual(recovered.key,key);
        assert.deepEqual(recovered.record,record);
        assert.equal(recovered.recordUndefined,record===undefined?true:undefined);
    }
    const envelope={snapshot:{x:-0}};
    assert.deepEqual(JSON.parse(saveHistoryExport({key,record:{envelope}},true)),envelope);
    const cyclic={};cyclic.self=cyclic;
    assert.throws(()=>saveHistoryExport({key,record:cyclic},false),/circular|cyclic/i);
    for(const record of [new ArrayBuffer(2),new Uint8Array([7,9]),new Date(),new Map([['a',1]]),NaN,{nested:undefined},[,1]])
        assert.throws(()=>saveHistoryExport({key,record},false),/E_HISTORY_EXPORT/);
});
