//! Unified Native Player for Windows, Linux, macOS, and Android.
//!
//! Combines `nir-runtime` + `nir-render` + zero-copy `memmap2` `.nirpak` (desktop & Android APK fd) +
//! multi-worker priority image decode pool (`urgent` ahead of prefetch) + `EventLoopProxy` wakeup +
//! streaming/LRU background `rodio`/`symphonia` audio + async indexed local saves.

pub mod audio;
pub mod pak;
pub mod storage;

use anyhow::{Context, Result};
use audio::AudioHost;
use nir_ir::{AssetId, AssetKind, MediaCodec, UiAction, FEATURE_DELTA_IMAGE};
use nir_render::{
    composite_delta_rgba, decode_image, parse_ktx2_level0, wgpu, DecodedKtx2, Renderer,
    RendererBackend,
};
use nir_runtime::{HostCommand, Runtime};
use pak::PakArchive;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use storage::LocalStorage;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};
use winit::window::{Fullscreen, Window, WindowId};

#[derive(Debug, Clone, Copy)]
enum UserEvent {
    ImageDecoded,
}

enum DecodedImage {
    Ok {
        id: AssetId,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    Ktx2 {
        id: AssetId,
        ktx: DecodedKtx2,
    },
    Err {
        id: AssetId,
        error: String,
    },
}

struct DecodeRequest {
    pak: Arc<PakArchive>,
    delta_bases: Arc<HashSet<AssetId>>,
    id: AssetId,
}

fn collect_delta_bases(pak: &PakArchive) -> Arc<HashSet<AssetId>> {
    let mut set = HashSet::new();
    if pak.program.features & FEATURE_DELTA_IMAGE != 0 {
        for a in &pak.program.assets {
            if let Some(d) = &a.delta {
                set.insert(d.base);
            }
        }
    }
    Arc::new(set)
}

type BaseCacheEntry = (u32, u32, Arc<Vec<u8>>, u64);

/// Small LRU cache of decoded premultiplied base images shared by `DecodePool` workers so
/// flipping through 20 consecutive CG/sprite expressions decodes the base image ONCE!
struct BaseRgbaCache {
    entries: HashMap<AssetId, BaseCacheEntry>,
    clock: u64,
}

impl BaseRgbaCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            clock: 0,
        }
    }

    fn get(&mut self, id: AssetId) -> Option<(u32, u32, Arc<Vec<u8>>)> {
        self.clock = self.clock.wrapping_add(1);
        let tick = self.clock;
        if let Some((w, h, px, last)) = self.entries.get_mut(&id) {
            *last = tick;
            return Some((*w, *h, Arc::clone(px)));
        }
        None
    }

    fn insert(&mut self, id: AssetId, w: u32, h: u32, px: Arc<Vec<u8>>) {
        const MAX_BASES: usize = 8;
        self.clock = self.clock.wrapping_add(1);
        self.entries.insert(id, (w, h, px, self.clock));
        while self.entries.len() > MAX_BASES {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, _, _, age))| *age)
                .map(|(&k, _)| k)
            {
                self.entries.remove(&oldest);
            } else {
                break;
            }
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
    }
}

/// Priority work queue shared across background image decode worker threads:
/// `urgent` (visible scene) requests always execute ahead of `prefetch` (lookahead) requests!
struct DecodeQueueState {
    urgent: VecDeque<DecodeRequest>,
    prefetch: VecDeque<DecodeRequest>,
    closed: bool,
}

struct DecodePool {
    state: Arc<(Mutex<DecodeQueueState>, Condvar)>,
    base_cache: Arc<Mutex<BaseRgbaCache>>,
}

impl DecodePool {
    fn spawn(res_tx: mpsc::Sender<DecodedImage>, proxy: EventLoopProxy<UserEvent>) -> Self {
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            .clamp(2, 4);
        let state = Arc::new((
            Mutex::new(DecodeQueueState {
                urgent: VecDeque::new(),
                prefetch: VecDeque::new(),
                closed: false,
            }),
            Condvar::new(),
        ));
        let base_cache = Arc::new(Mutex::new(BaseRgbaCache::new()));
        for idx in 0..workers {
            let q = Arc::clone(&state);
            let bc = Arc::clone(&base_cache);
            let tx = res_tx.clone();
            let p = proxy.clone();
            let _ = std::thread::Builder::new()
                .name(format!("nir-img-decode-{idx}"))
                .spawn(move || loop {
                    let req = {
                        let (lock, cv) = &*q;
                        let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
                        loop {
                            if let Some(r) = guard.urgent.pop_front() {
                                break r;
                            }
                            if let Some(r) = guard.prefetch.pop_front() {
                                break r;
                            }
                            if guard.closed {
                                return;
                            }
                            guard = cv.wait(guard).unwrap_or_else(|e| e.into_inner());
                        }
                    };
                    let msg = decode_asset_for_pool(&req.pak, &req.delta_bases, req.id, &bc);
                    if tx.send(msg).is_err() {
                        return;
                    }
                    let _ = p.send_event(UserEvent::ImageDecoded);
                });
        }
        Self { state, base_cache }
    }

