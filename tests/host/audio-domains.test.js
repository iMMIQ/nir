import {test} from 'node:test';
import assert from 'node:assert/strict';
import {AudioDomains,audioVoiceKey} from '../../crates/nir-platform-web/host.js';

class Context {
  state='running';currentTime=0;resumes=0;
  suspend(){this.state='suspended';return Promise.resolve();}
  resume(){this.resumes++;this.state='running';return Promise.resolve();}
  close(){this.state='closed';return Promise.resolve();}
  advance(seconds){if(this.state==='running')this.currentTime+=seconds;}
}
const flush=()=>new Promise(resolve=>setImmediate(resolve));

class PendingContext extends Context {
  pending=[];
  resume(){
    this.resumes++;
    return new Promise((resolve,reject)=>this.pending.push({
      resolve:()=>{this.state='running';resolve();},reject,
    }));
  }
}

const audibleVoice=()=>({domain:'story',bus:'voice',stopped:false,gain:{gain:{value:1}}});

test('menu and background interruptions replace native targets without allowing stale completion to release pause',async()=>{
  let clock=0;
  const domains=new AudioDomains(PendingContext,()=>{},()=>clock),voice=audibleVoice();
  domains.setPaused('story',false);domains.unlock();
  const context=domains.context('story','voice');clock=2000;
  assert.deepEqual(domains.recoveryState([voice]),{status:'waiting',canRetry:false});
  for(const busPause of [true,false]){
    const before=context.resumes;
    if(busPause)domains.setBusPaused('story','voice',true);else domains.setPaused('story',true);
    assert.equal(domains.blocked([voice]),false);
    assert.equal(domains.snapshot().story.buses.voice.resume_pending,true);
    if(busPause)domains.setBusPaused('story','voice',false);else domains.setPaused('story',false);
    assert.equal(context.resumes,before+1,'our suspension needs a new native running target');
    for(let i=0;i<100;i++)domains.unlock();
    assert.equal(context.resumes,before+1,'own pause preserves permission; repeated gestures still coalesce');
    clock+=2000;
    assert.deepEqual(domains.recoveryState([voice]),{status:'waiting',canRetry:false});
  }
  domains.setPaused('story',true);context.pending[0].resolve();await flush();
  assert.equal(context.state,'suspended','an obsolete native result cannot release background policy');
  assert.equal(domains.snapshot().story.buses.voice.resume_pending,true,'old result cannot acknowledge the new target');
  context.pending.at(-1).resolve();await flush();
  assert.equal(context.state,'suspended');
  assert.equal(domains.snapshot().story.buses.voice.resume_pending,false);
  const before=context.resumes;domains.setPaused('story',false);
  assert.equal(context.resumes,before+1,'a settled target permits the next policy resume');
  context.pending.at(-1).resolve();await flush();
  assert.equal(domains.blocked([voice]),false);domains.close();
});

test('resume measurements distinguish pending, rejected and settled output without counting coalesced input',async()=>{
  let clock=0;
  const domains=new AudioDomains(PendingContext,()=>{},()=>clock);
  domains.setPaused('story',false);domains.unlock();
  const context=domains.context('story','voice'),status=()=>domains.snapshot().story.buses.voice;
  clock=450;
  assert.equal(status().resume_pending,true);assert.equal(status().resume_wait_ms,450);
  assert.equal(status().last_resume_ms,null);assert.equal(status().resume_attempts,1);
  clock=500;context.pending[0].reject(new Error('output denied'));await flush();
  assert.equal(status().resume_pending,false);assert.equal(status().resume_wait_ms,0);
  assert.equal(status().resume_error,true);assert.equal(status().resume_rejected,1);
  assert.equal(status().last_resume_ms,500);
  clock=700;domains.unlock();for(let i=0;i<100;i++)domains.unlock();
  assert.equal(status().resume_attempts,2);assert.equal(status().last_resume_ms,null);
  clock=1300;context.pending[1].resolve();await flush();
  assert.equal(status().resume_resolved,1);assert.equal(status().resume_rejected,1);
  assert.equal(status().last_resume_ms,600);assert.equal(status().max_resume_ms,600);
  assert.equal(status().resume_pending,false);assert.equal(status().resume_error,false);
  domains.close();
});

