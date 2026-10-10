import {test} from 'node:test';
import assert from 'node:assert/strict';
import {PersistenceWrites} from '../../crates/nir-platform-web/host.js';

function fixture() {
    const jobs=[],writes=[],events=[];
    const queue=new PersistenceWrites({
        request:(work,success,failure)=>jobs.push({work,success,failure}),
        writePreferences:value=>writes.push({kind:'preferences',value}),
        mergeProfile:value=>writes.push({kind:'profile',value}),
        writeProfileValues:value=>writes.push({kind:'profile_values',value}),
        stored:kind=>events.push({kind,ok:true}),
        failed:(kind,error)=>events.push({kind,error,ok:false}),
    });
    const finish=async(index,error)=>{
        await jobs[index].work();
        if(error)jobs[index].failure(error);else jobs[index].success();
    };
    return {queue,jobs,writes,events,finish};
}

test('blocked preference writes retain only the newest update and owner completion starts it',async()=>{
    const f=fixture();f.queue.submit('preferences',{volume:0});
    for(let i=1;i<=200;i++)f.queue.submit('preferences',{volume:i});
    assert.equal(f.jobs.length,1);
    await f.jobs[0].work();assert.equal(f.jobs.length,1); // No owner delivery yet.
    f.jobs[0].success();assert.equal(f.jobs.length,2);assert.deepEqual(f.events,[]);
    await f.finish(1);
    assert.deepEqual(f.writes,[{kind:'preferences',value:{volume:0}},{kind:'preferences',value:{volume:200}}]);
    assert.deepEqual(f.events,[{kind:'preferences',ok:true}]);
    assert.equal(f.queue.lanes.get('preferences').value,null);
});

test('a failed progress delta survives until a later successful union without retry spinning',async()=>{
    const f=fixture();const error=new Error('quota');f.queue.submit('profile',['first']);
    await f.finish(0,error);assert.equal(f.jobs.length,1);assert.deepEqual(f.events,[{kind:'profile',ok:false,error}]);
    f.queue.submit('profile',['later']);await f.finish(1);
    assert.deepEqual(f.writes[1].value,['first','later']);assert.equal(f.queue.lanes.get('profile').keys.size,0);
    assert.deepEqual(f.events[1],{kind:'profile',ok:true});
});

test('updates during a failing write merge in one follow-up and deduplicate already committed keys',async()=>{
    const f=fixture();f.queue.submit('profile',['first']);
    for(let i=0;i<200;i++)f.queue.submit('profile',['first',`next${i}`]);
    assert.equal(f.jobs.length,1);await f.finish(0,new Error('abort'));
    assert.equal(f.jobs.length,2);await f.finish(1);
    assert.equal(f.writes[1].value.length,201);assert.equal(f.jobs.length,2);
    f.queue.submit('profile',['last']);await f.finish(2);assert.deepEqual(f.writes[2].value,['last']);
});

test('synchronous admission rejection preserves both kinds for the next changes',async()=>{
    const events=[],writes=[];let reject=true;
    const queue=new PersistenceWrites({request:(work,ok,no)=>{if(reject)no(new Error('capacity'));else {work();ok();}},
        writePreferences:value=>writes.push(value),mergeProfile:keys=>writes.push(keys),
        stored:kind=>events.push(kind),failed:kind=>events.push(`failed:${kind}`)});
    queue.submit('preferences',{volume:.1});queue.submit('profile',['first']);
    assert.deepEqual(events,['failed:preferences','failed:profile']);assert.equal(writes.length,0);
    reject=false;queue.submit('preferences',{volume:.2});queue.submit('profile',['later']);
    assert.deepEqual(writes,[{volume:.2},['first','later']]);
    assert.deepEqual(events,['failed:preferences','failed:profile','preferences','profile']);
});

test('independent kinds have at most two active jobs and no late acknowledgements after close',async()=>{
    const f=fixture();f.queue.submit('preferences',{volume:.1});f.queue.submit('profile',['first']);
    f.queue.submit('preferences',{volume:.2});f.queue.submit('profile',['later']);assert.equal(f.jobs.length,2);
    f.queue.close();await f.finish(0);await f.finish(1,new Error('closed'));
    assert.equal(f.jobs.length,2);assert.deepEqual(f.events,[]);
    assert.equal(f.queue.lanes.get('profile').keys.size,0);assert.equal(f.queue.lanes.get('preferences').value,null);
});

test('explicit retry flushes retained progress once without repeating active or acknowledged writes',async()=>{
    const f=fixture();f.queue.submit('profile',['first']);assert.equal(f.queue.retry('profile'),false);
    await f.finish(0,new Error('blocked'));assert.equal(f.jobs.length,1);
    assert.equal(f.queue.retry('profile'),true);assert.equal(f.queue.retry('profile'),false);await f.finish(1);
    assert.deepEqual(f.writes.map(w=>w.value),[['first'],['first']]);assert.equal(f.queue.retry('profile'),false);
    f.queue.close();assert.equal(f.queue.retry('profile'),false);
});

test('mutable progress coalesces latest values while a prior write is pending and survives failure', async()=>{
    const f=fixture(),v=n=>({type:'i32',value:n});
    f.queue.submit('profile_values',{diagnosis:v(1)});
    f.queue.submit('profile_values',{diagnosis:v(0),other:v(7)});
    await f.finish(0);assert.equal(f.jobs.length,2);
    await f.finish(1,new Error('quota'));assert.equal(f.jobs.length,2);
    assert.equal(f.queue.retry('profile_values'),true);await f.finish(2);
    assert.deepEqual(f.writes[2].value,{diagnosis:v(0),other:v(7)});
    assert.equal(f.queue.retry('profile_values'),false);
});
