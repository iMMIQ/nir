// NIR v2 Modern Single-Page Web Host (Desktop + Mobile Optimized)
// HTTP Range streaming OR zero-copy JS-backed .nirpak | Idle-pausing rAF |
// Adaptive Mobile Budgets + DPR Cap | Multi-Touch Gestures + Safe Area Insets |
// iOS Safari Audio Unlock + Streaming BGM | Wake Lock | WebGL2 Context Loss Recovery |
// On-Demand Cached Fallback Font + PWA Support

const canvas = document.getElementById('stage');
const videoEl = document.getElementById('video-layer');
const videoOverlayEl = document.getElementById('video-overlay');
const videoPlayBtn = document.getElementById('video-play-btn');
const videoSkipBtn = document.getElementById('video-skip-btn');
const fsBtn = document.getElementById('fs-btn');
const shelfBtn = document.getElementById('shelf-btn');
const quickToolsEl = document.getElementById('quick-tools');
const copyBtn = document.getElementById('copy-btn');
const shotBtn = document.getElementById('shot-btn');
const syncPill = document.getElementById('sync-pill');
const safeProbeEl = document.getElementById('safe-area-probe');
const loaderEl = document.getElementById('loader');
const loaderText = document.getElementById('loader-text');
const loaderFill = document.getElementById('loader-fill');

let syncPillTimer = 0;
const showSyncPill = (msg, isError = false) => {
  if (!syncPill) return;
  syncPill.textContent = msg;
  syncPill.style.color = isError ? '#ff9ea3' : '#9fd0fb';
  syncPill.hidden = false;
  clearTimeout(syncPillTimer);
  syncPillTimer = setTimeout(() => {
    syncPill.hidden = true;
  }, isError ? 3200 : 1600);
};

const setProgress = (pct, msg) => {
  if (loaderFill) loaderFill.style.width = `${Math.round(pct)}%`;
  if (msg && loaderText) loaderText.textContent = msg;
};

// --- Adaptive Mobile Hardware Profile (prevents mobile Jetsam OOM & 3x overdraw!) ---
function detectDeviceProfile() {
  const coarsePointer =
    typeof window.matchMedia === 'function' && window.matchMedia('(pointer: coarse)').matches;
  const uaMobile = /Android|iPhone|iPad|iPod|Mobile|HarmonyOS|MicroMessenger/i.test(
    navigator.userAgent || '',
  );
  const hasTouch = (navigator.maxTouchPoints || 0) > 0 || 'ontouchstart' in window;
  const isMobile = coarsePointer || uaMobile;
  const isIOS =
    /iPhone|iPad|iPod/i.test(navigator.userAgent || '') ||
    (navigator.platform === 'MacIntel' && (navigator.maxTouchPoints || 0) > 1);
  const memGb = Number(navigator.deviceMemory || 0);
  const lowMem = (memGb > 0 && memGb <= 3) || (isIOS && Math.min(screen.width, screen.height) <= 390);
  return {
    isMobile,
    isIOS,
    hasTouch,
    lowMem,
    maxDpr: isMobile ? (lowMem ? 1.5 : 2.0) : 3.0,
    rangeCacheLimit: (isMobile ? (lowMem ? 16 : 32) : 64) * 1024 * 1024,
    audioDecodedLimit: (isMobile ? (lowMem ? 24 : 40) : 96) * 1024 * 1024,
    gpuCacheMb: isMobile ? (lowMem ? 64 : 80) : 0, // 0 = use Renderer backend default
    maxBaseBitmaps: isMobile ? 4 : 8,
    maxBgPrefetch: isMobile ? 2 : 3,
  };
}

const deviceProfile = detectDeviceProfile();

function readViewportMetrics() {
  const vv = window.visualViewport;
  const width = Math.max(1, Math.round(vv ? vv.width : window.innerWidth));
  const height = Math.max(1, Math.round(vv ? vv.height : window.innerHeight));
  const dpr = Math.min(window.devicePixelRatio || 1, deviceProfile.maxDpr);
  let top = 0;
  let right = 0;
  let bottom = 0;
  let left = 0;
  if (safeProbeEl) {
    const cs = getComputedStyle(safeProbeEl);
    top = parseFloat(cs.paddingTop) || 0;
    right = parseFloat(cs.paddingRight) || 0;
    bottom = parseFloat(cs.paddingBottom) || 0;
    left = parseFloat(cs.paddingLeft) || 0;
  }
  return { width, height, dpr, insets: { top, right, bottom, left } };
}

// --- OPFS (Origin Private File System) & Local File Zero-Copy `.nirpak` Helpers ---
async function openOpfsPakFile(slug) {
  if (!slug || !navigator.storage?.getDirectory) return null;
  try {
    const root = await navigator.storage.getDirectory();
    const dir = await root.getDirectoryHandle('nir-paks');
    const fh = await dir.getFileHandle(`${slug}.nirpak`);
    const file = await fh.getFile();
    return file && file.size >= 32 ? file : null;
  } catch {
    return null;
  }
}

async function consumeLocalPendingPakFile() {
  // 1. Check PWA File Handling API (`window.launchQueue`)
  if (window.launchQueue && typeof window.launchQueue.setConsumer === 'function') {
    const launched = await new Promise((resolve) => {
      let settled = false;
      const timer = setTimeout(() => { if (!settled) { settled = true; resolve(null); } }, 180);
      try {
        window.launchQueue.setConsumer(async (params) => {
          if (settled) return;
          settled = true;
          clearTimeout(timer);
          try {
            const handle = params?.files?.[0];
            resolve(handle ? await handle.getFile() : null);
          } catch {
            resolve(null);
          }
        });
      } catch {
        clearTimeout(timer);
        resolve(null);
      }
    });
    if (launched && launched.size >= 32) return launched;
  }
  // 2. Check IndexedDB handoff from Shelf ("Play Local Without Uploading")
  try {
    const db = await new Promise((resolve, reject) => {
      const req = indexedDB.open('nir_local_play_v1', 1);
      req.onupgradeneeded = () => req.result.createObjectStore('kv');
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error);
    });
    const file = await new Promise((resolve) => {
      const tx = db.transaction('kv', 'readonly');
      const req = tx.objectStore('kv').get('pending_file');
      req.onsuccess = () => resolve(req.result || null);
      req.onerror = () => resolve(null);
    });
    db.close();
    if (file && typeof file.slice === 'function' && file.size >= 32) return file;
  } catch { /* ignore */ }
  return null;
}

// --- Off-Main-Thread Image Decode & LiveMaker Delta Compositor Worker (Transferable ImageBitmap!) ---
class ImageDecodeWorkerPool {
  constructor(maxBases = 8) {
    this.worker = null;
    this.nextReqId = 1;
    this.pending = new Map();
    this.workerBaseIds = new Set();
    this.maxBases = maxBases;
    this.disabled = false;
    this._init();
  }

  _init() {
    if (
      typeof Worker === 'undefined' ||
      typeof OffscreenCanvas === 'undefined' ||
      typeof createImageBitmap !== 'function'
    ) {
      this.disabled = true;
      return;
    }
    const src = `
      const baseCache = new Map();
      let maxBases = ${Number(this.maxBases) || 8};
      function putBase(id, bmp) {
        const old = baseCache.get(id);
        if (old) { try { old.close(); } catch {} baseCache.delete(id); }
        baseCache.set(id, bmp);
        while (baseCache.size > maxBases) {
          const oldest = baseCache.keys().next().value;
          const ob = baseCache.get(oldest);
          baseCache.delete(oldest);
          try { ob.close(); } catch {}
        }
      }
      function getBase(id) {
        const hit = baseCache.get(id);
        if (!hit) return null;
        baseCache.delete(id);
        baseCache.set(id, hit);
        return hit;
      }
      async function compositeDelta(baseBmp, patchBmp, delta) {
        const w = baseBmp.width, h = baseBmp.height;
        const cvs = new OffscreenCanvas(w, h);
        const ctx = cvs.getContext('2d');
        ctx.globalCompositeOperation = 'copy';
        ctx.drawImage(baseBmp, 0, 0);
        const mask = delta.tile_mask;
        if (!mask || mask.length === 0) {
          ctx.globalCompositeOperation = 'source-over';
          ctx.drawImage(patchBmp, delta.x, delta.y);
        } else {
          const TILE = 16;
          const pw = patchBmp.width, ph = patchBmp.height;
          const txCount = Math.ceil(pw / TILE), tyCount = Math.ceil(ph / TILE);
          ctx.globalCompositeOperation = 'source-over';
          for (let pty = 0; pty < tyCount; pty++) {
            for (let ptx = 0; ptx < txCount; ptx++) {
              const bit = pty * txCount + ptx;
              if (((mask[bit >> 3] || 0) & (1 << (bit & 7))) === 0) continue;
              const sx = ptx * TILE, sy = pty * TILE;
              const sw = Math.min(TILE, pw - sx), sh = Math.min(TILE, ph - sy);
              ctx.clearRect(delta.x + sx, delta.y + sy, sw, sh);
              ctx.drawImage(patchBmp, sx, sy, sw, sh, delta.x + sx, delta.y + sy, sw, sh);
            }
          }
        }
        return createImageBitmap(cvs, { premultiplyAlpha: 'premultiply' });
      }
      self.onmessage = async (ev) => {
        const m = ev.data;
        if (m.op === 'clear_bases') {
          for (const b of baseCache.values()) { try { b.close(); } catch {} }
          baseCache.clear();
          return;
        }
        try {
          if (m.op === 'decode') {
            const bmp = await createImageBitmap(new Blob([m.bytes], { type: m.mime }), { premultiplyAlpha: 'premultiply' });
            self.postMessage({ reqId: m.reqId, bitmap: bmp }, [bmp]);
          } else if (m.op === 'delta') {
            let baseBmp = getBase(m.baseId);
            if (!baseBmp) {
              if (!m.baseBytes) throw new Error('missing baseBytes');
              baseBmp = await createImageBitmap(new Blob([m.baseBytes], { type: m.baseMime }), { premultiplyAlpha: 'none' });
              putBase(m.baseId, baseBmp);
            }
            const patchBmp = await createImageBitmap(new Blob([m.patchBytes], { type: m.patchMime }), { premultiplyAlpha: 'none' });
            try {
              const out = await compositeDelta(baseBmp, patchBmp, m.delta);
              self.postMessage({ reqId: m.reqId, bitmap: out, cachedBases: Array.from(baseCache.keys()) }, [out]);
            } finally {
              patchBmp.close();
            }
          }
        } catch (err) {
          self.postMessage({ reqId: m.reqId, error: String(err?.message ?? err) });
        }
      };
    `;
    try {
      const url = URL.createObjectURL(new Blob([src], { type: 'application/javascript' }));
      this.worker = new Worker(url);
      URL.revokeObjectURL(url);
      this.worker.onmessage = (ev) => {
        const { reqId, bitmap, error, cachedBases } = ev.data || {};
        if (Array.isArray(cachedBases)) {
          this.workerBaseIds = new Set(cachedBases);
        }
        const entry = this.pending.get(reqId);
        if (!entry) {
          if (bitmap) try { bitmap.close(); } catch {}
          return;
        }
        this.pending.delete(reqId);
        if (error) entry.reject(new Error(error));
        else entry.resolve(bitmap);
      };
      this.worker.onerror = () => {
        this.disabled = true;
        for (const p of this.pending.values()) p.reject(new Error('worker error'));
        this.pending.clear();
        try { this.worker?.terminate(); } catch {}
        this.worker = null;
      };
    } catch {
      this.disabled = true;
      this.worker = null;
    }
  }

  get available() {
    return !this.disabled && this.worker !== null;
  }

  hasCachedBase(baseId) {
    return this.workerBaseIds.has(baseId);
  }

  clearBases() {
    this.workerBaseIds.clear();
    if (this.worker) {
      try { this.worker.postMessage({ op: 'clear_bases' }); } catch {}
    }
  }

  decode(bytes, mime) {
    const reqId = this.nextReqId++;
    const copy = bytes.slice().buffer;
    return new Promise((resolve, reject) => {
      this.pending.set(reqId, { resolve, reject });
      this.worker.postMessage({ op: 'decode', reqId, bytes: copy, mime }, [copy]);
    });
  }

  decodeDelta(baseId, baseBytesOrNull, baseMime, patchBytes, patchMime, delta) {
    const reqId = this.nextReqId++;
    const patchBuf = patchBytes.slice().buffer;
    const transfers = [patchBuf];
    let baseBuf = null;
    if (baseBytesOrNull) {
      baseBuf = baseBytesOrNull.slice().buffer;
      transfers.push(baseBuf);
    }
    return new Promise((resolve, reject) => {
      this.pending.set(reqId, { resolve, reject });
      this.worker.postMessage(
        { op: 'delta', reqId, baseId, baseBytes: baseBuf, baseMime, patchBytes: patchBuf, patchMime, delta },
        transfers,
      );
    });
  }
}

// --- Streaming / OPFS / Local File Zero-Copy `.nirpak` Source (keeps media out of WASM linear memory!) ---
class PakSource {
  constructor(url, rangeCacheLimit, localFile = null) {
    this.url = url;
    this.localFile = localFile;
    this.mode = localFile ? 'file' : 'memory'; // 'file' | 'range' | 'memory'
    this.fullBytes = null;
    this.mediaBase = 0;
    this.rangeCache = new Map(); // key -> Uint8Array
    this.rangeCacheBytes = 0;
    this.rangeCacheLimit = rangeCacheLimit || 64 * 1024 * 1024;
    this.inflightRanges = new Map();
  }

  async open() {
    if (this.localFile) {
      if (this.localFile.size < 32) throw new Error('truncated local nirpak header');
      const hdr = new Uint8Array(await this.localFile.slice(0, 32).arrayBuffer());
      const dv = new DataView(hdr.buffer, hdr.byteOffset, 32);
      const progLen = Number(dv.getBigUint64(16, true));
      this.mediaBase = 32 + progLen;
      this.mode = 'file';
      setProgress(65, 'Reading local story index…');
      return new Uint8Array(await this.localFile.slice(0, this.mediaBase).arrayBuffer());
    }
    // Probe 32-byte `.nirpak` header via HTTP Range first for instant startup!
    try {
      const probe = await fetch(this.url, {
        headers: { Range: 'bytes=0-31' },
        cache: 'no-cache',
      });
      if (!probe.ok) throw new Error(`HTTP ${probe.status} fetching ${this.url}`);

      if (probe.status === 206) {
        const hdr = new Uint8Array(await probe.arrayBuffer());
        if (hdr.byteLength < 32) throw new Error('truncated nirpak header');
        const dv = new DataView(hdr.buffer, hdr.byteOffset, 32);
        const progLen = Number(dv.getBigUint64(16, true));
        this.mediaBase = 32 + progLen;
        this.mode = 'range';
        setProgress(55, 'Loading story index…');
        const idxRes = await fetch(this.url, {
          headers: { Range: `bytes=0-${this.mediaBase - 1}` },
          cache: 'no-cache',
        });
        if (idxRes.status === 206 || idxRes.status === 200) {
          const buf = new Uint8Array(await idxRes.arrayBuffer());
          return buf.byteLength > this.mediaBase ? buf.subarray(0, this.mediaBase) : buf;
        }
      } else {
        // Server returned full 200 OK; warn on mobile if payload is huge so users know why it takes longer
        const full = await this._readFullResponse(probe);
        return this._initMemory(full);
      }
    } catch {
      // Fallback to standard fetch without Range header
    }

    const res = await fetch(this.url, { cache: 'no-cache' });
    if (!res.ok) throw new Error(`HTTP ${res.status} fetching ${this.url}`);
    const full = await this._readFullResponse(res);
    return this._initMemory(full);
  }

