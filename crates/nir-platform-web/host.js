import { envelopePosition, focusIdentity, overlayControlPosition, samePointerTarget, audioBlockedAction, audioRecoveryConsumesKey, DomainElapsed, AudioDomains, audioLoopPlayback, audioLoopEndSeconds, audioVoiceKey } from './src_js/audio.js';
import { TraceRecorder, PerformanceRecorder, OwnerInbox, contentBatchEnvelope, ContentStagingBudget, acquireRequiredContentStage, postContentEviction, queueContentSkip, workPriority, WorkPool, SharedRequests, fetchContentBatch } from './src_js/scheduler.js';
import { UTF8_ENCODER, parseRuntimeProgram, normalizeCharacterVoices, characterVoiceGain, initialRuntimePreferences, validateAssetRequest, mediaMemorySnapshot, audioDecodedBudget, validateDecodedAudio, initialPreferences } from './src_js/preferences.js';
import { SAVE_DATABASE, saveKey, profileKey, PersistenceWrites, validateMetadataRecord, readMetadataRecord, readStartupMetadata, writeMetadataRecord, writePreferencesRecord, mergeProfileRecord, dispatchOwnerRequest, openSaveDatabase, SaveDatabaseConnection, readSaveRecord, stringifyPlayerData, inspectSaveRecord, listSaveRecords, listHistoryRecords, saveHistoryExport, validHistoryKey, commitSaveRecord, checkSavedRelease, validateHistoryTarget } from './src_js/storage.js';
import { initializeBackend, WORKER_PROTOCOL, SerialInputQueue, WorkerClient, RemoteEngine, isTestHookAllowed, connectWorker } from './src_js/worker_bridge.js';

export { envelopePosition, focusIdentity, overlayControlPosition, samePointerTarget, audioBlockedAction, audioRecoveryConsumesKey, DomainElapsed, AudioDomains, audioLoopPlayback, audioLoopEndSeconds, audioVoiceKey } from './src_js/audio.js';
export { TraceRecorder, PerformanceRecorder, OwnerInbox, contentBatchEnvelope, ContentStagingBudget, acquireRequiredContentStage, postContentEviction, queueContentSkip, workPriority, WorkPool, SharedRequests, fetchContentBatch } from './src_js/scheduler.js';
export { parseRuntimeProgram, normalizeCharacterVoices, characterVoiceGain, initialRuntimePreferences, validateAssetRequest, mediaMemorySnapshot, audioDecodedBudget, validateDecodedAudio, initialPreferences } from './src_js/preferences.js';
export { SAVE_DATABASE, saveKey, profileKey, PersistenceWrites, validateMetadataRecord, readMetadataRecord, readStartupMetadata, writeMetadataRecord, writePreferencesRecord, mergeProfileRecord, dispatchOwnerRequest, openSaveDatabase, SaveDatabaseConnection, readSaveRecord, stringifyPlayerData, inspectSaveRecord, listSaveRecords, listHistoryRecords, saveHistoryExport, validHistoryKey, commitSaveRecord, checkSavedRelease, validateHistoryTarget } from './src_js/storage.js';
export { initializeBackend, WORKER_PROTOCOL, SerialInputQueue, WorkerClient, RemoteEngine, isTestHookAllowed } from './src_js/worker_bridge.js';