test('pending device resumes coalesce across policy release and repeated gestures',async()=>{
  let clock=0;
  const domains=new AudioDomains(PendingContext,()=>{},()=>clock),voice=audibleVoice();
  domains.unlock();domains.setPaused('story',false);domains.setPaused('foreground_ui',false);
  for(let i=0;i<100;i++)domains.unlock();
  for(const route of domains.routes.values())assert.equal(route.context.resumes,1);
  assert.equal(domains.recoveryStatus([voice]),'pending');
  assert.deepEqual(domains.recoveryState([voice]),{status:'pending',canRetry:false});
  clock=1500;assert.equal(domains.recoveryStatus([voice]),'waiting');
  assert.deepEqual(domains.recoveryState([voice]),{status:'waiting',canRetry:false});
  assert.equal(domains.blocked([voice]),true);
  const context=domains.context('story','voice');context.pending[0].resolve();await flush();
  assert.equal(domains.blocked([voice]),false);
  assert.equal(context.resumes,1);domains.close();
});

test('recovery offers a gesture for policy waits, a retry for rejection and saves for closed output',async()=>{
  let clock=0;
  const domains=new AudioDomains(Context,()=>{},()=>clock),voice=audibleVoice();
  domains.setPaused('story',false);domains.unlock();await flush();
  const context=domains.context('story','voice');context.pending=[];context.resume=PendingContext.prototype.resume;
  domains.setBusPaused('story','voice',true);domains.setBusPaused('story','voice',false);
  clock=1500;
  assert.deepEqual(domains.recoveryState([voice]),{status:'blocked',canRetry:true});
  domains.unlock();
  assert.deepEqual(domains.recoveryState([voice]),{status:'pending',canRetry:false});
  context.pending[1].reject(new Error('gesture denied'));await flush();
  assert.deepEqual(domains.recoveryState([voice]),{status:'failed',canRetry:true});
  context.state='closed';
  assert.deepEqual(domains.recoveryState([voice]),{status:'failed',canRetry:false});
  const attempts=context.resumes;domains.unlock();
  assert.equal(context.resumes,attempts);
  domains.close();
});

test('one delayed gesture cannot disable a fresh gesture needed by another audible bus',async()=>{
  let clock=0;
  const domains=new AudioDomains(PendingContext,()=>{},()=>clock);
  domains.setPaused('story',false);domains.unlock();
  const voice=audibleVoice(),sfx={...audibleVoice(),bus:'sfx'};
  clock=1500;assert.deepEqual(domains.recoveryState([voice]),{status:'waiting',canRetry:false});
  const route=domains.route('story','sfx');route.pendingGesture=false;
  assert.deepEqual(domains.recoveryState([voice,sfx]),{status:'blocked',canRetry:true});
  sfx.gain={gain:{value:0}};
  assert.deepEqual(domains.recoveryState([voice,sfx]),{status:'waiting',canRetry:false});
  sfx.gain={gain:{value:1}};route.context.state='closed';
  assert.deepEqual(domains.recoveryState([voice,sfx]),{status:'failed',canRetry:false});
  domains.close();
});

test('a pending policy resume permits one fresh gesture and ignores obsolete rejection',async()=>{
  const domains=new AudioDomains(Context,()=>{}),voice=audibleVoice();
  domains.setPaused('story',false);domains.unlock();await flush();
  const context=domains.context('story','voice');context.pending=[];
  context.resume=PendingContext.prototype.resume;
  domains.setBusPaused('story','voice',true);domains.setBusPaused('story','voice',false);
  assert.equal(context.pending.length,1);
  domains.unlock();assert.equal(context.pending.length,2);
  for(let i=0;i<100;i++)domains.unlock();assert.equal(context.pending.length,2);
  context.pending[0].reject(new Error('obsolete policy attempt'));await flush();
  assert.equal(domains.recoveryStatus([voice]),'pending');
  assert.equal(domains.snapshot().story.buses.voice.resume_rejected,1);
  assert.equal(domains.snapshot().story.buses.voice.resume_pending,true);
  context.pending[1].resolve();await flush();assert.equal(domains.blocked([voice]),false);
  domains.close();
});

test('a pause replaces the interrupted running target and late resumes preserve current pause ownership',async()=>{
  const domains=new AudioDomains(PendingContext,()=>{});
  domains.setPaused('story',false);domains.unlock();
  const context=domains.context('story','voice');
  domains.setPaused('story',true);domains.setPaused('story',false);
  assert.equal(context.pending.length,2);
  domains.setPaused('story',true);
  context.pending[0].resolve();await flush();assert.equal(context.state,'suspended');
  domains.close();
});