  _initMemory(full) {
    if (full.byteLength < 32) throw new Error('truncated nirpak header');
    const dv = new DataView(full.buffer, full.byteOffset, 32);
    const progLen = Number(dv.getBigUint64(16, true));
    this.mediaBase = 32 + progLen;
    this.fullBytes = full;
    this.mode = 'memory';
    return full.subarray(0, this.mediaBase);
  }

  async _readFullResponse(res) {
    const total = Number(res.headers.get('content-length') || 0);
    if (deviceProfile.isMobile && total > 96 * 1024 * 1024) {
      setProgress(15, `服务器未开启断点流式加载，正在下载完整资源 (${Math.round(total / (1024 * 1024))} MB)…`);
    }
    if (!res.body || !total) {
      const buf = await res.arrayBuffer();
      setProgress(88);
      return new Uint8Array(buf);
    }
    const reader = res.body.getReader();
    const chunks = [];
    let loaded = 0;
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      chunks.push(value);
      loaded += value.byteLength;
      setProgress(15 + (loaded / total) * 73);
    }
    const out = new Uint8Array(loaded);
    let offset = 0;
    for (const c of chunks) {
      out.set(c, offset);
      offset += c.byteLength;
    }
    return out;
  }

  trimCache(targetBytes = 0) {
    while (this.rangeCacheBytes > targetBytes && this.rangeCache.size > 0) {
      const oldest = this.rangeCache.keys().next().value;
      const oldVal = this.rangeCache.get(oldest);
      this.rangeCache.delete(oldest);
      this.rangeCacheBytes -= oldVal.byteLength;
    }
  }

  async getAssetBytes(offset, size) {
    if (size === 0) return new Uint8Array(0);
    const start = this.mediaBase + offset;
    const end = start + size;
    if (this.mode === 'file' && this.localFile) {
      return new Uint8Array(await this.localFile.slice(start, end).arrayBuffer());
    }
    if (this.mode === 'memory' && this.fullBytes) {
      // Zero-copy subarray view directly over JS-owned `.nirpak`!
      return this.fullBytes.subarray(start, end);
    }
    const key = `${start}:${size}`;
    const hit = this.rangeCache.get(key);
    if (hit) {
      this.rangeCache.delete(key);
      this.rangeCache.set(key, hit);
      return hit;
    }
    if (this.inflightRanges.has(key)) {
      return this.inflightRanges.get(key);
    }
    const promise = (async () => {
      const res = await fetch(this.url, {
        headers: { Range: `bytes=${start}-${end - 1}` },
      });
      if (!res.ok) throw new Error(`HTTP ${res.status} range ${start}-${end - 1}`);
      const bytes = new Uint8Array(await res.arrayBuffer());
      const view = bytes.byteLength > size ? bytes.subarray(0, size) : bytes;
      this._putRangeCache(key, view);
      return view;
    })().finally(() => {
      this.inflightRanges.delete(key);
    });
    this.inflightRanges.set(key, promise);
    return promise;
  }

  _putRangeCache(key, view) {
    if (view.byteLength <= this.rangeCacheLimit / 2) {
      const prev = this.rangeCache.get(key);
      if (prev) {
        this.rangeCacheBytes -= prev.byteLength;
        this.rangeCache.delete(key);
      }
      this.rangeCache.set(key, view);
      this.rangeCacheBytes += view.byteLength;
      while (this.rangeCacheBytes > this.rangeCacheLimit && this.rangeCache.size > 1) {
        const oldest = this.rangeCache.keys().next().value;
        const oldVal = this.rangeCache.get(oldest);
        this.rangeCache.delete(oldest);
        this.rangeCacheBytes -= oldVal.byteLength;
      }
    }
  }

  /// Coalesces multiple nearby asset byte ranges (e.g. LPM menus, story lookahead bursts,
  /// gallery thumbs) into single sequential HTTP Range requests!
  prefetchBatch(items) {
    if (this.mode !== 'range' || !items || items.length < 2) return;
    const MAX_GAP = 32 * 1024;
    const MAX_WINDOW = 512 * 1024;

    const pending = [];
    const seen = new Set();
    for (const it of items) {
      if (!it || !it.size) continue;
      const start = this.mediaBase + it.offset;
      const key = `${start}:${it.size}`;
      if (seen.has(key) || this.rangeCache.has(key) || this.inflightRanges.has(key)) continue;
      seen.add(key);
      pending.push({ start, end: start + it.size, size: it.size, key });
    }
    if (pending.length < 2) return;
    pending.sort((a, b) => a.start - b.start);

    let i = 0;
    while (i < pending.length) {
      let j = i + 1;
      let winStart = pending[i].start;
      let winEnd = pending[i].end;
      while (
        j < pending.length &&
        pending[j].start - winEnd <= MAX_GAP &&
        Math.max(winEnd, pending[j].end) - winStart <= MAX_WINDOW
      ) {
        winEnd = Math.max(winEnd, pending[j].end);
        j += 1;
      }
      if (j - i >= 2) {
        const group = pending.slice(i, j);
        const batchPromise = (async () => {
          const res = await fetch(this.url, {
            headers: { Range: `bytes=${winStart}-${winEnd - 1}` },
          });
          if (!res.ok) throw new Error(`HTTP ${res.status} batch range ${winStart}-${winEnd - 1}`);
          return new Uint8Array(await res.arrayBuffer());
        })();
        for (const item of group) {
          const relStart = item.start - winStart;
          const p = batchPromise
            .then((buf) => {
              const slice = buf.subarray(relStart, relStart + item.size);
              this._putRangeCache(item.key, slice);
              return slice;
            })
            .finally(() => {
              this.inflightRanges.delete(item.key);
            });
          this.inflightRanges.set(item.key, p);
        }
      }
      i = j;
    }
  }
}

// --- Lightweight IndexedDB persistence with optional same-origin Cloud Sync ---
class SaveStore {
  constructor(dbName, saveApi = null, userPrefsApi = null, onSyncStatus = null) {
    this.dbName = dbName;
    this.saveApi = saveApi ? saveApi.replace(/\/+$/, '') : null;
    this.userPrefsApi = userPrefsApi ? userPrefsApi.replace(/\/+$/, '') : null;
    this.onSyncStatus = onSyncStatus || null;
    this.db = null;
    this.slotIndex = {};
    this.deletedSlots = {}; // slot -> deleted_at_unix
    this.autosaveSummary = null;
    this.quicksaveSummary = null;
    this.thumbnails = new Map(); // 'auto' | 'quick' | slotNumber -> dataUrl
  }
  async open() {
    this.db = await new Promise((resolve, reject) => {
      const req = indexedDB.open(this.dbName, 1);
      req.onupgradeneeded = () => req.result.createObjectStore('kv');
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error);
    });
  }
  _remotePut(key, val, keepalive = false) {
    if (!this.saveApi) return;
    fetch(`${this.saveApi}/${encodeURIComponent(key)}`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: val,
      keepalive,
    })
      .then((res) => {
        if (this.onSyncStatus && (key === 'autosave' || key === 'quicksave' || key.startsWith('slot_'))) {
          this.onSyncStatus(res.ok ? '☁ 云存档已同步' : '云存档同步失败（已本地保存）', !res.ok);
        }
      })
      .catch(() => {
        if (this.onSyncStatus && (key === 'autosave' || key === 'quicksave' || key.startsWith('slot_'))) {
          this.onSyncStatus('离线模式 · 存档已本地暂存', false);
        }
      });
  }
  _remoteDel(key) {
    if (!this.saveApi) return;
    fetch(`${this.saveApi}/${encodeURIComponent(key)}`, {
      method: 'DELETE',
    }).catch(() => {});
  }
  saveGlobalPrefs(prefs) {
    if (!this.userPrefsApi || !prefs) return;
    const shared = {
      master_volume: prefs.master_volume,
      bgm_volume: prefs.bgm_volume,
      voice_volume: prefs.voice_volume,
      sfx_volume: prefs.sfx_volume,
      text_speed: prefs.text_speed,
      font_scale: prefs.font_scale,
      auto_delay_ms: prefs.auto_delay_ms,
      auto_wait_voice: prefs.auto_wait_voice,
      continue_voice_on_advance: prefs.continue_voice_on_advance,
      skip_unread: prefs.skip_unread,
      portrait_reader: prefs.portrait_reader,
      reduced_motion: prefs.reduced_motion,
      bilingual: prefs.bilingual,
      dialogue_opacity: prefs.dialogue_opacity,
      ambient_backdrop: prefs.ambient_backdrop,
      image_sharpening: prefs.image_sharpening,
      text_outline: prefs.text_outline,
    };
    fetch(this.userPrefsApi, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(shared),
    }).catch(() => {});
  }
  async get(key) {
    if (!this.db) return null;
    return new Promise((resolve) => {
      const tx = this.db.transaction('kv', 'readonly');
      const req = tx.objectStore('kv').get(key);
      req.onsuccess = () => resolve(req.result ?? null);
      req.onerror = () => resolve(null);
    });
  }
  async set(key, val, { remote = true, keepalive = false } = {}) {
    if (remote) this._remotePut(key, val, keepalive);
    if (!this.db) return;
    await new Promise((resolve) => {
      const tx = this.db.transaction('kv', 'readwrite');
      tx.objectStore('kv').put(val, key);
      tx.oncomplete = () => resolve();
      tx.onerror = () => resolve();
    });
  }
  async del(key, { remote = true } = {}) {
    if (remote) this._remoteDel(key);
    if (!this.db) return;
    await new Promise((resolve) => {
      const tx = this.db.transaction('kv', 'readwrite');
      tx.objectStore('kv').delete(key);
      tx.oncomplete = () => resolve();
      tx.onerror = () => resolve();
    });
  }
  _injectThumb(snapshotJson, thumbDataUrl) {
    if (!thumbDataUrl) return snapshotJson;
    try {
      const snap = JSON.parse(snapshotJson);
      snap.thumbnail = thumbDataUrl;
      return JSON.stringify(snap);
    } catch {
      return snapshotJson;
    }
  }
  async saveSlot(slot, snapshotJson, thumbDataUrl = null) {
    const payload = this._injectThumb(snapshotJson, thumbDataUrl);
    await this.set(`slot_${slot}`, payload);
    delete this.deletedSlots[slot];
    if (thumbDataUrl) this.thumbnails.set(slot, thumbDataUrl);
    try {
      const snap = JSON.parse(payload);
      this.slotIndex[slot] = {
        slot,
        exists: true,
        saved_at_unix: snap.saved_at_unix ?? 0,
        summary: snap.summary ?? '',
        has_thumb: Boolean(snap.thumbnail),
      };
      await this.set('slots_index', JSON.stringify(this.slotIndex), { remote: false });
      await this.set('deleted_slots', JSON.stringify(this.deletedSlots), { remote: false });
    } catch { /* ignore */ }
  }
  async saveNamed(kind, snapshotJson, thumbDataUrl = null, { keepalive = false } = {}) {
    const payload = this._injectThumb(snapshotJson, thumbDataUrl);
    await this.set(kind, payload, { remote: true, keepalive });
    const thumbKey = kind === 'autosave' ? 'auto' : 'quick';
    if (thumbDataUrl) this.thumbnails.set(thumbKey, thumbDataUrl);
    try {
      const snap = JSON.parse(payload);
      const summary = {
        slot: kind === 'autosave' ? 0xffffffff : 0xfffffffe,
        exists: true,
        saved_at_unix: snap.saved_at_unix ?? 0,
        summary: snap.summary ?? '',
        has_thumb: Boolean(snap.thumbnail),
      };
      if (kind === 'autosave') this.autosaveSummary = summary;
      else this.quicksaveSummary = summary;
    } catch { /* ignore */ }
  }
  async deleteSlot(slot) {
    await this.del(`slot_${slot}`);
    delete this.slotIndex[slot];
    this.thumbnails.delete(slot);
    this.deletedSlots[slot] = Math.floor(Date.now() / 1000);
    await this.set('slots_index', JSON.stringify(this.slotIndex), { remote: false });
    await this.set('deleted_slots', JSON.stringify(this.deletedSlots), { remote: false });
  }
  async _fetchRemoteBundle() {
    if (!this.saveApi) return null;
    try {
      const res = await fetch(`${this.saveApi}/bundle`, { cache: 'no-store' });
      if (!res.ok) return null;
      return await res.json();
    } catch {
      return null;
    }
  }
  async _fetchGlobalPrefs() {
    if (!this.userPrefsApi) return null;
    try {
      const res = await fetch(this.userPrefsApi, { cache: 'no-store' });
      if (!res.ok) return null;
      return await res.json();
    } catch {
      return null;
    }
  }
  static _mergeProfile(localRaw, remoteRaw) {
    if (!remoteRaw) return localRaw;
    if (!localRaw) return remoteRaw;
    try {
      const a = JSON.parse(localRaw) || {};
      const b = JSON.parse(remoteRaw) || {};
      const merged = { ...b, ...a };
      if (Array.isArray(a.read_texts) || Array.isArray(b.read_texts)) {
        merged.read_texts = Array.from(new Set([...(b.read_texts || []), ...(a.read_texts || [])]));
      }
      if (Array.isArray(a.flags) || Array.isArray(b.flags)) {
        merged.flags = Array.from(new Set([...(b.flags || []), ...(a.flags || [])]));
      }
      if (a.values || b.values) {
        merged.values = { ...(b.values || {}), ...(a.values || {}) };
      }
      return JSON.stringify(merged);
    } catch {
      return localRaw;
    }
  }
  async loadInitialState(slotCount = 24) {
    const remotePromise = this._fetchRemoteBundle();
    const globalPrefsPromise = this._fetchGlobalPrefs();
    let prefsRaw = null;
    let profileRaw = null;
    let indexRaw = null;
    let deletedRaw = null;
    let autoRaw = null;
    let quickRaw = null;
    if (this.db) {
      ({ prefsRaw, profileRaw, indexRaw, deletedRaw, autoRaw, quickRaw } = await new Promise((resolve) => {
        const tx = this.db.transaction('kv', 'readonly');
        const os = tx.objectStore('kv');
        const rPrefs = os.get('prefs');
        const rProfile = os.get('profile');
        const rIdx = os.get('slots_index');
        const rDel = os.get('deleted_slots');
        const rAuto = os.get('autosave');
        const rQuick = os.get('quicksave');
        tx.oncomplete = () => resolve({
          prefsRaw: rPrefs.result ?? null,
          profileRaw: rProfile.result ?? null,
          indexRaw: rIdx.result ?? null,
          deletedRaw: rDel.result ?? null,
          autoRaw: rAuto.result ?? null,
          quickRaw: rQuick.result ?? null,
        });
        tx.onerror = () => resolve({
          prefsRaw: null, profileRaw: null, indexRaw: null, deletedRaw: null, autoRaw: null, quickRaw: null,
        });
      }));
    }

    if (deletedRaw) {
      try { this.deletedSlots = JSON.parse(deletedRaw) || {}; } catch { this.deletedSlots = {}; }
    }
    const parseNamed = (raw, key, virtualSlot) => {
      if (!raw) return null;
      try {
        const snap = JSON.parse(raw);
        if (typeof snap.thumbnail === 'string' && snap.thumbnail.startsWith('data:image/')) {
          this.thumbnails.set(key, snap.thumbnail);
        }
        return {
          slot: virtualSlot,
          exists: true,
          saved_at_unix: Number(snap.saved_at_unix ?? 0),
          summary: snap.summary ?? '',
          has_thumb: Boolean(snap.thumbnail),
        };
      } catch {
        return null;
      }
    };
    this.autosaveSummary = parseNamed(autoRaw, 'auto', 0xffffffff);
    this.quicksaveSummary = parseNamed(quickRaw, 'quick', 0xfffffffe);

    if (indexRaw) {
      try {
        this.slotIndex = JSON.parse(indexRaw) || {};
      } catch {
        this.slotIndex = {};
      }
    }
    // Load local slot thumbnails lazily from IDB for slots that exist
    if (this.db) {
      const legacy = await new Promise((resolve) => {
        const tx = this.db.transaction('kv', 'readonly');
        const os = tx.objectStore('kv');
        const reqs = [];
        for (let slot = 0; slot < slotCount; slot++) {
          if (!indexRaw || this.slotIndex[slot]?.exists) {
            reqs.push({ slot, req: os.get(`slot_${slot}`) });
          }
        }
        tx.oncomplete = () => resolve(reqs.map(({ slot, req }) => ({ slot, raw: req.result ?? null })));
        tx.onerror = () => resolve([]);
      });
      for (const { slot, raw } of legacy) {
        if (raw) {
          try {
            const snap = JSON.parse(raw);
            if (typeof snap.thumbnail === 'string' && snap.thumbnail.startsWith('data:image/')) {
              this.thumbnails.set(slot, snap.thumbnail);
            }
            this.slotIndex[slot] = {
              slot,
              exists: true,
              saved_at_unix: snap.saved_at_unix ?? 0,
              summary: snap.summary ?? '',
              has_thumb: Boolean(snap.thumbnail),
            };
          } catch { /* ignore */ }
        }
      }
      if (!indexRaw && Object.keys(this.slotIndex).length > 0) {
        this.set('slots_index', JSON.stringify(this.slotIndex), { remote: false });
      }
    }

    const [remote, globalPrefs] = await Promise.all([remotePromise, globalPrefsPromise]);
    if (remote && typeof remote === 'object') {
      if (typeof remote.prefs === 'string') {
        if (!prefsRaw) {
          prefsRaw = remote.prefs;
          await this.set('prefs', prefsRaw, { remote: false });
        }
      }
      if (typeof remote.profile === 'string') {
        const mergedProfile = SaveStore._mergeProfile(profileRaw, remote.profile);
        if (mergedProfile && mergedProfile !== profileRaw) {
          profileRaw = mergedProfile;
          await this.set('profile', profileRaw, { remote: false });
        }
      }
      if (typeof remote.autosave === 'string') {
        const rAuto = parseNamed(remote.autosave, 'auto', 0xffffffff);
        if (rAuto && (!this.autosaveSummary || rAuto.saved_at_unix > this.autosaveSummary.saved_at_unix)) {
          await this.set('autosave', remote.autosave, { remote: false });
          this.autosaveSummary = rAuto;
        }
      }
      if (typeof remote.quicksave === 'string') {
        const rQuick = parseNamed(remote.quicksave, 'quick', 0xfffffffe);
        if (rQuick && (!this.quicksaveSummary || rQuick.saved_at_unix > this.quicksaveSummary.saved_at_unix)) {
          await this.set('quicksave', remote.quicksave, { remote: false });
          this.quicksaveSummary = rQuick;
        }
      }
      if (remote.slots && typeof remote.slots === 'object') {
        let idxChanged = false;
        for (const [key, raw] of Object.entries(remote.slots)) {
          const m = /^slot_(\d+)$/.exec(key);
          if (!m || typeof raw !== 'string') continue;
          const slot = Number(m[1]);
          try {
            const rSnap = JSON.parse(raw);
            const rTs = Number(rSnap.saved_at_unix ?? 0);
            const delTs = Number(this.deletedSlots[slot] ?? 0);
            if (delTs > rTs) continue; // Respect newer local deletion tombstone
            const lTs = Number(this.slotIndex[slot]?.saved_at_unix ?? -1);
            if (!this.slotIndex[slot]?.exists || rTs > lTs) {
              await this.set(`slot_${slot}`, raw, { remote: false });
              if (typeof rSnap.thumbnail === 'string' && rSnap.thumbnail.startsWith('data:image/')) {
                this.thumbnails.set(slot, rSnap.thumbnail);
              }
              this.slotIndex[slot] = {
                slot,
                exists: true,
                saved_at_unix: rTs,
                summary: rSnap.summary ?? '',
                has_thumb: Boolean(rSnap.thumbnail),
              };
              idxChanged = true;
            }
          } catch { /* ignore corrupt remote slot */ }
        }
        if (idxChanged) {
          await this.set('slots_index', JSON.stringify(this.slotIndex), { remote: false });
        }
      }
    }

    // Inherit user-level global preferences if this game has never saved custom prefs yet!
    if (!prefsRaw && globalPrefs && typeof globalPrefs === 'object' && Object.keys(globalPrefs).length > 0) {
      prefsRaw = JSON.stringify(globalPrefs);
    }

    const slots = [];
    for (let slot = 0; slot < slotCount; slot++) {
      slots.push(this.slotIndex[slot] ?? { slot, exists: false, saved_at_unix: 0, summary: '', has_thumb: false });
    }
    return {
      prefsJson: prefsRaw ?? '{}',
      profileJson: profileRaw ?? '{}',
      slots,
      autosave: this.autosaveSummary,
      quicksave: this.quicksaveSummary,
    };
  }
}

