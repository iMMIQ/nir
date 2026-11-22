// Development-server-only helper. It is never copied into a published release.
const initial=document.currentScript.dataset.release;
const root=new URL(document.currentScript.dataset.root||'/',location.origin);
const banner=document.createElement('aside');banner.id='nir-dev-status';banner.role='status';banner.setAttribute('aria-live','polite');
banner.hidden=true;document.body.append(banner);
let stopped=false,timer=null,scenarioStarted=false;

async function fetchStatus(){
  const controller=new AbortController();
  const abortTimer=setTimeout(()=>controller.abort(new Error('timeout')),5000);
  try{
    const r=await fetch(new URL('__nir_dev/status',root),{cache:'no-store',signal:controller.signal});
    if(!r.ok)throw Error('preview unavailable');
    return await r.json();
  }finally{clearTimeout(abortTimer);}
}

// Replays a verified scenario from the legal New Game entry in dev preview.
async function replayScenario(plan){
  const dev=window.__nirDev;
  if(!dev||!plan||!Array.isArray(plan.choices))return;
  let choiceCursor=0,started=false;
  for(let step=0;step<2000&&!stopped;step++){
    const s=dev.state();
    if(s.screen==='Ended'||s.error)return;
    if(!s.loading){
      if(s.screen==='Title'&&!started){
        started=true;
        await dev.action({type:'new_game'});
      }else if(s.screen==='Story'){
        if(s.choice&&choiceCursor<plan.choices.length){
          const option=plan.choices[choiceCursor++];
          await dev.action({type:'choose',option});
          if(choiceCursor>=plan.choices.length)return;
        }else if(s.has_dialogue&&choiceCursor<plan.choices.length){
          await dev.action({type:'advance'});
        }else if(choiceCursor>=plan.choices.length){
          return;
        }
      }
    }
    await new Promise(resolve=>setTimeout(resolve,60));
  }
}

function scheduleNext(){
  clearTimeout(timer);timer=null;
  if(!stopped&&!document.hidden)timer=setTimeout(poll,500);
}

async function poll(){
  if(stopped||document.hidden)return;
  try{
    const status=await fetchStatus();
    if(!status.building&&!status.error&&status.release!==initial&&/^[0-9a-f]{64}$/.test(status.release)){
      const target=new URL(`releases/${status.release}/index.html`,root);
      target.search=location.search;target.hash=location.hash;
      location.replace(target.href);return;
    }
    if(!scenarioStarted&&status.scenario&&!status.building&&!status.error){
      scenarioStarted=true;
      void replayScenario(status.scenario);
    }
    banner.hidden=!status.stale;
    banner.textContent=status.error?`构建失败，当前显示上次有效预览 / Build failed; keeping previous preview\n${status.error}`:status.building?'正在构建，当前预览尚未更新 / Building; showing previous preview':'';
  }catch{banner.hidden=false;banner.textContent='开发服务器连接中断，当前预览未更新 / Preview server disconnected';}
  scheduleNext();
}

document.addEventListener('visibilitychange',()=>{if(document.hidden){clearTimeout(timer);timer=null;}else if(!stopped){void poll();}});
window.addEventListener('pagehide',()=>{stopped=true;clearTimeout(timer);timer=null;});
window.addEventListener('pageshow',e=>{if(e.persisted){stopped=false;void poll();}});
void poll();
