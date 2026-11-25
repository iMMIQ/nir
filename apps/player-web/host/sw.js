// NIR v2 PWA Service Worker with Offline `.nirpak` HTTP Range 206 Slicing!
// - Caches heavy immutable runtime shell assets (.wasm / fallback font / JS / CSS)
// - Serves explicitly offline-cached `.nirpak` archives (`nir-offline-paks-v1`) with exact
//   spec-compliant HTTP 206 Partial Content Range slicing (works in airplanes / subways!)
// - Passes non-cached `.nirpak` streams straight through to the network with zero overhead!

const SHELL_CACHE = 'nir-shell-v2';
const OFFLINE_PAK_CACHE = 'nir-offline-paks-v1';
const KEEP_CACHES = new Set([SHELL_CACHE, OFFLINE_PAK_CACHE, 'nir-fonts-v1']);

self.addEventListener('install', (event) => {
  event.waitUntil(self.skipWaiting());
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) =>
        Promise.all(keys.filter((k) => !KEEP_CACHES.has(k)).map((k) => caches.delete(k))),
      )
      .then(() => self.clients.claim()),
  );
});

async function serveOfflinePak(req) {
  const cache = await caches.open(OFFLINE_PAK_CACHE);
  const url = new URL(req.url);
  const hit = await cache.match(url.pathname);
  if (!hit) {
    return fetch(req);
  }
  const rangeHdr = req.headers.get('range');
  if (!rangeHdr) {
    return hit;
  }
  const m = /^bytes=(\d*)-(\d*)$/i.exec(rangeHdr.trim());
  if (!m) {
    return hit;
  }
  const blob = await hit.blob();
  const total = blob.size;
  let start = m[1] ? Number(m[1]) : 0;
  let end = m[2] ? Number(m[2]) : total - 1;
  if (!m[1] && m[2]) {
    start = Math.max(0, total - Number(m[2]));
    end = total - 1;
  }
  end = Math.min(end, total - 1);
  if (Number.isNaN(start) || Number.isNaN(end) || start > end || start >= total) {
    return new Response('Range Not Satisfiable', {
      status: 416,
      headers: { 'Content-Range': `bytes */${total}` },
    });
  }
  const slice = blob.slice(start, end + 1, 'application/octet-stream');
  return new Response(slice, {
    status: 206,
    statusText: 'Partial Content',
    headers: {
      'Content-Type': 'application/octet-stream',
      'Content-Length': String(end - start + 1),
      'Content-Range': `bytes ${start}-${end}/${total}`,
      'Accept-Ranges': 'bytes',
    },
  });
}

self.addEventListener('fetch', (event) => {
  const req = event.request;
  if (req.method !== 'GET') return;

  const url = new URL(req.url);
  if (url.origin !== self.location.origin) return;
  if (url.pathname.includes('/__nir_events') || url.pathname.startsWith('/api/')) return;

  // Offline-cached `.nirpak` with exact HTTP Range 206 support!
  if (url.pathname.endsWith('.nirpak')) {
    event.respondWith(serveOfflinePak(req));
    return;
  }

  if (req.headers.has('range')) return;

  const isHeavyStatic =
    url.pathname.endsWith('.wasm') ||
    url.pathname.endsWith('.otf') ||
    url.pathname.endsWith('.ttf');

  if (isHeavyStatic) {
    event.respondWith(
      caches.open(SHELL_CACHE).then(async (cache) => {
        const hit = await cache.match(req);
        if (hit) return hit;
        const res = await fetch(req);
        if (res.ok && res.status === 200) {
          cache.put(req, res.clone()).catch(() => {});
        }
        return res;
      }),
    );
    return;
  }

  const isShell =
    url.pathname.endsWith('/') ||
    url.pathname.endsWith('.html') ||
    url.pathname.endsWith('.js') ||
    url.pathname.endsWith('.css') ||
    url.pathname.endsWith('.webmanifest');

  if (isShell) {
    event.respondWith(
      fetch(req)
        .then((res) => {
          if (res.ok && res.status === 200) {
            caches.open(SHELL_CACHE).then((c) => c.put(req, res.clone())).catch(() => {});
          }
          return res;
        })
        .catch(() => caches.match(req)),
    );
  }
});