    fn enqueue(&self, req: DecodeRequest, urgent: bool) {
        let (lock, cv) = &*self.state;
        let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        if urgent {
            // If already queued in prefetch, remove it from prefetch and promote to urgent!
            if let Some(pos) = guard.prefetch.iter().position(|r| r.id == req.id) {
                guard.prefetch.remove(pos);
            }
            guard.urgent.push_back(req);
        } else {
            guard.prefetch.push_back(req);
        }
        cv.notify_one();
    }

    fn promote(&self, id: AssetId) {
        let (lock, cv) = &*self.state;
        let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(pos) = guard.prefetch.iter().position(|r| r.id == id) {
            if let Some(req) = guard.prefetch.remove(pos) {
                guard.urgent.push_back(req);
                cv.notify_one();
            }
        }
    }

    fn clear_pending(&self) {
        let (lock, _) = &*self.state;
        let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        guard.urgent.clear();
        guard.prefetch.clear();
        if let Ok(mut bc) = self.base_cache.lock() {
            bc.clear();
        }
    }
}

fn decode_asset_for_pool(
    pak: &PakArchive,
    delta_bases: &HashSet<AssetId>,
    id: AssetId,
    base_cache: &Mutex<BaseRgbaCache>,
) -> DecodedImage {
    let Some(meta) = pak.program.asset(id) else {
        return DecodedImage::Err {
            id,
            error: "asset metadata missing".into(),
        };
    };
    let Some(bytes) = pak.asset_bytes(id) else {
        return DecodedImage::Err {
            id,
            error: "asset missing from nirpak".into(),
        };
    };

    if meta.codec == MediaCodec::Ktx2 {
        return match parse_ktx2_level0(bytes) {
            Ok(ktx) => DecodedImage::Ktx2 { id, ktx },
            Err(e) => DecodedImage::Err {
                id,
                error: e.to_string(),
            },
        };
    }

    if let Some(delta) = &meta.delta {
        let base_id = delta.base;
        let cached_base = base_cache.lock().ok().and_then(|mut g| g.get(base_id));
        let (bw, bh, base_rgba) = if let Some(hit) = cached_base {
            hit
        } else {
            let Some(base_bytes) = pak.asset_bytes(base_id) else {
                return DecodedImage::Err {
                    id,
                    error: "delta base asset missing from nirpak".into(),
                };
            };
            match decode_image(base_bytes) {
                Ok((w, h, rgba)) => {
                    let arc = Arc::new(rgba);
                    if let Ok(mut g) = base_cache.lock() {
                        g.insert(base_id, w, h, Arc::clone(&arc));
                    }
                    (w, h, arc)
                }
                Err(e) => {
                    return DecodedImage::Err {
                        id,
                        error: format!("delta base decode: {e}"),
                    };
                }
            }
        };
        let (pw, ph, patch_rgba) = match decode_image(bytes) {
            Ok(v) => v,
            Err(e) => {
                return DecodedImage::Err {
                    id,
                    error: format!("delta patch decode: {e}"),
                };
            }
        };
        return match composite_delta_rgba(&base_rgba, bw, bh, &patch_rgba, pw, ph, delta) {
            Ok(rgba) => DecodedImage::Ok {
                id,
                width: bw,
                height: bh,
                rgba,
            },
            Err(e) => DecodedImage::Err {
                id,
                error: e.to_string(),
            },
        };
    }

    match decode_image(bytes) {
        Ok((width, height, rgba)) => {
            // Only clone & cache decoded RGBA when this asset is actually referenced as a delta base!
            if delta_bases.contains(&id) {
                if let Ok(mut g) = base_cache.lock() {
                    g.insert(id, width, height, Arc::new(rgba.clone()));
                }
            }
            DecodedImage::Ok {
                id,
                width,
                height,
                rgba,
            }
        }
        Err(e) => DecodedImage::Err {
            id,
            error: e.to_string(),
        },
    }
}

impl Drop for DecodePool {
    fn drop(&mut self) {
        let (lock, cv) = &*self.state;
        let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        guard.closed = true;
        guard.urgent.clear();
        guard.prefetch.clear();
        cv.notify_all();
    }
}

/// Runs the unified native player on Desktop (Windows / Linux / macOS).
pub fn run_desktop(pak_path: &Path, watch_hot_reload: bool) -> Result<()> {
    let pak = Arc::new(PakArchive::open_file(pak_path)?);
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .context("creating winit event loop")?;
    run_with_event_loop(
        pak,
        None,
        watch_hot_reload,
        event_loop,
        #[cfg(target_os = "android")]
        None,
    )
}

