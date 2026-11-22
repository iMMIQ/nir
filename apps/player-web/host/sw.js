// NIR v2 Lightweight PWA Service Worker
// Caches heavy immutable runtime shell assets (.wasm / fallback font / JS / CSS)
// while passing HTTP Range `.nirpak` streams straight through to the network!

const CACHE_NAME = 'nir-shell-v1';

self.addEventListener('install', (event) => {
  event.waitUntil(self.skipWaiting());
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) =>
        Promise.all(keys.filter((k) => k !== CACHE_NAME).map((k) => caches.delete(k))),
      )
      .then(() => self.clients.claim()),
  );
});

self.addEventListener('fetch', (event) => {
  const req = event.request;
  if (req.method !== 'GET') return;
  // Never intercept HTTP Range requests or SSE/API/nirpak media streams!
  if (req.headers.has('range')) return;

  const url = new URL(req.url);
  if (url.origin !== self.location.origin) return;
  if (url.pathname.endsWith('.nirpak') || url.pathname.includes('/__nir_events')) return;

  const isHeavyStatic =
    url.pathname.endsWith('.wasm') ||
    url.pathname.endsWith('.otf') ||
    url.pathname.endsWith('.ttf');

  if (isHeavyStatic) {
    // Cache-first for large WASM / Pan-CJK fallback font blobs
    event.respondWith(
      caches.open(CACHE_NAME).then(async (cache) => {
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
    // Network-first with offline cache fallback for HTML/JS/CSS shell
    event.respondWith(
      fetch(req)
        .then((res) => {
          if (res.ok && res.status === 200) {
            caches.open(CACHE_NAME).then((c) => c.put(req, res.clone())).catch(() => {});
          }
          return res;
        })
        .catch(() => caches.match(req)),
    );
  }
});
