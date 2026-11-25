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
const safeProbeEl = document.getElementById('safe-area-probe');
const loaderEl = document.getElementById('loader');
const loaderText = document.getElementById('loader-text');
const loaderFill = document.getElementById('loader-fill');

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

// --- Streaming / Zero-Copy `.nirpak` Source (keeps media out of WASM linear memory!) ---
class PakSource {
  constructor(url, rangeCacheLimit) {
    this.url = url;
    this.mode = 'memory'; // 'range' | 'memory'
    this.fullBytes = null;
    this.mediaBase = 0;
    this.rangeCache = new Map(); // key -> Uint8Array
    this.rangeCacheBytes = 0;
    this.rangeCacheLimit = rangeCacheLimit || 64 * 1024 * 1024;
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
  navigator.serviceWorker.register(new URL('./sw.js', import.meta.url).href).catch(() => {});
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
  setProgress(5, 'Initializing NIR Engine…');
  if (typeof globalThis.WebAssembly === 'undefined') {
    throw new ReferenceError('WebAssembly is not defined');
  }
  const cfg = globalThis.__NIR_CONFIG__ || {};
  registerServiceWorker(cfg);

  const qs = new URLSearchParams(location.search);
  const pakUrl = cfg.pakUrl || qs.get('pak') || qs.get('file') || './game.nirpak';
  const wasmJsUrl = cfg.wasmJsUrl
    ? new URL(cfg.wasmJsUrl, location.href).href
    : new URL('./player_web.js', import.meta.url).href;
  const wasmBinUrl = cfg.wasmUrl ? new URL(cfg.wasmUrl, location.href).href : null;
  const gameKey = cfg.gameId || qs.get('game') || null;
  const saveApi = cfg.saveApi || qs.get('save_api') || null;

  let pak = new PakSource(pakUrl, deviceProfile.rangeCacheLimit);
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
            if (!contextLost) engine.upload_image_bitmap(cmd.id, composited);
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
          if (!contextLost) engine.upload_image_bitmap(cmd.id, bmp);
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

  // --- Unified Pointer + Multi-Touch Gesture Input ---
  const unlockAudio = () => audio.ensureContext();
  window.addEventListener('touchend', unlockAudio, { passive: true });
  window.addEventListener('click', unlockAudio, { passive: true });

  const localPos = (e) => {
    const rect = canvas.getBoundingClientRect();
    return [e.clientX - rect.left, e.clientY - rect.top];
  };

  canvas.addEventListener('pointerdown', (e) => {
    unlockAudio();
    const [x, y] = localPos(e);
    if (e.pointerType === 'touch') {
      try { canvas.setPointerCapture(e.pointerId); } catch {}
      engine.touch_start(e.pointerId, x, y);
      scheduleFrame();
      return;
    }
    if (e.button === 2) {
      engine.cancel_or_menu();
      scheduleFrame();
      return;
    }
    if (e.button === 0 && e.isPrimary) {
      engine.pointer_down(x, y);
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
    engine.pointer_move(x, y);
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
      engine.pointer_up(x, y);
      scheduleFrame();
    }
  });

  canvas.addEventListener('pointercancel', (e) => {
    if (e.pointerType === 'touch') {
      engine.touch_cancel(e.pointerId);
    } else {
      engine.pointer_cancel();
    }
    scheduleFrame();
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
    if (cmdsJson) {
      const cmds = JSON.parse(cmdsJson);
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
