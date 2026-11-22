import { UTF8_ENCODER } from './preferences.js';

export const SAVE_DATABASE='nir-player-isolated-v1';
export const saveKey=(gameId,profile,digest,slot)=>[gameId,profile,digest,slot];
export const profileKey=(gameId,profile)=>[gameId,profile];

// One in-flight write per kind, with one latest preference snapshot and the
// union of uncommitted progress. Failures wait for a new change, not a timer.
// Keep completion delivery on the owner through the supplied request function.
export class PersistenceWrites {
    constructor({request,writePreferences,mergeProfile,writeProfileValues,stored,failed}) {
        Object.assign(this,{request,writePreferences,mergeProfile,writeProfileValues,stored,failed});
        this.lanes=new Map(['preferences','profile','profile_values'].map(kind=>[kind,{active:false,dirty:false,value:null,keys:new Set(),patch:Object.create(null)}]));
        this.closed=false;
    }
    submit(kind,value) {
        if(this.closed)return;
        const lane=this.lanes.get(kind);if(!lane)throw new Error('E_PERSISTENCE_KIND');
        if(kind==='profile')for(const key of value)lane.keys.add(key);
        else if(kind==='profile_values')Object.assign(lane.patch,value);
        else lane.value=value;
        lane.dirty=true;this.pump(kind,lane);
    }
    pump(kind,lane) {
        if(this.closed||lane.active||!lane.dirty)return;
        lane.active=true;lane.dirty=false;
        const value=kind==='profile'?[...lane.keys]:kind==='profile_values'?{...lane.patch}:lane.value;
        let settled=false;
        const finish=(ok,error)=>{
            if(settled)return;settled=true;lane.active=false;
            if(this.closed)return;
            if(ok){
                if(kind==='profile')for(const key of value)lane.keys.delete(key);
                else if(kind==='profile_values'){
                    for(const key of Object.keys(value))if(lane.patch[key]===value[key])delete lane.patch[key];
                }
                else if(!lane.dirty)lane.value=null;
            }
            try {
                if(!ok)this.failed(kind,error);
                else if(!lane.dirty)this.stored(kind);
            } finally {this.pump(kind,lane);}
        };
        try {
            this.request(pending=>kind==='profile'?this.mergeProfile(value,pending):kind==='profile_values'?this.writeProfileValues(value,pending):this.writePreferences(value,pending),
                ()=>finish(true),error=>finish(false,error),{pending:error=>{
                    // A warning is not a terminal result. Keep this lane and
                    // its newest edits until the original transaction settles.
                    if(!this.closed&&!settled)this.failed(kind,error);
                }});
        }catch(error){finish(false,error);}
    }
    close() {
        this.closed=true;
        for(const lane of this.lanes.values()){lane.value=null;lane.keys.clear();lane.patch=Object.create(null);lane.dirty=false;}
    }
    retry(kind) {
        if(this.closed)return false;
        const lane=this.lanes.get(kind);if(!lane)throw new Error('E_PERSISTENCE_KIND');
        if(lane.active||(kind==='profile'?!lane.keys.size:kind==='profile_values'?!Object.keys(lane.patch).length:lane.value===null))return false;
        lane.dirty=true;this.pump(kind,lane);return true;
    }
}

