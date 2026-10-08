import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import {createHash} from 'node:crypto';
import {execFile,spawn} from 'node:child_process';
import {promisify} from 'node:util';
import {closeScaleFixture} from '../performance/fixtures.js';
import {realtimePcmProcessor} from './realtime-pcm.js';

const run=promisify(execFile),sha=bytes=>createHash('sha256').update(bytes).digest('hex');

// Authored test input only. The compiler emits MP3; the player never receives
// or requests WAV. A distinctive intro prevents a one-cycle alignment error
// from hiding an accidentally repeated intro or skipped section.
function authoredTone() {
  const rate=44100,frames=rate,bytes=Buffer.alloc(44+frames*4);
  bytes.write('RIFF');bytes.writeUInt32LE(bytes.length-8,4);bytes.write('WAVEfmt ',8);
  bytes.writeUInt32LE(16,16);bytes.writeUInt16LE(1,20);bytes.writeUInt16LE(2,22);
  bytes.writeUInt32LE(rate,24);bytes.writeUInt32LE(rate*4,28);bytes.writeUInt16LE(4,32);
  bytes.writeUInt16LE(16,34);bytes.write('data',36);bytes.writeUInt32LE(frames*4,40);
  for(let i=0;i<frames;i++) {
    const t=i/rate,edge=Math.min(1,t/.005,(1-t)/.005);
    let sample=.28*Math.sin(2*Math.PI*220*t)+.11*Math.sin(2*Math.PI*330*t);
    if(t<.2)sample+=.12*Math.sin(2*Math.PI*(410*t+530*t*t))*Math.sin(Math.PI*t/.2)**2;
    if(t>.4)sample+=.08*Math.sin(2*Math.PI*(170*t+210*t*t))*Math.sin(Math.PI*(t-.4)/.6)**2;
    let right=.24*Math.sin(2*Math.PI*260*t)+.13*Math.sin(2*Math.PI*390*t);
    if(t<.2)right+=.10*Math.sin(2*Math.PI*(610*t+310*t*t))*Math.sin(Math.PI*t/.2)**2;
    if(t>.4)right+=.07*Math.sin(2*Math.PI*(150*t+340*t*t))*Math.sin(Math.PI*(t-.4)/.6)**2;
    bytes.writeInt16LE(Math.round(sample*edge*32767),44+i*4);
    bytes.writeInt16LE(Math.round(right*edge*32767),46+i*4);
  }
  return bytes;
}

