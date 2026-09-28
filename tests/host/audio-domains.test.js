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
