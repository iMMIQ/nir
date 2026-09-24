import fs from 'node:fs/promises';

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