// --- Zero-Reencode Ogg Opus -> CoreAudio CAF v1 Remuxer (enables native Opus on iOS Safari!) ---
function isOggOpusPayload(mime, bytes) {
  if (mime && mime.toLowerCase().includes('opus')) return true;
  if (!bytes || bytes.byteLength < 36) return false;
  if (bytes[0] !== 0x4f || bytes[1] !== 0x67 || bytes[2] !== 0x67 || bytes[3] !== 0x53) {
    return false;
  }
  const lim = Math.min(bytes.byteLength - 8, 64);
  for (let i = 26; i <= lim; i++) {
    if (
      bytes[i] === 0x4f && bytes[i + 1] === 0x70 && bytes[i + 2] === 0x75 && bytes[i + 3] === 0x73 &&
      bytes[i + 4] === 0x48 && bytes[i + 5] === 0x65 && bytes[i + 6] === 0x61 && bytes[i + 7] === 0x64
    ) {
      return true;
    }
  }
  return false;
}

function opusPacketFrameCount48k(pkt) {
  if (!pkt || pkt.byteLength === 0) return 960;
  const toc = pkt[0];
  const cfg = toc >> 3;
  let frameSamples;
  if (cfg < 12) {
    frameSamples = [480, 960, 1920, 2880][cfg & 3];
  } else if (cfg < 16) {
    frameSamples = [480, 960][cfg & 1];
  } else {
    frameSamples = [120, 240, 480, 960][cfg & 3];
  }
  const code = toc & 3;
  let frames;
  if (code === 0) {
    frames = 1;
  } else if (code === 1 || code === 2) {
    frames = 2;
  } else {
    frames = pkt.byteLength >= 2 ? (pkt[1] & 0x3f) : 1;
  }
  return frameSamples * Math.max(1, frames);
}

function writeCafVarInt(out, value) {
  let v = value >>> 0;
  const tmp = [];
  tmp.push(v & 0x7f);
  v >>>= 7;
  while (v > 0) {
    tmp.push((v & 0x7f) | 0x80);
    v >>>= 7;
  }
  for (let i = tmp.length - 1; i >= 0; i--) {
    out.push(tmp[i]);
  }
}

function oggOpusToCaf(oggBytes) {
  if (!oggBytes || oggBytes.byteLength < 36) return null;
  const dv = new DataView(oggBytes.buffer, oggBytes.byteOffset, oggBytes.byteLength);
  let pos = 0;
  const len = oggBytes.byteLength;

  const packets = [];
  let pendingParts = [];
  let pendingLen = 0;
  let lastGranule = 0n;

  const flushPacket = () => {
    if (pendingParts.length === 1) {
      packets.push(pendingParts[0]);
    } else if (pendingParts.length > 1) {
      const merged = new Uint8Array(pendingLen);
      let off = 0;
      for (const p of pendingParts) {
        merged.set(p, off);
        off += p.byteLength;
      }
      packets.push(merged);
    }
    pendingParts = [];
    pendingLen = 0;
  };

  while (pos + 27 <= len) {
    if (
      oggBytes[pos] !== 0x4f ||
      oggBytes[pos + 1] !== 0x67 ||
      oggBytes[pos + 2] !== 0x67 ||
      oggBytes[pos + 3] !== 0x53
    ) {
      break;
    }
    const headerType = oggBytes[pos + 5];
    const granule = dv.getBigUint64(pos + 6, true);
    const numSegs = oggBytes[pos + 26];
    const segStart = pos + 27;
    const bodyStart = segStart + numSegs;
    if (bodyStart > len) break;

    if ((headerType & 0x01) === 0 && pendingLen > 0) {
      flushPacket();
    }
    let bodyOff = bodyStart;
    for (let s = 0; s < numSegs; s++) {
      const segLen = oggBytes[segStart + s];
      if (bodyOff + segLen > len) return null;
      if (segLen > 0) {
        pendingParts.push(oggBytes.subarray(bodyOff, bodyOff + segLen));
        pendingLen += segLen;
      }
      bodyOff += segLen;
      if (segLen < 255) {
        flushPacket();
      }
    }
    if (granule !== 0xffffffffffffffffn && packets.length >= 2) {
      lastGranule = granule;
    }
    pos = bodyOff;
  }
  if (pendingLen > 0) flushPacket();
  if (packets.length < 3) return null;

  const head = packets[0];
  if (
    head.byteLength < 19 ||
    head[0] !== 0x4f || head[1] !== 0x70 || head[2] !== 0x75 || head[3] !== 0x73 ||
    head[4] !== 0x48 || head[5] !== 0x65 || head[6] !== 0x61 || head[7] !== 0x64
  ) {
    return null;
  }
  const channels = head[9];
  const preSkip = head[10] | (head[11] << 8);
  if (channels < 1 || channels > 2) return null;

  const audioPkts = packets.slice(2);
  let totalPayload = 0;
  let totalSamples = 0;
  let constFrames = opusPacketFrameCount48k(audioPkts[0]);
  const pktSamples = new Int32Array(audioPkts.length);
  for (let i = 0; i < audioPkts.length; i++) {
    const p = audioPkts[i];
    totalPayload += p.byteLength;
    const f = opusPacketFrameCount48k(p);
    pktSamples[i] = f;
    totalSamples += f;
    if (f !== constFrames) constFrames = 0;
  }

  let validFrames = totalSamples - preSkip;
  if (lastGranule > BigInt(preSkip)) {
    const gValid = Number(lastGranule - BigInt(preSkip));
    if (gValid > 0 && gValid <= totalSamples - preSkip) {
      validFrames = gValid;
    }
  }
  validFrames = Math.max(0, validFrames);
  const remainderFrames = Math.max(0, totalSamples - preSkip - validFrames);

  const paktVarInts = [];
  for (let i = 0; i < audioPkts.length; i++) {
    writeCafVarInt(paktVarInts, audioPkts[i].byteLength);
    if (constFrames === 0) {
      writeCafVarInt(paktVarInts, pktSamples[i]);
    }
  }

  // CAF v1 layout: file header (8) + desc (12+32) + pakt (12+24+varints) + data (12+4+payload)
  const totalSize = 8 + (12 + 32) + (12 + 24 + paktVarInts.length) + (12 + 4 + totalPayload);
  const out = new Uint8Array(totalSize);
  const view = new DataView(out.buffer);
  let w = 0;
  const putFourCC = (s) => {
    out[w++] = s.charCodeAt(0);
    out[w++] = s.charCodeAt(1);
    out[w++] = s.charCodeAt(2);
    out[w++] = s.charCodeAt(3);
  };

  // File header: 'caff', version=1, flags=0
  putFourCC('caff');
  view.setUint16(w, 1, false); w += 2;
  view.setUint16(w, 0, false); w += 2;

  // 'desc' chunk (32 bytes)
  putFourCC('desc');
  view.setBigInt64(w, 32n, false); w += 8;
  view.setFloat64(w, 48000.0, false); w += 8;
  putFourCC('opus');
  view.setUint32(w, 0, false); w += 4; // formatFlags
  view.setUint32(w, 0, false); w += 4; // bytesPerPacket (VBR)
  view.setUint32(w, constFrames, false); w += 4; // framesPerPacket (960 or 0 if variable)
  view.setUint32(w, channels, false); w += 4; // channelsPerFrame
  view.setUint32(w, 0, false); w += 4; // bitsPerChannel

  // 'pakt' chunk
  putFourCC('pakt');
  view.setBigInt64(w, BigInt(24 + paktVarInts.length), false); w += 8;
  view.setBigInt64(w, BigInt(audioPkts.length), false); w += 8;
  view.setBigInt64(w, BigInt(validFrames), false); w += 8;
  view.setInt32(w, preSkip, false); w += 4;
  view.setInt32(w, remainderFrames, false); w += 4;
  for (let i = 0; i < paktVarInts.length; i++) {
    out[w++] = paktVarInts[i];
  }

  // 'data' chunk
  putFourCC('data');
  view.setBigInt64(w, BigInt(4 + totalPayload), false); w += 8;
  view.setUint32(w, 0, false); w += 4; // editCount
  for (let i = 0; i < audioPkts.length; i++) {
    out.set(audioPkts[i], w);
    w += audioPkts[i].byteLength;
  }

  return out;
}