export async function start({wasm,release,releaseDigest,releaseRoot,executable,fetchObject,fail,startupTrace=[],sha256,wasmBytes}) {
    const params=new URL(location.href).searchParams;
    const testMode=params.has('test')&&isTestHookAllowed(release);
    const trace=new TraceRecorder({enabled:params.get('trace')!=='0'&&(params.has('diagnostics')||testMode)});
    const performanceStats=trace.enabled?new PerformanceRecorder(64):null;
    let traceContext={session:1,device:1};
    const observe=(stage,fields={})=>trace.record(stage,{...traceContext,...fields});
    for(const row of startupTrace)observe(row.stage,row);
    let canvas=document.querySelector('#stage');
    const shell=document.querySelector('#shell');
    const replaceCanvas=()=>{
        try{canvas.getContext('webgl2')?.getExtension('WEBGL_lose_context')?.loseContext();}catch{}
        const next=canvas.cloneNode(false);canvas.replaceWith(next);canvas=next;
    };
    const program=parseRuntimeProgram(executable);
    const accent=program.theme.accent;
    document.querySelector('#focus-ring').style.borderColor=`rgba(${accent.slice(0,3).map(channel=>Math.round(channel*255)).join(',')},${accent[3]})`;
    const metrics={boot:performance.now(),titleMs:null,firstLineMs:null,resourceFailures:0,frames:0,audioStarts:0,deviceRecoveries:0,peakResidentBytes:0,startInputMs:null,firstLineAfterStartMs:null,contentStagingBytes:0,peakContentStagingBytes:0,contentStagingBudgetBytes:CONTENT_STAGING_LIMIT,contentStagingReservations:0,contentStagingWaiters:0};
    const syncContentStagingMetrics=budget=>{
        metrics.contentStagingBytes=budget.used;
        metrics.peakContentStagingBytes=budget.peak;
        metrics.contentStagingBudgetBytes=budget.limit;
        metrics.contentStagingReservations=budget.reservations.size;
        metrics.contentStagingWaiters=budget.waiting.length;
    };
    let viewportOffset={left:0,top:0,right:0,bottom:0};
    const refreshSafeArea=()=>{
        const style=getComputedStyle(document.documentElement);
        for(const edge of ['left','top','right','bottom'])viewportOffset[edge]=Math.max(0,parseFloat(style.getPropertyValue(`--nir-safe-${edge}`))||0);
    };
    refreshSafeArea();
    const size=()=>{const dpr=Math.min(devicePixelRatio||1,2);const vv=window.visualViewport;const vw=vv?vv.width:innerWidth,vh=vv?vv.height:innerHeight;const width=Math.max(1,vw-viewportOffset.left-viewportOffset.right),height=Math.max(1,vh-viewportOffset.top-viewportOffset.bottom);return {width,height,dpr};};
    let {width,height,dpr}=size();canvas.width=Math.round(width*dpr);canvas.height=Math.round(height*dpr);
    if(!['dev','release'].includes(release.profile)||!releaseDigestPattern.test(releaseDigest))throw new Error('E_RELEASE_IDENTITY');
    const sharedKey=profileKey(release.game_id,release.profile);
    const slotKey=slot=>saveKey(release.game_id,release.profile,releaseDigest,slot);
    const storage=new SaveDatabaseConnection();
    const startup=await storage.runRead(db=>readStartupMetadata(db,sharedKey)).catch(error=>({
        preferences:null,profile:[],failures:['preferences','profile'].map(kind=>({kind,message:String(error)})),
    }));
    if(program.requires?.includes('story.profile-value.v1')){
        try{startup.profileValues=await storage.runRead(db=>readMetadataRecord(db,'profile_values',sharedKey));}
        catch(error){startup.profileValues={};startup.failures.push({kind:'profile_values',message:String(error)});}
    }
    const metadataReadFailures=new Map(startup.failures.map(f=>[f.kind,f.message])),metadataWriteFailures=new Set(),metadataPendingWrites=new Set();
    let preferences=initialRuntimePreferences(program,startup.preferences,navigator.languages||[],matchMedia('(prefers-reduced-motion: reduce)').matches);
    observe('preferences_loaded');
    const createStart=String(Math.round(performance.now()*1000));
    const workerTimingsRing={capacity:4096,buf:new Array(4096),head:0,len:0,dropped:0,incomplete:false};
    const pushWorkerTiming=row=>{
        if(workerTimingsRing.len<workerTimingsRing.capacity){
            workerTimingsRing.buf[(workerTimingsRing.head+workerTimingsRing.len)%workerTimingsRing.capacity]=row;
            workerTimingsRing.len++;
        }else{
            workerTimingsRing.buf[workerTimingsRing.head]=row;
            workerTimingsRing.head=(workerTimingsRing.head+1)%workerTimingsRing.capacity;
            workerTimingsRing.dropped++;
        }
    };
    const workerTimingsSnapshot=()=>{
        const rows=new Array(workerTimingsRing.len);
        for(let i=0;i<workerTimingsRing.len;i++)rows[i]=workerTimingsRing.buf[(workerTimingsRing.head+i)%workerTimingsRing.capacity];
        return {capacity:workerTimingsRing.capacity,rows,dropped:workerTimingsRing.dropped,incomplete:workerTimingsRing.incomplete};
    };
    const threading={runtime:'main',asset:'inline',protocol:WORKER_PROTOCOL,fallback_reason:null};
    const workerOptions={release,releaseRoot,fetchObject,wasmBytes,testMode};
    let runtimeClient=null,assetClient=null;
    if(typeof Worker==='function'&&release.engine.asset_worker&&params.get('assets')!=='main'){
        try{assetClient=await connectWorker('asset',workerOptions);threading.asset='worker';}
        catch(error){if(params.get('worker')==='required')throw error;threading.asset_fallback_reason=String(error);}
    }
    if(assetClient){
        fetchObject=async(id,signal,observer=()=>{})=>{
            const descriptor=release.objects[id];if(!descriptor)throw new Error('E_OBJECT_REFERENCE');
            const reply=await assetClient.call('fetch',{url:new URL(descriptor.path,releaseRoot).href,digest:id,bytes:descriptor.bytes},[],{signal,timeout:0});
            const bytes=reply.bytes;threading.asset_wasm_memory_bytes=reply.wasmMemoryBytes;
            if(trace.enabled){if(reply.resource){const row={...reply.resource};for(const key of ['startTime','fetchStart','domainLookupStart','domainLookupEnd','connectStart','connectEnd','requestStart','responseStart','responseEnd'])if(row[key])row[key]+=assetClient.clockOffsetUs/1000;pushWorkerTiming(row);}else workerTimingsRing.incomplete=true;}
            observer('object_verified',{object:id,bytes:bytes.byteLength});return bytes;
        };
    }
    const originalWasm=wasm;
    const AudioContext=window.AudioContext||window.webkitAudioContext;
    let audioDomains=null,audio=null;
    function decoderSampleRate() {
        if(!audioDomains){audioDomains=new AudioDomains(AudioContext);audio=audioDomains.context('story');}
        const rate=audio.sampleRate;
        if(!Number.isSafeInteger(rate)||rate<=0||rate>0xffffffff)throw new Error('E_AUDIO_RATE: invalid decoder rate');
        return rate;
    }
    let initialized;
    const createRuntime=async(backend)=>{
        if(!runtimeClient||runtimeClient.closed)runtimeClient=await connectWorker('runtime',workerOptions);
        const offscreen=canvas.transferControlToOffscreen();
        try{
            const reply=await runtimeClient.call('create',{canvas:offscreen,size:{width,height,dpr},executable,release:releaseDigest,title:release.title,preferences:JSON.stringify(preferences),backend,audio_sample_rate:decoderSampleRate(),profiling:trace.enabled},[offscreen]);
            const remote=new RemoteEngine(runtimeClient,reply.snapshot);remote.size={width,height,dpr};threading.runtime='worker';return remote;
        }catch(error){runtimeClient.close();throw error;}
    };
    const workersAvailable=typeof Worker==='function'&&typeof OffscreenCanvas==='function'&&typeof canvas.transferControlToOffscreen==='function'&&release.engine.runtime_worker;
    try{
        if(params.get('worker')!=='main'&&workersAvailable){
            try{
                runtimeClient=await connectWorker('runtime',workerOptions);
                initialized=await initializeBackend({requested:params.get('backend')||'auto',probe:()=>runtimeClient.call('probe',null),create:createRuntime,replaceCanvas});
            }catch(error){runtimeClient?.close();if(params.get('worker')==='required')throw error;threading.fallback_reason=String(error);replaceCanvas();}
        }else if(params.get('worker')==='required')throw new Error('E_WORKER_UNAVAILABLE');
        if(!initialized){
            await originalWasm.default({module_or_path:wasmBytes});
            initialized=await initializeBackend({requested:params.get('backend')||'auto',probe:()=>originalWasm.probe_backend(),create:backend=>originalWasm.Engine.create(executable,releaseDigest,release.title,'stage',JSON.stringify(preferences),backend,decoderSampleRate()),replaceCanvas});
        }
    }catch(error){audioDomains?.close();runtimeClient?.close();assetClient?.close();storage.close();throw error;}
    const {engine,fallbackReason}=initialized;
    if(engine.remote){
        wasm={create_gpu:async(_id,backend)=>{
            await engine.sync();let transfer=[],replacement;
            if(backend==='webgl2'){replacement=canvas.transferControlToOffscreen();transfer=[replacement];}
            const reply=await runtimeClient.call('gpu',{backend,canvas:replacement},transfer,{control:true});
            return {snapshot:reply.snapshot,free(){}};
        }};
    }
    const activeBackend=engine.backend();
    // Release startup-only copies of raw WASM and executable JSON now that
    // the runtime/asset workers have instantiated them.
    workerOptions.wasmBytes=null;
    wasmBytes=null;
    executable=null;
    engine.set_profiling(trace.enabled);
    if(testMode&&typeof engine.set_tracing==='function')engine.set_tracing(true);
    // Apply metadata before exposing input or submitting boot presentation.
    engine.host_event('profile',stringifyPlayerData(startup.profile));
    if(startup.profileValues)engine.host_event('profile_values',stringifyPlayerData(startup.profileValues));
    for(const failure of startup.failures)engine.host_event('persistence_read_failed',stringifyPlayerData(failure));
    if(engine.remote)await engine.sync();
    preferences=JSON.parse(engine.state()).preferences;
    observe('wasm_initialized');
    observe('engine_created',{start_us:createStart,end_us:String(Math.round(performance.now()*1000))});
    document.title=release.title;
    const historyButton=document.createElement('button');historyButton.id='nir-history-button';historyButton.type='button';historyButton.hidden=true;historyButton.textContent='发行存档 / Save history';historyButton.onclick=()=>{if(state().screen==='Story')void action({type:'menu'});historyPanel.hidden=false;document.querySelector('#actions').inert=true;historyClose.focus();void refreshHistory();};document.body.append(historyButton);
    let devBanner=null;
    if(release.profile==='dev'){devBanner=document.createElement('div');devBanner.id='nir-dev-banner';devBanner.textContent='Development build · saves are isolated';document.body.append(devBanner);}
    const historyPanel=document.createElement('section');historyPanel.id='nir-history-panel';historyPanel.hidden=true;historyPanel.setAttribute('role','dialog');historyPanel.setAttribute('aria-modal','true');historyPanel.setAttribute('aria-label','Save history');
    const historyInner=document.createElement('div');historyInner.className='nir-history-inner';
    const historyHeading=document.createElement('h2');historyHeading.textContent='发行存档 / Save history';historyInner.append(historyHeading);
    const historyClose=document.createElement('button');historyClose.type='button';historyClose.textContent='关闭 / Close';historyClose.onclick=()=>{historyGeneration++;historyReadController?.abort();historyPanel.hidden=true;document.querySelector('#actions').inert=false;historyButton.focus();};historyInner.append(historyClose);
    const historyRefresh=document.createElement('button');historyRefresh.type='button';historyRefresh.id='nir-history-refresh';historyRefresh.style.minHeight='44px';historyRefresh.textContent='刷新 / Refresh';historyRefresh.onclick=()=>void refreshHistory();historyInner.append(historyRefresh);
    const historyMessage=document.createElement('p');historyMessage.setAttribute('role','status');historyInner.append(historyMessage);
    const historyTable=document.createElement('table'),historyHead=document.createElement('thead'),historyHeadRow=document.createElement('tr'),historyBody=document.createElement('tbody');
    for(const heading of ['Release / version','Slot','Saved','Status','Actions']){const th=document.createElement('th');th.textContent=heading;historyHeadRow.append(th);}
    historyHead.append(historyHeadRow);historyTable.append(historyHead,historyBody);historyInner.append(historyTable);historyPanel.append(historyInner);document.body.append(historyPanel);
    historyPanel.addEventListener('keydown',e=>{e.stopPropagation();if(e.key==='Escape'){e.preventDefault();historyClose.click();}else if(e.key==='Tab'){const buttons=[...historyPanel.querySelectorAll('button:not(:disabled)')];const first=buttons[0],last=buttons.at(-1);if(e.shiftKey&&document.activeElement===first){e.preventDefault();last.focus();}else if(!e.shiftKey&&document.activeElement===last){e.preventDefault();first.focus();}}});
    const historyReadPool=new WorkPool(2);
    async function inspectHistoryEntry(entry,signal){
      return historyReadPool.run(async()=>{
        const record=await storage.run(db=>readSaveRecord(db,entry.key,{signal}));
        signal?.throwIfAborted();
        let error=null;
        try{
            const slotCount=program.player?.save_slots||3;
            if(!validHistoryKey(entry.key,slotCount))throw new Error('E_SAVE_IDENTITY: invalid stored key');
            if(await inspectSaveRecord(record,entry.key,(json,key)=>inspectSlot(json,key,{signal,timeout:0}))===null)throw new Error('E_SAVE_MISSING');
        }catch(e){signal?.throwIfAborted();if(/\bE_WORKER_/.test(String(e)))throw e;error=String(e).slice(0,1024);}
        signal?.throwIfAborted();
        return {record,error};
      },signal,{priority:'required',phase:'saved_release'});
    }
    async function downloadSaveRecord(entry){
        const generation=historyGeneration,signal=historyReadController?.signal;
        const key=entry.key,digest=typeof key[2]==='string'&&releaseDigestPattern.test(key[2])?key[2].slice(0,12):'unknown';
        const slot=Number.isInteger(key[3])?key[3]:'unknown';
        try{
            // Re-read and inspect at the click. Never export an earlier checked
            // copy after another tab replaces the persisted record.
            const {record,error}=await checkSavedRelease(signal=>inspectHistoryEntry(entry,signal),{signal}),readable=!error;
            if(generation!==historyGeneration||disposed)return;
            const json=saveHistoryExport({key,record},readable);
            const url=URL.createObjectURL(new Blob([json],{type:'application/json'}));
            const a=document.createElement('a');a.href=url;a.download=`${release.game_id}-${digest}-slot-${slot}.${readable?'nir-save':'nir-save-record'}.json`;a.click();setTimeout(()=>URL.revokeObjectURL(url),1000);
        }catch(error){if(generation===historyGeneration&&!disposed)historyMessage.textContent=historyCheckError(error,'Unable to export this record');}
    }
    function historyCheckError(error,prefix){return error?.operation==='release_check'?'检查超时，请刷新 / Check timed out; refresh to retry.':`${prefix}: ${error}`;}
    let historyGeneration=0,historyReadController=null;
    async function refreshHistory(){
        const generation=++historyGeneration;historyReadController?.abort();
        const controller=new AbortController();historyReadController=controller;
        const signal=controller.signal;historyRefresh.disabled=true;
        historyMessage.textContent='Checking saved releases…';historyBody.replaceChildren();
        const statuses=[];
        try{
          await checkSavedRelease(async checkSignal=>{
            const entries=await storage.run(db=>listHistoryRecords(db,release.game_id,release.profile,{signal:checkSignal}));
            if(generation!==historyGeneration||disposed)return;
            if(!entries.length){historyMessage.textContent='No saves for this game and profile.';return;}
            historyMessage.textContent=`${entries.length} saved slot${entries.length===1?'':'s'}`;
            const verifiedTargets=new Map(),checks=[];
            const targets=new SharedRequests(async(digest,signal)=>{
                if(verifiedTargets.has(digest))return verifiedTargets.get(digest);
                const target=await validateHistoryTarget({releaseRoot,digest,gameId:release.game_id,profile:release.profile,sha256,signal});
                if(target.available)verifiedTargets.set(digest,target);
                return target;
            });
            for(const entry of entries){
                const {key,savedAt,version}=entry;
                const row=document.createElement('tr');
                const identity=document.createElement('td');const digest=document.createElement('code');digest.textContent=typeof key[2]==='string'?key[2].slice(0,256):'Unknown release';
                identity.append(document.createTextNode(`${version} · `),digest);
                const slot=document.createElement('td');slot.textContent=Number.isInteger(key[3])&&key[3]>=0&&key[3]<3?String(key[3]+1):'Unknown';
                const date=document.createElement('td');date.textContent=savedAt?new Date(savedAt).toLocaleString(preferences.ui_locale||undefined):'Unknown';
                const status=document.createElement('td');status.className='nir-history-status';status.textContent='Checking…';
                statuses.push(status);
                const actions=document.createElement('td');const exportButton=document.createElement('button');exportButton.type='button';exportButton.textContent='Export';exportButton.disabled=true;actions.append(exportButton);
                const openButton=document.createElement('button');openButton.type='button';openButton.textContent='Open release';openButton.disabled=true;actions.append(openButton);
                row.append(identity,slot,date,status,actions);historyBody.append(row);
                checks.push(async()=>{
                    try{
                      await checkSavedRelease(async rowSignal=>{
                        const {error}=await inspectHistoryEntry(entry,rowSignal);
                        if(generation!==historyGeneration||disposed)return;
                        exportButton.textContent=error?'Export record':'Export';exportButton.disabled=false;
                        exportButton.onclick=async()=>{
                            if(generation!==historyGeneration||disposed)return;
                            exportButton.disabled=true;
                            try{await downloadSaveRecord(entry);}
                            finally{if(generation===historyGeneration&&!disposed)exportButton.disabled=false;}
                        };
                        if(error){status.textContent=`Unreadable save: ${error}`;return;}
                        const target=await targets.get(key[2],rowSignal);
                        if(generation!==historyGeneration||disposed)return;
                        status.textContent=target.status;
                        if(target.available){
                            openButton.disabled=false;openButton.onclick=async()=>{
                                openButton.disabled=true;
                                try{
                                    const latest=await checkSavedRelease(signal=>inspectHistoryEntry(entry,signal),{signal});
                                    if(generation!==historyGeneration||disposed)return;
                                    if(latest.error){status.textContent=`Unreadable save: ${latest.error}`;exportButton.textContent='Export record';return;}
                                    const targetUrl=new URL(target.url);targetUrl.search=location.search;location.assign(targetUrl.href);
                                }catch(error){if(generation===historyGeneration&&!disposed){status.textContent=historyCheckError(error,'Unable to check save');openButton.disabled=false;}}
                            };
                        }
                      },{signal:checkSignal});
                    }catch(error){if(generation===historyGeneration&&!disposed)status.textContent=historyCheckError(error,'Unable to check save');}
                });
            }
            // Only lightweight metadata/DOM is retained for the whole list.
            // Large snapshot reads and Worker inspection have two consumers.
            let next=0;
            const checkNext=async()=>{while(next<checks.length&&!checkSignal.aborted&&generation===historyGeneration&&!disposed)await checks[next++]();};
            await Promise.all([checkNext(),checkNext()]);
          },{signal,timeoutMs:10000});
        }catch(error){if(generation===historyGeneration&&!disposed){historyMessage.textContent=historyCheckError(error,'Unable to read save history');for(const status of statuses)if(status.textContent==='Checking…')status.textContent=historyCheckError(error,'Unable to check save');}}
        finally{if(generation===historyGeneration&&!disposed)historyRefresh.disabled=false;}
    }
    const buffers=new Map(),voices=new Map(),bytesCache=new Map(),imageStaging=new Map(),assetDescriptors=new Map(),requests=new SharedRequests((id,signal)=>fetchObject(id,signal,observe)),preparations=new Map(),contentPreparations=new Map(),contentStaging=new ContentStagingBudget(CONTENT_STAGING_LIMIT,syncContentStagingMetrics);
    const mediaPeaks={encoded_cache_bytes:0,decoded_audio_bytes:0,image_staging_bytes:0};
    function mediaMemory() {
        const current=mediaMemorySnapshot(bytesCache,buffers,voices,imageStaging);
        if(trace.enabled)for(const key of Object.keys(mediaPeaks))mediaPeaks[key]=Math.max(mediaPeaks[key],current[key]);
        return {...current,peaks:trace.enabled?{...mediaPeaks}:null};
    }
    let raf=0,lastTime=null,sequence=0,disposed=false,recovering=false,pendingFocusId=null;
    const inbox=new OwnerInbox(256,128,8,observe,()=>traceContext),resourcePool=new WorkPool(4),decodePool=new WorkPool(2),uploadPool=new WorkPool(1);
    const decodeRequests=new SharedRequests((id,signal,source)=>decodePool.run(()=>audio.decodeAudioData(source.bytes.slice(0)),signal,{priority:source.priority,group:source.group,deadline:source.deadline,phase:'audio_decode'}));
    let ownerTimer=null,pendingElapsed=new DomainElapsed(),wakeRequestedAt=null,pendingWakeWaitStartUs=null,pendingWakeWaitEndUs=null,cachedHostState=null;
    function invalidateHostState(){cachedHostState=null;}
    function mutateEngine(run) {try{return run();}finally{invalidateHostState();}}
    function hostEvent(kind,value) {mutateEngine(()=>engine.host_event(kind,typeof value==='string'?value:stringifyPlayerData(value)));}
    function reportHostFailure(error,operation) {
        observe('diagnostic',{domain:'host',code:'E_HOST',operation});
        // A thrown WASM call may still own its mutable Engine borrow until this
        // callback returns. Report the failure in a later owner turn.
        queueMicrotask(()=>{if(!disposed)deliver(()=>hostEvent('host_failed',String(error)),'control');});
    }
    function wake() {
        if(disposed||ownerTimer!==null)return;
        if(performanceStats)wakeRequestedAt=performance.now();
        ownerTimer=setTimeout(()=>{
            ownerTimer=null;const now=performance.now();
            if(performanceStats){pendingWakeWaitStartUs=Math.round(wakeRequestedAt*1000);pendingWakeWaitEndUs=Math.round(now*1000);wakeRequestedAt=null;}
            frame(now);
        },0);
    }
    function deliver(fn,kind='completion',group=null) {
        if(disposed)return Promise.resolve(false);
        return new Promise(resolve=>{
            if(!inbox.push(()=>{try{fn();if(engine.remote)pendingDeliveries.push(resolve);else resolve(true);}catch(e){reportHostFailure(e,'dispatch');resolve(false);}},kind,()=>resolve(false),group)){
                fail('E_EVENT_QUEUE: host inbox admission limit');resolve(false);dispose();return;
            }
            wake();
        });
    }
    function post(slot,fn,terminal=true){
        if(disposed){slot.cancel();return Promise.resolve(false);}
        const done=slot.post(()=>{try{return fn();}catch(e){if(slot.kind==='control')throw e;reportHostFailure(e,'completion');return true;}},{terminal});wake();return done;
    }
    function request(work,success,failure,{kind='completion',group=null,replace=false,pending}={}){
        if(replace)inbox.cancelGroup(group);
        const slot=inbox.reserve(kind,group);
        if(!slot){failure(new Error('E_REQUEST_CAPACITY: no terminal slot'));return null;}
        void dispatchOwnerRequest(work,slot,{post,success,failure,pending});
        return slot;
    }
    let touchInput=matchMedia('(pointer: coarse)').matches;
    engine.set_touch_input(touchInput);
    if(engine.remote){engine.onUpdate=()=>{invalidateHostState();wake();};runtimeClient.onFailure=error=>{if(!disposed){fail(error);dispose();}};}
    if(assetClient)assetClient.onFailure=error=>{if(!disposed){fail(error);dispose();}};
    let metadataRetryPending=false,metadataRetryController=null;
    const metadataPanel=document.createElement('div');metadataPanel.id='nir-storage-recovery';metadataPanel.hidden=true;
    Object.assign(metadataPanel.style,{position:'fixed',bottom:'12px',right:'12px',zIndex:'999',padding:'10px',borderRadius:'8px',background:'#182535',color:'#fff',font:'14px sans-serif',maxWidth:'min(420px, calc(100vw - 24px))',boxSizing:'border-box'});
    const metadataStatus=document.createElement('span');metadataStatus.id='nir-storage-status';metadataStatus.setAttribute('role','status');metadataStatus.setAttribute('aria-live','polite');
    const metadataRetryButton=document.createElement('button');metadataRetryButton.id='nir-storage-retry';metadataRetryButton.type='button';
    Object.assign(metadataRetryButton.style,{marginLeft:'8px',padding:'8px 12px',minHeight:'44px',borderRadius:'6px',font:'14px sans-serif'});
    metadataPanel.append(metadataStatus,metadataRetryButton);document.body.append(metadataPanel);
    metadataPanel.addEventListener('keydown',e=>e.stopPropagation());
    const recoverableMetadataKinds=()=>[...new Set([...metadataReadFailures.keys(),...metadataWriteFailures])].filter(kind=>!metadataPendingWrites.has(kind));
    metadataRetryButton.onclick=e=>{
        e.stopPropagation();if(disposed||metadataRetryPending||!recoverableMetadataKinds().length)return;
        metadataRetryPending=true;updateMetadataRecovery();
        void deliver(retryMetadata,'control').then(accepted=>{if(!accepted&&!disposed){metadataRetryPending=false;updateMetadataRecovery();}});
    };
    function updateMetadataRecovery() {
        if(disposed)return;
        const chinese=state().locale?.startsWith('zh'),unread=metadataReadFailures.size>0,unsaved=metadataWriteFailures.size>0;
        // Keep the dialogue area clear. Recovery stays available on the title
        // and menu pages, including a request begun before returning to Story.
        metadataPanel.hidden=state().screen==='Story'||(!unread&&!unsaved&&!metadataRetryPending);
        const message=metadataRetryPending?(chinese?'正在恢复设置与已读进度…':'Restoring settings and read progress…'):
            metadataPendingWrites.size?(chinese?'仍在确认设置或已读进度的保存结果，当前阅读已保留。':'Still confirming saved settings or read progress. Your current reading is kept.'):
            unread?(chinese?'设置或已读进度尚未恢复，当前阅读已保留。':'Some settings or read progress could not be restored. Your current reading is kept.'):
            (chinese?'设置或已读进度尚未保存，当前阅读已保留。':'Some settings or read progress could not be saved. Your current reading is kept.');
        if(metadataStatus.textContent!==message)metadataStatus.textContent=message;
        const label=chinese?'重试':'Retry';if(metadataRetryButton.textContent!==label)metadataRetryButton.textContent=label;metadataRetryButton.disabled=metadataRetryPending||!recoverableMetadataKinds().length;
        if(metadataPanel.hidden&&metadataPanel.contains(document.activeElement))document.activeElement.blur();
    }
    function retryMetadata() {
        if(disposed)return;
        const kinds=recoverableMetadataKinds();
        const controller=new AbortController();metadataRetryController=controller;
        const finish=()=>{if(metadataRetryController===controller){metadataRetryController=null;metadataRetryPending=false;}updateMetadataRecovery();};
        const failed=(kind,error)=>{metadataReadFailures.set(kind,String(error));hostEvent('persistence_read_failed',{kind,message:String(error)});};
        request(()=>storage.run(db=>Promise.all(kinds.map(async kind=>{
            try{return {kind,value:await readMetadataRecord(db,kind,sharedKey,{signal:controller.signal})};}
            catch(error){return {kind,error};}
        }))),rows=>{
            try{for(const row of rows){
                if(row.error){failed(row.kind,row.error);continue;}
                metadataReadFailures.delete(row.kind);
                if(row.kind==='preferences')hostEvent('preferences_recovered',initialRuntimePreferences(program,row.value,navigator.languages||[],matchMedia('(prefers-reduced-motion: reduce)').matches));
                else if(row.kind==='profile_values'){hostEvent('profile_values_recovered',row.value);persistence.retry('profile_values');}
                else {hostEvent('profile_recovered',row.value);persistence.retry('profile');}
            }}finally{finish();}
        },error=>{try{for(const kind of kinds)failed(kind,error);}finally{finish();}},{group:'metadata-retry'});
    }
    let audioBlocked=false;
    const audioRecoveryPanel=document.createElement('div');audioRecoveryPanel.id='nir-audio-recovery';audioRecoveryPanel.hidden=true;
    Object.assign(audioRecoveryPanel.style,{position:'fixed',left:'50%',top:'16px',transform:'translateX(-50%)',zIndex:'1000',padding:'12px',borderRadius:'8px',background:'#182535',color:'#fff',border:'1px solid #b6c8dd',font:'16px sans-serif',width:'min(480px, calc(100vw - 32px))',boxSizing:'border-box',overflowY:'auto',overscrollBehavior:'contain'});
    const audioResumeButton=document.createElement('button');
    audioResumeButton.id='nir-audio-resume';audioResumeButton.type='button';audioResumeButton.hidden=true;
    Object.assign(audioResumeButton.style,{padding:'12px 18px',minHeight:'44px',borderRadius:'8px',background:'#182535',color:'#fff',border:'1px solid #b6c8dd',font:'16px sans-serif'});
    audioResumeButton.onclick=e=>{e.stopPropagation();unlock();};
    const audioRecoveryStatus=document.createElement('p');audioRecoveryStatus.id='nir-audio-recovery-status';audioRecoveryStatus.setAttribute('role','status');audioRecoveryStatus.style.margin='8px 0';
    const audioRecoverySaves=document.createElement('button');audioRecoverySaves.id='nir-audio-recovery-saves';audioRecoverySaves.type='button';audioRecoverySaves.hidden=true;
    Object.assign(audioRecoverySaves.style,{padding:'12px 18px',minHeight:'44px',borderRadius:'8px',background:'#182535',color:'#fff',border:'1px solid #b6c8dd',font:'16px sans-serif'});
    audioRecoverySaves.onclick=e=>{e.stopPropagation();if(!disposed&&state().screen==='Story'&&!state().loading)action({type:'saves'},state());};
    audioRecoveryPanel.append(audioResumeButton,audioRecoveryStatus,audioRecoverySaves);document.body.append(audioRecoveryPanel);
    function checkAudioOutput() {
        if(disposed)return;
        const blocked=audioDomains.blocked(voices.values());
        if(audioBlocked!==blocked) {
            audioBlocked=blocked;
            mutateEngine(()=>engine.audio_blocked(blocked));
            observe(blocked?'audio_output_wait':'audio_output_ready');
        }
        audioRecoveryPanel.hidden=audioResumeButton.hidden=!blocked||document.hidden;
        const chinese=state().locale?.startsWith('zh'),recovery=audioDomains.recoveryState(voices.values()),status=recovery.status;
        audioResumeButton.disabled=!recovery.canRetry;
        const label=recovery.canRetry?(chinese?'点击恢复声音':'Tap to resume sound'):
            status==='failed'?(chinese?'声音不可用':'Sound unavailable'):(chinese?'正在恢复声音…':'Resuming sound…');
        if(audioResumeButton.textContent!==label)audioResumeButton.textContent=label;
        const message=status==='failed'?(recovery.canRetry?
            (chinese?'声音尚未恢复。可重试；若仍无声，请先保存进度，再重新打开页面。':'Sound has not resumed. Try again; if it stays unavailable, save your progress before reopening this page.'):
            (chinese?'声音不可用。请先保存进度，再重新打开页面。':'Sound is unavailable. Save your progress before reopening this page.')):
            status==='waiting'?(chinese?'仍在等待声音恢复，阅读位置保持不变。你可以先保存进度。':'Still waiting for sound. Your reading position is held. You can save your progress.'):
            status==='pending'?(chinese?'正在恢复声音，阅读位置保持不变。':'Resuming sound. Your reading position is held.'):
            (chinese?'点击恢复声音后继续阅读，本次点击不会翻页。':'Resume sound to continue. This tap will keep your place.');
        if(audioRecoveryStatus.textContent!==message)audioRecoveryStatus.textContent=message;
        audioRecoverySaves.hidden=!['failed','waiting'].includes(status)||state().screen!=='Story';audioRecoverySaves.disabled=state().loading;
        const saveLabel=chinese?'打开存档':'Open saves';if(audioRecoverySaves.textContent!==saveLabel)audioRecoverySaves.textContent=saveLabel;
    }
    // Coalesce state changes from all four contexts into one owner wake.
    audioDomains.changed=wake;
    function unlock() { audioDomains.unlock(); }
    function stopVoice(id) {
        const v=voices.get(id);if(!v)return;v.stopped=true;v.slot.cancel();
        try{v.source?.stop();}catch{}
        // stop() awaits a render quantum. A suspended context may not run
        // another one until the next user gesture, so release the retired
        // source's PCM and callback without resuming the paused audio graph.
        if(v.source){v.source.onended=null;try{v.source.buffer=null;}catch{}try{v.source.disconnect();}catch{}}
        try{v.gain?.disconnect();}catch{}try{v.envelope?.disconnect();}catch{}voices.delete(id);
    }
    function playVoice(c) {
        const key=audioVoiceKey(c);
        let context=audioDomains.context(c.domain,c.bus);
        if(context.state==='closed')context=audioDomains.recreateClosed(c.domain,c.bus)||context;
        stopVoice(key);
        const failed=e=>mutateEngine(()=>engine.audio_failed_in(c.domain,c.task,c.session,String(e)));
        const slot=inbox.reserve('completion',`audio:${key}`,{session:c.session,task:c.task});
        if(!slot){failed('E_REQUEST_CAPACITY');return;}
        const buffer=buffers.get(c.asset);
        if(!buffer){post(slot,()=>failed('E_AUDIO_BUFFER'));return;}
        let v;
        try {
            if(c.loop_region&&!c.looped)throw new Error('E_AUDIO_LOOP: region on finite voice');
            const loop=c.loop_region?audioLoopPlayback(c.loop_region,c.position_us,buffer.sampleRate,buffer.length,assetDescriptors.get(c.asset)?.duration_us):null;
            const source=context.createBufferSource(),gain=context.createGain(),envelope=context.createGain();source.buffer=buffer;source.loop=c.looped;envelope.gain.value=c.envelope??1;
            if(loop)source.loopStart=loop.startFrame/buffer.sampleRate;
            if(c.looped)source.loopEnd=audioLoopEndSeconds(loop?loop.endFrame:buffer.length,buffer.sampleRate);
            gain.gain.value=(c.gain??1)*(preferences[`${c.bus}_volume`]??.5)*(c.bus==='voice'?characterVoiceGain(preferences,c.character):1);source.connect(envelope).connect(gain).connect(context.destination);
            v={domain:c.domain,session:c.session,task:c.task,position:Number(c.position_us)/1e6,context,source,gain,envelope,slot,eventGain:c.gain??1,bus:c.bus,character:c.character??'',asset:c.asset,stopped:false};voices.set(key,v);
            let offset=loop?loop.offsetFrame/buffer.sampleRate:Number(c.position_us)/1e6;
            if(!loop){if(c.looped)offset%=buffer.duration;else offset=Math.min(offset,Math.max(0,buffer.duration-.001));}
            source.onended=()=>{
                source.onended=null;try{source.buffer=null;}catch{}try{source.disconnect();}catch{}try{gain.disconnect();}catch{}try{envelope.disconnect();}catch{}
                if(!v.stopped&&!c.looped)post(slot,()=>{
                    if(voices.get(key)!==v||v.stopped)return;
                    voices.delete(key);
                    mutateEngine(()=>engine.audio_ended_in(c.domain,c.task,c.session));
                });
            };
            source.start(0,offset);v.started=context.currentTime;metrics.audioStarts++;
        } catch(e){
            if(v){v.stopped=true;v.source.onended=null;try{v.source.buffer=null;}catch{}try{v.source.disconnect();}catch{}try{v.gain.disconnect();}catch{}try{v.envelope.disconnect();}catch{}voices.delete(key);}
            post(slot,()=>failed(e));
        }
    }
    function sampleAudioPositions() {
        const groups=new Map();
        for(const v of voices.values()) {
            if(v.domain!=='story'||v.stopped||v.started===undefined)continue;
            let position=v.position+Math.max(0,v.context.currentTime-v.started);
            if(!v.source.loop)position=Math.min(position,v.source.buffer.duration);
            const positions=groups.get(v.session)||[];
            positions.push({task:v.task,position_us:String(Math.round(position*1e6)),envelope:envelopePosition(v.envelopePlan,v.context.currentTime)});
            groups.set(v.session,positions);
        }
        for(const [session,positions] of groups)mutateEngine(()=>engine.audio_positions_in('story',session,JSON.stringify(positions)));
    }
    async function asset(id,a,signal,context) {
        if(!a)throw new Error(`E_ASSET_DESCRIPTOR: ${id}`);
        signal.throwIfAborted();
        if(bytesCache.has(a.object)){
            const bytes=bytesCache.get(a.object);
            if(bytes.byteLength!==a.bytes)throw Object.assign(new Error(`E_ASSET_SIZE: ${id}`),{code:'E_ASSET_SIZE'});
            observe('bytes_cache_hit',context);return bytes;
        }
        observe('fetch_started',context);
        const bytes=await requests.get(a.object,signal,{settleOnAbort:true});signal.throwIfAborted();
        if(bytes.byteLength!==a.bytes)throw Object.assign(new Error(`E_ASSET_SIZE: ${id}`),{code:'E_ASSET_SIZE'});
        observe('fetch_verified',{...context,bytes:bytes.byteLength});
        bytesCache.set(a.object,bytes);if(trace.enabled)mediaMemory();return bytes;
    }
    function cancelPreparation(request) {
        inbox.cancelGroup(request);
        const job=preparations.get(request);
        const acknowledge=()=>deliver(()=>{prune(true);hostEvent('assets_cancelled',{request});},'control');
        if(job){
            if(job.cancelAckQueued)return;
            job.cancelAckQueued=true;job.controller.abort();
            // The final consumer of an uncancellable decoder waits for its
            // actual settlement before Rust may release the retired budget.
            job.done.then(acknowledge);
        }else acknowledge();
    }
    function promotePreparation(request,session) {
        const job=preparations.get(request);
        if(!job||job.session!==session||job.signal.aborted)return false;
        job.priority='required';
        resourcePool.promoteGroup(request);decodePool.promoteGroup(request);uploadPool.promoteGroup(request);
        return true;
    }
    function prune(force=false) {
        if(disposed||(recovering&&!force))return;
        const retained=JSON.parse(engine.retained_descriptors());
        for(const [id,descriptor] of Object.entries(retained))assetDescriptors.set(id,descriptor);
        const keep=new Set(Object.keys(retained));for(const v of voices.values())keep.add(v.asset);
        const objects=new Set([...keep].map(id=>assetDescriptors.get(id)?.object).filter(Boolean));
        for(const id of buffers.keys())if(!keep.has(id))buffers.delete(id);
        for(const id of assetDescriptors.keys())if(!keep.has(id))assetDescriptors.delete(id);
        for(const id of bytesCache.keys())if(!objects.has(id))bytesCache.delete(id);
    }
    async function prepare(c) {
        let descriptors;
        try { descriptors=validateAssetRequest(c.assets,c.descriptors); }
        catch(error) { mutateEngine(()=>engine.resource_failed(c.request,String(error)));return; }
        for(const [id,descriptor] of Object.entries(descriptors))assetDescriptors.set(id,descriptor);
        const terminal=inbox.reserve('completion',c.request,{session:c.session,device:c.device});
        if(!terminal){mutateEngine(()=>engine.resource_failed(c.request,'E_REQUEST_CAPACITY'));return;}
        const controller=new AbortController(),signal=controller.signal;
        let resolveDone;
        const done=new Promise(resolve=>resolveDone=resolve);
        const job={controller,signal,session:c.session,priority:workPriority(c.priority),deadline:Number.isFinite(c.deadline_ms)?c.deadline_ms:null,nodes:new Map(),done,resolveDone,cancelAckQueued:false};
        preparations.set(c.request,job);
        const failed=(message,id='',stage='admission',code='E_PREPARE')=>{
            observe('resource_failed',{request:c.request,session:c.session,device:c.device,asset:id,code,operation:stage,domain:'prepare'});
            mutateEngine(()=>engine.resource_fault(c.request,id,code,stage,String(message)));controller.abort();
        };
        let next=0;
        async function worker(){while(next<c.assets.length&&!disposed&&!signal.aborted){
            const id=c.assets[next++],slot=inbox.reserve('resource',c.request,{session:c.session,device:c.device});
            if(!slot){await post(terminal,()=>failed('E_REQUEST_CAPACITY'));return;}
            const descriptor=descriptors[id];
            // The awaited stages are the node's dependencies: verified bytes
            // precede audio decode, then owner upload, then readiness.
            const node={asset:id,stage:'fetch_verify'};job.nodes.set(id,node);
            let stage='fetch';const context={request:c.request,session:c.session,device:c.device,asset:id,object:descriptor?.object};
            observe('resource_queued',context);
            try {
                const bytes=await resourcePool.run(async()=>{
                    observe('resource_admitted',context);
                    return asset(id,descriptor,signal,context);
                },signal,{priority:job.priority,group:c.request,deadline:job.deadline,phase:'fetch_verify'});
                signal.throwIfAborted();
                if(descriptor.kind==='audio'){
                    node.stage=stage='audio_decode';observe('audio_decode_started',context);
                    if(!buffers.has(id)){
                        const shared=decodeRequests.jobs.get(id);
                        if(shared)decodePool.promoteGroup(shared.context.group,job.priority);
                        const buffer=await decodeRequests.get(id,signal,{settleOnAbort:true,context:{bytes,priority:job.priority,group:c.request,deadline:job.deadline}});
                        signal.throwIfAborted();validateDecodedAudio(descriptor,buffer,audio.sampleRate);
                        buffers.set(id,buffer);if(trace.enabled)mediaMemory();
                    }
                    observe('audio_decode_ready',context);
                    // Resource readiness is independent of device unlock.
                    // Sources may be scheduled on a suspended context; a user
                    // gesture unlocks output separately. Never hold a page's
                    // resource lease waiting for resume() to settle.
                }
                let decoded=null;
                if(descriptor.kind==='image'&&assetClient){
                    node.stage=stage='image_decode';
                    decoded=await decodePool.run(()=>{const copy=bytes.slice(0);return assetClient.call('decode',{bytes:copy,width:descriptor.width,height:descriptor.height},[copy],{signal,timeout:0});},signal,{priority:job.priority,group:c.request,deadline:job.deadline,phase:'image_decode'});
                    imageStaging.set(`${c.request}:${id}`,decoded.pixels);if(trace.enabled)mediaMemory();
                    threading.asset_wasm_memory_bytes=decoded.wasmMemoryBytes;signal.throwIfAborted();observe('decode_allocate',{...context,bytes:decoded.pixels.byteLength,start_us:String(Number(decoded.start_us)+(assetClient.clockOffsetUs||0)),end_us:String(Number(decoded.end_us)+(assetClient.clockOffsetUs||0))});observe('image_decode_ready',context);
                }
                node.stage=stage='decode_upload';
                let complete=false;
                while(!complete&&!signal.aborted&&!disposed&&slot.state==='pending'){
                    // Re-admit each bounded owner step. A synchronous PNG
                    // decode remains atomic inside its one owner callback.
                    await uploadPool.run(()=>post(slot,()=>{
                        if(signal.aborted)return true;
                        try {complete=mutateEngine(()=>{if(decoded){const pixels=decoded.pixels?new Uint8Array(decoded.pixels):new Uint8Array();const done=engine.resource_decoded(c.request,id,decoded.width,decoded.height,pixels);decoded.pixels=null;return done;}return engine.resource(c.request,id,new Uint8Array(bytes));});if(complete){node.stage='ready';observe('ordered_use_ready',context);}return complete;}
                        catch(e){metrics.resourceFailures++;failed(e,id,stage,'E_RESOURCE_DECODE_UPLOAD');return true;}
                    },done=>done),signal,{priority:job.priority,group:c.request,deadline:job.deadline,phase:'decode_upload'});
                }
            }catch(e){
                node.stage='failed';
                if(!signal.aborted&&!disposed){metrics.resourceFailures++;await post(slot,()=>failed(e,id,stage,typeof e?.code==='string'?e.code:stage==='fetch'?'E_RESOURCE_FETCH':'E_AUDIO_DECODE'));}
            } finally {imageStaging.delete(`${c.request}:${id}`);if(signal.aborted||disposed)slot.cancel();}
        }}
        try {
            await Promise.all(Array.from({length:Math.min(4,c.assets.length)},worker));
            if(!signal.aborted&&!disposed)await post(terminal,()=>{});
            else terminal.cancel();
        } finally {if(preparations.get(c.request)===job)preparations.delete(c.request);job.resolveDone();}
    }
    function cancelContent(request) {
        const job=contentPreparations.get(request);
        inbox.cancelGroup(`content:${request}`);
        job?.controller.abort();
        if(job)job.cancelled=true;
    }
    function contentDetail(envelope) {
        return JSON.stringify({reason:envelope.reason,total_bytes:envelope.totalBytes,max_bytes:envelope.maxBytes});
    }
    function postContentSkip(job,envelope) {
        return queueContentSkip(job,envelope,{
            post,
            isActive:()=>!disposed,
            skip:skipped=>skipContent(job,skipped),
        });
    }
    function skipContent(job,skipped) {
        observe('module_skipped',{request:job.request,session:job.session,code:skipped.code,bytes:skipped.totalBytes??0});
        mutateEngine(()=>engine.content_skipped(job.request,skipped.code,contentDetail(skipped)));
    }
    function promoteContent(request,session) {
        const job=contentPreparations.get(request);
        if(!job||job.signal.aborted||job.session!==session)return false;
        job.priority='required';job.maxBytes=null;
        resourcePool.promoteGroup(job.group);
        // A queued skip checks this priority when its owner callback runs. It
        // then leaves the reservation pending for this request's result.
        return true;
    }
    async function admitContentStage(job,totalBytes) {
        if(job.priority==='prefetch')return contentStaging.tryReserve(job.group,totalBytes);
        return acquireRequiredContentStage(job,contentStaging,totalBytes,contentPreparations.values());
    }
    async function runContent(job) {
        const {request,session,group,signal}=job;
        const bytes=[],fetched=new Map();job.bytes=bytes;
        try {
            let envelope=contentBatchEnvelope(job.objects,release.objects,{priority:job.priority,maxBytes:job.maxBytes});
            if(!envelope.ok){
                if(job.priority==='prefetch'){
                    if(!await postContentSkip(job,envelope))return;
                    envelope=contentBatchEnvelope(job.objects,release.objects,{priority:'required'});
                    if(!envelope.ok)throw new Error(envelope.code);
                }else throw new Error(envelope.code);
            }
            job.totalBytes=envelope.totalBytes;
            let staged=await admitContentStage(job,envelope.totalBytes);
            if(!staged&&job.priority==='prefetch'){
                const skipped={ok:false,code:'E_PREFETCH_LIMIT',reason:'staging_capacity',totalBytes:envelope.totalBytes,maxBytes:contentStaging.available,prefetch:true};
                if(!await postContentSkip(job,skipped))return;
                staged=await admitContentStage(job,envelope.totalBytes);
            }
            // Promotion can race the synchronous prefetch admission result.
            // Re-enter the required waiter path instead of treating that stale
            // speculative denial as a terminal capacity error.
            if(!staged&&job.priority==='required')staged=await admitContentStage(job,envelope.totalBytes);
            if(!staged)throw new Error('E_CONTENT_STAGE_LIMIT');
            job.staged=true;signal.throwIfAborted();
            for(;;){
                bytes.length=0;
                try{
                    job.state='fetching';
                    bytes.push(...await fetchContentBatch(job,{pool:resourcePool,requests,fetched,observe}));
                    signal.throwIfAborted();
                    job.state='delivering';
                    await post(job.terminal,()=>{
                        if(!signal.aborted&&engine.accepts_content(request)){
                            mutateEngine(()=>engine.content_ready(request,bytes.map(b=>new Uint8Array(b))));
                            observe('module_delivered',{request,session,kind:job.priority,bytes:job.totalBytes});
                        }
                    });
                    break;
                }catch(error){
                    if(signal.aborted||disposed||job.priority!=='prefetch')throw error;
                    const skipped={ok:false,code:'E_PREFETCH_FAILED',reason:'fetch_failed',totalBytes:job.totalBytes,maxBytes:job.maxBytes,prefetch:true};
                    if(!await postContentSkip(job,skipped))return;
                    // A same-batch demand may promote while the skip completion
                    // is queued. Retry the failed object under the same request.
                    if(job.priority!=='required')return;
                }
            }
        }catch(error){
            if(job.stageEviction)await postContentEviction(job,{post,isActive:()=>!disposed,skip:skipped=>skipContent(job,skipped)});
            else if(!signal.aborted&&!disposed)await post(job.terminal,()=>{
                observe('module_failed',{request,session,code:'E_MODULE_PREPARE'});
                mutateEngine(()=>engine.content_failed(request,String(error)));
            });
            else job.terminal?.cancel();
        }finally{
            bytes.length=0;fetched.clear();job.bytes=null;
            if(job.staged)contentStaging.release(group);
            if(contentPreparations.get(request)===job)contentPreparations.delete(request);
        }
    }
    function prepareContent(c) {
        const group=`content:${c.request}`;
        const terminal=inbox.reserve('completion',group,{session:c.session});
        if(!terminal){mutateEngine(()=>engine.content_failed(c.request,'E_REQUEST_CAPACITY'));return;}
        const controller=new AbortController();
        const priority=c.priority==='prefetch'?'prefetch':'required';
        const job={request:c.request,session:c.session,objects:c.objects,group,priority,maxBytes:priority==='prefetch'?c.max_bytes:null,controller,signal:controller.signal,terminal,state:'starting',staged:false,cancelled:false};
        contentPreparations.set(c.request,job);
        void runContent(job);
    }
    function listSaves() {
        const slotCount=program.player?.save_slots||3;
        return request(()=>storage.runRead(db=>listSaveRecords(db,release.game_id,release.profile,releaseDigest,inspectSlot,{slotCount})),
            rows=>hostEvent('slots',rows),e=>hostEvent('load_failed',String(e)),{group:'slots',replace:true});
    }
    function inspectSlot(json,key,options) {
        const value={json,slot:key[3],release:key[2],game:key[0]};
        return runtimeClient?runtimeClient.call('inspect-save',value,[],options):originalWasm.inspect_save_slot(json,value.slot,value.release,value.game);
    }
    function save(c) {
        const context={request:c.job,session:state().session,operation:'save'};observe('storage_started',context);
        const metadata={gameId:release.game_id,profile:release.profile,releaseDigest,slot:c.slot,version:release.version||'',title:release.title||''};
        return request(onPending=>storage.run(db=>commitSaveRecord(db,slotKey(c.slot),c.envelope,metadata,c.expected_revision,inspectSlot,{onPending,locale:preferences.ui_locale})),()=>{observe('storage_committed',context);hostEvent('saved',{job:c.job,slot:c.slot,revision:c.envelope.revision});if(historyPanel&&!historyPanel.hidden)void refreshHistory();},
            e=>{const code=e?.name==='QuotaExceededError'?'E_STORAGE_QUOTA':String(e).includes('E_SAVE_CONFLICT')?'E_SAVE_CONFLICT':'E_STORAGE';observe('diagnostic',{...context,domain:'storage',code});hostEvent('save_failed',{job:c.job,code,message:String(e)});},{group:`save:${c.job}`,pending:error=>{observe('storage_pending',context);hostEvent('save_pending',{job:c.job,message:String(error)});}});
    }
    function load(command) {
        const session=state().session;
        return request(()=>storage.runRead(async db=>{const key=slotKey(command.slot),s=await readSaveRecord(db,key);if(s===undefined)throw new Error('E_SAVE_MISSING');await inspectSaveRecord(s,key,inspectSlot);return s.envelope;}),
            value=>{if(state().session===session)hostEvent('slot_loaded',{job:command.job,envelope:value});},
            e=>{if(state().session===session)hostEvent('slot_load_failed',{job:command.job,message:String(e)});},{group:'load',replace:true});
    }
    function importSave(){
        inbox.cancelGroup('load');const slot=inbox.reserve('completion','load'),session=state().session;
        if(!slot){hostEvent('load_failed','E_REQUEST_CAPACITY');return;}
        const input=document.createElement('input');input.type='file';input.accept='.json,application/json';
        input.oncancel=()=>post(slot,()=>{});
        input.onchange=async()=>{
            if(slot.state==='cancelled')return;
            try{
                const f=input.files[0];if(!f){post(slot,()=>{});return;}
                if(f.size>16*1024*1024)throw new Error('E_SAVE_LIMIT');
                const data=await f.text();post(slot,()=>{if(state().session===session)hostEvent('loaded',data);});
            }catch(e){post(slot,()=>{if(state().session===session)hostEvent('load_failed',String(e));});}
        };
        try{input.click();}catch(e){post(slot,()=>hostEvent('load_failed',String(e)));}
    }
    const persistence=new PersistenceWrites({request,
        writePreferences:(value,onPending)=>{
            // Temporary defaults are not a recovered disk snapshot. Hold user
            // edits until a successful read can merge the untouched fields.
            if(metadataReadFailures.has('preferences'))throw new Error('E_PREFERENCES_UNREAD: restore preferences before saving changes');
            return storage.run(db=>writePreferencesRecord(db,sharedKey,value,{onPending}));
        },
        mergeProfile:(keys,onPending)=>storage.run(db=>mergeProfileRecord(db,sharedKey,keys,{onPending})),
        writeProfileValues:(values,onPending)=>storage.run(db=>writeMetadataRecord(db,'profile_values',sharedKey,values,{onPending})),
        stored:kind=>{metadataPendingWrites.delete(kind);metadataWriteFailures.delete(kind);hostEvent('persistence_stored',{kind});updateMetadataRecovery();},
        failed:(kind,error)=>{if(error?.code==='E_STORAGE_UNCERTAIN')metadataPendingWrites.add(kind);else metadataPendingWrites.delete(kind);metadataWriteFailures.add(kind);hostEvent('persistence_failed',{kind,message:String(error)});updateMetadataRecovery();},
    });
    function flush() {if(disposed)return;
        if(trace.enabled){const current=state();traceContext={session:current.session,device:current.device,locale:current.locale};}
        for(const c of JSON.parse(engine.commands())){
        switch(c.type){
            case 'observation':observe(c.stage,c);break;
            case 'resource_stage':observe(c.stage,c);break;
            case 'diagnostic':{const d=c.diagnostic;observe('diagnostic',{code:d.code,location:d.location,...d.details,asset:d.details?.references?.[0]});break;}
            case 'get_content':prepareContent(c);break;
            case 'cancel_content':cancelContent(c.request);break;
            case 'promote_content':promoteContent(c.request,c.session);break;
            case 'get_assets':prepare(c);break;
            case 'promote_assets':promotePreparation(c.request,c.session);break;
            case 'cancel_assets':cancelPreparation(c.request);break;
            case 'audio_start':if(c.session===state().session)playVoice(c);break;
            case 'audio_character':{
                const voice=voices.get(audioVoiceKey(c));
                if(voice){voice.character=c.character;voice.gain.gain.value=voice.eventGain*(preferences[`${voice.bus}_volume`]??.5)*(voice.bus==='voice'?characterVoiceGain(preferences,voice.character):1);}
                break;
            }
            case 'audio_envelope':{
                const v=voices.get(audioVoiceKey(c));if(!v)break;
                const now=v.context.currentTime,param=v.envelope.gain;
                param.cancelScheduledValues(now);param.setValueAtTime(c.from,now);
                const duration=Number(c.duration_us)/1e6;
                v.envelopePlan={owner:c.owner,base:Number(c.elapsed_us),at:now,duration:Number(c.duration_us)};
                if(duration>0)param.linearRampToValueAtTime(c.to,now+duration);
                else param.setValueAtTime(c.to,now);
                break;
            }
            case 'audio_stop':stopVoice(audioVoiceKey(c));break;
            case 'audio_reset':audioDomains.route(c.domain);for(const [id,v] of [...voices])if(v.domain===c.domain)stopVoice(id);break;
            case 'audio_pause':audioDomains.setPaused(c.domain,c.paused);break;
            case 'audio_bus_pause':audioDomains.setBusPaused(c.domain,c.bus,c.paused);break;
            case 'save':save(c);break;case 'load':load(c);break;case 'list_saves':listSaves();break;
            case 'apply_preferences':preferences=c.preferences;for(const v of voices.values())v.gain.gain.value=v.eventGain*(preferences[`${v.bus}_volume`]??.5)*(v.bus==='voice'?characterVoiceGain(preferences,v.character):1);break;
            case 'persist_preferences':preferences=c.preferences;for(const v of voices.values())v.gain.gain.value=v.eventGain*(preferences[`${v.bus}_volume`]??.5)*(v.bus==='voice'?characterVoiceGain(preferences,v.character):1);persistence.submit('preferences',preferences);break;
            case 'persist_profile':persistence.submit('profile',c.keys);break;
            case 'persist_profile_values':persistence.submit('profile_values',c.values);break;
            case 'export':{
                let url;
                try {
                    url=URL.createObjectURL(new Blob([c.json],{type:'application/json'}));
                    const a=document.createElement('a');a.href=url;a.download=`${release.game_id}.nir-save.json`;a.click();
                }catch(error){hostEvent('export_failed',{job:c.job,message:String(error)});break;}
                finally {if(url)setTimeout(()=>URL.revokeObjectURL(url),1000);}
                // Browser accepted the download; its external file write or
                // cancellation is not observable from the page.
                hostEvent('exported',{job:c.job});break;
            }
            case 'import':importSave();break;
            case 'trace':if(testMode){traces.push({event:c.event,at:c.at});if(traces.length>4096)traces.splice(0,traces.length-4096);}break;
            default:throw new Error(`E_HOST_PROTOCOL: ${c.type}`);
        }
    }}
    function state(){return cachedHostState||(cachedHostState=JSON.parse(engine.host_state()));}
    function debugState(){return {...JSON.parse(engine.state()),execution:{...threading}};}
    function finishPerformanceTurn(turn) {
        if(!turn)return;
        try {
            if(!disposed){
                const rows=JSON.parse(engine.take_profile());
                for(const row of rows)performanceStats.record(row.stage,Number(row.start_us),Number(row.end_us),turn);
            }
        } finally {performanceStats.endTurn(turn,Math.round(performance.now()*1000));}
    }
    function action(a,context=state()) {
        if(disposed||recovering)return Promise.resolve(false);
        if(audioBlocked&&audioBlockedAction(a)){unlock();return Promise.resolve(false);}
        // A delayed query keeps the reading eligibility of its original input.
        if(context.loading&&(['advance','continue','choose','cancel_choice'].includes(a.type)||a.type==='hold_skip'&&a.pressed))return Promise.resolve(false);
        observe('input_received',{sequence:sequence+1,session:context.session});
        unlock();if(a.type==='new_game'&&metrics.startInputMs===null)metrics.startInputMs=performance.now();
        sequence=Math.max(sequence+1,state().sequence+1);const seq=sequence;
        return deliver(()=>{sampleAudioPositions();if(a.type==='title'||a.type==='new_game')inbox.cancelGroup('load');mutateEngine(()=>engine.action(JSON.stringify(a),context.interaction,seq,context.session));},'input');
    }
    let semanticSignature='',announcement='',announcementLocale='';
    function semantics(view) {
        // Semantic rectangles use the same CSS pixels as the canvas controls,
        // including after rotation and at high device pixel ratios. Keep the
        // recovery card below compact controls in the first two toolbar rows;
        // constrain its height so long explanations remain scrollable.
        if(!audioRecoveryPanel.hidden){
            let top=16;
            for(const n of view.nodes)if(n.rect[1]<=112&&n.rect[3]<=64)
                top=Math.max(top,n.rect[1]+n.rect[3]+8);
            audioRecoveryPanel.style.top=`${top}px`;
            audioRecoveryPanel.style.maxHeight=`${Math.max(0,height-top-16)}px`;
        }
        historyButton.hidden=state().screen!=='Menu'||state().menu_depth>0;
        if(!historyButton.hidden){
            const size=historyButton.getBoundingClientRect();
            const position=overlayControlPosition(view.nodes,width,height,size.width,size.height);
            if(position){historyButton.style.right=`${width-position[0]-size.width}px`;historyButton.style.top=`${position[1]}px`;}
            else historyButton.hidden=true;
        }
        document.documentElement.lang=view.locale||'zh-Hans';const s=state();const signature=JSON.stringify([view.nodes,view.locale,view.announcement_locale,s.interaction,s.session]);
        if(signature!==semanticSignature){semanticSignature=signature;const nav=document.querySelector('#actions'),focused=focusIdentity(document.activeElement?.dataset?.action);nav.replaceChildren();for(const n of view.nodes){const b=document.createElement('button');b.textContent=n.label;b.setAttribute('aria-label',n.label);b.lang=n.locale||view.locale||'zh-Hans';b.disabled=!n.enabled;if(['range','scrollbar'].includes(n.value?.type)){b.setAttribute('role','slider');b.setAttribute('aria-valuemin',n.value.min??0);b.setAttribute('aria-valuemax',n.value.max);b.setAttribute('aria-valuenow',n.value.value);if(n.value.type==='scrollbar')b.setAttribute('aria-orientation','vertical');}else if(n.value?.type==='toggle'){b.setAttribute('role','switch');b.setAttribute('aria-checked',String(n.value.checked));}b.dataset.action=JSON.stringify(n.action);b.dataset.control=String(n.id);b.dataset.rect=JSON.stringify(n.rect);const context={interaction:s.interaction,session:s.session};b.onclick=()=>action(n.action,{...context,loading:state().loading});b.onfocus=()=>{deliver(()=>{mutateEngine(()=>engine.focus_control(n.id));mutateEngine(()=>engine.hover(n.rect[0]+n.rect[2]/2,n.rect[1]+n.rect[3]/2));},'input');const ring=document.querySelector('#focus-ring');Object.assign(ring.style,{display:'block',left:`${n.rect[0]+viewportOffset.left}px`,top:`${n.rect[1]+viewportOffset.top}px`,width:`${n.rect[2]}px`,height:`${n.rect[3]}px`});};b.onblur=()=>{document.querySelector('#focus-ring').style.display='none';deliver(()=>mutateEngine(()=>engine.focus_control(undefined)),'input');};nav.append(b);if(focusIdentity(b.dataset.action)===focused)b.focus({preventScroll:true});}}
        if(pendingFocusId!==null){const target=pendingFocusId;pendingFocusId=null;if(target.session===s.session&&target.interaction===s.interaction&&target.screen===s.screen)document.querySelector(`#actions button[data-control="${target.id}"]`)?.focus({preventScroll:true});}
        const spokenLocale=view.announcement_locale||view.locale||'zh-Hans';if(view.announcement&&(view.announcement!==announcement||spokenLocale!==announcementLocale)){announcement=view.announcement;announcementLocale=spokenLocale;const live=document.querySelector('#announcement');live.lang=spokenLocale;live.textContent=announcement;}
    }
    let frameRunning=false,frameAgain=false,lastAudioSample=0,lastSubmittedFrames=0,lastSubmittedDevice=null;
    const pendingDeliveries=[];
    async function frame(now){if(frameRunning){frameAgain=true;return;}frameRunning=true;try{await runFrame(now);}finally{frameRunning=false;for(const resolve of pendingDeliveries.splice(0))resolve(!disposed);if(frameAgain&&!disposed){frameAgain=false;wake();}}}
    async function runFrame(now) {
        if(disposed)return;
        const perfTurn=performanceStats?performanceStats.beginTurn(Math.round(now*1000)):null;
        if(perfTurn&&pendingWakeWaitStartUs!==null){performanceStats.record('wake_wait',pendingWakeWaitStartUs,pendingWakeWaitEndUs,perfTurn);pendingWakeWaitStartUs=pendingWakeWaitEndUs=null;}
        const eventStartUs=perfTurn?Math.round(performance.now()*1000):0;
        try {
            mutateEngine(()=>engine.begin_turn());
            checkDevice();if(disposed){finishPerformanceTurn(perfTurn);return;}
            const outputWasBlocked=audioBlocked;checkAudioOutput();
            const before=state();traceContext={session:before.session,device:before.device};
            const elapsed=lastTime===null?0:Math.max(0,Math.round((now-lastTime)*1000));lastTime=now;
            inbox.drain({canRun:kind=>!disposed&&(!recovering||kind==='control')&&(kind==='control'||engine.pending_events()<112)});await engine.sync?.();invalidateHostState();if(disposed){finishPerformanceTurn(perfTurn);return;}flush();checkAudioOutput();updateMetadataRecovery();await engine.sync?.();invalidateHostState();
            if(recovering){if(inbox.hasControl)wake();if(perfTurn)performanceStats.record('event_handling',eventStartUs,Math.round(performance.now()*1000),perfTurn);finishPerformanceTurn(perfTurn);return;}
            const after=state();
            // checkAudioOutput can release the barrier before `before` is
            // captured. Discard the preceding blocked interval even then;
            // foreground UI keeps its independent elapsed time.
            pendingElapsed.add(elapsed,{hidden:document.hidden,before,after,storyBlocked:outputWasBlocked||audioBlocked});
            if(!inbox.hasInput){const [story,foreground]=pendingElapsed.take();if(!engine.remote||now-lastAudioSample>=100){sampleAudioPositions();lastAudioSample=now;}mutateEngine(()=>engine.tick_domains(story,foreground));}
            mutateEngine(()=>engine.continue_turn());await engine.sync?.();invalidateHostState();flush();
            if(perfTurn)performanceStats.record('event_handling',eventStartUs,Math.round(performance.now()*1000),perfTurn);
            const current=size();if(current.width!==width||current.height!==height||current.dpr!==dpr){({width,height,dpr}=current);if(!engine.remote){canvas.width=Math.round(width*dpr);canvas.height=Math.round(height*dpr);}}
            let view=JSON.parse(await mutateEngine(()=>engine.draw(width,height,dpr)));await engine.sync?.();invalidateHostState();flush();prune();checkAudioOutput();await engine.sync?.();invalidateHostState();
            // Remote draw queues resize and initially returns the previous
            // snapshot. Publish rectangles from the completed owner turn,
            // matching the canvas and its pointer hit tests after rotation.
            if(engine.remote)view=JSON.parse(engine.snapshot.view);
            const semanticStartUs=perfTurn?Math.round(performance.now()*1000):0;semantics(view);
            if(perfTurn)performanceStats.record('semantics',semanticStartUs,Math.round(performance.now()*1000),perfTurn);
            const s=state();
            if(s.frames>0&&(s.frames!==lastSubmittedFrames||s.device!==lastSubmittedDevice)){observe('render_submitted',{session:s.session,device:s.device,frames:s.frames});lastSubmittedFrames=s.frames;lastSubmittedDevice=s.device;}
            metrics.frames=s.frames;metrics.peakResidentBytes=Math.max(metrics.peakResidentBytes,s.resident_bytes);syncContentStagingMetrics(contentStaging);
            metrics.maxTurnUploadBytes=Math.max(metrics.maxTurnUploadBytes||0,s.turn_upload_bytes);metrics.uploadSteps=s.upload_steps;
            metrics.activeRequests=inbox.slots.size;metrics.requestHighWater=inbox.reservedHighWater;metrics.acceptedRequests=inbox.accepted;metrics.completedRequests=inbox.completed;metrics.cancelledRequests=inbox.cancelled;
            metrics.inboxHighWater=inbox.highWater;metrics.maxTurnWork=Math.max(metrics.maxTurnWork||0,s.turn_work);
            if(view.ready){shell.hidden=true;if(metrics.titleMs===null)metrics.titleMs=performance.now()-metrics.boot;if(s.has_dialogue&&metrics.firstLineMs===null){metrics.firstLineMs=performance.now()-metrics.boot;metrics.firstLineAfterStartMs=performance.now()-metrics.startInputMs;metrics.navigationToFirstLineMs=performance.now();observe('first_line_submitted');}}
            if(inbox.length||(!engine.remote&&engine.pending_events()))wake();
            else if(!engine.remote&&engine.needs_clock()&&!document.hidden)schedule();
            finishPerformanceTurn(perfTurn);
        }catch(e){
            try{finishPerformanceTurn(perfTurn);}catch{}
            if(!disposed){fail(e);dispose();}
        }
    }
    function schedule() {if(disposed||recovering)return;if(!raf)raf=requestAnimationFrame(()=>{raf=0;wake();});}
    const setInputMode=touch=>{if(touchInput===touch)return;touchInput=touch;mutateEngine(()=>engine.set_touch_input(touch));schedule();};
    let down=null,barPointer=null;
    // The host retains capture until release even if a page change cancels the
    // shared gesture, so that release cannot become a click in the new page.
    const barGesture=async(phase,x,y)=>{try{const consumed=await mutateEngine(()=>engine.pointer_gesture(phase,x,y,0));schedule();return consumed;}catch(error){if(!disposed)reportHostFailure(error,'gesture');return false;}};
    const cancelPointer=()=>{
        down=null;pointerQueue.clear();
        if(barPointer===null)barGesture(3,0,0);
        if(barPointer!==null){const id=barPointer;barPointer=null;barGesture(3,0,0);if(canvas.hasPointerCapture(id))canvas.releasePointerCapture(id);}
    };
    const scrollAction=(view,delta,page=false)=>view.menu
        ? {type:'menu_history_scroll',...view.menu,input:{type:page?'page':'step',delta}}
        : {type:'scroll',region:view.region,delta};
    const scrollAt=(x,y)=>state().scrolls.find(v=>x>=v.rect[0]&&x<=v.rect[0]+v.rect[2]&&y>=v.rect[1]&&y<=v.rect[1]+v.rect[3]);
    const downAsync=async(e,valid)=>{if(e.button!==0&&e.button!==2)return;pendingFocusId=null;if(document.activeElement?.closest('#actions'))document.activeElement.blur();mutateEngine(()=>engine.focus_control(undefined));unlock();if(e.button===0&&barPointer===null&&await barGesture(0,e.clientX,e.clientY)){if(!valid())return;barPointer=e.pointerId;try{canvas.setPointerCapture(e.pointerId);}catch{}down=null;return;}if(!valid())return;const context=e.context,target=JSON.parse(await engine.pointer_action(e.clientX,e.clientY,e.button));if(!valid())return;down={button:e.button,action:target,context,x:e.clientX,y:e.clientY,scroll:e.button===0?scrollAt(e.clientX,e.clientY):null};};
    const upAsync=async(e,valid)=>{
        if(e.button===0&&barPointer!==e.pointerId)await barGesture(2,e.clientX,e.clientY);
        if(barPointer===e.pointerId&&e.button===0){barGesture(2,e.clientX,e.clientY);const id=barPointer;barPointer=null;if(canvas.hasPointerCapture(id))canvas.releasePointerCapture(id);return;}
        if(!down||e.button!==down.button)return;
        const current=state();
        if(current.session!==down.context.session||current.interaction!==down.context.interaction||current.screen!==down.context.screen){down=null;return;}
        const dy=e.clientY-down.y;
        if(down.scroll&&Math.abs(dy)>30&&Math.abs(e.clientX-down.x)<80){action(scrollAction(down.scroll,dy<0?1:-1),down.context);}
        else if((down.action?.type==='menu_value'&&typeof down.action.value==='number')||Math.hypot(e.clientX-down.x,dy)<20){const hit=JSON.parse(await engine.pointer_action(e.clientX,e.clientY,e.button));if(valid()&&down&&samePointerTarget(down.action,hit))action(hit,{...down.context,loading:down.context.loading||e.context.loading});}
        down=null;
    };
    let hoverTarget;
    const moveAsync=async(e,valid)=>{if(e.pointerType==='mouse')setInputMode(false);if(barPointer===e.pointerId){barGesture(1,e.clientX,e.clientY);return;}const action=await engine.hit(e.clientX,e.clientY);if(!valid())return;const s=state(),target=JSON.stringify([s.session,s.interaction,s.screen,action]);if(target===hoverTarget&&!state().history_scrollbar)return;hoverTarget=target;deliver(()=>mutateEngine(()=>engine.hover(e.clientX,e.clientY)),'input');};
    const pointerQueue=new SerialInputQueue(error=>reportHostFailure(error,'pointer'));
    const pointerEvent=(fn,e)=>{const s=state(),context={session:s.session,interaction:s.interaction,screen:s.screen,loading:s.loading};const data={button:e.button,pointerId:e.pointerId,clientX:e.clientX-viewportOffset.left,clientY:e.clientY-viewportOffset.top,context,pointerType:e.pointerType};pointerQueue.push(fn,data,fn===moveAsync?`move:${e.pointerId}`:null);};
    const onDown=e=>{const blocked=audioBlocked;unlock();if(blocked)cancelPointer();pointerEvent(async(data,valid)=>{
        setInputMode(data.pointerType==='touch');
        await downAsync(data,valid);
        if(blocked&&down&&audioBlockedAction(down.action))down=null;
    },e);};
    const onUp=e=>{unlock();pointerEvent(upAsync,e);};
    const onMove=e=>pointerEvent(moveAsync,e);
    const onLeave=()=>pointerQueue.push(()=>{if(barPointer!==null)return;down=null;hoverTarget=undefined;barGesture(3,0,0);deliver(()=>mutateEngine(()=>engine.hover(-1,-1)),'input');},null);
    const onWheel=(e)=>{const view=scrollAt(e.clientX-viewportOffset.left,e.clientY-viewportOffset.top);if(view&&e.deltaY){e.preventDefault();action(scrollAction(view,e.deltaY>0?1:-1));}};
    const heldControls=new Set();
    const releaseHeld=()=>{if(!heldControls.size)return;heldControls.clear();action({type:'hold_skip',pressed:false});};
    const onEditingFocus=e=>{if(e.target?.isContentEditable||e.target?.matches?.('input,textarea,select'))releaseHeld();};
    const onKeyUp=e=>{if(e.key==='Control'){if(!heldControls.delete(e.code))return;if(!heldControls.size)action({type:'hold_skip',pressed:false});}};
    const handleKey=async(e)=>{
        setInputMode(false);
        if(audioBlocked&&(e.key===' '||e.key==='Enter')){
            if(e.repeat){e.preventDefault();return;}
            if(audioRecoveryPanel.contains(document.activeElement))return;
            const focused=document.activeElement;
            const focusedAction=focused?.tagName==='BUTTON'&&!focused.disabled&&focused.closest('#actions')
                ?JSON.parse(focused.dataset.action):null;
            if(audioRecoveryConsumesKey(audioBlocked,e.key,focusedAction)){e.preventDefault();unlock();return;}
        }
        if(!historyPanel.hidden){if(e.key==='Escape'){e.preventDefault();historyClose.click();}return;}
        if(e.isComposing||document.activeElement?.isContentEditable||document.activeElement?.matches('input,textarea,select'))return;
        if(e.key==='Control'&&!e.repeat&&!e.isComposing&&!e.metaKey&&!e.altKey){
            if(document.activeElement?.isContentEditable||document.activeElement?.matches('input,textarea,select'))return;
            heldControls.add(e.code);action({type:'hold_skip',pressed:true});return;
        }
        // Repeated reader keys must also suppress a focused button's native
        // activation; ignoring the event still lets Enter click it again.
        if(e.repeat&&(e.key===' '||e.key==='Enter')&&document.activeElement?.closest('#actions')){e.preventDefault();return;}
        if(e.isComposing||e.repeat||e.ctrlKey||e.metaKey||e.altKey)return;
        if(e.key==='PageUp'||e.key==='PageDown'){
            const s=state(),view=s.scrolls.find(v=>v.region==='choices')||s.scrolls[0];
            if(view){e.preventDefault();action(scrollAction(view,e.key==='PageDown'?1:-1,true),s);}return;
        }
        if(e.key==='Escape'){e.preventDefault();const s=state();action({type:s.choice_cancellable?'cancel_choice':s.menu_depth>0||['Menu','Settings','Saves','History'].includes(s.screen)?'close':'menu'});return;}
        // Let browser chrome and host-owned controls remain reachable at the
        // boundary; only movement within the player uses its semantic order.
        if(e.key==='Tab'&&document.activeElement?.closest('#actions')) {
            const buttons=[...document.querySelectorAll('#actions button:not(:disabled)')];
            const rect=JSON.parse(document.activeElement.dataset.rect),x=rect[0]+rect[2]/2,y=rect[1]+rect[3]/2;
            const canFollow=state().scrolls.some(v=>x>=v.rect[0]&&x<=v.rect[0]+v.rect[2]&&y>=v.rect[1]&&y<=v.rect[1]+v.rect[3]&&(e.shiftKey?v.offset>0:v.offset<v.max));
            if(!canFollow&&document.activeElement===(e.shiftKey?buttons[0]:buttons.at(-1)))return;
        }
        const valueDirection={ArrowLeft:0,ArrowDown:5,ArrowRight:1,ArrowUp:4,Home:2,End:3}[e.key];
        if(valueDirection!==undefined&&document.activeElement?.closest('#actions')){
            const focused=document.activeElement;
            e.preventDefault();const a=JSON.parse(await engine.control_value_action(Number(focused.dataset.control),focused.dataset.action,valueDirection));
            if(a){e.preventDefault();action(a,state());return;}
        }
        const direction={Tab:e.shiftKey?0:1,ArrowLeft:2,ArrowRight:3,ArrowUp:4,ArrowDown:5}[e.key];
        if(direction!==undefined && (!document.activeElement?.matches('button')||document.activeElement.closest('#actions'))) {
            e.preventDefault();const id=await mutateEngine(()=>engine.navigate_focus(direction));
            if(id!==undefined){e.preventDefault();const s=state();pendingFocusId={id,session:s.session,interaction:s.interaction,screen:s.screen};schedule();}return;
        }
        if(document.activeElement?.tagName==='BUTTON')return;
        if(e.key.toLowerCase()==='h'){e.preventDefault();action({type:'toggle_interface'});return;}
        if(e.key===' '||e.key==='Enter'){e.preventDefault();unlock();const s=state(),a=JSON.parse(await engine.primary_action());if(a)action(a,s);}

    };
    const onKey=e=>{void handleKey(e).catch(error=>reportHostFailure(error,'keyboard'));};
    const onVisibility=()=>{const hidden=document.hidden;if(hidden){releaseHeld();cancelPointer();}deliver(()=>mutateEngine(()=>engine.hidden(hidden)),'control');};
    const onResize=()=>{cancelPointer();refreshSafeArea();hoverTarget=undefined;schedule();};
    let glContextLost=false;
    const onCancel=()=>cancelPointer();
    // Touch releases implicit capture immediately on pointerup. Process that
    // notification after the queued up; it must not cancel a pending tap.
    const onLostCapture=e=>{const id=e.pointerId;pointerQueue.push(()=>{if(barPointer===id)cancelPointer();},null);};
    const onBlur=()=>{releaseHeld();cancelPointer();};
    const onGlLost=e=>{e.preventDefault();glContextLost=true;cancelPointer();deliver(checkDevice,'control');};
    const onContextMenu=e=>e.preventDefault();
    function bindCanvas(){canvas.addEventListener('contextmenu',onContextMenu);canvas.addEventListener('pointermove',onMove);canvas.addEventListener('pointerleave',onLeave);canvas.addEventListener('wheel',onWheel,{passive:false});canvas.addEventListener('pointerdown',onDown);canvas.addEventListener('pointerup',onUp);canvas.addEventListener('pointercancel',onCancel);canvas.addEventListener('lostpointercapture',onLostCapture);canvas.addEventListener('webglcontextlost',onGlLost);}
    function unbindCanvas(){canvas.removeEventListener('contextmenu',onContextMenu);canvas.removeEventListener('pointermove',onMove);canvas.removeEventListener('pointerleave',onLeave);canvas.removeEventListener('wheel',onWheel);canvas.removeEventListener('pointerdown',onDown);canvas.removeEventListener('pointerup',onUp);canvas.removeEventListener('pointercancel',onCancel);canvas.removeEventListener('lostpointercapture',onLostCapture);canvas.removeEventListener('webglcontextlost',onGlLost);}
    bindCanvas();window.addEventListener('keydown',onKey);window.addEventListener('keyup',onKeyUp);window.addEventListener('blur',onBlur);document.addEventListener('focusin',onEditingFocus);document.addEventListener('visibilitychange',onVisibility);window.addEventListener('resize',onResize);window.visualViewport?.addEventListener('resize',onResize);
    function checkDevice(){
        if(disposed||recovering)return;
        const validation=engine.gpu_error();if(validation&&!glContextLost){observe('diagnostic',{domain:'render',code:'E_GPU_VALIDATION',operation:'render'});fail(`E_GPU_VALIDATION: ${validation}`);dispose();return;}
        if(!glContextLost&&!engine.device_lost())return;
        cancelPointer();recovering=true;observe('device_loss_detected');metrics.deviceRecoveries++;mutateEngine(()=>engine.begin_recovery());
        const recovery=inbox.reserve('control','device');
        if(!recovery){fail('E_REQUEST_CAPACITY: device recovery');dispose();return;}
        const backend=activeBackend;
        if(backend==='webgl2'){unbindCanvas();replaceCanvas();bindCanvas();glContextLost=false;}
        wasm.create_gpu('stage',backend).then(gpu=>{
            if(disposed||recovery.state==='cancelled'){gpu.free();return;}
            post(recovery,()=>{mutateEngine(()=>engine.replace_gpu(gpu));recovering=false;lastTime=performance.now();});
        },e=>post(recovery,()=>{fail(`E_DEVICE_RECOVERY: ${e}`);dispose();}));
    }
    const poll=setInterval(()=>{if(!disposed&&!recovering&&(glContextLost||engine.device_lost()||engine.gpu_error()))deliver(checkDevice,'control');},500);
    const traces=[];
    const disabledPerformance={enabled:false,turn_capacity:64,total_turns:0,dropped_turns:0,stage_semantics:'inclusive_non_additive',stages:{},turns:[]};
    const diagnostics=()=>{
        const performance=performanceStats?performanceStats.snapshot():{...disabledPerformance};
        if(performanceStats&&!disposed)performance.text_cache=JSON.parse(engine.text_cache_stats());
        return {format:1,worker_resource_timing:workerTimingsSnapshot(),execution:{...threading,actual_adapters:(engine.snapshot?.adapters||[]).map(row=>({...row,atMs:row.atMs+(runtimeClient?.clockOffsetUs||0)/1000})),runtime_pending:runtimeClient?.pending.size||0,runtime_high_water:runtimeClient?.highWater||0,asset_pending:assetClient?.pending.size||0,asset_high_water:assetClient?.highWater||0},release:releaseDigest,engine:release.engine.wasm,backend:activeBackend,fallback_reason:fallbackReason,...trace.snapshot(),performance,resource_memory:{media:mediaMemory(),renderer:state().renderer_memory},content_staging:{encoded_bytes:contentStaging.used,peak_encoded_bytes:contentStaging.peak,budget_encoded_bytes:contentStaging.limit,reservations:contentStaging.reservations.size,waiting_demands:contentStaging.waiting.length},host_work:{story_clock:state().story_clock,resource_pool_active:resourcePool.active,resource_pool_waiting:resourcePool.waiting.length,decode_pool_active:decodePool.active,decode_pool_waiting:decodePool.waiting.length,upload_pool_active:uploadPool.active,upload_pool_waiting:uploadPool.waiting.length,shared_fetches:requests.jobs.size,content_jobs:contentPreparations.size,media_jobs:preparations.size,request_slots:inbox.slots.size,pending_owner_callbacks:inbox.length,audio_state:audio.state,audio_paused:audioDomains.paused('story'),audio_domains:audioDomains.snapshot(),pending_media:[...preparations].slice(0,128).map(([request,job])=>({request,session:job.session,priority:job.priority,aborted:job.signal.aborted,stages:[...job.nodes.values()].slice(0,128).map(node=>({asset:node.asset,stage:node.stage}))})),pending_content:[...contentPreparations.values()].slice(0,128).map(job=>({request:job.request,session:job.session,priority:job.priority,state:job.state,staged:job.staged,aborted:job.signal.aborted}))},measurement:{clock:'performance.now; navigation origin',stage_timing:'inclusive, non-additive intervals',gpu_time:'unmeasured',physical_memory:'unmeasured'}};
    };
    if(trace.enabled)window.nirDiagnostics={snapshot:diagnostics,download(){const url=URL.createObjectURL(new Blob([JSON.stringify(diagnostics(),null,2)],{type:'application/json'}));const a=document.createElement('a');a.href=url;a.download='nir-diagnostics.json';a.click();setTimeout(()=>URL.revokeObjectURL(url),1000);}};
    if(release.profile==='dev')window.__nirDev={state:debugState,action};
    if(testMode)window.__nir={state:debugState,action,metrics,traces,diagnostics,inspectSave:inspectSlot,resourceTimings:()=>[...performance.getEntriesByType('resource').map(e=>e.toJSON()),...workerTimingsSnapshot().rows.map(row=>({...row,name:new URL(release.objects[row.object].path,releaseRoot).href}))],actualAdapters:()=>[...(window.__nirActualAdapters||[]),...(diagnostics().execution.actual_adapters||[])],needsClock:()=>engine.needs_clock(),rawAction:(a,token,seq,epoch)=>deliver(()=>mutateEngine(()=>engine.action(JSON.stringify(a),token,seq,epoch)),'input'),loseDevice:()=>engine.simulate_device_loss(),hidden:(v)=>deliver(()=>mutateEngine(()=>engine.hidden(v)))};
    const onPageHide=e=>{if(e.persisted){cancelPointer();deliver(()=>mutateEngine(()=>engine.hidden(true)));}else{dispose();}};
    const onPageShow=e=>{if(e.persisted){deliver(()=>mutateEngine(()=>engine.hidden(false)));}};
    function dispose(){if(disposed)return;cancelPointer();pointerQueue.close();disposed=true;metadataRetryController?.abort();metadataPanel.remove();historyGeneration++;historyReadController?.abort();audioRecoveryPanel.remove();historyPanel.remove();historyButton.remove();devBanner?.remove();clearTimeout(ownerTimer);persistence.close();inbox.clear();for(const request of [...contentPreparations.keys()])cancelContent(request);for(const request of [...preparations.keys()])cancelPreparation(request);cancelAnimationFrame(raf);clearInterval(poll);for(const id of [...voices.keys()])stopVoice(id);audioDomains.close();assetClient?.close();storage.close();unbindCanvas();window.removeEventListener('keydown',onKey);window.removeEventListener('keyup',onKeyUp);window.removeEventListener('blur',onBlur);document.removeEventListener('focusin',onEditingFocus);window.removeEventListener('resize',onResize);window.visualViewport?.removeEventListener('resize',onResize);document.removeEventListener('visibilitychange',onVisibility);window.removeEventListener('pagehide',onPageHide);window.removeEventListener('pageshow',onPageShow);engine.free();}
    window.addEventListener('pagehide',onPageHide);
    window.addEventListener('pageshow',onPageShow);
}