/// Runs the unified native player on Android (`NativeActivity`), memory-mapping
/// page-aligned STORED `assets/game.nirpak` directly from the APK via `open_file_descriptor`!
#[cfg(target_os = "android")]
pub fn run_android(android_app: android_activity::AndroidApp) -> Result<()> {
    use std::ffi::CString;
    use std::fs::File;
    use winit::platform::android::EventLoopBuilderExtAndroid;

    let name = CString::new("game.nirpak").unwrap();
    let mut asset = android_app
        .asset_manager()
        .open(&name)
        .context("opening assets/game.nirpak from APK AssetManager")?;

    // Zero-copy mmap directly from APK fd when stored uncompressed (4096-byte page-aligned)!
    let pak = match asset.open_file_descriptor() {
        Ok(opened) => {
            let file = File::from(opened.fd);
            match PakArchive::from_fd_region(&file, opened.offset as u64, opened.size) {
                Ok(mapped) => Arc::new(mapped),
                Err(_) => Arc::new(PakArchive::from_bytes(asset.buffer()?.to_vec())?),
            }
        }
        Err(_) => Arc::new(PakArchive::from_bytes(asset.buffer()?.to_vec())?),
    };

    let data_dir = android_app.internal_data_path();
    let app_handle = android_app.clone();
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .with_android_app(android_app)
        .build()
        .context("creating Android event loop")?;
    run_with_event_loop(pak, data_dir, false, event_loop, Some(app_handle))
}

/// Returns the host's local UTC offset in minutes (e.g. `+480` for UTC+8) so save timestamps
/// render in local time instead of UTC.
fn local_tz_offset_mins() -> i32 {
    #[cfg(unix)]
    {
        unsafe {
            let now = libc::time(std::ptr::null_mut());
            let mut tm: libc::tm = std::mem::zeroed();
            if !libc::localtime_r(&now, &mut tm).is_null() {
                return (tm.tm_gmtoff / 60) as i32;
            }
        }
    }
    0
}

/// Initializes a native GPU renderer, preferring the platform's primary API
/// (Vulkan on Linux/Android, DX12 on Windows, Metal on macOS) and falling back
/// to OpenGL/GLES when primary initialization fails.
fn create_native_renderer(
    win: &Arc<Window>,
    width: u32,
    height: u32,
) -> Result<(wgpu::Instance, Renderer)> {
    let primary_backend = if cfg!(target_os = "windows") {
        RendererBackend::Dx12
    } else if cfg!(target_os = "macos") {
        RendererBackend::Metal
    } else {
        RendererBackend::Vulkan
    };

    let primary_instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::PRIMARY,
        ..Default::default()
    });
    let primary_err = match primary_instance.create_surface(win.clone()) {
        Ok(surface) => match pollster::block_on(Renderer::new(
            primary_instance.clone(),
            surface,
            width,
            height,
            primary_backend,
        )) {
            Ok(renderer) => return Ok((primary_instance, renderer)),
            Err(e) => format!("{primary_backend:?} renderer init failed: {e}"),
        },
        Err(e) => format!("{primary_backend:?} surface creation failed: {e}"),
    };

    #[cfg(not(target_os = "macos"))]
    {
        let gl_instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::GL,
            ..Default::default()
        });
        match gl_instance.create_surface(win.clone()) {
            Ok(surface) => match pollster::block_on(Renderer::new(
                gl_instance.clone(),
                surface,
                width,
                height,
                RendererBackend::Gles,
            )) {
                Ok(renderer) => Ok((gl_instance, renderer)),
                Err(gl_err) => {
                    anyhow::bail!("{primary_err}; GLES fallback failed: {gl_err}");
                }
            },
            Err(gl_err) => {
                anyhow::bail!("{primary_err}; GLES fallback surface failed: {gl_err}");
            }
        }
    }

    #[cfg(target_os = "macos")]
    anyhow::bail!("{primary_err}")
}

fn run_with_event_loop(
    pak: Arc<PakArchive>,
    storage_base: Option<PathBuf>,
    watch_hot_reload: bool,
    event_loop: EventLoop<UserEvent>,
    #[cfg(target_os = "android")] android_app: Option<android_activity::AndroidApp>,
) -> Result<()> {
    let storage = LocalStorage::open(&pak.program.meta.game_id, storage_base)?;
    let prefs = storage.load_preferences();
    let profile = storage.load_profile();
    let slots = storage.list_slot_summaries(pak.program.meta.save_slots);

    let audio = AudioHost::new(&prefs);
    let mut runtime = Runtime::new(pak.program.clone(), prefs, profile, slots)
        .map_err(|e| anyhow::anyhow!("initializing runtime: {e}"))?;
    runtime.set_tz_offset_mins(local_tz_offset_mins());
    runtime.set_autosave_summary(storage.named_summary("autosave", u32::MAX));
    runtime.set_quicksave_summary(storage.named_summary("quicksave", u32::MAX - 1));
    if cfg!(target_os = "android") {
        runtime.set_input_mode(nir_ui::InputMode::Touch);
    }

    let (res_tx, res_rx) = mpsc::channel::<DecodedImage>();
    let decode_pool = DecodePool::spawn(res_tx, event_loop.create_proxy());

    let delta_bases = collect_delta_bases(&pak);
    let mut app = NativeApp {
        pak,
        delta_bases,
        runtime,
        storage,
        audio,
        window: None,
        instance: wgpu::Instance::new(&wgpu::InstanceDescriptor::default()),
        renderer: None,
        decode_pool,
        res_rx,
        pending_decodes: BTreeMap::new(),
        last_frame: Instant::now(),
        last_reload_check: Instant::now(),
        last_back_press: None,
        next_wakeup_at: None,
        was_idle_sleeping: false,
        watch_hot_reload,
        cursor_logical: [0.0, 0.0],
        left_mouse_down: false,
        scale_factor: 1.0,
        window_size: [1280, 720],
        should_quit: false,
        init_error: None,
        #[cfg(target_os = "android")]
        android_app,
        #[cfg(target_os = "android")]
        keep_screen_on: false,
    };
    event_loop.run_app(&mut app).context("winit event loop")?;
    app.runtime.flush_pending_saves();
    app.process_commands();
    app.storage.flush();
    if let Some(err) = app.init_error.take() {
        return Err(err);
    }
    Ok(())
}