// --- WebAudio API Mixer with Mobile Streaming BGM, iOS Opus-in-CAF Remux & Bounded LRU ---
class WebAudioMixer {
  constructor(decodedLimit, onCodecUnsupported) {
    this.ctx = null;
    this.master = null;
    this.buses = {};
    this.channels = new Map();
    this.decodedCache = new Map(); // assetId -> { buf: AudioBuffer, bytes: number }
    this.decodedBytes = 0;
    this.decodedLimit = decodedLimit || 96 * 1024 * 1024;
    this.inflightDecode = new Map();
    this.hostSuspended = false;
    this.preferCafOpus = deviceProfile.isIOS;
    this.warnedOgg = false;
    this.onCodecUnsupported = onCodecUnsupported || null;
    this.prefs = { master_volume: 0.9, bgm_volume: 0.75, voice_volume: 1.0, sfx_volume: 0.85 };
  }

  ensureContext() {
    if (!this.ctx) {
      const Ctx = window.AudioContext || window.webkitAudioContext;
      if (!Ctx) return null;
      this.ctx = new Ctx();
      this.master = this.ctx.createGain();
      this.master.connect(this.ctx.destination);
      for (const name of ['bgm', 'voice', 'sfx']) {
        const g = this.ctx.createGain();
        g.connect(this.master);
        this.buses[name] = g;
      }
      this.applyPreferences(this.prefs);
    }
    // Check `!== 'running'` (not just `'suspended'`) so iOS Safari `'interrupted'` state also recovers!
    if (!this.hostSuspended && this.ctx.state !== 'running') {
      this.ctx.resume().catch(() => {});
    }
    // Also kick any streaming HTMLAudioElement that was blocked by autoplay before first gesture
    if (!this.hostSuspended) {
      for (const ch of this.channels.values()) {
        if (ch.mediaEl && ch.pendingPlay && ch.mediaEl.paused) {
          ch.mediaEl.play().then(() => { ch.pendingPlay = false; }).catch(() => {});
        }
      }
    }
    return this.ctx;
  }

  suspendAll() {
    this.hostSuspended = true;
    for (const ch of this.channels.values()) {
      if (ch.mediaEl && !ch.mediaEl.paused) {
        ch.wasPlayingBeforeSuspend = true;
        ch.mediaEl.pause();
      }
    }
    if (this.ctx && this.ctx.state === 'running') {
      this.ctx.suspend().catch(() => {});
    }
  }

  resumeAll() {
    this.hostSuspended = false;
    if (this.ctx && this.ctx.state !== 'running') {
      this.ctx.resume().catch(() => {});
    }
    for (const ch of this.channels.values()) {
      if (ch.mediaEl && (ch.wasPlayingBeforeSuspend || ch.pendingPlay)) {
        ch.wasPlayingBeforeSuspend = false;
        ch.mediaEl.play().then(() => { ch.pendingPlay = false; }).catch(() => {});
      }
    }
  }

  applyPreferences(p) {
    this.prefs = p;
    if (!this.ctx) return;
    const now = this.ctx.currentTime;
    this.master.gain.setTargetAtTime(p.master_volume ?? 0.9, now, 0.02);
    this.buses.bgm.gain.setTargetAtTime(p.bgm_volume ?? 0.75, now, 0.02);
    this.buses.voice.gain.setTargetAtTime(p.voice_volume ?? 1.0, now, 0.02);
    this.buses.sfx.gain.setTargetAtTime(p.sfx_volume ?? 0.85, now, 0.02);
  }

  _putDecoded(assetId, audioBuf) {
    const bytes = audioBuf.length * audioBuf.numberOfChannels * 4;
    if (bytes > this.decodedLimit * 0.65) {
      // Avoid evicting the entire voice/SFX cache for a single giant decoded track
      return;
    }
    const prev = this.decodedCache.get(assetId);
    if (prev) {
      this.decodedBytes -= prev.bytes;
      this.decodedCache.delete(assetId);
    }
    this.decodedCache.set(assetId, { buf: audioBuf, bytes });
    this.decodedBytes += bytes;
    this._evictLru(assetId);
  }

  _evictLru(keepAsset) {
    if (this.decodedBytes <= this.decodedLimit) return;
    const activeAssets = new Set([keepAsset]);
    for (const ch of this.channels.values()) {
      activeAssets.add(ch.asset);
    }
    for (const [id, entry] of this.decodedCache) {
      if (this.decodedBytes <= this.decodedLimit) break;
      if (activeAssets.has(id)) continue;
      this.decodedCache.delete(id);
      this.decodedBytes -= entry.bytes;
    }
  }

  async _decodeAsset(ctx, assetId, mime, fetchBytes) {
    const hit = this.decodedCache.get(assetId);
    if (hit) {
      this.decodedCache.delete(assetId);
      this.decodedCache.set(assetId, hit);
      return hit.buf;
    }
    if (this.inflightDecode.has(assetId)) {
      return this.inflightDecode.get(assetId);
    }
    const task = (async () => {
      const bytes = await fetchBytes();
      if (!bytes || bytes.byteLength === 0) return null;
      const isOpus = isOggOpusPayload(mime, bytes);
      let audioBuf = null;
      if (this.preferCafOpus && isOpus) {
        const caf = oggOpusToCaf(bytes);
        if (caf) {
          try {
            audioBuf = await ctx.decodeAudioData(caf.buffer);
          } catch { /* fall through to native decode */ }
        }
      }
      if (!audioBuf) {
        const copy = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
        try {
          audioBuf = await ctx.decodeAudioData(copy);
        } catch (nativeErr) {
          if (isOpus) {
            const caf = oggOpusToCaf(bytes);
            if (caf) {
              audioBuf = await ctx.decodeAudioData(caf.buffer);
              this.preferCafOpus = true;
            } else {
              throw nativeErr;
            }
          } else {
            throw nativeErr;
          }
        }
      }
      this._putDecoded(assetId, audioBuf);
      return audioBuf;
    })()
      .catch(() => {
        if (!this.warnedOgg && mime && mime.startsWith('audio/ogg') && this.onCodecUnsupported) {
          this.warnedOgg = true;
          this.onCodecUnsupported(mime);
        }
        return null;
      })
      .finally(() => {
        this.inflightDecode.delete(assetId);
      });
    this.inflightDecode.set(assetId, task);
    return task;
  }

  preload(assetId, mime, size, fetchBytes) {
    // Skip pre-decoding large BGM tracks into float32 PCM on mobile!
    if (deviceProfile.isMobile && size > 320 * 1024) return;
    if (!this.ctx || this.decodedCache.has(assetId) || this.inflightDecode.has(assetId)) return;
    this._decodeAsset(this.ctx, assetId, mime, fetchBytes);
  }

  _canStreamMediaElement(cmd) {
    if (!deviceProfile.isMobile) return false;
    if (cmd.bus !== 'bgm' || cmd.size < 256 * 1024) return false;
    // Sample-accurate intro+loop regions still use WebAudio AudioBufferSourceNode
    if (cmd.looped && cmd.loop_start_s != null && cmd.loop_end_s != null && cmd.loop_end_s > cmd.loop_start_s) {
      return false;
    }
    const probe = document.createElement('audio');
    if (!probe.canPlayType) return false;
    if (this.preferCafOpus && cmd.mime && cmd.mime.toLowerCase().includes('opus')) {
      return Boolean(probe.canPlayType('audio/x-caf'));
    }
    return Boolean(probe.canPlayType(cmd.mime || 'audio/mpeg'));
  }

  async _playStreamingBgm(ctx, cmd, playToken, fetchBytes, onEnded) {
    const bytes = await fetchBytes();
    const slot = this.channels.get(cmd.channel);
    if (!bytes || bytes.byteLength === 0 || !slot || slot.pending !== playToken) return false;

    let blobBytes = bytes;
    let blobMime = cmd.mime || 'audio/mpeg';
    if (this.preferCafOpus && isOggOpusPayload(cmd.mime, bytes)) {
      const caf = oggOpusToCaf(bytes);
      if (caf) {
        blobBytes = caf;
        blobMime = 'audio/x-caf';
      }
    }
    const blobUrl = URL.createObjectURL(new Blob([blobBytes], { type: blobMime }));
    const el = new Audio();
    el.preload = 'auto';
    el.loop = Boolean(cmd.looped);
    el.src = blobUrl;

    let mediaNode;
    try {
      mediaNode = ctx.createMediaElementSource(el);
    } catch {
      URL.revokeObjectURL(blobUrl);
      return false;
    }

    const gainNode = ctx.createGain();
    const targetGain = Math.max(0, cmd.gain ?? 1.0);
    const now = ctx.currentTime;
    if (cmd.fade_in_ms > 0) {
      gainNode.gain.setValueAtTime(0.0001, now);
      gainNode.gain.linearRampToValueAtTime(targetGain, now + cmd.fade_in_ms / 1000);
    } else {
      gainNode.gain.setValueAtTime(targetGain, now);
    }
    mediaNode.connect(gainNode);
    gainNode.connect(this.buses[cmd.bus] ?? this.master);

    const entry = {
      mediaEl: el,
      mediaNode,
      blobUrl,
      gainNode,
      bus: cmd.bus,
      asset: cmd.asset,
      pendingPlay: false,
      wasPlayingBeforeSuspend: false,
    };
    el.onended = () => {
      const cur = this.channels.get(cmd.channel);
      if (cur && cur.mediaEl === el) {
        this.channels.delete(cmd.channel);
        URL.revokeObjectURL(blobUrl);
        try { mediaNode.disconnect(); } catch {}
        onEnded(cmd.channel);
      }
    };
    this.channels.set(cmd.channel, entry);
    if (!this.hostSuspended) {
      el.play().catch(() => {
        entry.pendingPlay = true;
      });
    } else {
      entry.wasPlayingBeforeSuspend = true;
    }
    return true;
  }

  async play(cmd, fetchBytes, onEnded) {
    const ctx = this.ensureContext();
    if (!ctx) return;
    this.stop(cmd.channel, 0);
    const playToken = Symbol('play');
    this.channels.set(cmd.channel, { pending: playToken, asset: cmd.asset, bus: cmd.bus });

    if (this._canStreamMediaElement(cmd)) {
      const ok = await this._playStreamingBgm(ctx, cmd, playToken, fetchBytes, onEnded);
      if (ok) return;
      const slot = this.channels.get(cmd.channel);
      if (!slot || slot.pending !== playToken) return;
    }

    const audioBuf = await this._decodeAsset(ctx, cmd.asset, cmd.mime, fetchBytes);
    const slot = this.channels.get(cmd.channel);
    if (!audioBuf || !slot || slot.pending !== playToken) return;

    const src = ctx.createBufferSource();
    src.buffer = audioBuf;
    src.loop = Boolean(cmd.looped);
    if (cmd.looped && cmd.loop_start_s != null && cmd.loop_end_s != null && cmd.loop_end_s > cmd.loop_start_s) {
      src.loopStart = cmd.loop_start_s;
      src.loopEnd = cmd.loop_end_s;
    }
    const gainNode = ctx.createGain();
    const targetGain = Math.max(0, cmd.gain ?? 1.0);
    const now = ctx.currentTime;
    if (cmd.fade_in_ms > 0) {
      gainNode.gain.setValueAtTime(0.0001, now);
      gainNode.gain.linearRampToValueAtTime(targetGain, now + cmd.fade_in_ms / 1000);
    } else {
      gainNode.gain.setValueAtTime(targetGain, now);
    }
    src.connect(gainNode);
    gainNode.connect(this.buses[cmd.bus] ?? this.master);
    src.onended = () => {
      const cur = this.channels.get(cmd.channel);
      if (cur && cur.src === src) {
        this.channels.delete(cmd.channel);
        onEnded(cmd.channel);
      }
    };
    this.channels.set(cmd.channel, { src, gainNode, bus: cmd.bus, asset: cmd.asset });
    src.start(0);
  }

  stop(channel, fadeOutMs) {
    const cur = this.channels.get(channel);
    if (!cur || !this.ctx) return;
    this.channels.delete(channel);
    if (cur.mediaEl) {
      const cleanupMedia = () => {
        try {
          cur.mediaEl.pause();
          cur.mediaEl.src = '';
          cur.mediaNode?.disconnect();
        } catch {}
        if (cur.blobUrl) URL.revokeObjectURL(cur.blobUrl);
      };
      if (fadeOutMs > 0 && cur.gainNode) {
        const now = this.ctx.currentTime;
        cur.gainNode.gain.cancelScheduledValues(now);
        cur.gainNode.gain.setValueAtTime(cur.gainNode.gain.value, now);
        cur.gainNode.gain.linearRampToValueAtTime(0.0001, now + fadeOutMs / 1000);
        setTimeout(cleanupMedia, fadeOutMs + 40);
      } else {
        cleanupMedia();
      }
      return;
    }
    if (!cur.src) return;
    const now = this.ctx.currentTime;
    try {
      if (fadeOutMs > 0) {
        cur.gainNode.gain.cancelScheduledValues(now);
        cur.gainNode.gain.setValueAtTime(cur.gainNode.gain.value, now);
        cur.gainNode.gain.linearRampToValueAtTime(0.0001, now + fadeOutMs / 1000);
        cur.src.stop(now + fadeOutMs / 1000 + 0.02);
      } else {
        cur.src.stop();
      }
    } catch { /* already stopped */ }
  }

  setGain(channel, gain) {
    const cur = this.channels.get(channel);
    if (cur && cur.gainNode && this.ctx) {
      cur.gainNode.gain.setTargetAtTime(gain, this.ctx.currentTime, 0.02);
    }
  }

  clearCache() {
    this.decodedCache.clear();
    this.decodedBytes = 0;
  }
}

// --- Optional PWA Service Worker Registration ---
function registerServiceWorker(cfg) {
  if (cfg.disableSw || !('serviceWorker' in navigator)) return;
  const secure = location.protocol === 'https:' || location.hostname === 'localhost' || location.hostname === '127.0.0.1';
  if (!secure) return;
  const swUrl = cfg.swUrl
    ? new URL(cfg.swUrl, location.href).href
    : new URL('./sw.js', import.meta.url).href;
  navigator.serviceWorker.register(swUrl).catch(() => {});
}

async function fetchWithCacheApi(url) {
  try {
    if (typeof caches !== 'undefined') {
      const cache = await caches.open('nir-fonts-v1');
      const hit = await cache.match(url);
      if (hit && hit.ok) {
        return new Uint8Array(await hit.arrayBuffer());
      }
      const res = await fetch(url);
      if (res.ok) {
        cache.put(url, res.clone()).catch(() => {});
        return new Uint8Array(await res.arrayBuffer());
      }
      return null;
    }
  } catch { /* ignore Cache API error */ }
  const res = await fetch(url);
  if (!res.ok) return null;
  return new Uint8Array(await res.arrayBuffer());
}

