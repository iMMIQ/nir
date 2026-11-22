// NIR v2 Modern Single-Page Web Host
// HTTP Range streaming OR zero-copy JS-backed .nirpak | Idle-pausing rAF |
// Priority createImageBitmap GPU decode | Bounded LRU WebAudio + Lookahead | IndexedDB slot index

const canvas = document.getElementById('stage');
const videoEl = document.getElementById('video-layer');
const loaderEl = document.getElementById('loader');
const loaderText = document.getElementById('loader-text');
const loaderFill = document.getElementById('loader-fill');

const setProgress = (pct, msg) => {
  if (loaderFill) loaderFill.style.width = `${Math.round(pct)}%`;
  if (msg && loaderText) loaderText.textContent = msg;
};

// --- Streaming / Zero-Copy `.nirpak` Source (keeps media out of WASM linear memory!) ---
class PakSource {
  constructor(url) {
    this.url = url;
    this.mode = 'memory'; // 'range' | 'memory'
    this.fullBytes = null;
    this.mediaBase = 0;
    this.rangeCache = new Map(); // key -> Uint8Array
    this.rangeCacheBytes = 0;
    this.rangeCacheLimit = 64 * 1024 * 1024;
    this.inflightRanges = new Map();
  }

  async open() {
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
        // Server returned full 200 OK; read with progress bar once into JS memory (never copied to WASM!)
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

  async getAssetBytes(offset, size) {
    if (size === 0) return new Uint8Array(0);
    const start = this.mediaBase + offset;
    const end = start + size;
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

    // Group adjacent ranges within MAX_GAP and total span <= MAX_WINDOW
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
  constructor(dbName, saveApi = null) {
    this.dbName = dbName;
    this.saveApi = saveApi ? saveApi.replace(/\/+$/, '') : null;
    this.db = null;
    this.slotIndex = {};
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
    }).catch(() => {});
  }
  _remoteDel(key) {
    if (!this.saveApi) return;
    fetch(`${this.saveApi}/${encodeURIComponent(key)}`, {
      method: 'DELETE',
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
  async saveSlot(slot, snapshotJson) {
    await this.set(`slot_${slot}`, snapshotJson);
    try {
      const snap = JSON.parse(snapshotJson);
      this.slotIndex[slot] = {
        slot,
        exists: true,
        saved_at_unix: snap.saved_at_unix ?? 0,
        summary: snap.summary ?? '',
      };
      await this.set('slots_index', JSON.stringify(this.slotIndex), { remote: false });
    } catch { /* ignore */ }
  }
  async deleteSlot(slot) {
    await this.del(`slot_${slot}`);
    delete this.slotIndex[slot];
    await this.set('slots_index', JSON.stringify(this.slotIndex), { remote: false });
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
      if (a.vars || b.vars) {
        merged.vars = { ...(b.vars || {}), ...(a.vars || {}) };
      }
      return JSON.stringify(merged);
    } catch {
      return localRaw;
    }
  }
  /// Reads prefs, profile, and slot summaries in a SINGLE IndexedDB readonly transaction,
  /// optionally reconciling with same-origin cloud saves (`saveApi`)!
  async loadInitialState(slotCount = 24) {
    const remotePromise = this._fetchRemoteBundle();
    let prefsRaw = null;
    let profileRaw = null;
    let indexRaw = null;
    if (this.db) {
      ({ prefsRaw, profileRaw, indexRaw } = await new Promise((resolve) => {
        const tx = this.db.transaction('kv', 'readonly');
        const os = tx.objectStore('kv');
        const rPrefs = os.get('prefs');
        const rProfile = os.get('profile');
        const rIdx = os.get('slots_index');
        tx.oncomplete = () => resolve({
          prefsRaw: rPrefs.result ?? null,
          profileRaw: rProfile.result ?? null,
          indexRaw: rIdx.result ?? null,
        });
        tx.onerror = () => resolve({ prefsRaw: null, profileRaw: null, indexRaw: null });
      }));
    }

    if (indexRaw) {
      try {
        this.slotIndex = JSON.parse(indexRaw) || {};
      } catch {
        this.slotIndex = {};
      }
    } else if (this.db) {
      // One-time migration from legacy per-slot keys using a single readonly transaction!
      const legacy = await new Promise((resolve) => {
        const tx = this.db.transaction('kv', 'readonly');
        const os = tx.objectStore('kv');
        const reqs = [];
        for (let slot = 0; slot < slotCount; slot++) {
          reqs.push({ slot, req: os.get(`slot_${slot}`) });
        }
        tx.oncomplete = () => resolve(reqs.map(({ slot, req }) => ({ slot, raw: req.result ?? null })));
        tx.onerror = () => resolve([]);
      });
      this.slotIndex = {};
      for (const { slot, raw } of legacy) {
        if (raw) {
          try {
            const snap = JSON.parse(raw);
            this.slotIndex[slot] = {
              slot,
              exists: true,
              saved_at_unix: snap.saved_at_unix ?? 0,
              summary: snap.summary ?? '',
            };
          } catch { /* ignore */ }
        }
      }
      if (Object.keys(this.slotIndex).length > 0) {
        this.set('slots_index', JSON.stringify(this.slotIndex), { remote: false });
      }
    }

    const remote = await remotePromise;
    if (remote && typeof remote === 'object') {
      if (!prefsRaw && typeof remote.prefs === 'string') {
        prefsRaw = remote.prefs;
        await this.set('prefs', prefsRaw, { remote: false });
      }
      if (typeof remote.profile === 'string') {
        const mergedProfile = SaveStore._mergeProfile(profileRaw, remote.profile);
        if (mergedProfile && mergedProfile !== profileRaw) {
          profileRaw = mergedProfile;
          await this.set('profile', profileRaw, { remote: false });
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
            const lTs = Number(this.slotIndex[slot]?.saved_at_unix ?? -1);
            if (!this.slotIndex[slot]?.exists || rTs > lTs) {
              await this.set(`slot_${slot}`, raw, { remote: false });
              this.slotIndex[slot] = {
                slot,
                exists: true,
                saved_at_unix: rTs,
                summary: rSnap.summary ?? '',
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

    const slots = [];
    for (let slot = 0; slot < slotCount; slot++) {
      slots.push(this.slotIndex[slot] ?? { slot, exists: false, saved_at_unix: 0, summary: '' });
    }
    return {
      prefsJson: prefsRaw ?? '{}',
      profileJson: profileRaw ?? '{}',
      slots,
    };
  }
}

// --- WebAudio API Mixer with Bounded LRU Cache (96 MiB) + Lookahead Pre-decoding ---
class WebAudioMixer {
  constructor() {
    this.ctx = null;
    this.master = null;
    this.buses = {};
    this.channels = new Map();
    this.decodedCache = new Map(); // assetId -> { buf: AudioBuffer, bytes: number }
    this.decodedBytes = 0;
    this.decodedLimit = 96 * 1024 * 1024;
    this.inflightDecode = new Map();
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
    if (this.ctx.state === 'suspended') {
      this.ctx.resume().catch(() => {});
    }
    return this.ctx;
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
  async _decodeAsset(ctx, assetId, fetchBytes) {
    const hit = this.decodedCache.get(assetId);
    if (hit) {
      // Touch LRU order
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
      const copy = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
      const audioBuf = await ctx.decodeAudioData(copy);
      this._putDecoded(assetId, audioBuf);
      return audioBuf;
    })()
      .catch(() => null)
      .finally(() => {
        this.inflightDecode.delete(assetId);
      });
    this.inflightDecode.set(assetId, task);
    return task;
  }
  preload(assetId, fetchBytes) {
    if (!this.ctx || this.decodedCache.has(assetId) || this.inflightDecode.has(assetId)) return;
    this._decodeAsset(this.ctx, assetId, fetchBytes);
  }
  async play(cmd, fetchBytes, onEnded) {
    const ctx = this.ensureContext();
    if (!ctx) return;
    this.stop(cmd.channel, 0);
    const playToken = Symbol('play');
    this.channels.set(cmd.channel, { pending: playToken, asset: cmd.asset, bus: cmd.bus });

    const audioBuf = await this._decodeAsset(ctx, cmd.asset, fetchBytes);
    // Abort if channel was stopped or replaced while awaiting async decode!
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

async function main() {
  setProgress(5, 'Initializing NIR Engine…');
  if (typeof globalThis.WebAssembly === 'undefined') {
    throw new ReferenceError('WebAssembly is not defined');
  }
  const cfg = globalThis.__NIR_CONFIG__ || {};
  const qs = new URLSearchParams(location.search);
  const pakUrl = cfg.pakUrl || qs.get('pak') || qs.get('file') || './game.nirpak';
  const wasmJsUrl = cfg.wasmJsUrl
    ? new URL(cfg.wasmJsUrl, location.href).href
    : new URL('./player_web.js', import.meta.url).href;
  const wasmBinUrl = cfg.wasmUrl ? new URL(cfg.wasmUrl, location.href).href : null;
  const gameKey = cfg.gameId || qs.get('game') || null;
  const saveApi = cfg.saveApi || qs.get('save_api') || null;

  let pak = new PakSource(pakUrl);
  const store = new SaveStore(gameKey ? `nir_saves_v2:${gameKey}` : 'nir_saves_v2', saveApi);

  const [{ default: initWasm, WebEngine }, headerBytes, initialSaves] = await Promise.all([
    import(/* @vite-ignore */ wasmJsUrl),
    pak.open(),
    store.open().then(() => store.loadInitialState(24)).catch(() => ({
      prefsJson: '{}', profileJson: '{}', slots: [],
    })),
  ]);
  if (wasmBinUrl) {
    await initWasm({ module_or_path: wasmBinUrl });
  } else {
    await initWasm();
  }

  setProgress(92, 'Starting GPU renderer…');
  const dpr = Math.min(window.devicePixelRatio || 1, 3);
  canvas.width = Math.max(1, Math.round(window.innerWidth * dpr));
  canvas.height = Math.max(1, Math.round(window.innerHeight * dpr));

  const engine = await WebEngine.create(
    canvas,
    headerBytes,
    initialSaves.prefsJson,
    initialSaves.profileJson,
    JSON.stringify(initialSaves.slots),
    dpr,
  );
  document.title = engine.title();

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

  // Start fetching shared Web CJK fallback font in parallel with WASM + header (browser immutable-cached!)
  const fallbackFontPromise = (async () => {
    const loaded = [];
    for (const url of fallbackFontUrls) {
      try {
        const res = await fetch(url);
        if (res.ok) {
          const bytes = new Uint8Array(await res.arrayBuffer());
          if (bytes.byteLength >= 256) {
            loaded.push(bytes);
          }
        }
      } catch { /* optional fallback font */ }
    }
    return loaded;
  })();

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
  let fallbackFontsLoaded = 0;
  const applyFallbackFonts = (fontBlobs) => {
    for (const bytes of fontBlobs) {
      try {
        engine.load_fallback_font(bytes);
        fallbackFontsLoaded += 1;
      } catch { /* ignore invalid font */ }
    }
    if (globalThis.__nir) {
      globalThis.__nir.fallbackFonts = fallbackFontsLoaded;
    }
  };
  if (bundledFontCount === 0) {
    // Wait for shared CJK fallback font when the .nirpak has no bundled font so frame 1 is never blank!
    setProgress(96, 'Loading CJK font…');
    applyFallbackFonts(await fallbackFontPromise);
  }

  const audio = new WebAudioMixer();
  try { audio.applyPreferences(JSON.parse(initialSaves.prefsJson)); } catch { /* default */ }

  // Priority image decode scheduler: urgent images dispatch immediately; background prefetches
  // are concurrency-limited (max 3) so lookahead never starves visible scene assets!
  const inflightImages = new Set();
  const prefetchQueue = [];
  let activePrefetches = 0;
  const MAX_BG_PREFETCH = 3;
  // Small LRU of decoded straight base ImageBitmaps for instant delta CG/sprite compositing!
  const baseBitmapCache = new Map(); // baseId -> Promise<ImageBitmap>
  const MAX_BASE_BITMAPS = 8;

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

  // Idle-pausing rAF scheduler (0% CPU/GPU when waiting for click!)
  let rafId = 0;
  let lastTs = performance.now();

  const scheduleFrame = () => {
    if (rafId === 0) {
      lastTs = performance.now();
      rafId = requestAnimationFrame(frame);
    }
  };

  const dispatchImageDecode = (cmd, isPrefetch) => {
    if (inflightImages.has(cmd.id)) return;
    inflightImages.add(cmd.id);
    if (isPrefetch) activePrefetches += 1;

    let task;
    if (cmd.mime === 'image/ktx2') {
      task = pak.getAssetBytes(cmd.offset, cmd.size).then((bytes) => {
        inflightImages.delete(cmd.id);
        engine.upload_ktx2_bytes(cmd.id, bytes);
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
            engine.upload_image_bitmap(cmd.id, composited);
            composited.close();
            scheduleFrame();
          } finally {
            patchBmp.close();
          }
        });
    } else {
      task = pak.getAssetBytes(cmd.offset, cmd.size)
        .then((bytes) => {
          const blob = new Blob([bytes], { type: cmd.mime });
          return createImageBitmap(blob, { premultiplyAlpha: 'premultiply' });
        })
        .then((bmp) => {
          inflightImages.delete(cmd.id);
          engine.upload_image_bitmap(cmd.id, bmp);
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
    while (activePrefetches < MAX_BG_PREFETCH && prefetchQueue.length > 0) {
      const next = prefetchQueue.shift();
      if (!inflightImages.has(next.id)) {
        dispatchImageDecode(next, true);
      }
    }
  };

  const enqueueImageDecode = (cmd) => {
    if (inflightImages.has(cmd.id)) return;
    if (cmd.urgent) {
      // Remove from prefetch queue if already waiting there and run immediately!
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

  const onResize = () => {
    const r = Math.min(window.devicePixelRatio || 1, 3);
    const w = Math.max(1, Math.round(window.innerWidth * r));
    const h = Math.max(1, Math.round(window.innerHeight * r));
    canvas.width = w;
    canvas.height = h;
    engine.resize(w, h, r);
    scheduleFrame();
  };
  window.addEventListener('resize', onResize);

  const unlockAudio = () => audio.ensureContext();
  canvas.addEventListener('pointermove', (e) => {
    const rect = canvas.getBoundingClientRect();
    engine.pointer_move(e.clientX - rect.left, e.clientY - rect.top);
    if (engine.needs_frame()) scheduleFrame();
  });
  canvas.addEventListener('pointerdown', (e) => {
    unlockAudio();
    if (e.button === 2) {
      engine.cancel_or_menu();
      scheduleFrame();
      return;
    }
    if (e.button === 0) {
      const rect = canvas.getBoundingClientRect();
      engine.pointer_click(e.clientX - rect.left, e.clientY - rect.top);
      scheduleFrame();
    }
  });
  canvas.addEventListener('contextmenu', (e) => e.preventDefault());
  canvas.addEventListener('wheel', (e) => {
    e.preventDefault();
    const act = e.deltaY < 0 ? 'Rollback' : 'Advance';
    engine.dispatch_action_json(JSON.stringify(act));
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
    }
    if (handled) scheduleFrame();
  });
  window.addEventListener('keyup', (e) => {
    if (e.code === 'ControlLeft' || e.code === 'ControlRight') {
      engine.dispatch_action_json(JSON.stringify({ HoldSkip: false }));
      scheduleFrame();
    }
  });

  // Flush debounced profile saves when hiding or closing tab!
  const flushSaves = () => {
    const cmdsJson = engine.flush_pending_saves();
    if (cmdsJson) {
      for (const cmd of JSON.parse(cmdsJson)) {
        handleCommand(cmd);
      }
    }
  };
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'hidden') flushSaves();
  });
  window.addEventListener('pagehide', flushSaves);

  // Dev server live hot-reload (silently ignored in production static builds)
  try {
    const es = new EventSource('/__nir_events');
    es.addEventListener('reload', async () => {
      try {
        const nextPak = new PakSource(`./game.nirpak?t=${Date.now()}`);
        const freshHeader = await nextPak.open();
        pak = nextPak;
        engine.hot_reload(freshHeader);
        inflightImages.clear();
        prefetchQueue.length = 0;
        for (const p of baseBitmapCache.values()) {
          p.then((b) => b.close()).catch(() => {});
        }
        baseBitmapCache.clear();
        audio.clearCache();
        await loadAllFonts();
        scheduleFrame();
      } catch { /* retry on next edit */ }
    });
    es.onerror = () => es.close();
  } catch { /* static hosting without dev server */ }

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
        audio.preload(cmd.asset, () => pak.getAssetBytes(cmd.offset, cmd.size));
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
            videoEl.src = url;
            videoEl.hidden = false;
            const finish = () => {
              videoEl.hidden = true;
              videoEl.pause();
              URL.revokeObjectURL(url);
              engine.notify_video_finished();
              scheduleFrame();
            };
            videoEl.onended = finish;
            videoEl.onclick = cmd.skippable ? finish : null;
            videoEl.play().catch(finish);
          })
          .catch(() => {
            engine.notify_video_finished();
            scheduleFrame();
          });
        break;
      case 'stop_video':
        if (videoEl) {
          videoEl.pause();
          videoEl.hidden = true;
        }
        break;
      case 'save_slot':
        store.saveSlot(cmd.slot, cmd.snapshot_json);
        break;
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
      case 'save_profile':
        store.set('profile', cmd.profile_json, { keepalive: true });
        break;
      case 'save_preferences':
        audio.applyPreferences(cmd.prefs);
        store.set('prefs', JSON.stringify(cmd.prefs));
        break;
    }
  };

  setProgress(100, 'Ready');
  loaderEl?.classList.add('hidden');
  globalThis.__nir = {
    ready: true,
    title: engine.title(),
    gameId: engine.game_id(),
    backend: engine.backend_info(),
    pakMode: pak.mode,
    fallbackFonts: fallbackFontsLoaded,
    error: null,
  };

  if (bundledFontCount > 0) {
    // Background-attach CJK fallback font after bundled fonts so missing UI/CJK glyphs render cleanly!
    fallbackFontPromise.then((blobs) => {
      if (blobs.length > 0) {
        applyFallbackFonts(blobs);
        scheduleFrame();
      }
    });
  }

  const frame = (ts) => {
    rafId = 0;
    const dt = ts - lastTs;
    lastTs = ts;
    const cmdsJson = engine.tick_and_render(dt, Date.now() / 1000);
    if (cmdsJson) {
      const cmds = JSON.parse(cmdsJson);
      // Coalesce adjacent HTTP Range requests across the whole command batch!
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
    }
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