// Missing records are a first visit. Incompatible originals are never reset
// by fallback negotiation or a later preference/progress write.
export function validateMetadataRecord(kind,value) {
    const invalid=()=>{throw new Error(kind==='profile'?'E_PROFILE_RECORD: unreadable progress':'E_PREFERENCES_RECORD: unreadable preferences');};
    if(value===undefined)return kind==='profile'?[]:kind==='profile_values'?{}:null;
    if(kind==='profile_values'){
        const bad=()=>{throw new Error('E_PROFILE_VALUES_RECORD: unreadable progress values');};
        if(!value||typeof value!=='object'||Array.isArray(value)||![Object.prototype,null].includes(Object.getPrototypeOf(value))||Object.keys(value).length>4096)bad();
        for(const [key,item] of Object.entries(value)){
            if(!key||UTF8_ENCODER.encode(key).length>1024||!item||typeof item!=='object'||Array.isArray(item)||Object.keys(item).length!==2||!Object.hasOwn(item,'type')||!Object.hasOwn(item,'value'))bad();
            if(item.type==='i32'){if(!Number.isInteger(item.value)||item.value < -2147483648||item.value > 2147483647)bad();}
            else if(item.type==='bool'){if(typeof item.value!=='boolean')bad();}
            else if(item.type==='string'){if(typeof item.value!=='string'||UTF8_ENCODER.encode(item.value).length>65536)bad();}
            else if(item.type==='f80'){if(typeof item.value!=='string'||! /^[0-9a-f]{20}$/.test(item.value))bad();const bits=BigInt('0x'+item.value),exp=Number((bits>>64n)&32767n),integer=Number((bits>>63n)&1n);if(exp===32767||(integer!==0)!==(exp!==0))bad();}
            else bad();
        }
        return value;
    }
    if(kind==='profile'){
        if(!Array.isArray(value)||value.length>100000)invalid();
        for(const key of value)if(typeof key!=='string'||UTF8_ENCODER.encode(key).length>1024)invalid(); // Includes sparse holes.
        return value;
    }
    if(kind!=='preferences')throw new Error('E_PERSISTENCE_KIND');
    if(!value||typeof value!=='object'||Array.isArray(value)||![Object.prototype,null].includes(Object.getPrototypeOf(value)))invalid();
    const numeric=['font_scale','text_speed','auto_wait_scale','bgm_volume','voice_volume','sfx_volume'];
    const boolean=['reduced_motion','auto_wait_voice','voice_continue'],locale=['locale','ui_locale','text_locale'];
    const known=new Set([...numeric,...boolean,...locale,'character_voices']);
    for(const key of Object.keys(value))if(!known.has(key))invalid();
    for(const key of numeric)if(Object.hasOwn(value,key)&&(typeof value[key]!=='number'||!Number.isFinite(value[key])))invalid();
    for(const key of boolean)if(Object.hasOwn(value,key)&&typeof value[key]!=='boolean')invalid();
    for(const key of locale)if(Object.hasOwn(value,key)&&typeof value[key]!=='string')invalid();
    if(Object.hasOwn(value,'character_voices')){
        const voices=value.character_voices;
        if(!voices||typeof voices!=='object'||Array.isArray(voices)||![Object.prototype,null].includes(Object.getPrototypeOf(voices))||Object.keys(voices).length>128)invalid();
        for(const [id,voice] of Object.entries(voices)){
            if(!id||UTF8_ENCODER.encode(id).length>256||!voice||typeof voice!=='object'||Array.isArray(voice)||
                Object.keys(voice).some(key=>!['volume','muted'].includes(key))||
                typeof voice.volume!=='number'||!Number.isFinite(voice.volume)||typeof voice.muted!=='boolean')invalid();
        }
    }
    return value;
}
export function readMetadataRecord(db,kind,key,{timeoutMs=3000,signal}={}) {
    return new Promise((resolve,reject)=>{
        if(!Number.isSafeInteger(timeoutMs)||timeoutMs<=0||timeoutMs>0x7fffffff){reject(new RangeError('Invalid metadata read timeout'));return;}
        if(signal?.aborted){reject(signal.reason||new Error('E_STORAGE_ABORT: metadata read cancelled'));return;}
        let tx,r,value,received=false,settled=false;
        const finish=error=>{
            if(settled)return;
            settled=true;clearTimeout(timer);signal?.removeEventListener('abort',abort);
            if(r)r.onsuccess=null;
            if(tx){tx.oncomplete=tx.onabort=tx.onerror=null;}
            // Read results become visible only after transaction completion.
            // Abort stalled reads; never reset records or replay a write.
            if(error){try{tx?.abort();}catch{}reject(error);}else resolve(value);
        };
        const timer=setTimeout(()=>finish(new Error(`E_STORAGE_TIMEOUT: ${kind} read did not complete`)),timeoutMs);
        const abort=()=>finish(signal.reason||new Error('E_STORAGE_ABORT: metadata read cancelled'));
        signal?.addEventListener('abort',abort,{once:true});
        try{
            const storeKind=kind==='profile_values'?'profile':kind;
            tx=db.transaction(storeKind,'readonly');r=tx.objectStore(storeKind).get(kind==='profile_values'?[...key,'values']:key);
            r.onsuccess=()=>{
                if(settled)return;
                try{value=validateMetadataRecord(kind,r.result);received=true;}catch(error){finish(error);}
            };
            tx.oncomplete=()=>finish(received?null:new Error('E_STORAGE_READ: missing metadata result'));
            tx.onabort=tx.onerror=()=>finish(tx.error||r.error||new Error('E_STORAGE_ABORT'));
        }catch(error){finish(error);}
    });
}
export async function readStartupMetadata(db,key,options) {
    const failures=[];
    const [preferences,profile]=await Promise.all(['preferences','profile'].map(async kind=>{
        try{return await readMetadataRecord(db,kind,key,options);}
        catch(error){failures.push({kind,message:String(error)});return kind==='preferences'?null:[];}
    }));
    return {preferences,profile,failures};
}
export function writeMetadataRecord(db,kind,key,value,{timeoutMs=3000,signal,onPending}={}) {
    return new Promise((resolve,reject)=>{
        if(!Number.isSafeInteger(timeoutMs)||timeoutMs<=0||timeoutMs>0x7fffffff){reject(new RangeError('Invalid metadata write timeout'));return;}
        if(signal?.aborted){reject(signal.reason||new Error('E_STORAGE_ABORT: metadata write cancelled'));return;}
        try{validateMetadataRecord(kind,value);}catch(error){reject(error);return;}
        let tx,r,put,settled=false,written=false,putSucceeded=false,blocked=false,failure=null;
        const cleanup=()=>{
            clearTimeout(timer);signal?.removeEventListener('abort',abort);
            if(r)r.onsuccess=null;
            if(put)put.onsuccess=null;
            if(tx)tx.oncomplete=tx.onabort=tx.onerror=null;
        };
        const finish=error=>{
            if(settled)return;
            settled=true;cleanup();if(error)reject(error);else resolve();
        };
        const stop=error=>{
            if(settled||blocked)return;
            blocked=true;failure=error;clearTimeout(timer);signal?.removeEventListener('abort',abort);
            try{
                tx?.abort();
                // A successful abort request marks the transaction aborted
                // synchronously; late request callbacks cannot queue a put.
                finish(error);
            }catch{
                if(!written){finish(error);return;}
                // abort() can fail once commit has started. Neither a timer
                // nor an exception proves its outcome. Retain native terminal
                // handlers and the original owner slot; never replay it.
                const pending=Object.assign(new Error(`E_STORAGE_UNCERTAIN: ${kind} write is awaiting confirmation`),{code:'E_STORAGE_UNCERTAIN'});
                try{onPending?.(pending);}catch(noticeError){console.error('E_STORAGE_NOTICE',noticeError);}
            }
        };
        const timer=setTimeout(()=>stop(new Error(`E_STORAGE_TIMEOUT: ${kind} write did not complete`)),timeoutMs);
        const abort=()=>stop(signal.reason||new Error('E_STORAGE_ABORT: metadata write cancelled'));
        signal?.addEventListener('abort',abort,{once:true});
        try{
            const storeKind=kind==='profile_values'?'profile':kind,recordKey=kind==='profile_values'?[...key,'values']:key;
            tx=db.transaction(storeKind,'readwrite');const store=tx.objectStore(storeKind);r=store.get(recordKey);
            r.onsuccess=()=>{
                if(settled||blocked)return;
                try{
                    const prior=validateMetadataRecord(kind,r.result);
                    const next=kind==='profile'?[...new Set([...prior,...value])].sort():kind==='profile_values'?{...prior,...value}:value;
                    validateMetadataRecord(kind,next);
                    put=store.put(next,recordKey);written=true;
                    put.onsuccess=()=>{if(!settled)putSucceeded=true;};
                }catch(error){stop(error);}
            };
            tx.oncomplete=()=>finish(putSucceeded?null:failure||put?.error||new Error('E_STORAGE_WRITE: metadata was not written'));
            // Error events may still be cancelled by another listener. Only
            // native abort/completion decides the transaction's final result.
            tx.onerror=()=>{failure=tx.error||put?.error||r.error||new Error('E_STORAGE_WRITE');};
            tx.onabort=()=>finish(failure||tx.error||put?.error||r.error||new Error('E_STORAGE_ABORT'));
        }catch(error){stop(error);}
    });
}
export function writePreferencesRecord(db,key,value,options) {
    return writeMetadataRecord(db,'preferences',key,value,options);
}
export function mergeProfileRecord(db,key,keys,options) {
    return writeMetadataRecord(db,'profile',key,keys,options);
}

