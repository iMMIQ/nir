// CDP throughput is bytes/second; rates below use decimal Mbps.
export const networkProfiles = [
  { id: 'loopback', latencyMs: 0, downloadMbps: null, uploadMbps: null },
  { id: 'shaped', latencyMs: 40, downloadMbps: 20, uploadMbps: 5 },
];

export async function applyNetwork(context, page, profile) {
  const session = await context.newCDPSession(page);
  await session.send('Network.enable');
  await session.send('Network.emulateNetworkConditions', {
    offline: false,
    latency: profile.latencyMs,
    downloadThroughput: profile.downloadMbps === null ? -1 : profile.downloadMbps * 1_000_000 / 8,
    uploadThroughput: profile.uploadMbps === null ? -1 : profile.uploadMbps * 1_000_000 / 8,
    connectionType: profile.id === 'loopback' ? 'ethernet' : 'cellular4g',
  });
  return session;
}

export function positiveInteger(value, fallback, name, minimum = 1) {
  const number = value === undefined ? fallback : Number(value);
  if (!Number.isSafeInteger(number) || number < minimum) throw new Error(`${name} must be an integer >= ${minimum}`);
  return number;
}
