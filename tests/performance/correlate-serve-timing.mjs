// Usage: node correlate-serve-timing.mjs REPORT.json SERVER.log [COUNT] [PRESSURE.json]
import fs from 'node:fs/promises';

const [reportFile, logFile, countText = '12', pressureFile] = process.argv.slice(2);
if (!reportFile || !logFile) throw Error('usage: correlate-serve-timing.mjs REPORT.json SERVER.log [COUNT]');
const count = Number(countText);
if (!Number.isSafeInteger(count) || count < 1) throw Error('COUNT must be positive');
const report = JSON.parse(await fs.readFile(reportFile, 'utf8'));
const pressure = pressureFile ? JSON.parse(await fs.readFile(pressureFile, 'utf8')).samples : [];
function pressureDelta(startSeconds, finishSeconds, field) {
  const before = pressure.findLast(sample => sample.wallTime <= startSeconds);
  const after = pressure.find(sample => sample.wallTime >= finishSeconds);
  return before && after ? Number(((after.io[field] - before.io[field]) / 1000).toFixed(2)) : null;
}
const server = new Map();
for (const line of (await fs.readFile(logFile, 'utf8')).split('\n')) {
  if (!line.startsWith('nir_serve ')) continue;
  const fields = Object.fromEntries(line.slice('nir_serve '.length).split(' ').map(field => field.split('=')));
  const record = server.get(fields.id) ?? { path: fields.path };
  record[fields.phase] = { wallUs: Number(fields.wall_us), elapsedUs: Number(fields.elapsed_us), bytes: Number(fields.bytes) };
  server.set(fields.id, record);
}
const requests = new Map();
for (const event of report.network ?? []) {
  const row = requests.get(event.id) ?? {};
  row[event.type] = event;
  requests.set(event.id, row);
}
const rows = [];
for (const [id, row] of requests) {
  const { request, response, finished } = row;
  if (!request?.url.startsWith('/objects/') || !finished) continue;
  const timing = response?.timing;
  // Cache hits can replay X-NIR-Serve-Id from the original response. That ID
  // does not represent a server request made during this CDP request.
  const liveServeId = response?.fromDiskCache ? undefined : response?.serveId;
  const phases = liveServeId ? server.get(String(liveServeId)) : undefined;
  const startWallMs = request.wallTime === undefined ? undefined : request.wallTime * 1000;
  const receivedWallMs = phases?.received ? phases.received.wallUs / 1000 : undefined;
  const doneWallMs = phases?.done ? phases.done.wallUs / 1000 : undefined;
  const orderedResponse = response && response.at <= finished.at ? response : undefined;
  rows.push({ id, path: request.url, serveId: liveServeId ?? null,
    fromDiskCache: response?.fromDiskCache ?? null,
    encodedBytes: finished.encodedBytes,
    totalMs: Number(((finished.at - request.at) * 1000).toFixed(2)),
    requestToHeadersMs: orderedResponse ? Number(((orderedResponse.at - request.at) * 1000).toFixed(2)) : null,
    headersToFinishMs: orderedResponse ? Number(((finished.at - orderedResponse.at) * 1000).toFixed(2)) : null,
    cdpHeaderIntervalMs: timing?.receiveHeadersEnd ?? null,
    requestToServerReceivedMs: startWallMs === undefined || receivedWallMs === undefined ? null
      : Number((receivedWallMs - startWallMs).toFixed(2)),
    serverLookupMs: phases?.ready ? Number((phases.ready.elapsedUs / 1000).toFixed(2)) : null,
    serverSendMs: phases?.done && phases?.ready
      ? Number(((phases.done.elapsedUs - phases.ready.elapsedUs) / 1000).toFixed(2)) : null,
    serverDoneToClientFinishedMs: startWallMs === undefined || doneWallMs === undefined ? null
      : Number((startWallMs + (finished.at - request.at) * 1000 - doneWallMs).toFixed(2)),
    ioFullStallMs: request.wallTime === undefined ? null
      : pressureDelta(request.wallTime, request.wallTime + finished.at - request.at, 'full'),
    ioSomeStallMs: request.wallTime === undefined ? null
      : pressureDelta(request.wallTime, request.wallTime + finished.at - request.at, 'some'),
  });
}
rows.sort((a, b) => b.totalMs - a.totalMs);
console.log(JSON.stringify({ format: 1, requestCount: rows.length, matchedServerCount: rows.filter(x => x.serveId && server.has(String(x.serveId))).length,
  longest: rows.slice(0, count) }, null, 2));