// Progress notices reuse a reserved owner slot. If native completion arrives
// before its warning is delivered, serialize both so the terminal is not lost.
export async function dispatchOwnerRequest(work,slot,{post,success,failure,pending}) {
    let notices=Promise.resolve(),accepting=true,value,error,ok=false;
    const notify=notice=>{
        if(!accepting||!pending||slot.state==='cancelled')return Promise.resolve(false);
        notices=notices.then(()=>post(slot,()=>pending(notice),false));return notices;
    };
    try{
        await Promise.resolve();
        if(slot.state==='cancelled')throw new Error('E_REQUEST_CANCELLED');
        value=await work(notify);ok=true;
    }catch(e){error=e;}
    accepting=false;await notices;
    await post(slot,()=>ok?success(value):failure(error));
}
const releaseDigestPattern=/^[0-9a-f]{64}$/;

export async function openSaveDatabase(indexedDBFactory=indexedDB,{timeoutMs=3000,signal,waitOnBlocked=false}={}) {
    if(!Number.isSafeInteger(timeoutMs)||timeoutMs<=0||timeoutMs>0x7fffffff)throw new RangeError('Invalid storage open timeout');
    if(signal?.aborted)throw signal.reason||new Error('E_STORAGE_ABORT: database open cancelled');
    const db=await new Promise((resolve,reject)=>{
        let settled=false,r;
        const finish=(error,database)=>{
            if(settled){database?.close();return;}
            settled=true;clearTimeout(timer);signal?.removeEventListener('abort',abort);
            if(error){try{r?.transaction?.abort();}catch{}reject(error);}else resolve(database);
        };
        const timer=setTimeout(()=>finish(new Error('E_STORAGE_TIMEOUT: database did not open')),timeoutMs);
        const abort=()=>finish(signal.reason||new Error('E_STORAGE_ABORT: database open cancelled'));
        signal?.addEventListener('abort',abort,{once:true});
        try{r=indexedDBFactory.open(SAVE_DATABASE,1);}catch(error){finish(error);return;}
        r.onblocked=()=>{if(!waitOnBlocked)finish(new Error('E_STORAGE_BLOCKED: database is in use'));};
        r.onupgradeneeded=()=>{
            // A timed-out or disposed caller must not change a database when
            // its previously blocked request eventually resumes.
            if(settled){try{r.transaction?.abort();}catch{}return;}
            try{
                const database=r.result;
                if(!database.objectStoreNames.contains('saves'))database.createObjectStore('saves');
                for(const name of ['preferences','profile'])if(!database.objectStoreNames.contains(name))database.createObjectStore(name);
            }catch(error){finish(error);}
        };
        r.onsuccess=()=>finish(null,r.result);r.onerror=()=>finish(r.error||new Error('E_STORAGE_OPEN: database unavailable'));
    });
    db.onversionchange=()=>db.close();
    return db;
}