test('own bus suspension cannot strand an unresolved authorized resume when the bus reopens',async()=>{
  const domains=new AudioDomains(PendingContext,()=>{}),voice=audibleVoice();
  domains.setPaused('story',false);domains.unlock();
  const context=domains.context('story','voice');
  domains.setBusPaused('story','voice',true);
  for(let i=0;i<100;i++)domains.unlock();
  assert.equal(context.pending.length,1);
  assert.equal(domains.snapshot().story.buses.voice.pending_suspended,true);
  domains.setPaused('story',true);domains.setBusPaused('story','voice',false);
  assert.equal(context.pending.length,1,'background pause still owns the bus');
  domains.setPaused('story',false);
  assert.equal(context.pending.length,2,'resubmit after our suspension; old native promise may never resolve');
  assert.equal(domains.snapshot().story.buses.voice.pending_suspended,false);
  assert.equal(domains.snapshot().story.buses.voice.pending_gesture,true,'an own pause retains the existing permission identity');
  context.pending[1].resolve();await flush();
  assert.equal(domains.blocked([voice]),false);
  assert.equal(context.state,'running');
  context.pending[0].reject(new Error('obsolete interrupted attempt'));await flush();
  assert.equal(domains.blocked([voice]),false);
  assert.equal(domains.snapshot().story.buses.voice.resume_pending,false);
  domains.close();
});

test('output errors freeze audible routes even before the browser publishes suspension',async()=>{
  class Events extends Context {
    handlers=new Map();
    addEventListener(name,fn){this.handlers.set(name,fn);}
    dispatch(name){this.handlers.get(name)?.();}
  }
  const domains=new AudioDomains(Events,()=>{}),voice=audibleVoice();
  domains.setPaused('story',false);domains.unlock();await flush();
  const context=domains.context('story','voice');context.dispatch('error');
  assert.equal(context.state,'running');assert.equal(domains.blocked([voice]),true);
  assert.equal(domains.recoveryStatus([voice]),'failed');
  voice.gain.gain.value=0;assert.equal(domains.blocked([voice]),false);
  voice.gain.gain.value=1;domains.setBusPaused('story','voice',true);
  assert.equal(domains.blocked([voice]),false);
  domains.setBusPaused('story','voice',false);await flush();
  assert.equal(domains.blocked([voice]),false);
  domains.close();context.dispatch('error');assert.equal(domains.blocked([voice]),false);
});

test('synchronous device resume errors stay recoverable and a healthy retry clears them',async()=>{
  const warnings=[],domains=new AudioDomains(Context,e=>warnings.push(e.message)),voice=audibleVoice();
  domains.setPaused('story',false);domains.unlock();await flush();
  const context=domains.context('story','voice');context.suspend();
  context.resume=()=>{throw new Error('device unavailable');};
  assert.doesNotThrow(()=>domains.unlock());
  assert.deepEqual(warnings,['device unavailable']);
  assert.equal(domains.recoveryStatus([voice]),'failed');assert.equal(domains.blocked([voice]),true);
  context.resume=Context.prototype.resume;domains.unlock();await flush();
  assert.equal(domains.blocked([voice]),false);domains.close();
});

test('output recovery demands only active audible routes, independent of policy suspension',async()=>{
  const domains=new AudioDomains(Context);
  domains.setPaused('story',false);
  const voice={domain:'story',bus:'voice',stopped:false,gain:{gain:{value:1}}};
  assert.equal(domains.blocked([]),false);
  assert.equal(domains.blocked([voice]),true);
  voice.gain.gain.value=0;
  assert.equal(domains.blocked([voice]),false);
  voice.gain.gain.value=1;
  domains.setBusPaused('story','voice',true);
  assert.equal(domains.blocked([voice]),false);
  domains.unlock();await flush();
  domains.setBusPaused('story','voice',false);
  assert.equal(domains.blocked([voice]),false);
  // Browser/device interruption is neither a Player pause nor a task end.
  domains.context('story','voice').state='interrupted';
  assert.equal(domains.blocked([voice]),true);
  domains.setPaused('story',true);
  assert.equal(domains.blocked([voice]),false);
  domains.setPaused('story',false);await flush();
  assert.equal(domains.blocked([voice]),false);
  domains.close();assert.equal(domains.blocked([voice]),false);
});