async function main() {
  const cfg = globalThis.__NIR_CONFIG__ || {};
  if (loaderEl && cfg.coverUrl) {
    loaderEl.style.setProperty('--loader-cover', `url("${cfg.coverUrl}")`);
    loaderEl.classList.add('has-cover');
  }
  setProgress(5, cfg.gameTitle ? `正在载入 ${cfg.gameTitle}…` : 'Initializing NIR Engine…');
  if (typeof globalThis.WebAssembly === 'undefined') {
    throw new ReferenceError('WebAssembly is not defined');
  }
  registerServiceWorker(cfg);

  const qs = new URLSearchParams(location.search);
  const pakUrl = cfg.pakUrl || qs.get('pak') || qs.get('file') || './game.nirpak';
  const wasmJsUrl = cfg.wasmJsUrl
    ? new URL(cfg.wasmJsUrl, location.href).href
    : new URL('./player_web.js', import.meta.url).href;
  const wasmBinUrl = cfg.wasmUrl ? new URL(cfg.wasmUrl, location.href).href : null;
  const gameKey = cfg.gameId || qs.get('game') || null;
  const saveApi = cfg.saveApi || qs.get('save_api') || null;
  const userPrefsApi = cfg.userPrefsApi || null;
  const shelfUrl = cfg.shelfUrl || null;
  const translationsApi = cfg.translationsApi || null;
  const wantResume = qs.get('resume') === '1' || qs.get('resume') === 'true';
  const wantLocal = qs.get('local') === '1';

  // Prefer OPFS offline `.nirpak` or local dragged/launched `.nirpak` for zero-RAM instant slicing!
  const slugMatch = /\/games\/([^/?#]+)\/game\.nirpak/.exec(pakUrl);
  const pakSlug = cfg.slug || (slugMatch ? decodeURIComponent(slugMatch[1]) : null);
  let localPakFile = null;
  if (wantLocal) {
    localPakFile = await consumeLocalPendingPakFile();
  } else if (pakSlug) {
    localPakFile = await openOpfsPakFile(pakSlug);
  }
  if (navigator.storage?.persist) {
    navigator.storage.persist().catch(() => {});
  }

  let pak = new PakSource(pakUrl, deviceProfile.rangeCacheLimit, localPakFile);
  const store = new SaveStore(
    localPakFile
      ? `nir_saves_v2:local:${localPakFile.name}`
      : (gameKey ? `nir_saves_v2:${gameKey}` : 'nir_saves_v2'),
    localPakFile ? null : saveApi,
    localPakFile ? null : userPrefsApi,
    showSyncPill,
  );

  const [{ default: initWasm, WebEngine }, headerBytes, initialSaves] = await Promise.all([
    import(/* @vite-ignore */ wasmJsUrl),
    pak.open(),
    store.open().then(() => store.loadInitialState(24)).catch(() => ({
      prefsJson: '{}', profileJson: '{}', slots: [], autosave: null, quicksave: null,
    })),
  ]);
  if (wasmBinUrl) {
    await initWasm({ module_or_path: wasmBinUrl });
  } else {
    await initWasm();
  }

  setProgress(92, 'Starting GPU renderer…');
  const initialView = readViewportMetrics();
  canvas.width = Math.max(1, Math.round(initialView.width * initialView.dpr));
  canvas.height = Math.max(1, Math.round(initialView.height * initialView.dpr));

  const engine = await WebEngine.create(
    canvas,
    headerBytes,
    initialSaves.prefsJson,
    initialSaves.profileJson,
    JSON.stringify(initialSaves.slots),
    initialView.dpr,
  );
  document.title = engine.title();

  // Apply adaptive mobile GPU cache cap & initial safe-area insets + touch modality
  if (deviceProfile.gpuCacheMb > 0) {
    engine.set_texture_cache_limit_mb(deviceProfile.gpuCacheMb);
  }
  engine.set_safe_insets(
    initialView.insets.top,
    initialView.insets.right,
    initialView.insets.bottom,
    initialView.insets.left,
  );
  if (deviceProfile.isMobile) {
    engine.set_input_mode(true);
  }
  engine.set_host_has_shelf(Boolean(shelfUrl));
  engine.set_tz_offset_mins(-new Date().getTimezoneOffset());
  if (initialSaves.autosave) {
    engine.set_autosave_summary_json(JSON.stringify(initialSaves.autosave));
  }
  if (initialSaves.quicksave) {
    engine.set_quicksave_summary_json(JSON.stringify(initialSaves.quicksave));
  }

  // Coordinate DOM corner chrome clearance with Canvas HUD capsules so they never collide!
  const syncChromePads = () => {
    const leftPad = shelfBtn && !shelfBtn.hidden ? Math.ceil(shelfBtn.offsetWidth || 78) + 12 : 0;
    const fsVis = Boolean(fsBtn && !fsBtn.hidden);
    if (quickToolsEl) {
      quickToolsEl.classList.toggle('shifted-left', fsVis);
    }
    const qtVis = Boolean(quickToolsEl && !quickToolsEl.hidden);
    const rightPad =
      (fsVis ? Math.ceil(fsBtn.offsetWidth || 40) + 12 : 0) +
      (qtVis ? Math.ceil(quickToolsEl.offsetWidth || 84) + 12 : 0);
    engine.set_host_top_chrome_pads(leftPad, rightPad);
  };

  // Optional back-to-shelf button when launched from NIR Shelf
  if (shelfBtn && shelfUrl) {
    shelfBtn.hidden = false;
    shelfBtn.addEventListener('click', () => {
      try {
        dispatchCmdJson(engine.emit_autosave());
        dispatchCmdJson(engine.flush_pending_saves());
      } catch { /* ignore */ }
      location.href = shelfUrl;
    });
  }
  syncChromePads();

  const fallbackFontUrls = (() => {
    const out = [];
    if (Array.isArray(cfg.fallbackFonts)) {
      for (const u of cfg.fallbackFonts) {
        if (typeof u === 'string' && u) out.push(new URL(u, location.href).href);
      }
    }
    if (typeof cfg.fallbackFontUrl === 'string' && cfg.fallbackFontUrl) {
      const href = new URL(cfg.fallbackFontUrl, location.href).href;
      if (!out.includes(href)) out.push(href);
    }
    if (out.length === 0) {
      out.push(new URL('./fallback-cjk.otf', import.meta.url).href);
    }
    return out;
  })();

  let fallbackFontStarted = false;
  let fallbackFontsLoaded = 0;
  let cachedFallbackBlobs = [];
  const loadFallbackFontsLazy = async () => {
    if (fallbackFontStarted) return;
    fallbackFontStarted = true;
    const loaded = [];
    for (const url of fallbackFontUrls) {
      try {
        const bytes = await fetchWithCacheApi(url);
        if (bytes && bytes.byteLength >= 256) {
          loaded.push(bytes);
        }
      } catch { /* optional fallback font */ }
    }
    cachedFallbackBlobs = loaded;
    for (const bytes of loaded) {
      try {
        engine.load_fallback_font(bytes);
        fallbackFontsLoaded += 1;
      } catch { /* ignore invalid font */ }
    }
    if (globalThis.__nir) {
      globalThis.__nir.fallbackFonts = fallbackFontsLoaded;
    }
    if (loaded.length > 0) {
      scheduleFrame();
    }
  };

  const loadAllFonts = async () => {
    const fonts = JSON.parse(engine.font_assets_json() || '[]');
    let bundledCount = 0;
    await Promise.all(fonts.map(async (f) => {
      try {
        const bytes = await pak.getAssetBytes(f.offset, f.size);
        engine.load_font(f.id, bytes);
        bundledCount += 1;
      } catch { /* ignore */ }
    }));
    return bundledCount;
  };
  const bundledFontCount = await loadAllFonts();
  if (bundledFontCount === 0) {
    // Only fetch the shared Pan-CJK fallback font eagerly when `.nirpak` bundles no font!
    // When bundled fonts exist, we save 6.1 MiB on mobile and only lazy-load if a missing glyph appears.
    setProgress(96, 'Loading CJK font…');
    await loadFallbackFontsLazy();
  }

  const audio = new WebAudioMixer(deviceProfile.audioDecodedLimit, () => {
    engine.show_toast('当前浏览器不支持 OGG/Opus 音频，建议使用 MP3 音频构建', 4500);
    scheduleFrame();
  });
  try { audio.applyPreferences(JSON.parse(initialSaves.prefsJson)); } catch { /* default */ }

  // Screen Wake Lock during Auto/Skip reading on mobile
  let wakeLockSentinel = null;
  let wakeLockPending = false;
  const syncWakeLock = () => {
    if (!navigator.wakeLock || document.visibilityState !== 'visible') {
      if (wakeLockSentinel) {
        wakeLockSentinel.release().catch(() => {});
        wakeLockSentinel = null;
      }
      return;
    }
    const want = engine.wants_wake_lock();
    if (want && !wakeLockSentinel && !wakeLockPending) {
      wakeLockPending = true;
      navigator.wakeLock
        .request('screen')
        .then((s) => {
          wakeLockSentinel = s;
          s.addEventListener('release', () => {
            if (wakeLockSentinel === s) wakeLockSentinel = null;
          });
        })
        .catch(() => {})
        .finally(() => {
          wakeLockPending = false;
        });
    } else if (!want && wakeLockSentinel) {
      wakeLockSentinel.release().catch(() => {});
      wakeLockSentinel = null;
    }
  };

  // Subtle mobile fullscreen / landscape button (hidden when fullscreen or unsupported)
  const syncFullscreenButton = () => {
    if (!fsBtn) return;
    const canFs = Boolean(
      deviceProfile.isMobile &&
        document.fullscreenEnabled &&
        document.documentElement.requestFullscreen,
    );
    const inFs = Boolean(document.fullscreenElement);
    fsBtn.hidden = !canFs || inFs;
    syncChromePads();
  };
  if (fsBtn) {
    fsBtn.addEventListener('click', async () => {
      audio.ensureContext();
      try {
        await document.documentElement.requestFullscreen();
        if (screen.orientation && screen.orientation.lock) {
          screen.orientation.lock('landscape').catch(() => {});
        }
      } catch { /* ignore */ }
      syncFullscreenButton();
    });
    document.addEventListener('fullscreenchange', syncFullscreenButton);
    syncFullscreenButton();
  }

  // Priority image decode scheduler
  const inflightImages = new Set();
  const prefetchQueue = [];
  let activePrefetches = 0;
  const MAX_BG_PREFETCH = deviceProfile.maxBgPrefetch;
  const baseBitmapCache = new Map(); // baseId -> Promise<ImageBitmap>
  const MAX_BASE_BITMAPS = deviceProfile.maxBaseBitmaps;

  const clearBaseBitmapCache = () => {
    for (const p of baseBitmapCache.values()) {
      p.then((b) => b.close()).catch(() => {});
    }
    baseBitmapCache.clear();
  };

  const getBaseBitmap = (baseId, offset, size, mime) => {
    const hit = baseBitmapCache.get(baseId);
    if (hit) {
      baseBitmapCache.delete(baseId);
      baseBitmapCache.set(baseId, hit);
      return hit;
    }
    const p = pak.getAssetBytes(offset, size).then((bytes) => {
      const blob = new Blob([bytes], { type: mime });
      return createImageBitmap(blob, { premultiplyAlpha: 'none' });
    });
    baseBitmapCache.set(baseId, p);
    if (baseBitmapCache.size > MAX_BASE_BITMAPS) {
      const oldestKey = baseBitmapCache.keys().next().value;
      const oldestP = baseBitmapCache.get(oldestKey);
      baseBitmapCache.delete(oldestKey);
      oldestP.then((b) => b.close()).catch(() => {});
    }
    p.catch(() => baseBitmapCache.delete(baseId));
    return p;
  };

  const compositeDeltaBitmap = async (baseBmp, patchBmp, delta) => {
    const w = baseBmp.width;
    const h = baseBmp.height;
    const cvs = typeof OffscreenCanvas !== 'undefined'
      ? new OffscreenCanvas(w, h)
      : Object.assign(document.createElement('canvas'), { width: w, height: h });
    const ctx = cvs.getContext('2d');
    ctx.globalCompositeOperation = 'copy';
    ctx.drawImage(baseBmp, 0, 0);

    const mask = delta.tile_mask;
    if (!mask || mask.length === 0) {
      ctx.globalCompositeOperation = 'source-over';
      ctx.drawImage(patchBmp, delta.x, delta.y);
    } else {
      const TILE = 16;
      const pw = patchBmp.width;
      const ph = patchBmp.height;
      const txCount = Math.ceil(pw / TILE);
      const tyCount = Math.ceil(ph / TILE);
      ctx.globalCompositeOperation = 'source-over';
      for (let pty = 0; pty < tyCount; pty++) {
        for (let ptx = 0; ptx < txCount; ptx++) {
          const bit = pty * txCount + ptx;
          if (((mask[bit >> 3] || 0) & (1 << (bit & 7))) === 0) continue;
          const sx = ptx * TILE;
          const sy = pty * TILE;
          const sw = Math.min(TILE, pw - sx);
          const sh = Math.min(TILE, ph - sy);
          ctx.clearRect(delta.x + sx, delta.y + sy, sw, sh);
          ctx.drawImage(patchBmp, sx, sy, sw, sh, delta.x + sx, delta.y + sy, sw, sh);
        }
      }
    }
    return createImageBitmap(cvs, { premultiplyAlpha: 'premultiply' });
  };

  // Idle-pausing rAF scheduler (0% CPU/GPU when waiting for click/tap!)
  let rafId = 0;
  let lastTs = performance.now();
  let contextLost = false;
  let recoveringRenderer = false;

  const scheduleFrame = () => {
    if (rafId === 0 && !contextLost && document.visibilityState !== 'hidden') {
      lastTs = performance.now();
      rafId = requestAnimationFrame(frame);
    }
  };

  const decodeWorker = new ImageDecodeWorkerPool(deviceProfile.maxBaseBitmaps);

  // Shared 48x48 adaptive style DNA sampler: probes ONLY title_bg / dialogue_bg / active BG (<0.3ms)
  // so legacy .nirpak titles automatically inherit their original color palette & composition!
  const styleProbeCanvas = document.createElement('canvas');
  styleProbeCanvas.width = 48;
  styleProbeCanvas.height = 48;
  const styleProbeCtx = styleProbeCanvas.getContext('2d', { willReadFrequently: true });
  let lastSyncedThemeVer = -1;
  const syncHostThemeVars = () => {
    if (typeof engine.theme_version !== 'function') return;
    const ver = engine.theme_version();
    if (ver === lastSyncedThemeVer) return;
    lastSyncedThemeVer = ver;
    try {
      const vars = JSON.parse(engine.theme_css_json() || '{}');
      const rootStyle = document.documentElement.style;
      for (const [k, v] of Object.entries(vars)) {
        rootStyle.setProperty(k, String(v));
      }
    } catch {}
  };
  const uploadBitmapWithStyle = (assetId, bmp) => {
    if (contextLost) return;
    if (styleProbeCtx && typeof engine.wants_style_sample === 'function' && engine.wants_style_sample(assetId)) {
      try {
        const sw = Math.max(16, Math.min(48, bmp.width || 48));
        const sh = Math.max(16, Math.min(48, bmp.height || 48));
        styleProbeCanvas.width = sw;
        styleProbeCanvas.height = sh;
        styleProbeCtx.clearRect(0, 0, sw, sh);
        styleProbeCtx.drawImage(bmp, 0, 0, sw, sh);
        const imgData = styleProbeCtx.getImageData(0, 0, sw, sh);
        engine.submit_style_sample(assetId, sw, sh, imgData.data);
        syncHostThemeVars();
      } catch {}
    }
    engine.upload_image_bitmap(assetId, bmp);
  };
  syncHostThemeVars();

  const dispatchImageDecode = (cmd, isPrefetch) => {
    if (inflightImages.has(cmd.id) || contextLost) return;
    inflightImages.add(cmd.id);
    if (isPrefetch) activePrefetches += 1;

    let task;
    if (cmd.mime === 'image/ktx2') {
      task = pak.getAssetBytes(cmd.offset, cmd.size).then((bytes) => {
        inflightImages.delete(cmd.id);
        if (!contextLost) engine.upload_ktx2_bytes(cmd.id, bytes);
        scheduleFrame();
      });
    } else if (cmd.delta && decodeWorker.available) {
      const d = cmd.delta;
      const needBase = !decodeWorker.hasCachedBase(d.base_id);
      task = Promise.all([
        needBase ? pak.getAssetBytes(d.base_offset, d.base_size) : Promise.resolve(null),
        pak.getAssetBytes(cmd.offset, cmd.size),
      ])
        .then(([baseBytes, patchBytes]) =>
          decodeWorker.decodeDelta(d.base_id, baseBytes, d.base_mime, patchBytes, cmd.mime, d),
        )
        .then((bmp) => {
          inflightImages.delete(cmd.id);
          uploadBitmapWithStyle(cmd.id, bmp);
          bmp.close();
          scheduleFrame();
        });
    } else if (cmd.delta) {
      const d = cmd.delta;
      task = Promise.all([
        getBaseBitmap(d.base_id, d.base_offset, d.base_size, d.base_mime),
        pak.getAssetBytes(cmd.offset, cmd.size).then((bytes) =>
          createImageBitmap(new Blob([bytes], { type: cmd.mime }), { premultiplyAlpha: 'none' }),
        ),
      ])
        .then(async ([baseBmp, patchBmp]) => {
          try {
            const composited = await compositeDeltaBitmap(baseBmp, patchBmp, d);
            inflightImages.delete(cmd.id);
            uploadBitmapWithStyle(cmd.id, composited);
            composited.close();
            scheduleFrame();
          } finally {
            patchBmp.close();
          }
        });
    } else if (decodeWorker.available) {
      task = pak.getAssetBytes(cmd.offset, cmd.size)
        .then((bytes) => decodeWorker.decode(bytes, cmd.mime))
        .then((bmp) => {
          inflightImages.delete(cmd.id);
          uploadBitmapWithStyle(cmd.id, bmp);
          bmp.close();
          scheduleFrame();
        });
    } else {
      task = pak.getAssetBytes(cmd.offset, cmd.size)
        .then((bytes) => {
          const blob = new Blob([bytes], { type: cmd.mime });
          return createImageBitmap(blob, { premultiplyAlpha: 'premultiply' });
        })
        .then((bmp) => {
          inflightImages.delete(cmd.id);
          uploadBitmapWithStyle(cmd.id, bmp);
          bmp.close();
          scheduleFrame();
        });
    }

    task
      .catch((err) => {
        inflightImages.delete(cmd.id);
        engine.mark_asset_failed(cmd.id, String(err));
        scheduleFrame();
      })
      .finally(() => {
        if (isPrefetch) {
          activePrefetches = Math.max(0, activePrefetches - 1);
          pumpPrefetchQueue();
        }
      });
  };

  const pumpPrefetchQueue = () => {
    while (activePrefetches < MAX_BG_PREFETCH && prefetchQueue.length > 0 && !contextLost) {
      const next = prefetchQueue.shift();
      if (!inflightImages.has(next.id)) {
        dispatchImageDecode(next, true);
      }
    }
  };

  const enqueueImageDecode = (cmd) => {
    if (inflightImages.has(cmd.id)) return;
    if (cmd.urgent) {
      const idx = prefetchQueue.findIndex((q) => q.id === cmd.id);
      if (idx >= 0) prefetchQueue.splice(idx, 1);
      dispatchImageDecode(cmd, false);
    } else {
      if (!prefetchQueue.some((q) => q.id === cmd.id)) {
        prefetchQueue.push(cmd);
      }
      pumpPrefetchQueue();
    }
  };

  // Dynamic viewport + safe area resize handler (handles iOS Safari URL bar & rotation!)
  let resizeQueued = false;
  const applyResize = () => {
    resizeQueued = false;
    const m = readViewportMetrics();
    const w = Math.max(1, Math.round(m.width * m.dpr));
    const h = Math.max(1, Math.round(m.height * m.dpr));
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
    }
    engine.set_safe_insets(m.insets.top, m.insets.right, m.insets.bottom, m.insets.left);
    engine.resize(w, h, m.dpr);
    scheduleFrame();
  };
  const onResize = () => {
    if (!resizeQueued) {
      resizeQueued = true;
      requestAnimationFrame(applyResize);
    }
  };
  window.addEventListener('resize', onResize);
  window.addEventListener('orientationchange', () => setTimeout(onResize, 80));
  window.visualViewport?.addEventListener('resize', onResize);

  // WebGL2 / WebGPU Context Loss Recovery (common on mobile backgrounding!)
  const recoverGpuRenderer = async () => {
    if (recoveringRenderer) return;
    recoveringRenderer = true;
    try {
      const m = readViewportMetrics();
      const w = Math.max(1, Math.round(m.width * m.dpr));
      const h = Math.max(1, Math.round(m.height * m.dpr));
      canvas.width = w;
      canvas.height = h;
      await engine.recreate_renderer(canvas, w, h, m.dpr);
      if (deviceProfile.gpuCacheMb > 0) {
        engine.set_texture_cache_limit_mb(deviceProfile.gpuCacheMb);
      }
      inflightImages.clear();
      prefetchQueue.length = 0;
      await loadAllFonts();
      for (const blob of cachedFallbackBlobs) {
        try { engine.load_fallback_font(blob); } catch {}
      }
      contextLost = false;
      scheduleFrame();
    } catch (e) {
      console.error('GPU renderer recovery failed:', e);
    } finally {
      recoveringRenderer = false;
    }
  };
  canvas.addEventListener('webglcontextlost', (e) => {
    e.preventDefault();
    contextLost = true;
    if (rafId) {
      cancelAnimationFrame(rafId);
      rafId = 0;
    }
  });
  canvas.addEventListener('webglcontextrestored', () => {
    recoverGpuRenderer();
  });

  // --- Cinema Idle Chrome Auto-Fade (Desktop + Mobile!) & Pointer Cursor Feedback ---
  let idleChromeTimer = 0;
  let chromeIsIdle = false;
  let mobileGestureHintShown = false;
  let prevStoryReading = false;
  const setChromeIdle = (idle) => {
    chromeIsIdle = idle;
    if (shelfBtn) shelfBtn.classList.toggle('chrome-idle', idle);
    if (fsBtn) fsBtn.classList.toggle('chrome-idle', idle);
    if (quickToolsEl && !engine.is_interface_hidden()) {
      quickToolsEl.classList.toggle('chrome-idle', idle);
    }
    canvas.classList.toggle('cursor-idle', idle && !deviceProfile.isMobile);
  };
  const scheduleChromeFade = () => {
    clearTimeout(idleChromeTimer);
    const delayMs = deviceProfile.isMobile ? 2800 : 3200;
    idleChromeTimer = setTimeout(() => {
      if (engine.is_story_reading() && !engine.has_hovered_hit()) {
        setChromeIdle(true);
      }
    }, delayMs);
  };
  const wakeChrome = () => {
    setChromeIdle(false);
    scheduleChromeFade();
  };
  const syncPointerCursor = () => {
    if (!deviceProfile.isMobile) {
      canvas.classList.toggle('pointer-hover', engine.has_hovered_hit());
    }
  };
  wakeChrome();

  // --- Mobile / Browser History Back Guard (`popstate`) ---
  // Prevents OS edge-back swipe from abruptly exiting the game while inside Menu/Saves/Settings/History/CG!
  let backGuardActive = false;
  let lastExitWarnMs = 0;
  try {
    if (typeof history !== 'undefined' && typeof history.pushState === 'function') {
      history.pushState({ nirGuard: 1 }, '', location.href);
      backGuardActive = true;
      window.addEventListener('popstate', () => {
        if (!backGuardActive) return;
        if (engine.can_go_back()) {
          try { history.pushState({ nirGuard: 1 }, '', location.href); } catch {}
          engine.cancel_or_menu();
          wakeChrome();
          triggerHaptic(10);
          scheduleFrame();
          return;
        }
        const now = performance.now();
        if (engine.is_story_reading() && now - lastExitWarnMs > 2500) {
          lastExitWarnMs = now;
          try {
            history.pushState({ nirGuard: 1 }, '', location.href);
            dispatchCmdJson(engine.emit_autosave());
            dispatchCmdJson(engine.flush_pending_saves());
          } catch {}
          engine.show_toast(
            shelfUrl ? '再返回一次将回到书架（已自动存档）' : '再返回一次将离开页面（已自动存档）',
            2400,
          );
          wakeChrome();
          triggerHaptic(12);
          scheduleFrame();
          return;
        }
        backGuardActive = false;
        try {
          dispatchCmdJson(engine.emit_autosave());
          dispatchCmdJson(engine.flush_pending_saves());
        } catch {}
        if (shelfUrl) {
          location.href = shelfUrl;
        } else {
          history.back();
        }
      });
    }
  } catch { /* ignore history API restrictions */ }

  // --- Unified Pointer + Multi-Touch Gesture Input ---
  const unlockAudio = () => audio.ensureContext();
  window.addEventListener('touchend', unlockAudio, { passive: true });
  window.addEventListener('click', unlockAudio, { passive: true });

  const localPos = (e) => {
    const rect = canvas.getBoundingClientRect();
    return [e.clientX - rect.left, e.clientY - rect.top];
  };

  let primaryPointerDown = false;
  canvas.addEventListener('pointerdown', (e) => {
    unlockAudio();
    const [x, y] = localPos(e);
    if (e.pointerType === 'touch') {
      // Only wake top-edge DOM chrome when tapping near top edge or outside quiet reading so CG stays clean!
      if (y <= 76 || !engine.is_story_reading()) {
        wakeChrome();
      }
      try { canvas.setPointerCapture(e.pointerId); } catch {}
      engine.touch_start(e.pointerId, x, y);
      scheduleFrame();
      return;
    }
    wakeChrome();
    if (e.button === 2) {
      engine.cancel_or_menu();
      syncPointerCursor();
      scheduleFrame();
      return;
    }
    if (e.button === 0 && e.isPrimary) {
      primaryPointerDown = true;
      engine.pointer_down(x, y);
      syncPointerCursor();
      scheduleFrame();
    }
  });

  canvas.addEventListener('pointermove', (e) => {
    const [x, y] = localPos(e);
    if (e.pointerType === 'touch') {
      engine.touch_move(e.pointerId, x, y);
      if (engine.needs_frame()) scheduleFrame();
      return;
    }
    wakeChrome();
    if (primaryPointerDown) {
      engine.pointer_drag(x, y);
    } else {
      engine.pointer_move(x, y);
    }
    syncPointerCursor();
    if (engine.needs_frame()) scheduleFrame();
  });

  canvas.addEventListener('pointerup', (e) => {
    unlockAudio();
    const [x, y] = localPos(e);
    if (e.pointerType === 'touch') {
      try { canvas.releasePointerCapture(e.pointerId); } catch {}
      engine.touch_end(e.pointerId, x, y);
      scheduleFrame();
      return;
    }
    if (e.button === 0 && e.isPrimary) {
      primaryPointerDown = false;
      engine.pointer_up(x, y);
      syncPointerCursor();
      scheduleFrame();
    }
  });

  canvas.addEventListener('pointercancel', (e) => {
    if (e.pointerType === 'touch') {
      engine.touch_cancel(e.pointerId);
    } else {
      primaryPointerDown = false;
      engine.pointer_cancel();
    }
    scheduleFrame();
  });

  canvas.addEventListener('contextmenu', (e) => e.preventDefault());
  canvas.addEventListener('wheel', (e) => {
    e.preventDefault();
    engine.scroll_wheel(e.deltaY);
    scheduleFrame();
  }, { passive: false });

  window.addEventListener('keydown', (e) => {
    unlockAudio();
    let handled = false;
    if (e.code === 'ControlLeft' || e.code === 'ControlRight') {
      engine.dispatch_action_json(JSON.stringify({ HoldSkip: true }));
      handled = true;
    } else if (!e.repeat) {
      if (e.code === 'Space' || e.code === 'Enter') { engine.dispatch_action_json('"Advance"'); handled = true; }
      else if (e.code === 'Escape') { engine.cancel_or_menu(); handled = true; }
      else if (e.code === 'KeyA') { engine.dispatch_action_json('"ToggleAuto"'); handled = true; }
      else if (e.code === 'KeyS') { engine.dispatch_action_json('"ToggleSkip"'); handled = true; }
      else if (e.code === 'KeyH') { engine.dispatch_action_json('"ToggleHideUi"'); handled = true; }
      else if (e.code === 'PageUp') { engine.dispatch_action_json('"Rollback"'); handled = true; }
      else if (e.code === 'F5') { e.preventDefault(); engine.dispatch_action_json('"QuickSave"'); handled = true; }
      else if (e.code === 'F9') { e.preventDefault(); engine.dispatch_action_json('"QuickLoad"'); handled = true; }
      else if (e.code === 'KeyV' || e.code === 'KeyR') { engine.dispatch_action_json('"ReplayVoice"'); handled = true; }
      else if (e.code === 'KeyB') { engine.dispatch_action_json('"ToggleBilingual"'); handled = true; }
      else if (e.code === 'KeyC' && e.shiftKey) { toggleAutoCopyHook(); handled = true; }
      else if (e.code === 'KeyC' && !e.ctrlKey && !e.metaKey) { copyCurrentDialogue(true); handled = true; }
      else if (e.code === 'KeyP') { captureAndSaveScreenshot(); handled = true; }
    }
    if (handled) scheduleFrame();
  });
  window.addEventListener('keyup', (e) => {
    if (e.code === 'ControlLeft' || e.code === 'ControlRight') {
      engine.dispatch_action_json(JSON.stringify({ HoldSkip: false }));
      scheduleFrame();
    }
  });

  // --- Mobile Lifecycle (pause audio/video/clocks & flush saves on background/lockscreen!) ---
  let videoWasPlayingBeforeHide = false;
  const dispatchCmdJson = (cmdsJson) => {
    if (!cmdsJson) return;
    for (const cmd of JSON.parse(cmdsJson)) {
      handleCommand(cmd);
    }
  };

  const onEnterBackground = () => {
    dispatchCmdJson(engine.set_host_paused(true));
    audio.suspendAll();
    if (videoEl && !videoEl.hidden && !videoEl.paused) {
      videoWasPlayingBeforeHide = true;
      videoEl.pause();
    }
    if (rafId) {
      cancelAnimationFrame(rafId);
      rafId = 0;
    }
    syncWakeLock();
    // Free transient mobile decode caches when backgrounded to minimize Jetsam kills!
    if (deviceProfile.isMobile) {
      clearBaseBitmapCache();
      decodeWorker.clearBases();
      if (deviceProfile.lowMem) {
        pak.trimCache(8 * 1024 * 1024);
      }
    }
  };

  const onReturnForeground = () => {
    dispatchCmdJson(engine.set_host_paused(false));
    audio.resumeAll();
    if (videoEl && !videoEl.hidden && videoWasPlayingBeforeHide) {
      videoWasPlayingBeforeHide = false;
      videoEl.play().catch(() => {});
    }
    syncWakeLock();
    scheduleFrame();
  };

  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'hidden') {
      onEnterBackground();
    } else {
      onReturnForeground();
    }
  });
  window.addEventListener('pagehide', onEnterBackground);
  window.addEventListener('pageshow', (e) => {
    if (e.persisted) onReturnForeground();
  });

  // Dev server live hot-reload (silently ignored in production static builds)
  try {
    const es = new EventSource('/__nir_events');
    es.addEventListener('reload', async () => {
      try {
        const nextPak = new PakSource(`./game.nirpak?t=${Date.now()}`, deviceProfile.rangeCacheLimit);
        const freshHeader = await nextPak.open();
        pak = nextPak;
        engine.hot_reload(freshHeader);
        inflightImages.clear();
        prefetchQueue.length = 0;
        clearBaseBitmapCache();
        audio.clearCache();
        await loadAllFonts();
        scheduleFrame();
      } catch { /* retry on next edit */ }
    });
    es.onerror = () => es.close();
  } catch { /* static hosting without dev server */ }

  // Active video cleanup hook
  let activeVideoFinish = null;
  const clearVideoOverlay = () => {
    if (videoOverlayEl) videoOverlayEl.hidden = true;
    if (videoPlayBtn) videoPlayBtn.onclick = null;
    if (videoSkipBtn) videoSkipBtn.onclick = null;
  };

  // Clean 160x90 WebP thumbnail capture cropped strictly to `stage_rect` (zero letterbox bars!)
  // and cached during Story reading so manual saves inside the Saves overlay never capture the menu UI!
  const thumbCanvas = document.createElement('canvas');
  thumbCanvas.width = 160;
  thumbCanvas.height = 90;
  const thumbCtx = thumbCanvas.getContext('2d');
  let cleanStoryThumb = null;
  let lastThumbCaptureMs = 0;
  const captureStageThumb = () => {
    if (!thumbCtx || !canvas || canvas.width === 0 || canvas.height === 0) return null;
    try {
      const sr = engine.stage_rect_phys();
      let sx = 0, sy = 0, sw = canvas.width, sh = canvas.height;
      if (sr && sr.length === 4 && sr[2] > 16 && sr[3] > 16) {
        sx = Math.max(0, Math.min(canvas.width - 1, sr[0]));
        sy = Math.max(0, Math.min(canvas.height - 1, sr[1]));
        sw = Math.max(1, Math.min(canvas.width - sx, sr[2]));
        sh = Math.max(1, Math.min(canvas.height - sy, sr[3]));
      }
      // Center-crop stage to 16:9 aspect ratio for crisp, borderless save thumbnails
      const targetAr = thumbCanvas.width / thumbCanvas.height;
      const srcAr = sw / sh;
      if (srcAr > targetAr) {
        const cropW = sh * targetAr;
        sx += (sw - cropW) * 0.5;
        sw = cropW;
      } else if (srcAr < targetAr) {
        const cropH = sw / targetAr;
        sy += (sh - cropH) * 0.35; // Slight upper bias keeps character faces nicely framed!
        sh = cropH;
      }
      thumbCtx.drawImage(canvas, sx, sy, sw, sh, 0, 0, thumbCanvas.width, thumbCanvas.height);
      let url = thumbCanvas.toDataURL('image/webp', 0.72);
      if (!url || !url.startsWith('data:image/webp')) {
        url = thumbCanvas.toDataURL('image/jpeg', 0.75);
      }
      return url && url.length < 32768 ? url : null;
    } catch {
      return null;
    }
  };
  const uploadThumbDataUrl = (target, dataUrl) => {
    if (!dataUrl || contextLost) return;
    fetch(dataUrl)
      .then((r) => r.blob())
      .then((b) => createImageBitmap(b, { premultiplyAlpha: 'premultiply' }))
      .then((bmp) => {
        if (!contextLost) {
          if (target === 'auto') engine.upload_autosave_thumb_bitmap(bmp);
          else if (target === 'quick') engine.upload_quicksave_thumb_bitmap(bmp);
          else engine.upload_slot_thumb_bitmap(target, bmp);
        }
        bmp.close();
        scheduleFrame();
      })
      .catch(() => {});
  };
  // Upload any previously stored save thumbnails lazily
  for (const [key, dataUrl] of store.thumbnails.entries()) {
    uploadThumbDataUrl(key, dataUrl);
  }
  // --- Multi-Tab Sync (BroadcastChannel) + Concurrent Play Guard (Web Locks API) ---
  const syncChannel = typeof BroadcastChannel !== 'undefined' ? new BroadcastChannel('nir-sync') : null;
  const broadcastSaveSync = (kind, snapshotJson, thumb) => {
    if (!syncChannel) return;
    try {
      const snap = JSON.parse(snapshotJson || '{}');
      syncChannel.postMessage({
        type: 'save-updated',
        kind,
        gameId: engine.game_id(),
        slug: pakSlug,
        summary: snap.summary || '',
        savedAt: snap.saved_at_unix || Math.floor(Date.now() / 1000),
        thumb: thumb || null,
      });
    } catch { /* ignore */ }
  };
  if (navigator.locks && typeof navigator.locks.request === 'function') {
    const lockName = `nir-game-session:${engine.game_id() || pakUrl}`;
    navigator.locks.request(lockName, { ifAvailable: true }, (lock) => {
      if (!lock) {
        engine.show_toast('检测到另一标签页正在运行本作，请注意避免存档覆盖', 4200);
        scheduleFrame();
        return;
      }
      // Hold exclusive lock for the lifetime of this tab
      return new Promise(() => {});
    }).catch(() => {});
  }

  // --- Subtle Haptic Feedback (Vibration API + Gamepad Dual-Rumble) ---
  let activeGamepadIndex = -1;
  let lastShakeHapticMs = 0;
  const triggerHaptic = (ms = 14, strong = 0.25) => {
    try {
      if (deviceProfile.isMobile && typeof navigator.vibrate === 'function') {
        navigator.vibrate(ms);
      }
      if (activeGamepadIndex >= 0 && navigator.getGamepads) {
        const gp = navigator.getGamepads()[activeGamepadIndex];
        const act = gp?.vibrationActuator;
        if (act && typeof act.playEffect === 'function') {
          act.playEffect('dual-rumble', {
            startDelay: 0,
            duration: Math.max(40, ms * 3),
            weakMagnitude: strong,
            strongMagnitude: strong * 0.6,
          }).catch(() => {});
        }
      }
    } catch { /* ignore */ }
  };

  // --- Texthooker Clipboard Copy & Auto-Copy Hook (great for Japanese visual novel learners!) ---
  let autoCopyHook = false;
  let lastDialogueKey = '';
  let lastDialogueObj = null;

  const copyCurrentDialogue = async (explicitToast = false) => {
    const raw = engine.current_dialogue_json();
    if (!raw) {
      if (explicitToast) showSyncPill('当前无台词可复制');
      return;
    }
    try {
      const d = JSON.parse(raw);
      const line = (d.text || '').trim();
      if (!line) return;
      if (navigator.clipboard && typeof navigator.clipboard.writeText === 'function') {
        await navigator.clipboard.writeText(line);
        if (explicitToast) showSyncPill('📋 已复制台词');
      }
    } catch {
      if (explicitToast) showSyncPill('复制需要页面处于焦点状态', true);
    }
  };

  const toggleAutoCopyHook = () => {
    autoCopyHook = !autoCopyHook;
    if (copyBtn) {
      copyBtn.classList.toggle('active', autoCopyHook);
      copyBtn.textContent = autoCopyHook ? '🔗' : '📋';
    }
    showSyncPill(autoCopyHook ? '🔗 已开启台词自动复制 (Texthooker)' : '已关闭台词自动复制');
    if (autoCopyHook) copyCurrentDialogue(false);
  };

  // --- Clean Borderless Full-Res PNG Screenshot + Web Share API ---
  const captureAndSaveScreenshot = () => {
    if (!canvas || canvas.width === 0 || canvas.height === 0) return;
    try {
      const sr = engine.stage_rect_phys();
      let sx = 0, sy = 0, sw = canvas.width, sh = canvas.height;
      if (sr && sr.length === 4 && sr[2] > 16 && sr[3] > 16) {
        sx = Math.max(0, Math.round(sr[0]));
        sy = Math.max(0, Math.round(sr[1]));
        sw = Math.min(canvas.width - sx, Math.round(sr[2]));
        sh = Math.min(canvas.height - sy, Math.round(sr[3]));
      }
      const out = document.createElement('canvas');
      out.width = Math.max(1, sw);
      out.height = Math.max(1, sh);
      out.getContext('2d').drawImage(canvas, sx, sy, sw, sh, 0, 0, out.width, out.height);
      const safeName = (engine.title() || 'nir-cg').replace(/[\\/:*?"<>|\s]+/g, '_').slice(0, 40);
      const fname = `${safeName}-${new Date().toISOString().replace(/[:.]/g, '-').slice(0, 19)}.png`;
      out.toBlob(async (blob) => {
        if (!blob) return;
        const file = new File([blob], fname, { type: 'image/png' });
        if (deviceProfile.isMobile && navigator.canShare && navigator.canShare({ files: [file] })) {
          try {
            await navigator.share({ files: [file], title: engine.title() });
            return;
          } catch { /* fall back to direct download */ }
        }
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = fname;
        a.click();
        setTimeout(() => URL.revokeObjectURL(url), 4000);
        showSyncPill('📷 已保存无黑边截图');
      }, 'image/png');
    } catch {
      showSyncPill('截图保存失败', true);
    }
  };

  if (quickToolsEl) {
    quickToolsEl.hidden = false;
    syncChromePads();
  }
  if (copyBtn) {
    let copyHoldTimer = 0;
    let copyLongPressed = false;
    copyBtn.addEventListener('click', () => {
      if (copyLongPressed) { copyLongPressed = false; return; }
      copyCurrentDialogue(true);
    });
    copyBtn.addEventListener('contextmenu', (e) => {
      e.preventDefault();
      toggleAutoCopyHook();
    });
    copyBtn.addEventListener('pointerdown', (e) => {
      if (e.pointerType !== 'touch') return;
      copyLongPressed = false;
      clearTimeout(copyHoldTimer);
      copyHoldTimer = setTimeout(() => {
        copyLongPressed = true;
        triggerHaptic(14);
        toggleAutoCopyHook();
      }, 480);
    });
    copyBtn.addEventListener('pointerup', () => clearTimeout(copyHoldTimer));
    copyBtn.addEventListener('pointercancel', () => clearTimeout(copyHoldTimer));
  }
  if (shotBtn) {
    shotBtn.addEventListener('click', () => {
      triggerHaptic(10);
      captureAndSaveScreenshot();
    });
  }

  // --- Media Session API (Lock-Screen Metadata + Bluetooth / Media Key Controls) ---
  const coverArtwork = cfg.coverUrl
    ? [{ src: new URL(cfg.coverUrl, location.href).href, sizes: '640x360', type: 'image/jpeg' }]
    : [];
  const syncMediaSession = (dlg) => {
    if (!('mediaSession' in navigator) || typeof MediaMetadata === 'undefined') return;
    try {
      const title = engine.title() || cfg.gameTitle || 'NIR Player';
      const artist = (dlg?.speaker || '').trim() || 'NIR Visual Novel';
      const album = (dlg?.text || '').trim().slice(0, 64);
      navigator.mediaSession.metadata = new MediaMetadata({
        title,
        artist,
        album,
        artwork: coverArtwork,
      });
    } catch { /* ignore */ }
  };
  if ('mediaSession' in navigator) {
    const safeBind = (action, fn) => {
      try { navigator.mediaSession.setActionHandler(action, fn); } catch {}
    };
    safeBind('play', () => { audio.ensureContext(); engine.dispatch_action_json('"ToggleAuto"'); scheduleFrame(); });
    safeBind('pause', () => { engine.dispatch_action_json('"ToggleAuto"'); scheduleFrame(); });
    safeBind('nexttrack', () => { engine.dispatch_action_json('"Advance"'); scheduleFrame(); });
    safeBind('previoustrack', () => { engine.dispatch_action_json('"Rollback"'); scheduleFrame(); });
    syncMediaSession(null);
  }

  // --- Gamepad API Controller (Xbox / PlayStation / Switch / Steam Deck / 8BitDo Micro) ---
  // Zero overhead when no gamepad is connected!
  let gamepadRaf = 0;
  const gpPrev = new Map(); // btnIndex -> pressed
  let gpHoldSkip = false;
  let gpLastAxisMs = 0;
  const pollGamepads = () => {
    gamepadRaf = 0;
    if (activeGamepadIndex < 0 || !navigator.getGamepads || document.visibilityState === 'hidden') return;
    const gp = navigator.getGamepads()[activeGamepadIndex];
    if (!gp || !gp.connected) {
      activeGamepadIndex = -1;
      if (gpHoldSkip) {
        gpHoldSkip = false;
        engine.dispatch_action_json(JSON.stringify({ HoldSkip: false }));
        scheduleFrame();
      }
      return;
    }
    const pressed = (idx) => Boolean(gp.buttons?.[idx]?.pressed);
    const edge = (idx) => {
      const cur = pressed(idx);
      const prev = gpPrev.get(idx) || false;
      gpPrev.set(idx, cur);
      return cur && !prev;
    };
    let acted = false;
    if (edge(0) || edge(5)) { engine.dispatch_action_json('"Advance"'); acted = true; } // A / R1
    if (edge(1)) { engine.cancel_or_menu(); acted = true; } // B
    if (edge(2)) { engine.dispatch_action_json('"OpenHistory"'); acted = true; } // X
    if (edge(3)) { engine.dispatch_action_json('"ToggleHideUi"'); acted = true; } // Y
    if (edge(4) || edge(12)) { engine.scroll_wheel(-120); acted = true; } // L1 / D-Pad Up
    if (edge(13)) { engine.scroll_wheel(120); acted = true; } // D-Pad Down
    if (edge(9)) { engine.dispatch_action_json('"ToggleAuto"'); acted = true; } // Start
    if (edge(8)) { engine.dispatch_action_json('"QuickSave"'); acted = true; } // Select
    // Left stick vertical scroll (throttled)
    const ay = gp.axes?.[1] || 0;
    const nowMs = performance.now();
    if (Math.abs(ay) > 0.65 && nowMs - gpLastAxisMs > 240) {
      gpLastAxisMs = nowMs;
      engine.scroll_wheel(ay < 0 ? -120 : 120);
      acted = true;
    }
    // R2 trigger hold-skip
    const r2 = (gp.buttons?.[7]?.value || 0) > 0.45 || pressed(7);
    if (r2 !== gpHoldSkip) {
      gpHoldSkip = r2;
      engine.dispatch_action_json(JSON.stringify({ HoldSkip: r2 }));
      acted = true;
    }
    if (acted) {
      unlockAudio();
      scheduleFrame();
    }
    gamepadRaf = requestAnimationFrame(pollGamepads);
  };
  window.addEventListener('gamepadconnected', (e) => {
    activeGamepadIndex = e.gamepad.index;
    gpPrev.clear();
    engine.show_toast(`🎮 已连接手柄：${(e.gamepad.id || 'Gamepad').slice(0, 28)}`, 2400);
    scheduleFrame();
    if (!gamepadRaf) gamepadRaf = requestAnimationFrame(pollGamepads);
  });
  window.addEventListener('gamepaddisconnected', (e) => {
    if (e.gamepad.index === activeGamepadIndex) {
      activeGamepadIndex = -1;
      if (gpHoldSkip) {
        gpHoldSkip = false;
        engine.dispatch_action_json(JSON.stringify({ HoldSkip: false }));
        scheduleFrame();
      }
      engine.show_toast('手柄已断开', 1800);
      scheduleFrame();
    }
  });

  // --- Drag & Drop a local `.nirpak` directly onto the player for zero-upload instant play! ---
  window.addEventListener('dragover', (e) => {
    if (e.dataTransfer?.types?.includes('Files')) e.preventDefault();
  });
  window.addEventListener('drop', async (e) => {
    const file = e.dataTransfer?.files?.[0];
    if (!file || !file.name.toLowerCase().endsWith('.nirpak')) return;
    e.preventDefault();
    try {
      const nextPak = new PakSource(pakUrl, deviceProfile.rangeCacheLimit, file);
      const freshHeader = await nextPak.open();
      pak = nextPak;
      engine.hot_reload(freshHeader);
      inflightImages.clear();
      prefetchQueue.length = 0;
      clearBaseBitmapCache();
      decodeWorker.clearBases();
      audio.clearCache();
      await loadAllFonts();
      document.title = engine.title();
      engine.show_toast(`⚡ 已本地直开：${engine.title()}`, 2600);
      scheduleFrame();
    } catch (err) {
      engine.show_toast(`无法读取本地 .nirpak：${err?.message ?? err}`, 3500);
      scheduleFrame();
    }
  });

  // Load external translation patches (if hosted by Shelf)
  if (translationsApi) {
    fetch(translationsApi, { cache: 'no-store' })
      .then((r) => (r.ok ? r.json() : null))
      .then((data) => {
        if (!data || !data.patches || typeof data.patches !== 'object') return;
        let loadedAny = false;
        for (const [loc, patch] of Object.entries(data.patches)) {
          try {
            const count = engine.load_translation_patch(loc, JSON.stringify(patch));
            if (count > 0) loadedAny = true;
          } catch { /* ignore invalid patch */ }
        }
        if (loadedAny) scheduleFrame();
      })
      .catch(() => {});
  }

  let latestCapturedThumb = null;
  const handleCommand = (cmd) => {
    switch (cmd.op) {
      case 'decode_image':
        enqueueImageDecode(cmd);
        break;
      case 'load_font':
        pak.getAssetBytes(cmd.offset, cmd.size)
          .then((bytes) => {
            engine.load_font(cmd.id, bytes);
            scheduleFrame();
          })
          .catch(() => {});
        break;
      case 'preload_audio':
        audio.preload(cmd.asset, cmd.mime, cmd.size, () => pak.getAssetBytes(cmd.offset, cmd.size));
        break;
      case 'play_audio':
        audio.play(
          cmd,
          () => pak.getAssetBytes(cmd.offset, cmd.size),
          (ch) => {
            engine.notify_audio_ended(ch);
            scheduleFrame();
          },
        );
        break;
      case 'stop_audio':
        audio.stop(cmd.channel, cmd.fade_out_ms);
        break;
      case 'set_audio_gain':
        audio.setGain(cmd.channel, cmd.gain);
        break;
      case 'pause_audio_bus':
        if (audio.buses[cmd.bus] && audio.ctx) {
          const target = cmd.paused ? 0 : (audio.prefs[`${cmd.bus}_volume`] ?? 1.0);
          audio.buses[cmd.bus].gain.setTargetAtTime(target, audio.ctx.currentTime, 0.02);
        }
        break;
      case 'play_video':
        pak.getAssetBytes(cmd.offset, cmd.size)
          .then((bytes) => {
            if (!bytes || !videoEl) {
              engine.notify_video_finished();
              scheduleFrame();
              return;
            }
            const url = URL.createObjectURL(new Blob([bytes], { type: cmd.mime }));
            let done = false;
            const finish = () => {
              if (done) return;
              done = true;
              activeVideoFinish = null;
              clearVideoOverlay();
              videoEl.hidden = true;
              videoEl.pause();
              videoEl.src = '';
              URL.revokeObjectURL(url);
              engine.notify_video_finished();
              scheduleFrame();
            };
            activeVideoFinish = finish;
            videoEl.src = url;
            videoEl.hidden = false;
            videoEl.onended = finish;
            videoEl.onerror = finish;
            videoEl.onclick = cmd.skippable ? finish : null;
            videoEl.play().catch(() => {
              // Mobile autoplay policy blocked playback: present tap-to-play / skip UI instead of silently dropping the movie!
              if (videoOverlayEl && videoPlayBtn) {
                videoOverlayEl.hidden = false;
                if (videoSkipBtn) {
                  videoSkipBtn.hidden = !cmd.skippable;
                  videoSkipBtn.onclick = finish;
                }
                videoPlayBtn.onclick = () => {
                  clearVideoOverlay();
                  videoEl.play().catch(finish);
                };
              } else {
                finish();
              }
            });
          })
          .catch(() => {
            engine.notify_video_finished();
            scheduleFrame();
          });
        break;
      case 'stop_video':
        if (activeVideoFinish) {
          activeVideoFinish();
        } else if (videoEl) {
          clearVideoOverlay();
          videoEl.pause();
          videoEl.hidden = true;
        }
        break;
      case 'save_slot': {
        const thumb = latestCapturedThumb || captureStageThumb();
        store.saveSlot(cmd.slot, cmd.snapshot_json, thumb);
        if (thumb) uploadThumbDataUrl(cmd.slot, thumb);
        broadcastSaveSync('slot', cmd.snapshot_json, thumb);
        triggerHaptic(12);
        break;
      }
      case 'request_load_slot':
        store.get(`slot_${cmd.slot}`).then((json) => {
          if (json) {
            engine.apply_loaded_snapshot_json(json);
            scheduleFrame();
          }
        });
        break;
      case 'delete_slot':
        store.deleteSlot(cmd.slot);
        break;
      case 'save_auto': {
        const thumb = latestCapturedThumb || captureStageThumb();
        store.saveNamed('autosave', cmd.snapshot_json, thumb, { keepalive: true });
        if (thumb) uploadThumbDataUrl('auto', thumb);
        broadcastSaveSync('autosave', cmd.snapshot_json, thumb);
        break;
      }
      case 'request_load_auto':
        store.get('autosave').then((json) => {
          if (json) {
            engine.apply_loaded_snapshot_json(json);
            scheduleFrame();
          }
        });
        break;
      case 'save_quick': {
        const thumb = latestCapturedThumb || captureStageThumb();
        store.saveNamed('quicksave', cmd.snapshot_json, thumb);
        if (thumb) uploadThumbDataUrl('quick', thumb);
        broadcastSaveSync('quicksave', cmd.snapshot_json, thumb);
        triggerHaptic(14);
        break;
      }
      case 'request_load_quick':
        store.get('quicksave').then((json) => {
          if (json) {
            engine.apply_loaded_snapshot_json(json);
            scheduleFrame();
          }
        });
        break;
      case 'save_profile':
        store.set('profile', cmd.profile_json, { keepalive: true });
        break;
      case 'save_preferences':
        audio.applyPreferences(cmd.prefs);
        store.set('prefs', JSON.stringify(cmd.prefs));
        store.saveGlobalPrefs(cmd.prefs);
        break;
      case 'return_to_shelf':
        if (shelfUrl) {
          location.href = shelfUrl;
        }
        break;
    }
  };

  if (wantResume) {
    engine.try_resume_latest();
  }

  setProgress(100, 'Ready');
  loaderEl?.classList.add('hidden');
  globalThis.__nir = {
    ready: true,
    title: engine.title(),
    gameId: engine.game_id(),
    backend: engine.backend_info(),
    pakMode: pak.mode,
    mobile: deviceProfile.isMobile,
    fallbackFonts: fallbackFontsLoaded,
    error: null,
  };

  const frame = (ts) => {
    rafId = 0;
    if (contextLost || document.visibilityState === 'hidden') return;
    if (engine.is_device_lost()) {
      recoverGpuRenderer();
      return;
    }
    const dt = ts - lastTs;
    lastTs = ts;
    const cmdsJson = engine.tick_and_render(dt, Date.now() / 1000);
    // Keep a fresh, clean (menu-free, borderless) stage thumbnail while actively reading Story!
    if (engine.is_story_reading() && ts - lastThumbCaptureMs > 650) {
      const snap = captureStageThumb();
      if (snap) {
        cleanStoryThumb = snap;
        lastThumbCaptureMs = ts;
      }
    }
    if (cmdsJson) {
      const cmds = JSON.parse(cmdsJson);
      const hasSaveCmd = cmds.some((c) => c.op === 'save_slot' || c.op === 'save_auto' || c.op === 'save_quick');
      latestCapturedThumb = hasSaveCmd
        ? (engine.is_story_reading() ? (captureStageThumb() || cleanStoryThumb) : (cleanStoryThumb || captureStageThumb()))
        : null;
      const ranges = [];
      for (const cmd of cmds) {
        if (cmd.op === 'decode_image' || cmd.op === 'preload_audio' || cmd.op === 'play_audio') {
          ranges.push({ offset: cmd.offset, size: cmd.size });
          if (cmd.delta) {
            ranges.push({ offset: cmd.delta.base_offset, size: cmd.delta.base_size });
          }
        }
      }
      if (ranges.length >= 2) pak.prefetchBatch(ranges);
      for (const cmd of cmds) {
        handleCommand(cmd);
      }
      latestCapturedThumb = null;
    }
    // Sync live dialogue changes to Media Session lock-screen metadata & optional Texthooker clipboard!
    const dlgJson = engine.current_dialogue_json();
    if (dlgJson && dlgJson !== lastDialogueKey) {
      lastDialogueKey = dlgJson;
      try {
        lastDialogueObj = JSON.parse(dlgJson);
        syncMediaSession(lastDialogueObj);
        if (autoCopyHook && lastDialogueObj?.text && document.hasFocus()) {
          navigator.clipboard?.writeText(lastDialogueObj.text.trim()).catch(() => {});
        }
      } catch { /* ignore */ }
    }
    // Surface clean screenshot button prominently while UI is hidden to admire CGs,
    // and automatically re-surface top chrome whenever leaving Story (Menu / Title / Saves / Settings)
    const storyReadingNow = engine.is_story_reading();
    if (deviceProfile.isMobile) {
      if (!storyReadingNow && chromeIsIdle) {
        setChromeIdle(false);
      } else if (storyReadingNow && !prevStoryReading) {
        scheduleChromeFade();
      }
    }
    prevStoryReading = storyReadingNow;
    if (quickToolsEl) {
      const hiddenUi = engine.is_interface_hidden();
      if (copyBtn) copyBtn.hidden = hiddenUi;
      if (hiddenUi) quickToolsEl.classList.remove('chrome-idle');
    }
    // One-time subtle mobile gesture cheat-sheet toast on first entry into Story reading!
    if (deviceProfile.isMobile && storyReadingNow && !mobileGestureHintShown) {
      mobileGestureHintShown = true;
      try {
        if (localStorage.getItem('nir_mobile_gesture_hint_v1') !== '1') {
          localStorage.setItem('nir_mobile_gesture_hint_v1', '1');
          engine.show_toast('手势：上滑回看 · 下滑隐藏 · 右滑回退 · 长按快进 · 双指菜单', 4200);
        }
      } catch { /* ignore */ }
    }
    // Consume subtle touch button / slider / HoldSkip haptic pulse from Rust runtime
    const uiHapticMs = engine.take_haptic_ms();
    if (uiHapticMs > 0) {
      triggerHaptic(uiHapticMs, 0.22);
    }
    // Gentle haptic pulse when story triggers a dramatic screen Shake
    if (engine.is_shaking() && ts - lastShakeHapticMs > 520) {
      lastShakeHapticMs = ts;
      triggerHaptic(18, 0.42);
    }
    // Lazy-load 6.1 MiB Pan-CJK fallback font ONLY if a bundled font actually encounters a missing glyph!
    if (!fallbackFontStarted && engine.has_missing_glyphs()) {
      loadFallbackFontsLazy();
    }
    syncWakeLock();
    if (engine.needs_frame()) {
      rafId = requestAnimationFrame(frame);
    }
  };

  scheduleFrame();
}

main().catch((err) => {
  const msg = String(err?.message ?? err);
  setProgress(0, `Error: ${msg}`);
  if (loaderEl) {
    loaderEl.classList.remove('hidden');
    loaderEl.dataset.error = 'true';
    const diagUrl = globalThis.__NIR_CONFIG__?.diagnosticsUrl;
    if (diagUrl && !loaderEl.querySelector('.loader-diag')) {
      const link = document.createElement('a');
      link.className = 'loader-diag';
      link.href = diagUrl;
      link.textContent = '打开浏览器环境检测页 →';
      link.style.cssText = 'display:inline-block;margin-top:12px;color:#88c9ff;font-size:13px;';
      loaderEl.querySelector('.loader-card')?.appendChild(link);
    }
  }
  globalThis.__nir = { ready: false, error: msg };
  console.error(err);
});