// Reconnect only for a new storage operation. Never replay a save or replace
// persisted data after an uncertain write result. Concurrent operations share
// the open attempt; errors remain explicit results for their original owners.
export class SaveDatabaseConnection {
    constructor(openDatabase=signal=>openSaveDatabase(indexedDB,{signal})) {
        this.openDatabase=openDatabase;this.db=null;this.pending=null;this.openController=null;this.closed=false;
    }
    connect() {
        if(this.closed)return Promise.reject(new Error('E_STORAGE_CLOSED: player disposed'));
        if(this.db)return Promise.resolve(this.db);
        if(this.pending)return this.pending;
        const controller=new AbortController();this.openController=controller;
        const pending=Promise.resolve().then(()=>{
            if(this.closed)throw new Error('E_STORAGE_CLOSED: player disposed');
            return this.openDatabase(controller.signal);
        }).then(db=>{
            if(this.closed){db.close();throw new Error('E_STORAGE_CLOSED: player disposed');}
            this.db=db;
            const invalidate=()=>{if(this.db===db)this.db=null;};
            db.onversionchange=()=>{invalidate();db.close();};db.onclose=invalidate;
            return db;
        }).finally(()=>{if(this.pending===pending){this.pending=null;this.openController=null;}});
        this.pending=pending;return pending;
    }
    async run(work) {
        const db=await this.connect();
        if(this.closed)throw new Error('E_STORAGE_CLOSED: player disposed');
        try{return await work(db);}
        catch(error){
            if(error?.name==='InvalidStateError'&&this.db===db){this.db=null;db.close();}
            throw error;
        }
    }
    /// Idempotent read-only helper: retries once after reconnecting if Safari/iOS
    /// invalidated the handle with `InvalidStateError`.
    async runRead(work) {
        try{return await this.run(work);}
        catch(error){
            if(error?.name==='InvalidStateError'&&!this.closed)return await this.run(work);
            throw error;
        }
    }
    close() {this.closed=true;this.openController?.abort();this.db?.close();this.db=null;}
}