test('menu pause freezes Story while foreground playback continues',async()=>{
  const domains=new AudioDomains(Context);
  domains.setPaused('story',false);domains.setPaused('foreground_ui',false);
  // A player resume does not manufacture a device unlock.
  assert.equal(domains.context('story').state,'suspended');
  domains.unlock();await flush();
  domains.setPaused('story',true);
  domains.context('story').advance(1);domains.context('foreground_ui').advance(1);
  assert.equal(domains.context('story').currentTime,0);
  assert.equal(domains.context('foreground_ui').currentTime,1);
  const resumes=domains.context('story').resumes;
  domains.unlock();await flush();
  assert.equal(domains.context('story').resumes,resumes);
  assert.equal(domains.context('story').state,'suspended');
  domains.setPaused('foreground_ui',true);
  domains.context('foreground_ui').advance(1);
  assert.equal(domains.context('foreground_ui').currentTime,1);
  domains.setPaused('foreground_ui',false);
  assert.equal(domains.context('story').state,'suspended');
  assert.equal(domains.context('foreground_ui').state,'running');
  domains.close();
});

test('late unlock resolution reapplies the current pause and close cannot reopen contexts',async()=>{
  class Deferred extends Context {
    resume(){return new Promise(resolve=>{this.finish=()=>{this.state='running';resolve();};});}
  }
  const domains=new AudioDomains(Deferred);
  domains.unlock();
  domains.setPaused('story',true);
  domains.setPaused('foreground_ui',false);
  domains.context('story').finish();domains.context('foreground_ui').finish();await flush();
  assert.equal(domains.context('story').state,'suspended');
  assert.equal(domains.context('foreground_ui').state,'running');
  domains.close();domains.unlock();domains.setPaused('story',false);
  assert.equal(domains.context('story').state,'closed');
});

test('unknown domains fail closed and voice identities include domain and session',()=>{
  const domains=new AudioDomains(Context);
  assert.throws(()=>domains.setPaused('dialogue',false),/E_AUDIO_DOMAIN/);
  assert.throws(()=>domains.context('__proto__'),/E_AUDIO_DOMAIN/);
  assert.throws(()=>audioVoiceKey({domain:'unknown',session:1,task:7}),/E_AUDIO_DOMAIN/);
  assert.throws(()=>audioVoiceKey({domain:'story',session:1,task:-1}),/E_AUDIO_ID/);
  const keys=new Set([
    audioVoiceKey({domain:'story',session:1,task:7}),
    audioVoiceKey({domain:'foreground_ui',session:1,task:7}),
    audioVoiceKey({domain:'story',session:2,task:7}),
  ]);
  assert.equal(keys.size,3);
  domains.close();
});

test('menu bus suspension preserves music and cannot release background ownership',async()=>{
  const domains=new AudioDomains(Context);
  domains.setPaused('story',false);domains.setPaused('foreground_ui',false);
  domains.unlock();await flush();
  const music=domains.context('story','bgm'),voice=domains.context('story','voice'),sfx=domains.context('story','sfx');
  domains.setBusPaused('story','voice',true);domains.setBusPaused('story','sfx',true);
  for(const context of [music,voice,sfx])context.advance(2);
  assert.equal(music.currentTime,2);assert.equal(voice.currentTime,0);assert.equal(sfx.currentTime,0);
  assert.equal(domains.context('foreground_ui').state,'running');
  // Closing an overlay while hidden cannot wake any Story bus.
  domains.setPaused('story',true);
  domains.setBusPaused('story','voice',false);domains.setBusPaused('story','sfx',false);
  for(const context of [music,voice,sfx])assert.equal(context.state,'suspended');
  domains.setPaused('story',false);
  for(const context of [music,voice,sfx])assert.equal(context.state,'running');
  // Returning from background with the overlay still open restores music only.
  domains.setBusPaused('story','voice',true);domains.setPaused('story',true);domains.setPaused('story',false);
  // A just-resolved resume still settles at its promise boundary; a policy
  // suspension in that same turn must finish before the new resume is issued.
  await flush();
  assert.equal(music.state,'running');assert.equal(voice.state,'suspended');
  assert.throws(()=>domains.setBusPaused('story','unknown',false),/E_AUDIO_BUS/);
  domains.close();
});


test('output recovery keys preserve explicit navigation and consume only reading actions',async()=>{
    const {audioRecoveryConsumesKey}=await import('../../crates/nir-platform-web/host.js');
    for(const key of ['Enter',' ']){
        assert.equal(audioRecoveryConsumesKey(true,key,null),true);
        for(const type of ['advance','continue','choose','toggle_auto','toggle_skip'])
            assert.equal(audioRecoveryConsumesKey(true,key,{type}),true);
        for(const type of ['new_game','menu','close','title','save','load','retry','menu_control'])
            assert.equal(audioRecoveryConsumesKey(true,key,{type}),false);
        assert.equal(audioRecoveryConsumesKey(false,key,{type:'advance'}),false);
    }
    assert.equal(audioRecoveryConsumesKey(true,'Escape',null),false);
});
