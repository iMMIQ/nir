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
      if (view.byteLength <= this.rangeCacheLimit / 2) {
        this.rangeCache.set(key, view);
        this.rangeCacheBytes += view.byteLength;
        while (this.rangeCacheBytes > this.rangeCacheLimit && this.rangeCache.size > 1) {
          const oldest = this.rangeCache.keys().next().value;
          const oldVal = this.rangeCache.get(oldest);
          this.rangeCache.delete(oldest);
          this.rangeCacheBytes -= oldVal.byteLength;
        }
      }
      return view;
    })().finally(() => {
      this.inflightRanges.delete(key);
    });
    this.inflightRanges.set(key, promise);
    return promise;
  }
}

// --- Lightweight IndexedDB persistence with single-key `slots_index` fast startup ---
class SaveStore {
  constructor(dbName) {
    this.dbName = dbName;
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
  async get(key) {
    if (!this.db) return null;
    return new Promise((resolve) => {
      const tx = this.db.transaction('kv', 'readonly');
      const req = tx.objectStore('kv').get(key);
      req.onsuccess = () => resolve(req.result ?? null);
      req.onerror = () => resolve(null);
    });
  }
  async set(key, val) {
    if (!this.db) return;
    await new Promise((resolve) => {
      const tx = this.db.transaction('kv', 'readwrite');
      tx.objectStore('kv').put(val, key);
      tx.oncomplete = () => resolve();
      tx.onerror = () => resolve();
    });
  }
  async del(key) {
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
      await this.set('slots_index', JSON.stringify(this.slotIndex));
    } catch { /* ignore */ }
  }
  async deleteSlot(slot) {
    await this.del(`slot_${slot}`);
    delete this.slotIndex[slot];
    await this.set('slots_index', JSON.stringify(this.slotIndex));
  }
  /// Reads prefs, profile, and slot summaries in a SINGLE IndexedDB readonly transaction!
  async loadInitialState(slotCount = 24) {
    if (!this.db) {
      return {
        prefsJson: '{}',
        profileJson: '{}',
        slots: Array.from({ length: slotCount }, (_, slot) => ({
          slot, exists: false, saved_at_unix: 0, summary: '',
        })),
      };
    }
    const { prefsRaw, profileRaw, indexRaw } = await new Promise((resolve) => {
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
    });

    if (indexRaw) {
      try {
        this.slotIndex = JSON.parse(indexRaw) || {};
      } catch {
        this.slotIndex = {};
      }
    } else {
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
        this.set('slots_index', JSON.stringify(this.slotIndex));
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
  let pak = new PakSource('./game.nirpak');
  const wasmUrl = new URL('./player_web.js', import.meta.url).href;
  const store = new SaveStore('nir_saves_v2');

  const [{ default: initWasm, WebEngine }, headerBytes, initialSaves] = await Promise.all([
    import(/* @vite-ignore */ wasmUrl),
    pak.open(),
    store.open().then(() => store.loadInitialState(24)).catch(() => ({
      prefsJson: '{}', profileJson: '{}', slots: [],
    })),
  ]);
  await initWasm();

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

  const loadAllFonts = async () => {
    const fonts = JSON.parse(engine.font_assets_json() || '[]');
    await Promise.all(fonts.map(async (f) => {
      try {
        const bytes = await pak.getAssetBytes(f.offset, f.size);
        engine.load_font(f.id, bytes);
      } catch { /* ignore */ }
    }));
  };
  await loadAllFonts();

  const audio = new WebAudioMixer();
  try { audio.applyPreferences(JSON.parse(initialSaves.prefsJson)); } catch { /* default */ }

  // Priority image decode scheduler: urgent images dispatch immediately; background prefetches
  // are concurrency-limited (max 3) so lookahead never starves visible scene assets!
  const inflightImages = new Set();
  const prefetchQueue = [];
  let activePrefetches = 0;
  const MAX_BG_PREFETCH = 3;

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

    pak.getAssetBytes(cmd.offset, cmd.size)
      .then((bytes) => {
        const blob = new Blob([bytes], { type: cmd.mime });
        return createImageBitmap(blob, { premultiplyAlpha: 'premultiply' });
      })
      .then((bmp) => {
        inflightImages.delete(cmd.id);
        engine.upload_image_bitmap(cmd.id, bmp);
        bmp.close();
        scheduleFrame();
      })
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
        store.set('profile', cmd.profile_json);
        break;
      case 'save_preferences':
        audio.applyPreferences(cmd.prefs);
        store.set('prefs', JSON.stringify(cmd.prefs));
        break;
    }
  };

  setProgress(100, 'Ready');
  loaderEl?.classList.add('hidden');

  const frame = (ts) => {
    rafId = 0;
    const dt = ts - lastTs;
    lastTs = ts;
    const cmdsJson = engine.tick_and_render(dt, Date.now() / 1000);
    if (cmdsJson) {
      for (const cmd of JSON.parse(cmdsJson)) {
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
  setProgress(0, `Error: ${err?.message ?? err}`);
  console.error(err);
});