function readSaveTransaction(db,operation,start,{timeoutMs=3000,signal}={}) {
    return new Promise((resolve,reject)=>{
        if(!Number.isSafeInteger(timeoutMs)||timeoutMs<=0||timeoutMs>0x7fffffff){reject(new RangeError('Invalid save read timeout'));return;}
        if(signal?.aborted){reject(signal.reason||new Error('E_STORAGE_ABORT: save read cancelled'));return;}
        let tx,value,received=false,settled=false;const requests=[];
        const finish=error=>{
            if(settled)return;
            settled=true;clearTimeout(timer);signal?.removeEventListener('abort',abort);
            for(const r of requests)r.onsuccess=null;
            if(tx)tx.oncomplete=tx.onabort=tx.onerror=null;
            if(error){try{tx?.abort();}catch{}reject(error);}else resolve(value);
        };
        const timer=setTimeout(()=>finish(new Error(`E_STORAGE_TIMEOUT: ${operation} did not complete`)),timeoutMs);
        const abort=()=>finish(signal.reason||new Error('E_STORAGE_ABORT: save read cancelled'));
        signal?.addEventListener('abort',abort,{once:true});
        const track=r=>{requests.push(r);return r;};
        const result=v=>{if(!settled){value=v;received=true;}};
        // Native events can already be queued when cancellation removes their
        // handlers. Guard saved callbacks as well, especially cursor.continue().
        const active=()=>!settled;
        try{
            tx=db.transaction('saves','readonly');
            start(tx.objectStore('saves'),track,result,active,finish);
            tx.oncomplete=()=>finish(received?null:new Error('E_STORAGE_READ: missing save result'));
            tx.onabort=tx.onerror=()=>finish(tx.error||requests.find(r=>r.error)?.error||new Error('E_STORAGE_ABORT'));
        }catch(error){finish(error);}
    });
}
export function readSaveRecord(db,key,options) {
    return readSaveTransaction(db,'save read',(store,track,result,active)=>{
        const r=track(store.get(key));r.onsuccess=()=>{if(active())result(r.result);};
    },options);
}

// Typed Rust snapshot hashing distinguishes -0.0 from 0.0. Preserve that
// number through JSON transport, including browsers without JSON.rawJSON.
export function stringifyPlayerData(value,validate) {
    const prefix='__nir_json_negative_zero_',reserved=new Set();let marker=prefix+'0__',zeros=false;
    const encode=()=>JSON.stringify(value,function(key,v){
        if(validate)validate(this,key);
        if(key.startsWith(prefix))reserved.add(key);
        if(typeof v==='string'&&v.startsWith(prefix))reserved.add(v);
        if(typeof v==='number'&&Object.is(v,-0)){zeros=true;return marker;}
        return v;
    });
    let json=encode();if(!zeros)return json;
    if(reserved.has(marker)){let n=1;while(reserved.has(prefix+n+'__'))n++;marker=prefix+n+'__';json=encode();}
    return json.replaceAll(JSON.stringify(marker),'-0.0');
}