struct NativeApp {
    pak: Arc<PakArchive>,
    delta_bases: Arc<HashSet<AssetId>>,
    runtime: Runtime,
    storage: LocalStorage,
    audio: AudioHost,
    window: Option<Arc<Window>>,
    instance: wgpu::Instance,
    renderer: Option<Renderer>,
    decode_pool: DecodePool,
    res_rx: mpsc::Receiver<DecodedImage>,
    /// Tracks in-flight image decodes and whether they have already been promoted to `urgent`.
    pending_decodes: BTreeMap<AssetId, bool>,
    last_frame: Instant,
    last_reload_check: Instant,
    last_back_press: Option<Instant>,
    next_wakeup_at: Option<Instant>,
    was_idle_sleeping: bool,
    watch_hot_reload: bool,
    cursor_logical: [f32; 2],
    left_mouse_down: bool,
    scale_factor: f32,
    window_size: [u32; 2],
    should_quit: bool,
    init_error: Option<anyhow::Error>,
    #[cfg(target_os = "android")]
    android_app: Option<android_activity::AndroidApp>,
    #[cfg(target_os = "android")]
    keep_screen_on: bool,
}

impl NativeApp {
    fn sync_android_insets(&mut self) {
        #[cfg(target_os = "android")]
        if let Some(app) = &self.android_app {
            let rect = app.content_rect();
            let scale = self.scale_factor.max(0.5);
            let pw = self.window_size[0] as i32;
            let ph = self.window_size[1] as i32;
            if rect.right > rect.left && rect.bottom > rect.top && pw > 0 && ph > 0 {
                let insets = nir_ui::SafeInsets {
                    left: (rect.left.max(0) as f32) / scale,
                    top: (rect.top.max(0) as f32) / scale,
                    right: ((pw - rect.right).max(0) as f32) / scale,
                    bottom: ((ph - rect.bottom).max(0) as f32) / scale,
                };
                self.runtime.set_safe_insets(insets);
            }
        }
    }

    fn sync_wake_lock(&mut self) {
        #[cfg(target_os = "android")]
        if let Some(app) = &self.android_app {
            let want = self.runtime.is_auto_or_skip_active();
            if want != self.keep_screen_on {
                self.keep_screen_on = want;
                use android_activity::WindowManagerFlags;
                if want {
                    app.set_window_flags(
                        WindowManagerFlags::KEEP_SCREEN_ON,
                        WindowManagerFlags::empty(),
                    );
                } else {
                    app.set_window_flags(
                        WindowManagerFlags::empty(),
                        WindowManagerFlags::KEEP_SCREEN_ON,
                    );
                }
            }
        }
    }

    fn handle_back_action(&mut self) {
        if self.runtime.screen == nir_ui::Screen::Title && cfg!(target_os = "android") {
            let now = Instant::now();
            if self
                .last_back_press
                .is_some_and(|prev| now.duration_since(prev) <= Duration::from_millis(2500))
            {
                self.should_quit = true;
            } else {
                self.last_back_press = Some(now);
                let msg = self
                    .runtime
                    .messages()
                    .get(&self.runtime.prefs.ui_locale, "back-again-exit");
                self.runtime.show_toast(msg, 2200);
            }
        } else {
            self.runtime.cancel_or_menu();
        }
    }

    fn upload_all_fonts(&mut self) {
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let font_ids: Vec<AssetId> = self
            .pak
            .program
            .assets
            .iter()
            .filter(|a| a.kind == AssetKind::Font)
            .map(|a| a.id)
            .collect();
        for id in font_ids {
            if renderer.has_font(id) {
                self.runtime.mark_asset_loaded(id);
                continue;
            }
            if let Some(bytes) = self.pak.asset_bytes(id) {
                if renderer.load_font(id, bytes.to_vec()).is_ok() {
                    self.runtime.mark_asset_loaded(id);
                }
            }
        }
    }

