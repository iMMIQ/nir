import fs from 'node:fs/promises';

// Page Performance.getMetrics only covers the page isolate. Runtime/Asset
// Workers need their own CDP heap readings; keep them separate from process
// PSS and decoded-media payloads. Use this opt-in probe for lifecycle studies,
// without changing the timing overhead of existing endurance measurements.
export function createWorkerHeapProbe(browserSession) {
  const attached=new Map(),pending=new Map();let sequence=0,closed=false;
  function receive({sessionId,message}) {
    const result=JSON.parse(message),key=`${sessionId}:${result.id}`,request=pending.get(key);
    if(!request)return;
    pending.delete(key);clearTimeout(request.timer);
    if(result.error)request.reject(new Error(JSON.stringify(result.error)));else request.resolve(result.result);
  }
  browserSession.on('Target.receivedMessageFromTarget',receive);
  function call(sessionId,method,params={}) {
    const id=++sequence,key=`${sessionId}:${id}`;
    return new Promise((resolve,reject)=>{
      const timer=setTimeout(()=>{pending.delete(key);reject(new Error(`Worker CDP ${method} timed out`));},5000);
      pending.set(key,{resolve,reject,timer});
      browserSession.send('Target.sendMessageToTarget',{sessionId,message:JSON.stringify({id,method,params})}).catch(error=>{
        const request=pending.get(key);if(!request)return;
        pending.delete(key);clearTimeout(timer);reject(error);
      });
    });
  }
  return {
    async sample() {
      if(closed)throw new Error('Worker heap probe closed');
      const {targetInfos}=await browserSession.send('Target.getTargets');
      const targets=targetInfos.filter(target=>target.type==='worker');
      const results=await Promise.allSettled(targets.map(async target=>{
        let sessionId=attached.get(target.targetId);
        if(!sessionId){({sessionId}=await browserSession.send('Target.attachToTarget',{targetId:target.targetId,flatten:false}));attached.set(target.targetId,sessionId);}
        const role=await call(sessionId,'Runtime.evaluate',{expression:'globalThis.__nirWorker?.role',returnByValue:true});
        const heap=await call(sessionId,'Runtime.getHeapUsage');
        return {targetId:target.targetId,role:role.result?.value??'unknown',url:target.url,...heap};
      }));
      return {scope:'per-Worker V8 heap; excludes native decoder/driver allocations; backing storage reported separately',
        targets:targets.length,rows:results.filter(r=>r.status==='fulfilled').map(r=>r.value),
        errors:results.filter(r=>r.status==='rejected').map(r=>String(r.reason))};
    },
    async close() {
      closed=true;browserSession.off('Target.receivedMessageFromTarget',receive);
      for(const request of pending.values()){clearTimeout(request.timer);request.reject(new Error('Worker heap probe closed'));}pending.clear();
      await Promise.allSettled([...attached.values()].map(sessionId=>browserSession.send('Target.detachFromTarget',{sessionId})));attached.clear();
    },
  };
}

// Linux process RSS includes shared pages: report per process and an explicitly
// labelled sum, never present it as uniquely owned application memory.
export async function sampleProcessMemory(browserSession, pageSession) {
  const result = { processes: [], rssSumBytes: null, pssSumBytes: null, jsHeapUsedBytes: null,
    gpuMemory: { status: 'unmeasured', reason: 'no portable per-browser GPU allocation counter' } };
  try {
    const { processInfo } = await browserSession.send('SystemInfo.getProcessInfo');
    for (const process of processInfo) {
      const row = { pid: process.id, type: process.type, cpuSeconds: process.cpuTime };
      try {
        const text = await fs.readFile(`/proc/${process.id}/smaps_rollup`, 'utf8');
        for (const [field, label] of [['rssBytes', 'Rss'], ['pssBytes', 'Pss']]) {
          const value = text.match(new RegExp(`^${label}:\\s+(\\d+) kB$`, 'm'));
          if (value) row[field] = Number(value[1]) * 1024;
        }
      } catch (error) { row.unavailable = error.code || String(error); }
      result.processes.push(row);
    }
    for (const [total, field] of [['rssSumBytes', 'rssBytes'], ['pssSumBytes', 'pssBytes']]) {
      if (result.processes.length && result.processes.every(row => Number.isFinite(row[field])))
        result[total] = result.processes.reduce((sum, row) => sum + row[field], 0);
    }
  } catch (error) { result.processError = String(error); }
  try {
    const { metrics } = await pageSession.send('Performance.getMetrics');
    result.jsHeapUsedBytes = metrics.find(row => row.name === 'JSHeapUsedSize')?.value ?? null;
    result.dom = await pageSession.send('Memory.getDOMCounters');
  } catch (error) { result.pageError = String(error); }
  result.systemPressure = {};
  for (const resource of ['cpu', 'memory', 'io']) {
    try { result.systemPressure[resource] = (await fs.readFile(`/proc/pressure/${resource}`, 'utf8')).trim(); }
    catch { result.systemPressure[resource] = null; }
  }
  return result;
}

export function trend(rows, field) {
  const usable = rows.filter(row => Number.isFinite(row[field]));
  if (usable.length < 2) return { samples: usable.length, slopeBytesPerHour: null };
  const x = usable.map(row => row.elapsedMs / 3600000), y = usable.map(row => row[field]);
  const mx = x.reduce((a, b) => a + b, 0) / x.length, my = y.reduce((a, b) => a + b, 0) / y.length;
  const denominator = x.reduce((sum, value) => sum + (value - mx) ** 2, 0);
  return { samples: usable.length, min: Math.min(...y), max: Math.max(...y), first: y[0], last: y.at(-1),
    slopeBytesPerHour: denominator ? x.reduce((sum, value, i) => sum + (value - mx) * (y[i] - my), 0) / denominator : null };
}