export async function inspectSaveRecord(record,key,inspect) {
    if(record===undefined)return null;
    if(!record||typeof record!=='object'||Array.isArray(record)
        ||record.gameId!==key[0]||record.profile!==key[1]||record.releaseDigest!==key[2]||record.slot!==key[3]
        ||!record.envelope||typeof record.envelope!=='object'||Array.isArray(record.envelope))
        throw new Error('E_SAVE_IDENTITY: stored record belongs to another slot');
    if(typeof inspect!=='function')throw new Error('E_SAVE_INSPECT: validator is required');
    return await inspect(stringifyPlayerData(record.envelope),key);
}

export async function listSaveRecords(db,gameId,profile,release,inspect,options) {
    const slotCount=Number.isInteger(options?.slotCount)&&options.slotCount>=1&&options.slotCount<=64?options.slotCount:3;
    const keys=Array.from({length:slotCount},(_,slot)=>saveKey(gameId,profile,release,slot));
    let records;
    try{
        records=await readSaveTransaction(db,'save list',(store,track,result,active)=>{
            const values=new Array(slotCount);let done=0;
            for(let slot=0;slot<slotCount;slot++){
                const req=track(store.get(keys[slot]));
                req.onsuccess=()=>{
                    if(!active())return;
                    values[slot]=req.result;
                    if(++done===slotCount)result(values);
                };
            }
        },options);
    }catch(error){
        const message=String(error).slice(0,1024);
        return Array.from({length:slotCount},(_,slot)=>({slot,error:message}));
    }
    const rows=[];
    for(let slot=0;slot<slotCount;slot++){
        const key=keys[slot],record=records[slot];
        try{
            const revision=await inspectSaveRecord(record,key,inspect);
            if(revision!==null)rows.push({slot,revision,label:typeof record.label==='string'?record.label.slice(0,256):`#${revision}`});
        }catch(error){rows.push({slot,error:String(error).slice(0,1024)});}
    }
    return rows;
}

export function listHistoryRecords(db,gameId,profile,options) {
    return readSaveTransaction(db,'save history read',(store,track,result,active,finish)=>{
        const rows=[];
        // The entire key prefix is contiguous. A lower bound also includes
        // damaged keys with an unexpected third-key type, unlike an [] upper bound.
        const range=IDBKeyRange.lowerBound([gameId,profile]);
        const cursor=track(store.openCursor(range));
        cursor.onsuccess=()=>{
            if(!active())return;
            try{
            if(!cursor.result){result(rows.sort((a,b)=>b.savedAt-a.savedAt));return;}
            const key=cursor.result.primaryKey;
            if(!Array.isArray(key)||key[0]!==gameId||key[1]!==profile){result(rows.sort((a,b)=>b.savedAt-a.savedAt));return;}
            const record=cursor.result.value;
            const savedAt=typeof record?.saved_at==='number'&&Number.isFinite(record.saved_at)
                &&Math.abs(record.saved_at)<=8.64e15?record.saved_at:0;
            const version=typeof record?.version==='string'?record.version.slice(0,256):'Version unknown';
            rows.push({key,savedAt,version});cursor.result.continue();
            }catch(error){finish(error);}
        };
    },options);
}

export function saveHistoryExport(entry,readable) {
    if(readable)return stringifyPlayerData(entry.record.envelope);
    // Recovery exports are explicitly distinct from importable save envelopes.
    // Never manufacture metadata or discard the damaged persisted value.
    const recovery={format:'nir-save-record-recovery-v1',key:entry.key,
        record:entry.record,...(entry.record===undefined?{recordUndefined:true}:{})};
    return stringifyPlayerData(recovery,(parent,key)=>{
        const value=parent[key],type=typeof value;
        if(value===undefined&&parent===recovery&&key==='record'&&recovery.recordUndefined)return;
        if(value===null||type==='string'||type==='boolean'||(type==='number'&&Number.isFinite(value)))return;
        if(type==='object'&&(Array.isArray(value)||Object.getPrototypeOf(value)===Object.prototype||Object.getPrototypeOf(value)===null))return;
        // JSON silently drops binary, dates, maps and non-finite values. Keep
        // the original in IndexedDB and report an export failure instead.
        throw new TypeError('E_HISTORY_EXPORT: record contains non-JSON values');
    });
}