    fn drain_decoded_images(&mut self) -> bool {
        let mut any = false;
        while let Ok(msg) = self.res_rx.try_recv() {
            any = true;
            match msg {
                DecodedImage::Ok {
                    id,
                    width,
                    height,
                    rgba,
                } => {
                    self.pending_decodes.remove(&id);
                    if self.runtime.wants_style_sample(id) {
                        self.runtime.analyze_asset_rgba(id, width, height, &rgba);
                    }
                    if let Some(renderer) = &mut self.renderer {
                        let _ = renderer.upload_rgba_premultiplied(id, width, height, &rgba);
                        let evicted = renderer.take_evicted_assets();
                        if !evicted.is_empty() {
                            self.runtime.mark_assets_evicted(&evicted);
                        }
                    }
                    self.runtime.mark_asset_loaded(id);
                }
                DecodedImage::Ktx2 { id, ktx } => {
                    self.pending_decodes.remove(&id);
                    if let Some(renderer) = &mut self.renderer {
                        let _ = renderer.upload_decoded_ktx2(id, &ktx);
                        let evicted = renderer.take_evicted_assets();
                        if !evicted.is_empty() {
                            self.runtime.mark_assets_evicted(&evicted);
                        }
                    }
                    self.runtime.mark_asset_loaded(id);
                }
                DecodedImage::Err { id, error } => {
                    self.pending_decodes.remove(&id);
                    self.runtime.mark_asset_failed(id, &error);
                }
            }
        }
        any
    }

    fn process_commands(&mut self) {
        let cmds = self.runtime.take_commands();
        for cmd in cmds {
            match cmd {
                HostCommand::LoadAsset { id, urgent } => {
                    let Some(meta) = self.pak.program.asset(id) else {
                        continue;
                    };
                    match meta.kind {
                        AssetKind::Image => {
                            if self.renderer.as_ref().is_some_and(|r| r.has_texture(id)) {
                                self.runtime.mark_asset_loaded(id);
                            } else {
                                match self.pending_decodes.get(&id).copied() {
                                    None => {
                                        self.pending_decodes.insert(id, urgent);
                                        self.decode_pool.enqueue(
                                            DecodeRequest {
                                                pak: Arc::clone(&self.pak),
                                                delta_bases: Arc::clone(&self.delta_bases),
                                                id,
                                            },
                                            urgent,
                                        );
                                    }
                                    Some(false) if urgent => {
                                        self.pending_decodes.insert(id, true);
                                        self.decode_pool.promote(id);
                                    }
                                    _ => {}
                                }
                            }
                        }
                        AssetKind::Font => {
                            if let Some(renderer) = &mut self.renderer {
                                if !renderer.has_font(id) {
                                    if let Some(bytes) = self.pak.asset_bytes(id) {
                                        let _ = renderer.load_font(id, bytes.to_vec());
                                    }
                                }
                            }
                            self.runtime.mark_asset_loaded(id);
                        }
                        AssetKind::Audio => {
                            // Pre-decode short Voice/SFX clips on background audio worker!
                            self.audio.prefetch(&self.pak, id, meta.codec.ext());
                            self.runtime.mark_asset_loaded(id);
                        }
                        AssetKind::Video | AssetKind::Raw => {
                            self.runtime.mark_asset_loaded(id);
                        }
                    }
                }
                HostCommand::HintPinnedAssets(pinned) => {
                    if let Some(renderer) = &mut self.renderer {
                        let evicted = renderer.evict_lru(&pinned);
                        if !evicted.is_empty() {
                            self.runtime.mark_assets_evicted(&evicted);
                        }
                    }
                }
                HostCommand::PlayAudio {
                    channel,
                    bus,
                    asset,
                    gain,
                    looped,
                    fade_in_us,
                    loop_region,
                    ..
                } => {
                    if let Some(meta) = self.pak.program.asset(asset) {
                        self.audio.play(
                            &self.pak,
                            asset,
                            channel,
                            bus,
                            meta.codec.ext(),
                            meta.sample_rate,
                            meta.channels,
                            gain,
                            looped,
                            fade_in_us,
                            loop_region,
                        );
                    }
                }
                HostCommand::StopAudio {
                    channel,
                    fade_out_us,
                } => {
                    self.audio.stop(channel, fade_out_us);
                }
                HostCommand::SetAudioGain { channel, gain } => {
                    self.audio.set_channel_gain(channel, gain);
                }
                HostCommand::PauseAudioBus { bus, paused } => {
                    self.audio.set_bus_paused(bus, paused);
                }
                HostCommand::PlayVideo { .. } => {
                    // Lightweight fallback notice when no video decoder plugin is bound.
                    self.runtime.notify_video_finished();
                }
                HostCommand::StopVideo => {}
                HostCommand::SaveSlot { slot, snapshot } => {
                    if let Err(e) = self.storage.save_slot(slot, &snapshot) {
                        self.runtime.show_toast(format!("Save failed: {e}"), 3000);
                    }
                }
                HostCommand::RequestLoadSlot(slot) => match self.storage.load_slot(slot) {
                    Ok(snap) => self.runtime.apply_loaded_snapshot(snap),
                    Err(e) => self.runtime.show_toast(format!("Load failed: {e}"), 3000),
                },
                HostCommand::DeleteSlot(slot) => {
                    let _ = self.storage.delete_slot(slot);
                }
                HostCommand::SaveAuto { snapshot } => {
                    let _ = self.storage.save_named("autosave", &snapshot);
                }
                HostCommand::RequestLoadAuto => match self.storage.load_named("autosave") {
                    Ok(snap) => self.runtime.apply_loaded_snapshot(snap),
                    Err(e) => self.runtime.show_toast(format!("Load failed: {e}"), 3000),
                },
                HostCommand::SaveQuick { snapshot } => {
                    let _ = self.storage.save_named("quicksave", &snapshot);
                }
                HostCommand::RequestLoadQuick => match self.storage.load_named("quicksave") {
                    Ok(snap) => self.runtime.apply_loaded_snapshot(snap),
                    Err(e) => self.runtime.show_toast(format!("Load failed: {e}"), 3000),
                },
                HostCommand::SaveProfile(profile) => {
                    let _ = self.storage.save_profile(&profile);
                }
                HostCommand::SavePreferences(prefs) => {
                    self.audio.apply_preferences(&prefs);
                    let _ = self.storage.save_preferences(&prefs);
                }
                HostCommand::ReturnToShelf | HostCommand::Quit => {
                    self.should_quit = true;
                }
            }
        }
    }

