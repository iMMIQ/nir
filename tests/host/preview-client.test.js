import {test} from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import vm from 'node:vm';

const sourcePath=path.resolve('tools/novelc/src/preview-client.js');

async function bootPreviewClient({initial,fetchImpl,devHook,hidden=false}={}){
    const code=await fs.readFile(sourcePath,'utf8');
    const listeners={doc:new Map(),win:new Map()};
    const banner={attrs:{},setAttribute(k,v){this.attrs[k]=v;},hidden:true,textContent:''};
    let replacedUrl=null;
    const document={
        hidden,
        currentScript:{dataset:{release:initial||'a'.repeat(64),root:'/'}},
        createElement:()=>banner,
        body:{append:()=>{}},
        addEventListener:(ev,fn)=>listeners.doc.set(ev,fn),
    };
    const windowObj={
        __nirDev:devHook,
        addEventListener:(ev,fn)=>listeners.win.set(ev,fn),
    };
    const location={
        origin:'http://127.0.0.1:8080',
        search:'?backend=webgl2',
        hash:'#test',
        replace:url=>{replacedUrl=url;},
    };
    const context={
        console,URL,Promise,Error,AbortController,setTimeout,clearTimeout,
        document,window:windowObj,location,
        fetch:fetchImpl,
    };
    vm.runInNewContext(code,context,{filename:'preview-client.js'});
    return {
        banner,
        document,
        listeners,
        getReplacedUrl:()=>replacedUrl,
        stop:()=>listeners.win.get('pagehide')?.(),
    };
}

test('preview-client pauses polling while hidden, shows stale errors, and redirects on new release',async()=>{
    const initial='a'.repeat(64),next='b'.repeat(64);
    let calls=0;
    const responses=[
        {ok:true,json:async()=>({release:initial,building:false,stale:true,error:'E_SYNTAX: line 2'})},
        {ok:true,json:async()=>({release:next,building:false,stale:false,error:null})},
    ];
    const client=await bootPreviewClient({
        initial,
        hidden:true,
        fetchImpl:async()=>{calls++;return responses.shift();},
    });
    await new Promise(r=>setTimeout(r,20));
    // Hidden tab must not poll
    assert.equal(calls,0);

    // Become visible -> first poll surfaces build error banner
    client.document.hidden=false;
    client.listeners.doc.get('visibilitychange')();
    await new Promise(r=>setTimeout(r,20));
    assert.equal(calls,1);
    assert.equal(client.banner.hidden,false);
    assert.match(client.banner.textContent,/E_SYNTAX: line 2/);

    // Trigger next poll -> redirects preserving search and hash
    client.listeners.doc.get('visibilitychange')();
    await new Promise(r=>setTimeout(r,20));
    assert.equal(client.getReplacedUrl(),`http://127.0.0.1:8080/releases/${next}/index.html?backend=webgl2#test`);
    client.stop();
});

test('preview-client replays dev scenario from legal New Game entry',async()=>{
    const initial='a'.repeat(64);
    const actions=[];
    let step=0;
    const states=[
        {screen:'Title',loading:false,has_dialogue:false,choice:false,error:null},
        {screen:'Story',loading:false,has_dialogue:true,choice:false,error:null},
        {screen:'Story',loading:false,has_dialogue:false,choice:true,error:null},
        {screen:'Story',loading:false,has_dialogue:true,choice:false,error:null},
    ];
    const devHook={
        state:()=>states[Math.min(step,states.length-1)],
        action:async a=>{actions.push(a);step++;return true;},
    };
    const client=await bootPreviewClient({
        initial,
        devHook,
        fetchImpl:async()=>({ok:true,json:async()=>({release:initial,building:false,stale:false,error:null,scenario:{name:'walk',choices:['walk']}})}),
    });
    await new Promise(r=>setTimeout(r,260));
    client.stop();
    assert.deepEqual(JSON.parse(JSON.stringify(actions)),[
        {type:'new_game'},
        {type:'advance'},
        {type:'choose',option:'walk'},
    ]);
});