export function validHistoryKey(key,slotCount=3) {
    return Array.isArray(key)&&key.length===4&&typeof key[0]==='string'&&typeof key[1]==='string'
        &&typeof key[2]==='string'&&releaseDigestPattern.test(key[2])&&Number.isInteger(key[3])&&key[3]>=0&&key[3]<slotCount;
}

export async function commitSaveRecord(db,key,envelope,metadata,expectedRevision,inspect,{timeoutMs=3000,signal,onPending,locale}={}) {
    if(!Number.isSafeInteger(timeoutMs)||timeoutMs<=0||timeoutMs>0x7fffffff)throw new RangeError('Invalid save write timeout');
    const cancelled=()=>signal?.reason||new Error('E_STORAGE_ABORT: save write cancelled');
    if(signal?.aborted)throw cancelled();
    if(!Number.isInteger(expectedRevision)||expectedRevision<0||expectedRevision>=0xffffffff)
        throw new Error('E_SAVE_REVISION: invalid expected revision');
    const incoming={...metadata,envelope};
    const revision=await inspectSaveRecord(incoming,key,inspect);
    if(revision!==expectedRevision+1)throw new Error('E_SAVE_REVISION: next revision does not match');
    const current=await readSaveRecord(db,key,{timeoutMs,signal});
    const storedRevision=await inspectSaveRecord(current,key,inspect);
    if((storedRevision??0)!==expectedRevision)throw new Error('E_SAVE_CONFLICT: another tab changed this slot. Reopen the save menu.');
    if(signal?.aborted)throw cancelled();
    const checked=stringifyPlayerData(current);
    return new Promise((resolve,reject)=>{
        let tx,r,put,settled=false,blocked=false,written=false,putSucceeded=false,failure=null;
        const finish=error=>{
            if(settled)return;
            settled=true;clearTimeout(timer);signal?.removeEventListener('abort',abort);
            if(r)r.onsuccess=null;if(put)put.onsuccess=null;
            if(tx)tx.oncomplete=tx.onabort=tx.onerror=null;
            if(error)reject(error);else resolve();
        };
        const stop=error=>{
            if(settled||blocked)return;
            blocked=true;failure=error;clearTimeout(timer);signal?.removeEventListener('abort',abort);
            try{tx?.abort();finish(error);}
            catch{
                if(!written){finish(error);return;}
                // The request may already be committing. Keep its original
                // completion and slot ownership; a timeout is not an outcome.
                const pending=Object.assign(new Error('E_STORAGE_UNCERTAIN: save is awaiting confirmation'),{code:'E_STORAGE_UNCERTAIN'});
                try{onPending?.(pending);}catch(noticeError){console.error('E_STORAGE_NOTICE',noticeError);}
            }
        };
        const timer=setTimeout(()=>stop(new Error('E_STORAGE_TIMEOUT: save write did not complete')),timeoutMs);
        const abort=()=>stop(cancelled());signal?.addEventListener('abort',abort,{once:true});
        try{
            tx=db.transaction('saves','readwrite');const store=tx.objectStore('saves');r=store.get(key);
            r.onsuccess=()=>{
                if(settled||blocked)return;
                try{
                    // Worker inspection happens before this transaction. Compare
                    // its entire checked value here without yielding the lock.
                    if(stringifyPlayerData(r.result)!==checked){stop(new Error('E_SAVE_CONFLICT: another tab changed this slot. Reopen the save menu.'));return;}
                    put=store.put({...metadata,envelope,saved_at:Date.now(),label:new Date().toLocaleString(locale||undefined)},key);written=true;
                    put.onsuccess=()=>{if(!settled)putSucceeded=true;};
                }catch(error){stop(error);}
            };
            tx.oncomplete=()=>finish(putSucceeded?null:failure||put?.error||new Error('E_STORAGE_WRITE: save was not written'));
            tx.onerror=()=>{failure=tx.error||put?.error||r.error||new Error('E_STORAGE_WRITE');};
            tx.onabort=()=>finish(failure||tx.error||put?.error||r.error||new Error('E_STORAGE_ABORT'));
        }catch(error){stop(error);}
    });
}