    fn tick_and_render(&mut self) {
        let now = Instant::now();
        let delta_us = if self.was_idle_sleeping {
            self.was_idle_sleeping = false;
            0
        } else {
            now.saturating_duration_since(self.last_frame)
                .as_micros()
                .min(6_000_000) as u64
        };
        self.last_frame = now;
        self.next_wakeup_at = None;

        // Optional live hot-reload watch on `game.nirpak`
        if self.watch_hot_reload
            && now.duration_since(self.last_reload_check) >= Duration::from_millis(250)
        {
            self.last_reload_check = now;
            if let Some(new_pak) = self.pak.check_reloaded() {
                self.delta_bases = collect_delta_bases(&new_pak);
                self.pak = new_pak;
                self.pending_decodes.clear();
                self.decode_pool.clear_pending();
                self.audio.clear_cache();
                self.upload_all_fonts();
                self.runtime.apply_hot_reload(self.pak.program.clone());
            }
        }

        self.drain_decoded_images();
        for ended_ch in self.audio.poll_ended() {
            self.runtime.notify_audio_ended(ended_ch);
        }

        let unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.runtime.set_unix_time(unix);
        self.runtime.tick(delta_us);
        self.process_commands();
        self.sync_wake_lock();

        if self.renderer.as_ref().is_some_and(Renderer::is_lost) {
            if let (Some(win), Some(renderer)) = (&self.window, &mut self.renderer) {
                if let Ok(surface) = self.instance.create_surface(win.clone()) {
                    renderer.rebind_surface(surface);
                    renderer.clear_textures();
                    self.pending_decodes.clear();
                    self.runtime.invalidate_all_textures();
                    self.process_commands();
                }
            }
        }

        let lw = (self.window_size[0] as f32 / self.scale_factor).max(1.0);
        let lh = (self.window_size[1] as f32 / self.scale_factor).max(1.0);
        if let Some(packet) = self.runtime.prepare_frame(lw, lh) {
            if let Some(renderer) = &mut self.renderer {
                let _ = renderer.render(packet, self.scale_factor);
                if renderer.has_missing_glyphs() && renderer.ensure_system_fallback_fonts() {
                    self.runtime.mark_fonts_changed();
                }
            }
        }
    }
}