export async function buildRealtimePcmFixture({region,port}) {
  const cli=path.resolve(process.env.NIR_PERF_CLI||'dist/novelc');
  const temp=await fs.mkdtemp(path.join(os.tmpdir(),'nir-realtime-pcm-'));
  const project=path.join(temp,'story'),web=path.join(temp,'web');let server;
  try {
    const example=path.resolve('examples/rain-letters');
    await fs.cp(example,project,{recursive:true,filter(source){
      return !path.relative(example,source).split(path.sep).some(part=>['dist','reports','.nir','game.lock'].includes(part));
    }});
    await fs.writeFile(path.join(project,'assets/source/bgm.wav'),authoredTone());
    const source=path.join(project,'content/ch01/story.nir.json'),content=JSON.parse(await fs.readFile(source,'utf8'));
    const line=text=>({id:'line',scope:'interaction',effect:{type:'dialogue',text,speaker:'',reveal_us:'0'}});
    const music={type:'audio',asset:'audio.bgm',bus:'bgm',looped:true};
    if(region)music.loop_region={start_us:'200000',end_us:'400000'};
    const solid=color=>[{id:'solid',x:0,y:0,width:1280,height:720,color}];
    content.scenes={station:solid([1,0,0,1]),together:solid([0,0,1,1])};
    content.scenes.together.push({id:'aki',asset:'actor.aki',x:850,y:100,width:290,height:530});
    const stage=scene=>({id:'stage',scope:'scene',effect:{type:'stage_present',scene,duration_us:'0'}});
    content.cues={opening:{effects:[stage('station'),{id:'music',scope:'session',effect:music},line('intro')]},
      ordinary:{effects:[line('arrival')]},cold:{effects:[stage('together'),line('intro')]}};
    const activate=(cue,next)=>({ops:[],terminator:{type:'activate',cue,next}});
    const wait=next=>({ops:[],terminator:{type:'await',conditions:[{task:'line',milestone:{type:'finished'}}],next,on_cancelled:'failed',on_failed:'failed'}});
    content.functions.main={entry:'start',blocks:{start:activate('opening','wait_first'),wait_first:wait('ordinary'),
      ordinary:activate('ordinary','wait_second'),wait_second:wait('cold'),cold:activate('cold','wait_third'),
      wait_third:wait('ordinary'),failed:{ops:[],terminator:{type:'fault',code:'E_PCM_FIXTURE',message:'Unexpected cancellation.'}}}};
    await fs.writeFile(source,JSON.stringify(content,null,2)+'\n');
    const player=path.join(project,'config/player.toml');
    await fs.writeFile(player,(await fs.readFile(player,'utf8')).replace('prefetch_media = true','prefetch_media = false'));
    const transcripts=[];
    for(const [tool,args,maxBuffer] of [
      [cli,['-p',project,'resolve','--sdk',path.resolve('dist/sdk')],8*1024*1024],
      [cli,['-p',project,'build','--locked','--out',web],16*1024*1024],
      [cli,['-p',project,'check','--locked'],8*1024*1024],
      ['python3',['scripts/verify_release.py',web],8*1024*1024],
    ]){
      const result=await run(tool,args,{maxBuffer});
      transcripts.push({tool,args,exitCode:0,...result});
    }
    const channel=JSON.parse(await fs.readFile(path.join(web,'channels/stable.json'),'utf8'));
    const manifestBytes=await fs.readFile(path.join(web,`releases/${channel.release}.json`)),manifest=JSON.parse(manifestBytes);
    const program=JSON.parse(await fs.readFile(path.join(web,manifest.objects[manifest.program].path),'utf8')).program;
    const audio=Object.values(manifest.objects).filter(o=>o.media_type.startsWith('audio/'));
    if(audio.length!==1||audio.some(o=>o.media_type!=='audio/mpeg'))throw new Error('PCM fixture must contain exactly one MP3');
    for(const [key,file] of Object.entries({js:'player_web.js',wasm:'player_web_bg.wasm',host:'host.js',runtime_worker:'runtime-worker.js',asset_worker:'asset-worker.js'})) {
      if(sha(await fs.readFile(path.resolve('dist/sdk',file)))!==manifest.engine[key])throw new Error(`PCM fixture SDK mismatch: ${key}`);
    }
    const image=manifest.objects[program.assets['actor.aki'].object];
    // AudioWorklet requests bypass Playwright routing. Serve the diagnostic
    // module as a real same-origin file under the existing release CSP.
    await fs.writeFile(path.join(web,'__nir_test_pcm.js'),realtimePcmProcessor);
    const verification={release:channel.release,engine:manifest.engine,objects:Object.keys(manifest.objects).length,
      audio:audio.map(o=>({mediaType:o.media_type,path:o.path,bytes:o.bytes})),region,sourceChannels:2,
      sourceSha256:sha(await fs.readFile(source)),toneSha256:sha(authoredTone()),diagnosticModuleSha256:sha(Buffer.from(realtimePcmProcessor)),
      manifestSha256:sha(manifestBytes),compilerCheck:true,releaseVerifier:true};
    if(process.env.NIR_REALTIME_FIXTURE_ARCHIVE){
      const archive=path.join(process.env.NIR_REALTIME_FIXTURE_ARCHIVE,region?'region':'whole');
      await fs.mkdir(archive,{recursive:true});
      await fs.cp(web,path.join(archive,'web'),{recursive:true});
      await fs.cp(project,path.join(archive,'project'),{recursive:true,filter(source){
        return !path.relative(project,source).split(path.sep).some(part=>['dist','reports','.nir'].includes(part));
      }});
      await fs.writeFile(path.join(archive,'verification.json'),JSON.stringify(verification,null,2)+'\n');
      await fs.writeFile(path.join(archive,'commands.json'),JSON.stringify(transcripts,null,2)+'\n');
    }
    server=spawn(cli,['serve',web,'--port',String(port)],{stdio:'ignore'});
    const origin=`http://127.0.0.1:${port}`;let ready=false;
    for(let i=0;i<100;i++) {
      let served;try{const response=await fetch(`${origin}/channels/stable.json`);if(response.ok)served=await response.json();}catch{}
      if(served){if(served.release!==channel.release)throw new Error('different PCM fixture served');ready=true;break;}
      if(server.exitCode!==null)throw new Error('PCM server exited before ready');
      await new Promise(resolve=>setTimeout(resolve,100));
    }
    if(!ready)throw new Error('PCM server did not become ready');
    return {temp,project,web,server,origin,imagePath:image.path,imageMediaType:image.media_type,
      imageBytes:await fs.readFile(path.join(web,image.path)),
      verification};
  }catch(error){await closeScaleFixture({temp,server});throw error;}
}
