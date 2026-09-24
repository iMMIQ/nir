// Independent static server for restore network diagnostics.
// Usage: node static-server.mjs BUILD_DIR PORT
import { createServer } from 'node:http';
import { createReadStream } from 'node:fs';
import { stat } from 'node:fs/promises';
import path from 'node:path';
import { performance } from 'node:perf_hooks';

const root = path.resolve(process.argv[2] ?? '.');
const port = Number(process.argv[3]);
if (!Number.isInteger(port) || port < 1 || port > 65535) throw Error('usage: static-server.mjs BUILD_DIR PORT');
const timing = process.env.NIR_SERVE_TIMING !== undefined;
let nextId = 0;
const mime = { html: 'text/html; charset=utf-8', js: 'text/javascript; charset=utf-8',
  json: 'application/json', txt: 'text/plain; charset=utf-8', wasm: 'application/wasm',
  png: 'image/png', wav: 'audio/wav', otf: 'font/otf' };
const stamp = () => Math.round((performance.timeOrigin + performance.now()) * 1000);
function log(id, phase, pathname, start, bytes = 0) {
  if (id === undefined) return;
  console.error(`nir_serve id=${id} phase=${phase} wall_us=${stamp()} elapsed_us=${Math.round((performance.now() - start) * 1000)} bytes=${bytes} path=${pathname}`);
}
async function fileInfo(file) {
  try {
    const info = await stat(file);
    return info.isFile() ? info : undefined;
  } catch { return undefined; }
}
function acceptsGzip(header = '') {
  const gzip = header.split(',').map(x => x.trim()).find(x => /^gzip(?:\s*;|$)/i.test(x));
  return gzip !== undefined && !/;\s*q\s*=\s*0(?:\.0*)?(?:\s*;|$)/i.test(gzip);
}

const server = createServer(async (req, res) => {
  const start = performance.now();
  const id = timing ? ++nextId : undefined;
  const pathname = (req.url ?? '/').split('?')[0];
  log(id, 'received', pathname, start);
  try {
    const relative = pathname === '/' ? 'index.html' : pathname.slice(1);
    if (pathname.includes('%') || pathname.includes('\\') || relative.split('/').includes('..')) {
      res.writeHead(404).end('Not found');
      log(id, 'not_found', pathname, start);
      return;
    }
    const file = path.join(root, relative.endsWith('/') ? `${relative}index.html` : relative);
    const info = await fileInfo(file);
    if (!info) {
      res.writeHead(404).end('Not found');
      log(id, 'not_found', pathname, start);
      return;
    }
    const ext = path.extname(file).slice(1);
    const object = relative.split('/').includes('objects');
    const negotiable = object && ['json', 'js', 'wasm'].includes(ext);
    const gzFile = `${file}.gz`;
    const gzInfo = negotiable && acceptsGzip(req.headers['accept-encoding']) ? await fileInfo(gzFile) : undefined;
    const source = gzInfo ? gzFile : file;
    const size = gzInfo?.size ?? info.size;
    const headers = { 'Content-Type': mime[ext] ?? 'application/octet-stream',
      'Content-Length': size,
      'Cache-Control': object || relative.split('/').includes('releases')
        ? 'public, max-age=31536000, immutable' : 'no-cache',
      'X-Content-Type-Options': 'nosniff',
      'Content-Security-Policy': "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' blob:; media-src 'self' blob:; object-src 'none'; base-uri 'self'; frame-ancestors 'none'",
    };
    if (negotiable) headers.Vary = 'Accept-Encoding';
    if (gzInfo) headers['Content-Encoding'] = 'gzip';
    if (id !== undefined) headers['X-NIR-Serve-Id'] = String(id);
    res.once('finish', () => log(id, 'done', pathname, start, size));
    res.once('error', () => log(id, 'send_error', pathname, start, size));
    log(id, 'ready', pathname, start, size);
    res.writeHead(200, headers);
    createReadStream(source).on('error', error => res.destroy(error)).pipe(res);
  } catch (error) {
    if (!res.headersSent) res.writeHead(500).end('Server error');
    else res.destroy(error);
    log(id, 'send_error', pathname, start);
  }
});
server.listen(port, '127.0.0.1');