// One deadline spans database admission, inspection and network verification.
// Aborting settles the UI immediately; physical Worker reservations still last
// until their reply, and synchronous WASM cannot be preempted by a JS timer.
export function checkSavedRelease(work,{signal,timeoutMs=3000}={}) {
    if(!Number.isSafeInteger(timeoutMs)||timeoutMs<=0||timeoutMs>0x7fffffff)return Promise.reject(new RangeError('Invalid saved release check timeout'));
    const controller=new AbortController(),owned=controller.signal,start=performance.now();
    const timeout=()=>Object.assign(new Error('E_STORAGE_TIMEOUT: saved release check did not complete'),{code:'E_STORAGE_TIMEOUT',operation:'release_check'});
    return new Promise((resolve,reject)=>{
        let settled=false;
        const finish=(ok,value)=>{
            if(settled)return;
            settled=true;clearTimeout(timer);signal?.removeEventListener('abort',cancel);owned.removeEventListener('abort',abort);
            if(ok&&performance.now()-start>=timeoutMs){ok=false;value=timeout();}
            controller.abort(ok?new Error('E_STORAGE_ABORT: saved release check finished'):value);
            if(ok)resolve(value);else reject(value);
        };
        const cancel=()=>controller.abort(signal.reason||new Error('E_STORAGE_ABORT: saved release check cancelled'));
        const abort=()=>finish(false,owned.reason);
        const timer=setTimeout(()=>controller.abort(timeout()),timeoutMs);
        signal?.addEventListener('abort',cancel,{once:true});owned.addEventListener('abort',abort,{once:true});
        if(signal?.aborted)cancel();
        if(!owned.aborted)Promise.resolve().then(()=>{owned.throwIfAborted();return work(owned);}).then(value=>finish(true,value),error=>finish(false,error));
    });
}

export async function validateHistoryTarget({releaseRoot,digest,gameId,profile,signal,timeoutMs=3000,fetchImpl=fetch,subtle=globalThis.crypto?.subtle,sha256=async value=>Array.from(new Uint8Array(await subtle.digest('SHA-256',value)),x=>x.toString(16).padStart(2,'0')).join('')}) {
    if(!releaseDigestPattern.test(digest))return {available:false,status:'Invalid release digest'};
    const root=new URL(releaseRoot,location.href),manifest=new URL(`releases/${digest}.json`,root),entry=new URL(`releases/${digest}/index.html`,root);
    if(root.origin!==location.origin||manifest.origin!==root.origin||entry.origin!==root.origin||!manifest.pathname.startsWith(root.pathname)||!entry.pathname.startsWith(root.pathname))
        return {available:false,status:'Release is outside this site'};
    return checkSavedRelease(async signal=>{
      try{
        const response=await fetchImpl(manifest,{cache:'no-cache',signal});
        if(!response.ok)return {available:false,status:'Release resources unavailable'};
        const bytes=await response.arrayBuffer();
        const actual=await sha256(bytes);
        signal.throwIfAborted();
        if(actual!==digest)return {available:false,status:'Release verification failed'};
        const release=JSON.parse(new TextDecoder().decode(bytes));
        if(release.format!==1||release.game_id!==gameId||release.profile!==profile)return {available:false,status:'Different game or profile'};
        if(!releaseDigestPattern.test(release.launch?.html))return {available:false,status:'Release player verification failed'};
        const entryResponse=await fetchImpl(entry,{cache:'no-cache',signal});
        if(!entryResponse.ok)return {available:false,status:'Release player unavailable'};
        const htmlDigest=await sha256(await entryResponse.arrayBuffer());
        signal.throwIfAborted();
        if(htmlDigest!==release.launch.html)return {available:false,status:'Release player verification failed'};
        return {available:true,status:'Available',url:entry.href};
      }catch(error){signal.throwIfAborted();return {available:false,status:'Release resources unavailable'};}
    },{signal,timeoutMs});
}

