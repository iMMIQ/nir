// Bounded, read-only diagnostics around real browser input. No dispatch changes.
export async function installClosedPointerProbe(page) {
  await page.addInitScript(()=>{
    const rows=[],pending=new Map();let dropped=0;
    const push=row=>{if(rows.length===128){rows.shift();dropped++;}rows.push({at:performance.now(),...row});};
    const state=()=>{const s=window.__nir?.state();return s?Object.fromEntries(['screen','session','interaction','device','sequence','loading','paused','position','history_count','tick_us'].map(k=>[k,s[k]])):null;};
    const view=value=>{try{const v=typeof value==='string'?JSON.parse(value):value;return v?{width:v.width,height:v.height,nodes:v.nodes?.filter(n=>['menu','close','toggle_interface'].includes(n.action?.type)).map(n=>({id:n.id,action:n.action,rect:n.rect}))}:null;}catch{return null;}};
    for(const type of ['pointerdown','pointerup','pointercancel','lostpointercapture'])document.addEventListener(type,e=>push({type,pointer:e.pointerId,kind:e.pointerType,button:e.button,x:e.clientX,y:e.clientY,target:e.target.id,trusted:e.isTrusted,state:state()}),true);
    const Native=window.Worker;
    window.Worker=class extends Native {
      constructor(url,options){
        super(url,options);this.pointerProbe=options?.name==='nir-runtime';
        if(this.pointerProbe)this.addEventListener('message',e=>{
          const calls=pending.get(e.data.id);if(!calls||e.data.kind==='update')return;pending.delete(e.data.id);
          const snapshot=e.data.value?.snapshot;
          push({type:'rpc_reply',id:e.data.id,kind:e.data.kind,calls,results:e.data.value?.results,host:snapshot?.host,view:view(snapshot?.view)});
        });
      }
      postMessage(message,...args){
        if(this.pointerProbe&&message.kind==='batch'){
          const calls=message.value.filter(c=>['resize','pointer_action','pointer_gesture','action','focus_control'].includes(c.method));
          if(calls.length){pending.set(message.id,calls);push({type:'rpc_request',id:message.id,calls,state:state()});}
        }
        return super.postMessage(message,...args);
      }
    };
    window.closedPointerProbe=()=>({rows:[...rows],dropped,pending:[...pending.keys()],state:state(),
      viewport:{width:innerWidth,height:innerHeight,dpr:devicePixelRatio,scrollX,scrollY},
      canvas:(()=>{const c=document.querySelector('#stage'),r=c.getBoundingClientRect();return {width:c.width,height:c.height,rect:{x:r.x,y:r.y,width:r.width,height:r.height}};})(),
      controls:[...document.querySelectorAll('#actions button')].map(b=>{const rect=JSON.parse(b.dataset.rect),x=rect[0]+rect[2]/2,y=rect[1]+rect[3]/2;return {action:JSON.parse(b.dataset.action),rect,hit:document.elementFromPoint(x,y)?.id};}),
      metrics:window.__nir?.metrics,diagnostics:window.__nir?.diagnostics(),activeElement:document.activeElement?.id||document.activeElement?.tagName});
  });
}
