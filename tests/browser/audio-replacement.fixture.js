import fs from 'node:fs/promises';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {execFile,spawn} from 'node:child_process';
import {promisify} from 'node:util';
import {closeScaleFixture} from '../performance/fixtures.js';

const run=promisify(execFile);
const sha=bytes=>createHash('sha256').update(bytes).digest('hex');

// Compile our own neutral MP3 project. No previously deployed game or cached
// fixture can silently supply a different SDK to this regression.
export async function buildAudioReplacementFixture({port=4269}={}) {
  const cli=path.resolve(process.env.NIR_PERF_CLI||'dist/novelc');
  await fs.mkdir(path.resolve('target/tmp'),{recursive:true});
  const temp=await fs.mkdtemp(path.resolve('target/tmp/nir-audio-replacement-'));
  const project=path.join(temp,'story'),web=path.join(temp,'web');let server;
  try {
    const example=path.resolve('examples/rain-letters');
    await fs.cp(example,project,{recursive:true,filter(source){
      return !path.relative(example,source).split(path.sep).some(part=>['dist','reports','.nir','game.lock'].includes(part));
    }});
    const source=path.join(project,'content/ch01/story.nir.json');
    const content=JSON.parse(await fs.readFile(source,'utf8'));
    const solid=color=>[{id:'solid',x:0,y:0,width:1280,height:720,color}];
    content.scenes={station:solid([1,0,0,1]),together:solid([0,0,1,1])};
    content.scenes.together.push({id:'aki',asset:'actor.aki',x:850,y:100,width:290,height:530});
    const line=text=>({id:'line',scope:'interaction',effect:{type:'dialogue',text,speaker:'',reveal_us:'0'}});
    const stop={id:'music_stop',scope:'session',effect:{type:'audio_stop',target:'music',duration_us:'0'}};
    const play=asset=>({id:'music',scope:'session',effect:{type:'audio',asset,bus:'bgm',looped:true}});
    content.cues={
      opening:{effects:[{id:'stage',scope:'scene',effect:{type:'stage_present',scene:'station',duration_us:'0'}},play('audio.bgm'),line('intro')]},
      replacement:{effects:[stop,play('audio.bell'),{id:'stage',scope:'scene',effect:{type:'stage_present',scene:'together',duration_us:'0'}},line('arrival')]},
      ordinary:{effects:[line('intro')]},
      replay:{effects:[stop,play('audio.bell'),line('arrival')]},
    };
    const activate=(cue,next)=>({ops:[],terminator:{type:'activate',cue,next}});
    const wait=next=>({ops:[],terminator:{type:'await',conditions:[{task:'line',milestone:{type:'finished'}}],next,on_cancelled:'failed',on_failed:'failed'}});
    content.functions.main={entry:'start',blocks:{
      start:activate('opening','wait_first'),wait_first:wait('replace'),
      replace:activate('replacement','wait_second'),wait_second:wait('ordinary'),
      ordinary:activate('ordinary','wait_third'),wait_third:wait('replay'),
      replay:activate('replay','wait_fourth'),wait_fourth:wait('end'),
      end:{ops:[],terminator:{type:'end',outcome:'replacement-complete'}},
      failed:{ops:[],terminator:{type:'fault',code:'E_REPLACEMENT_FIXTURE',message:'Unexpected cancellation.'}},
    }};
    await fs.writeFile(source,JSON.stringify(content,null,2)+'\n');
    const player=path.join(project,'config/player.toml');
    await fs.writeFile(player,(await fs.readFile(player,'utf8')).replace('prefetch_media = true','prefetch_media = false'));
    await run(cli,['-p',project,'resolve','--sdk',path.resolve('dist/sdk')],{maxBuffer:8*1024*1024});
    await run(cli,['-p',project,'build','--locked','--out',web],{maxBuffer:16*1024*1024});
    await run(cli,['-p',project,'check','--locked'],{maxBuffer:8*1024*1024});
    await run('python3',['scripts/verify_release.py',web],{maxBuffer:8*1024*1024});
    const channel=JSON.parse(await fs.readFile(path.join(web,'channels/stable.json'),'utf8'));
    const manifestBytes=await fs.readFile(path.join(web,`releases/${channel.release}.json`));
    const manifest=JSON.parse(manifestBytes);
    const program=JSON.parse(await fs.readFile(path.join(web,manifest.objects[manifest.program].path),'utf8')).program;
    const audio=Object.values(manifest.objects).filter(o=>o.media_type.startsWith('audio/'));
    if(audio.length!==2||audio.some(o=>o.media_type!=='audio/mpeg'))throw new Error('replacement fixture must contain exactly two MP3 objects');
    for(const [key,file] of Object.entries({js:'player_web.js',wasm:'player_web_bg.wasm',host:'host.js',runtime_worker:'runtime-worker.js',asset_worker:'asset-worker.js'})) {
      if(sha(await fs.readFile(path.resolve('dist/sdk',file)))!==manifest.engine[key])throw new Error(`replacement SDK mismatch: ${key}`);
    }
    const replacementObject=manifest.objects[program.assets['audio.bell'].object];
    const replacementImage=manifest.objects[program.assets['actor.aki'].object];
    server=spawn(cli,['serve',web,'--port',String(port)],{stdio:'ignore'});
    const origin=`http://127.0.0.1:${port}`;let ready=false;
    for(let i=0;i<100;i++) {
      let served;
      try {const response=await fetch(`${origin}/channels/stable.json`);if(response.ok)served=await response.json();}catch{}
      if(served){if(served.release!==channel.release)throw new Error('different replacement release served');ready=true;break;}
      if(server.exitCode!==null)throw new Error('replacement server exited before ready');
      await new Promise(resolve=>setTimeout(resolve,100));
    }
    if(!ready)throw new Error('replacement server did not become ready');
    return {temp,project,web,server,origin,replacementPath:replacementObject.path,
      replacementBytes:await fs.readFile(path.join(web,replacementObject.path)),
      imagePath:replacementImage.path,imageBytes:await fs.readFile(path.join(web,replacementImage.path)),
      imageMediaType:replacementImage.media_type,
      verification:{release:channel.release,engine:manifest.engine,objects:Object.keys(manifest.objects).length,
        audio:audio.map(o=>({mediaType:o.media_type,path:o.path,bytes:o.bytes})),
        sourceSha256:sha(await fs.readFile(source)),manifestSha256:sha(manifestBytes),compilerCheck:true,releaseVerifier:true}};
  }catch(error){await closeScaleFixture({temp,server});throw error;}
}
