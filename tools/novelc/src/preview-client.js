// Development-server-only helper. It is never copied into a published release.
const initial=document.currentScript.dataset.release;
const root=new URL(document.currentScript.dataset.root||'/',location.origin);
const banner=document.createElement('aside');banner.id='nir-dev-status';banner.role='status';banner.setAttribute('aria-live','polite');
Object.assign(banner.style,{position:'fixed',bottom:'0',left:'0',right:'0',zIndex:'20',padding:'10px 16px',background:'#291d11',color:'#fff',font:'14px/1.5 system-ui',whiteSpace:'pre-wrap',maxHeight:'30vh',overflow:'auto'});
banner.hidden=true;document.body.append(banner);
let stopped=false,timer;
async function poll(){
  if(stopped)return;
  try{
    const r=await fetch(new URL('__nir_dev/status',root),{cache:'no-store'});if(!r.ok)throw Error('preview unavailable');
    const status=await r.json();
    if(!status.building&&!status.error&&status.release!==initial&&/^[0-9a-f]{64}$/.test(status.release)){
      const target=new URL(`releases/${status.release}/index.html`,root);
      target.search=location.search;target.hash=location.hash;
      location.replace(target.href);return;
    }
    banner.hidden=!status.stale;
    banner.textContent=status.error?`构建失败，当前显示上次有效预览 / Build failed; keeping previous preview\n${status.error}`:status.building?'正在构建，当前预览尚未更新 / Building; showing previous preview':'';
  }catch{banner.hidden=false;banner.textContent='开发服务器连接中断，当前预览未更新 / Preview server disconnected';}
  if(!stopped)timer=setTimeout(poll,500);
}
window.addEventListener('pagehide',()=>{stopped=true;clearTimeout(timer);});
window.addEventListener('pageshow',e=>{if(e.persisted){stopped=false;poll();}});
poll();