impl ApplicationHandler<UserEvent> for NativeApp {
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::ImageDecoded => {
                if self.drain_decoded_images() {
                    if let Some(win) = &self.window {
                        win.request_redraw();
                    }
                }
            }
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.runtime.set_host_paused(false);
        self.audio.set_host_paused(false);
        if self.window.is_none() {
            let sw = self.pak.program.meta.stage_width;
            let sh = self.pak.program.meta.stage_height;
            let attrs = Window::default_attributes()
                .with_title(&self.pak.program.meta.title)
                .with_inner_size(LogicalSize::new(sw.max(640) as f64, sh.max(360) as f64));
            let win = match event_loop.create_window(attrs) {
                Ok(w) => Arc::new(w),
                Err(e) => {
                    let err = anyhow::anyhow!("creating window failed: {e}");
                    eprintln!("player-native: {err:#}");
                    self.init_error = Some(err);
                    event_loop.exit();
                    return;
                }
            };
            let phys = win.inner_size();
            self.window_size = [phys.width.max(1), phys.height.max(1)];
            self.scale_factor = win.scale_factor() as f32;

            match create_native_renderer(&win, self.window_size[0], self.window_size[1]) {
                Ok((instance, renderer)) => {
                    self.instance = instance;
                    self.renderer = Some(renderer);
                    self.upload_all_fonts();
                }
                Err(err) => {
                    eprintln!("player-native: GPU initialization failed: {err:#}");
                    self.init_error = Some(err);
                    event_loop.exit();
                    return;
                }
            }
            self.window = Some(win);
            self.sync_android_insets();
            self.runtime.dirty = true;
            self.process_commands();
        } else if let Some(win) = self.window.clone() {
            if let Some(renderer) = &mut self.renderer {
                if let Ok(surface) = self.instance.create_surface(win.clone()) {
                    renderer.rebind_surface(surface);
                }
            }
            self.sync_android_insets();
            self.runtime.dirty = true;
            self.process_commands();
            win.request_redraw();
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        self.runtime.set_host_paused(true);
        self.audio.set_host_paused(true);
        self.sync_wake_lock();
        self.runtime.flush_pending_saves();
        self.process_commands();
        self.storage.flush();
        if let Some(renderer) = &mut self.renderer {
            renderer.release_surface();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                self.runtime.flush_pending_saves();
                self.process_commands();
                self.storage.flush();
                event_loop.exit();
            }
            WindowEvent::Focused(focused) => {
                if !focused {
                    self.left_mouse_down = false;
                    self.runtime.pointer_cancel();
                    self.runtime.dispatch(UiAction::HoldSkip(false));
                }
                if cfg!(target_os = "android") {
                    self.runtime.set_host_paused(!focused);
                    self.audio.set_host_paused(!focused);
                    self.sync_wake_lock();
                    if !focused {
                        self.runtime.flush_pending_saves();
                    }
                    self.process_commands();
                }
            }
            WindowEvent::Resized(size) => {
                self.window_size = [size.width.max(1), size.height.max(1)];
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(self.window_size[0], self.window_size[1]);
                }
                self.sync_android_insets();
                self.runtime.dirty = true;
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale_factor = scale_factor as f32;
                self.sync_android_insets();
                self.runtime.dirty = true;
            }
            WindowEvent::Touch(touch) => {
                let lx = touch.location.x as f32 / self.scale_factor;
                let ly = touch.location.y as f32 / self.scale_factor;
                match touch.phase {
                    TouchPhase::Started => self.runtime.touch_start(touch.id, lx, ly),
                    TouchPhase::Moved => self.runtime.touch_move(touch.id, lx, ly),
                    TouchPhase::Ended => self.runtime.touch_end(touch.id, lx, ly),
                    TouchPhase::Cancelled => self.runtime.touch_cancel(touch.id),
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_logical = [
                    position.x as f32 / self.scale_factor,
                    position.y as f32 / self.scale_factor,
                ];
                if self.left_mouse_down {
                    self.runtime
                        .pointer_drag(self.cursor_logical[0], self.cursor_logical[1]);
                } else {
                    self.runtime
                        .set_hover(self.cursor_logical[0], self.cursor_logical[1]);
                    if let Some(win) = &self.window {
                        win.set_cursor(if self.runtime.has_hovered_hit() {
                            winit::window::CursorIcon::Pointer
                        } else {
                            winit::window::CursorIcon::Default
                        });
                    }
                }
            }
            WindowEvent::CursorLeft { .. } => {
                if !self.left_mouse_down {
                    self.runtime.set_hover(-1.0, -1.0);
                    if let Some(win) = &self.window {
                        win.set_cursor(winit::window::CursorIcon::Default);
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => match (button, state) {
                (MouseButton::Left, ElementState::Pressed) => {
                    self.left_mouse_down = true;
                    self.runtime
                        .pointer_down(self.cursor_logical[0], self.cursor_logical[1]);
                }
                (MouseButton::Left, ElementState::Released) => {
                    self.left_mouse_down = false;
                    self.runtime
                        .pointer_up(self.cursor_logical[0], self.cursor_logical[1]);
                }
                (MouseButton::Right, ElementState::Pressed) => {
                    self.runtime.cancel_or_menu();
                }
                _ => {}
            },
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => (p.y as f32) / 30.0,
                };
                if dy.abs() > 0.2 {
                    // winit +y = scroll up (matches browser -deltaY)
                    self.runtime.scroll_wheel(-dy);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                if pressed
                    && !event.repeat
                    && matches!(event.logical_key, Key::Named(NamedKey::GoBack))
                {
                    self.handle_back_action();
                } else if let PhysicalKey::Code(code) = event.physical_key {
                    match code {
                        KeyCode::ControlLeft | KeyCode::ControlRight => {
                            self.runtime.dispatch(UiAction::HoldSkip(pressed));
                        }
                        KeyCode::Space | KeyCode::Enter if pressed && !event.repeat => {
                            self.runtime.dispatch(UiAction::Advance);
                        }
                        KeyCode::Escape | KeyCode::BrowserBack if pressed && !event.repeat => {
                            self.handle_back_action();
                        }
                        KeyCode::KeyA if pressed && !event.repeat => {
                            self.runtime.dispatch(UiAction::ToggleAuto);
                        }
                        KeyCode::KeyS if pressed && !event.repeat => {
                            self.runtime.dispatch(UiAction::ToggleSkip);
                        }
                        KeyCode::KeyH if pressed && !event.repeat => {
                            self.runtime.dispatch(UiAction::ToggleHideUi);
                        }
                        KeyCode::PageUp if pressed => {
                            self.runtime.dispatch(UiAction::Rollback);
                        }
                        KeyCode::F5 if pressed && !event.repeat => {
                            self.runtime.dispatch(UiAction::QuickSave);
                        }
                        KeyCode::F9 if pressed && !event.repeat => {
                            self.runtime.dispatch(UiAction::QuickLoad);
                        }
                        KeyCode::KeyV | KeyCode::KeyR if pressed && !event.repeat => {
                            self.runtime.dispatch(UiAction::ReplayVoice);
                        }
                        KeyCode::KeyB if pressed && !event.repeat => {
                            self.runtime.dispatch(UiAction::ToggleBilingual);
                        }
                        KeyCode::F11 if pressed && !event.repeat => {
                            if let Some(win) = &self.window {
                                if win.fullscreen().is_some() {
                                    win.set_fullscreen(None);
                                } else {
                                    win.set_fullscreen(Some(Fullscreen::Borderless(None)));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                self.tick_and_render();
                if self.should_quit {
                    self.runtime.flush_pending_saves();
                    self.process_commands();
                    self.storage.flush();
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.drain_decoded_images();
        for ended_ch in self.audio.poll_ended() {
            self.runtime.notify_audio_ended(ended_ch);
        }
        // NOTE: Do NOT poll while waiting on `pending_decodes` — background workers wake us
        // immediately via `EventLoopProxy::send_event(UserEvent::ImageDecoded)`!
        let now = Instant::now();
        let next_delay_us = match (
            self.runtime.next_wakeup_delay_us(),
            self.audio.next_wakeup_delay_us(),
        ) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        if self.runtime.dirty {
            self.next_wakeup_at = None;
            if let Some(win) = &self.window {
                win.request_redraw();
            }
            if let Some(delay_us) = next_delay_us {
                let wait_us = delay_us.max(16_000);
                event_loop
                    .set_control_flow(ControlFlow::WaitUntil(now + Duration::from_micros(wait_us)));
            } else {
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            return;
        }

        if let Some(delay_us) = next_delay_us {
            self.was_idle_sleeping = false;
            if delay_us <= 16_000 {
                self.next_wakeup_at = None;
                if let Some(win) = &self.window {
                    win.request_redraw();
                }
                event_loop
                    .set_control_flow(ControlFlow::WaitUntil(now + Duration::from_millis(16)));
            } else if self.next_wakeup_at.is_some_and(|at| now >= at) {
                // Exact timer deadline reached -> run one tick now!
                self.next_wakeup_at = None;
                if let Some(win) = &self.window {
                    win.request_redraw();
                }
                event_loop
                    .set_control_flow(ControlFlow::WaitUntil(now + Duration::from_millis(16)));
            } else {
                // Sleep precisely until next Auto / Toast / Typewriter / Audio / Timer event (0% CPU between wakes!)
                let target = self
                    .next_wakeup_at
                    .unwrap_or_else(|| now + Duration::from_micros(delay_us.min(6_000_000)));
                self.next_wakeup_at = Some(target);
                event_loop.set_control_flow(ControlFlow::WaitUntil(target));
            }
        } else if self.watch_hot_reload {
            self.next_wakeup_at = None;
            event_loop.set_control_flow(ControlFlow::WaitUntil(now + Duration::from_millis(250)));
            if let Some(win) = &self.window {
                win.request_redraw();
            }
        } else {
            self.next_wakeup_at = None;
            self.was_idle_sleeping = true;
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_timezone_offset_is_within_valid_range() {
        let mins = local_tz_offset_mins();
        assert!(
            (-720..=840).contains(&mins),
            "unexpected timezone offset: {mins} mins"
        );
    }

    #[test]
    fn native_wgpu_backends_are_compiled_in() {
        let enabled = wgpu::Instance::enabled_backend_features();
        #[cfg(target_os = "macos")]
        assert!(
            enabled.contains(wgpu::Backends::METAL),
            "macOS build must enable wgpu Metal backend (enabled: {enabled:?})"
        );
        #[cfg(target_os = "windows")]
        assert!(
            enabled.contains(wgpu::Backends::DX12) || enabled.contains(wgpu::Backends::VULKAN),
            "Windows build must enable wgpu DX12/Vulkan backend (enabled: {enabled:?})"
        );
        #[cfg(all(unix, not(target_os = "macos")))]
        assert!(
            enabled.contains(wgpu::Backends::VULKAN) && enabled.contains(wgpu::Backends::GL),
            "Linux/Android build must enable wgpu Vulkan + GLES backends (enabled: {enabled:?})"
        );

        // Verify a native Instance can be constructed without panicking and enumerate adapters cleanly.
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY | wgpu::Backends::GL,
            ..Default::default()
        });
        let _adapters = instance.enumerate_adapters(wgpu::Backends::PRIMARY | wgpu::Backends::GL);
    }
}
