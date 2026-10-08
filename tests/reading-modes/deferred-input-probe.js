// Delay real UI transport without changing Worker messages or native audio.
export async function installDeferredInputProbe(page){
  await page.addInitScript(()=>{
    const rows=[],pending=new Map(),held=[];let dropped=0,mode=null;
    const state=()=>{const s=globalThis.__nir?.state();return s?Object.fromEntries(['session','interaction','screen','loading','paused','sequence','position','dialogue','history_count'].map(k=>[k,s[k]])):null;};
    const push=row=>{if(rows.length===128){rows.shift();dropped++;}rows.push({atMs:performance.now(),...row});};
    for(const type of ['keydown','pointerdown','pointerup'])window.addEventListener(type,e=>push({type,key:e.key,kind:e.pointerType,trusted:e.isTrusted,repeat:e.repeat,focus:document.activeElement?.id,state:state()}),true);
    const Native=window.Worker;
    window.Worker=class extends Native{
      constructor(url,options){
        super(url,options);this.probe=options?.name==='nir-runtime';
        if(this.probe)this.addEventListener('message',e=>{
          const calls=pending.get(e.data.id);if(!calls||e.data.kind==='update')return;pending.delete(e.data.id);
          push({type:'reply',id:e.data.id,calls,results:e.data.value?.results,owner:e.data.value?.snapshot?.state});
        });
      }
      postMessage(message,...args){
        if(this.probe&&message.kind==='batch'){
          const calls=message.value.filter(c=>['primary_action','pointer_gesture','pointer_action','action'].includes(c.method));
          if(calls.length){pending.set(message.id,calls);push({type:'request',id:message.id,calls,state:state()});}
          if(calls.some(c=>mode==='primary'&&c.method==='primary_action'||mode==='pointer'&&c.method==='pointer_gesture'&&c.args[0]===0)){
            held.push(()=>super.postMessage(message,...args));return;
          }
        }
        return super.postMessage(message,...args);
      }
    };
    globalThis.deferredInputProbe={
      hold(value){mode=value;},flush(){mode=null;for(const send of held.splice(0))send();},
      snapshot(){return {rows:[...rows],dropped,held:held.length,pending:pending.size};},
    };
    globalThis.deferredInputAudio=[];const create=AudioContext.prototype.createBufferSource;
    AudioContext.prototype.createBufferSource=function(...args){
      const source=create.apply(this,args),context=this,row={source,context,start:null,duration:null,stops:0,ended:false};deferredInputAudio.push(row);
      const start=source.start,stop=source.stop;
      source.start=function(...args){row.duration=this.buffer.duration;const value=start.apply(this,args);row.start=context.currentTime;return value;};
      source.stop=function(...args){row.stops++;return stop.apply(this,args);};source.addEventListener('ended',()=>row.ended=true);return source;
    };
  });
}
