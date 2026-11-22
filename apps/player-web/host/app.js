// NIR v2 Modern Single-Page Web Host
// Direct ./game.nirpak load | Single WASM | createImageBitmap GPU decode | WebAudio | IndexedDB | Live Hot-Reload

const canvas = document.getElementById('stage');
const videoEl = document.getElementById('video-layer');
const loaderEl = document.getElementById('loader');
const loaderText = document.getElementById('loader-text');
const loaderFill = document.getElementById('loader-fill');

const setProgress = (pct, msg) => {
  if (loaderFill) loaderFill.style.width = `${Math.round(pct)}%`;
  if (msg && loaderText) loaderText.textContent = msg;
};

// --- Lightweight IndexedDB persistence ---
class SaveStore {
  constructor(dbName) {
    this.dbName = dbName;
    this.db = null;
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
  async listSlotSummaries(count = 24) {
    const out = [];
    for (let slot = 0; slot < count; slot++) {
      const raw = await this.get(`slot_${slot}`);
      if (raw) {
        try {
          const snap = JSON.parse(raw);
          out.push({
            slot,
            exists: true,
            saved_at_unix: snap.saved_at_unix ?? 0,
            summary: snap.summary ?? '',
          });
          continue;
        } catch { /* ignore malformed slot */ }
      }
      out.push({ slot, exists: false, saved_at_unix: 0, summary: '' });
    }
    return out;
  }
}

// --- WebAudio API Mixer (Master / BGM / Voice / SFX + Gapless Loop Regions) ---
class WebAudioMixer {
  constructor() {
    this.ctx = null;
    this.master = null;
    this.buses = {};
    this.channels = new Map();
    this.decodedCache = new Map();
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
  async play(cmd, bytes, onEnded) {
    const ctx = this.ensureContext();
    if (!ctx || !bytes) return;
    this.stop(cmd.channel, 0);
    let audioBuf = this.decodedCache.get(cmd.asset);
    if (!audioBuf) {
      try {
        audioBuf = await ctx.decodeAudioData(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength));
        this.decodedCache.set(cmd.asset, audioBuf);
      } catch {
        return;
      }
    }
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
    this.channels.set(cmd.channel, { src, gainNode, bus: cmd.bus });
    src.start(0);
  }
  stop(channel, fadeOutMs) {
    const cur = this.channels.get(channel);
    if (!cur || !this.ctx) return;
    this.channels.delete(channel);
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
    if (cur && this.ctx) {
      cur.gainNode.gain.setTargetAtTime(gain, this.ctx.currentTime, 0.02);
    }
  }
}

async function fetchPakWithProgress(url) {
  const res = await fetch(url, { cache: 'no-cache' });
  if (!res.ok) throw new Error(`HTTP ${res.status} fetching ${url}`);
  const total = Number(res.headers.get('content-length') || 0);
  if (!res.body || !total) {
    const buf = await res.arrayBuffer();
    setProgress(90);
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
    setProgress(15 + (loaded / total) * 75);
  }
  const out = new Uint8Array(loaded);
  let offset = 0;
  for (const c of chunks) {
    out.set(c, offset);
    offset += c.byteLength;
  }
  return out;
}

async function main() {
  setProgress(5, 'Initializing NIR Engine…');
  const wasmUrl = new URL('./player_web.js', import.meta.url).href;
  const [{ default: initWasm, WebEngine }, pakBytes] = await Promise.all([
    import(/* @vite-ignore */ wasmUrl),
    fetchPakWithProgress('./game.nirpak'),
  ]);
  await initWasm();

  setProgress(92, 'Opening saves…');
  const store = new SaveStore('nir_saves_v2');
  await store.open().catch(() => {});
  const prefsJson = (await store.get('prefs')) ?? '{}';
  const profileJson = (await store.get('profile')) ?? '{}';
  const slotsJson = JSON.stringify(await store.listSlotSummaries(24));

  const dpr = Math.min(window.devicePixelRatio || 1, 3);
  canvas.width = Math.max(1, Math.round(window.innerWidth * dpr));
  canvas.height = Math.max(1, Math.round(window.innerHeight * dpr));

  const engine = await WebEngine.create(canvas, pakBytes, prefsJson, profileJson, slotsJson, dpr);
  document.title = engine.title();

  const audio = new WebAudioMixer();
  try { audio.applyPreferences(JSON.parse(prefsJson)); } catch { /* default */ }
  const inflightImages = new Set();

  const onResize = () => {
    const r = Math.min(window.devicePixelRatio || 1, 3);
    const w = Math.max(1, Math.round(window.innerWidth * r));
    const h = Math.max(1, Math.round(window.innerHeight * r));
    canvas.width = w;
    canvas.height = h;
    engine.resize(w, h, r);
  };
  window.addEventListener('resize', onResize);

  const unlockAudio = () => audio.ensureContext();
  canvas.addEventListener('pointermove', (e) => {
    const rect = canvas.getBoundingClientRect();
    engine.pointer_move(e.clientX - rect.left, e.clientY - rect.top);
  });
  canvas.addEventListener('pointerdown', (e) => {
    unlockAudio();
    if (e.button === 2) {
      engine.cancel_or_menu();
      return;
    }
    if (e.button === 0) {
      const rect = canvas.getBoundingClientRect();
      engine.pointer_click(e.clientX - rect.left, e.clientY - rect.top);
    }
  });
  canvas.addEventListener('contextmenu', (e) => e.preventDefault());
  canvas.addEventListener('wheel', (e) => {
    e.preventDefault();
    const act = e.deltaY < 0 ? 'Rollback' : 'Advance';
    engine.dispatch_action_json(JSON.stringify(act));
  }, { passive: false });

  window.addEventListener('keydown', (e) => {
    unlockAudio();
    if (e.code === 'ControlLeft' || e.code === 'ControlRight') {
      engine.dispatch_action_json(JSON.stringify({ HoldSkip: true }));
    } else if (!e.repeat) {
      if (e.code === 'Space' || e.code === 'Enter') engine.dispatch_action_json('"Advance"');
      else if (e.code === 'Escape') engine.cancel_or_menu();
      else if (e.code === 'KeyA') engine.dispatch_action_json('"ToggleAuto"');
      else if (e.code === 'KeyS') engine.dispatch_action_json('"ToggleSkip"');
      else if (e.code === 'KeyH') engine.dispatch_action_json('"ToggleHideUi"');
      else if (e.code === 'PageUp') engine.dispatch_action_json('"Rollback"');
    }
  });
  window.addEventListener('keyup', (e) => {
    if (e.code === 'ControlLeft' || e.code === 'ControlRight') {
      engine.dispatch_action_json(JSON.stringify({ HoldSkip: false }));
    }
  });

  // Dev server live hot-reload (silently ignored in production static builds)
  try {
    const es = new EventSource('/__nir_events');
    es.addEventListener('reload', async () => {
      try {
        const fresh = await fetchPakWithProgress(`./game.nirpak?t=${Date.now()}`);
        engine.hot_reload(fresh);
        inflightImages.clear();
      } catch { /* retry on next edit */ }
    });
    es.onerror = () => es.close();
  } catch { /* static hosting without dev server */ }

  const handleCommand = (cmd) => {
    switch (cmd.op) {
      case 'decode_image': {
        if (inflightImages.has(cmd.id)) return;
        const bytes = engine.asset_bytes(cmd.id);
        if (!bytes) {
          engine.mark_asset_failed(cmd.id, 'missing bytes');
          return;
        }
        inflightImages.add(cmd.id);
        const blob = new Blob([bytes], { type: cmd.mime });
        createImageBitmap(blob, { premultiplyAlpha: 'premultiply' })
          .then((bmp) => {
            inflightImages.delete(cmd.id);
            engine.upload_image_bitmap(cmd.id, bmp);
            bmp.close();
          })
          .catch((err) => {
            inflightImages.delete(cmd.id);
            engine.mark_asset_failed(cmd.id, String(err));
          });
        break;
      }
      case 'play_audio': {
        const bytes = engine.asset_bytes(cmd.asset);
        audio.play(cmd, bytes, (ch) => engine.notify_audio_ended(ch));
        break;
      }
      case 'stop_audio':
        audio.stop(cmd.channel, cmd.fade_out_ms);
        break;
      case 'set_audio_gain':
        audio.setGain(cmd.channel, cmd.gain);
        break;
      case 'play_video': {
        const bytes = engine.asset_bytes(cmd.asset);
        if (!bytes || !videoEl) {
          engine.notify_video_finished();
          break;
        }
        const url = URL.createObjectURL(new Blob([bytes], { type: cmd.mime }));
        videoEl.src = url;
        videoEl.hidden = false;
        const finish = () => {
          videoEl.hidden = true;
          videoEl.pause();
          URL.revokeObjectURL(url);
          engine.notify_video_finished();
        };
        videoEl.onended = finish;
        videoEl.onclick = cmd.skippable ? finish : null;
        videoEl.play().catch(finish);
        break;
      }
      case 'stop_video':
        if (videoEl) {
          videoEl.pause();
          videoEl.hidden = true;
        }
        break;
      case 'save_slot':
        store.set(`slot_${cmd.slot}`, cmd.snapshot_json);
        break;
      case 'request_load_slot':
        store.get(`slot_${cmd.slot}`).then((json) => {
          if (json) engine.apply_loaded_snapshot_json(json);
        });
        break;
      case 'delete_slot':
        store.del(`slot_${cmd.slot}`);
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

  let lastTs = performance.now();
  const frame = (ts) => {
    const dt = ts - lastTs;
    lastTs = ts;
    const cmdsJson = engine.tick_and_render(dt, Date.now() / 1000);
    if (cmdsJson && cmdsJson !== '[]') {
      for (const cmd of JSON.parse(cmdsJson)) {
        handleCommand(cmd);
      }
    }
    requestAnimationFrame(frame);
  };
  requestAnimationFrame(frame);
}

main().catch((err) => {
  setProgress(0, `Error: ${err?.message ?? err}`);
  console.error(err);
});
