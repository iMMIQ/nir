//! Unified Native Player for Windows, Linux, macOS, and Android.
//!
//! Combines `nir-runtime` + `nir-render` + zero-copy `memmap2` `.nirpak` (desktop & Android APK fd) +
//! multi-worker priority image decode pool (`urgent` ahead of prefetch) + `EventLoopProxy` wakeup +
//! streaming/LRU background `rodio`/`symphonia` audio + async indexed local saves.

pub mod android_host;
pub mod android_video;
pub mod audio;
pub mod desktop;
pub mod gamepad;
pub mod linux_dbus;
pub mod pak;
pub mod shelf;
pub mod storage;

use anyhow::{Context, Result};
use audio::AudioHost;
use gamepad::{GamepadEvent, GamepadHost};
use linux_dbus::{LinuxDbusHost, MediaKeyAction};
use nir_ir::{AssetId, AssetKind, MediaCodec, UiAction, FEATURE_DELTA_IMAGE};
use nir_render::{
    composite_delta_rgba, decode_image, encode_png_rgba, parse_ktx2_level0, resize_rgba_icon, wgpu,
    DecodedKtx2, Renderer, RendererBackend,
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
use winit::window::{Fullscreen, Icon, Window, WindowId};

/// Hide idle cursor after 3.2s during quiet story reading (matches Web player cinema fade!).
const IDLE_CURSOR_HIDE_DELAY: Duration = Duration::from_millis(3200);
/// Poll Linux sysfs battery status every 60s when active (zero overhead).
const BATTERY_POLL_INTERVAL: Duration = Duration::from_secs(60);

/// Optional desktop launch settings passed from `main.rs` CLI arguments.
#[derive(Debug, Clone, Default)]
pub struct DesktopOptions {
    pub watch_hot_reload: bool,
    pub resume_latest: bool,
    pub patches: Vec<(String, PathBuf)>,
}

#[derive(Debug, Clone, Copy)]
enum UserEvent {
    ImageDecoded,
    GamepadInput,
    MediaKey,
    #[cfg(target_os = "android")]
    VideoFrame,
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
    run_desktop_with_options(
        pak_path,
        DesktopOptions {
            watch_hot_reload,
            ..Default::default()
        },
    )
}

/// Runs the unified native player on Desktop with CLI options (`--watch`, `--resume`, `--patch`).
pub fn run_desktop_with_options(pak_path: &Path, opts: DesktopOptions) -> Result<()> {
    let pak = Arc::new(PakArchive::open_file(pak_path)?);
    let recent_file = storage::default_data_dir().join("nir").join("recent_games.json");
    shelf::record_recent_game(&recent_file, pak_path);
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .context("creating winit event loop")?;
    run_with_event_loop(
        Some(pak),
        None,
        None,
        opts,
        event_loop,
        #[cfg(target_os = "android")]
        None,
    )
}

/// Runs the built-in universal `.nirpak` Shelf on Desktop (Windows / Linux / macOS).
pub fn run_desktop_shelf(shelf_dir: Option<&Path>, opts: DesktopOptions) -> Result<()> {
    let data_root = storage::default_data_dir();
    let mut scan_dirs = Vec::new();
    if let Some(dir) = shelf_dir {
        scan_dirs.push(dir.to_path_buf());
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let exe_games = parent.join("games");
            if exe_games.is_dir() {
                scan_dirs.push(exe_games);
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let cwd_games = cwd.join("games");
        if cwd_games.is_dir() {
            scan_dirs.push(cwd_games);
        }
    }
    scan_dirs.push(data_root.join("nir").join("games"));

    let shelf_state = shelf::ShelfState::new(scan_dirs, data_root, false, false);
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .context("creating winit event loop")?;
    run_with_event_loop(
        None,
        Some(shelf_state),
        None,
        opts,
        event_loop,
        #[cfg(target_os = "android")]
        None,
    )
}

/// Runs the unified native player on Android (`NativeActivity`):
/// - Single-game APK: memory-maps 16 KiB-aligned STORED `assets/game.nirpak` directly via `mmap`!
/// - Universal Player APK (no bundled `assets/game.nirpak`): launches the built-in touch Shelf
///   scanning `<external_data_path>/games/*.nirpak` so users can play any `.nirpak`!
#[cfg(target_os = "android")]
pub fn run_android(android_app: android_activity::AndroidApp) -> Result<()> {
    use std::ffi::CString;
    use std::fs::File;
    use winit::platform::android::EventLoopBuilderExtAndroid;

    let data_dir = android_app.internal_data_path();
    let ext_dir = android_app.external_data_path();

    let name = CString::new("game.nirpak").unwrap();
    let bundled_pak = if let Some(mut asset) = android_app.asset_manager().open(&name) {
        // Zero-copy mmap directly from APK fd when stored uncompressed & page-aligned (16 KiB)!
        let pak = match asset.open_file_descriptor() {
            Ok(opened) => {
                let file = File::from(opened.fd);
                match PakArchive::from_fd_region(&file, opened.offset as u64, opened.size) {
                    Ok(mapped) => {
                        android_host::log_info(&format!(
                            "game.nirpak zero-copy mmap ready (offset={}, size={})",
                            opened.offset, opened.size
                        ));
                        Arc::new(mapped)
                    }
                    Err(e) => {
                        android_host::log_warn(&format!(
                            "game.nirpak mmap failed ({e:#}); falling back to heap buffer (check 16 KiB APK alignment)"
                        ));
                        Arc::new(PakArchive::from_bytes(asset.buffer()?.to_vec())?)
                    }
                }
            }
            Err(e) => {
                android_host::log_warn(&format!(
                    "AssetManager::open_file_descriptor failed ({e:?}); falling back to heap buffer"
                ));
                Arc::new(PakArchive::from_bytes(asset.buffer()?.to_vec())?)
            }
        };
        Some(pak)
    } else {
        android_host::log_info(
            "No bundled assets/game.nirpak; launching Universal Android NIR Shelf mode",
        );
        None
    };

    let shelf_state = if bundled_pak.is_none() {
        let storage_root = data_dir
            .clone()
            .unwrap_or_else(storage::default_data_dir);
        let mut scan_dirs = Vec::new();
        if let Some(ext) = &ext_dir {
            scan_dirs.push(ext.join("games"));
            scan_dirs.push(ext.clone());
        }
        if let Some(int) = &data_dir {
            scan_dirs.push(int.join("games"));
        }
        Some(shelf::ShelfState::new(scan_dirs, storage_root, true, true))
    } else {
        None
    };

    let app_handle = android_app.clone();
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .with_android_app(android_app)
        .build()
        .context("creating Android event loop")?;
    run_with_event_loop(
        bundled_pak,
        shelf_state,
        data_dir,
        DesktopOptions::default(),
        event_loop,
        Some(app_handle),
    )
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

fn load_patch_dir(runtime: &mut Runtime, dir: &Path) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut loaded = 0usize;
    let mut paths: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        })
        .collect();
    paths.sort();
    for path in paths {
        let Some(locale) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(n) = runtime.load_translation_patch(locale, &text) {
                loaded += n;
            }
        }
    }
    loaded
}

fn load_startup_patches(
    runtime: &mut Runtime,
    pak: &PakArchive,
    storage: &LocalStorage,
    cli_patches: &[(String, PathBuf)],
) {
    if let Some(pak_dir) = pak.source_path().and_then(Path::parent) {
        let _ = load_patch_dir(runtime, &pak_dir.join("patches"));
    }
    let _ = load_patch_dir(runtime, &storage.patches_dir());
    for (locale, path) in cli_patches {
        if let Ok(text) = std::fs::read_to_string(path) {
            let _ = runtime.load_translation_patch(locale, &text);
        }
    }
}

fn run_with_event_loop(
    initial_pak: Option<Arc<PakArchive>>,
    shelf: Option<shelf::ShelfState>,
    storage_base: Option<PathBuf>,
    opts: DesktopOptions,
    event_loop: EventLoop<UserEvent>,
    #[cfg(target_os = "android")] android_app: Option<android_activity::AndroidApp>,
) -> Result<()> {
    let showing_shelf = initial_pak.is_none();
    let pak = initial_pak.unwrap_or_else(|| Arc::new(PakArchive::empty_placeholder()));
    let has_shelf = shelf.is_some();

    let (storage, prefs, profile, slots, imported_backup_count) = if !showing_shelf {
        let storage = LocalStorage::open(&pak.program.meta.game_id, storage_base.clone())?;
        #[cfg(target_os = "android")]
        let mut imported_count = 0usize;
        #[cfg(not(target_os = "android"))]
        let imported_count = 0usize;
        #[cfg(target_os = "android")]
        if let Some(ext_dir) = android_app.as_ref().and_then(|a| a.external_data_path()) {
            let import_path = ext_dir.join("saves-import.json");
            if import_path.is_file() {
                match storage.import_backup_json(&pak.program.meta.game_id, &import_path) {
                    Ok(n) => {
                        imported_count = n;
                        let _ = std::fs::rename(&import_path, ext_dir.join("saves-imported.json"));
                        android_host::log_info(&format!(
                            "Imported {n} saves from external backup"
                        ));
                    }
                    Err(e) => {
                        android_host::log_warn(&format!(
                            "Failed to import saves-import.json: {e:#}"
                        ));
                    }
                }
            }
        }
        let prefs = storage.load_preferences();
        let profile = storage.load_profile();
        let slots = storage.list_slot_summaries(pak.program.meta.save_slots);
        (Some(storage), prefs, profile, slots, imported_count)
    } else {
        (
            None,
            nir_ir::Preferences::default(),
            nir_vm::ProfileData::default(),
            Vec::new(),
            0usize,
        )
    };

    let audio = AudioHost::new(&prefs);
    let mut runtime = Runtime::new(pak.program.clone(), prefs, profile, slots)
        .map_err(|e| anyhow::anyhow!("initializing runtime: {e}"))?;
    runtime.set_tz_offset_mins(local_tz_offset_mins());
    runtime.set_host_has_shelf(has_shelf);
    if let Some(st) = &storage {
        runtime.set_autosave_summary(st.named_summary("autosave", u32::MAX));
        runtime.set_quicksave_summary(st.named_summary("quicksave", u32::MAX - 1));
        load_startup_patches(&mut runtime, &pak, st, &opts.patches);
        if st.is_concurrent_instance() {
            runtime.show_toast("检测到另一窗口正在运行本作，请注意避免存档覆盖", 4200);
        }
    }
    #[cfg(target_os = "android")]
    {
        runtime.set_input_mode(nir_ui::InputMode::Touch);
        if let Some(app) = &android_app {
            runtime.set_low_power_hint(android_host::read_low_power_hint(app));
            if !showing_shelf {
                if let Some(ext_dir) = app.external_data_path() {
                    let _ = load_patch_dir(&mut runtime, &ext_dir.join("patches"));
                }
            }
        }
        if imported_backup_count > 0 {
            runtime.show_toast(
                format!("已从外部备份导入 {imported_backup_count} 个存档"),
                3200,
            );
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = imported_backup_count;
        runtime.set_low_power_hint(desktop::read_low_battery_hint());
    }
    if !showing_shelf && opts.resume_latest {
        let _ = runtime.try_resume_latest();
    }

    let event_proxy = event_loop.create_proxy();
    let (res_tx, res_rx) = mpsc::channel::<DecodedImage>();
    let decode_pool = DecodePool::spawn(res_tx, event_proxy.clone());
    let gp_proxy = event_proxy.clone();
    let gamepad = GamepadHost::spawn(move || {
        let _ = gp_proxy.send_event(UserEvent::GamepadInput);
    });
    let dbus_proxy = event_proxy.clone();
    let dbus_title = if showing_shelf {
        "NIR Player".to_owned()
    } else {
        pak.program.meta.title.clone()
    };
    let dbus = LinuxDbusHost::spawn(dbus_title, move || {
        let _ = dbus_proxy.send_event(UserEvent::MediaKey);
    });

    let delta_bases = collect_delta_bases(&pak);
    let now = Instant::now();
    let mut app = NativeApp {
        pak,
        delta_bases,
        runtime,
        storage,
        storage_base,
        shelf,
        showing_shelf,
        shelf_touch_id: None,
        shelf_touch_start: [0.0, 0.0],
        shelf_touch_last_y: 0.0,
        shelf_touch_dragged: false,
        event_proxy,
        audio,
        gamepad,
        dbus,
        window: None,
        instance: wgpu::Instance::new(&wgpu::InstanceDescriptor::default()),
        renderer: None,
        decode_pool,
        res_rx,
        pending_decodes: BTreeMap::new(),
        last_frame: now,
        last_reload_check: now,
        last_battery_check: now,
        last_cursor_activity: now,
        last_back_press: None,
        next_wakeup_at: None,
        was_idle_sleeping: false,
        watch_hot_reload: opts.watch_hot_reload,
        cursor_logical: [0.0, 0.0],
        left_mouse_down: false,
        shift_held: false,
        ctrl_held: false,
        cursor_hidden: false,
        icon_applied: false,
        prev_story_reading: false,
        clean_story_thumb: None,
        clean_story_thumb_epoch: u64::MAX,
        thumbs_uploaded: false,
        auto_copy_hook: false,
        last_dialogue_epoch: u64::MAX,
        last_mpris_auto: false,
        scale_factor: 1.0,
        window_size: [1280, 720],
        should_quit: false,
        init_error: None,
        #[cfg(not(target_os = "android"))]
        clipboard: desktop::DesktopClipboard::new(),
        #[cfg(not(target_os = "android"))]
        video: desktop::DesktopVideoPlayer::new(),
        #[cfg(target_os = "android")]
        android_video: android_video::AndroidVideoPlayer::new(),
        #[cfg(target_os = "android")]
        android_app,
        #[cfg(target_os = "android")]
        keep_screen_on: false,
        #[cfg(target_os = "android")]
        last_low_power_hint: false,
        #[cfg(target_os = "android")]
        last_gpu_budget: 0,
    };
    event_loop.run_app(&mut app).context("winit event loop")?;
    app.save_and_flush_on_exit();
    if let Some(err) = app.init_error.take() {
        return Err(err);
    }
    Ok(())
}

struct NativeApp {
    pak: Arc<PakArchive>,
    delta_bases: Arc<HashSet<AssetId>>,
    runtime: Runtime,
    storage: Option<LocalStorage>,
    storage_base: Option<PathBuf>,
    shelf: Option<shelf::ShelfState>,
    showing_shelf: bool,
    shelf_touch_id: Option<u64>,
    shelf_touch_start: [f32; 2],
    shelf_touch_last_y: f32,
    shelf_touch_dragged: bool,
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    event_proxy: EventLoopProxy<UserEvent>,
    audio: AudioHost,
    gamepad: GamepadHost,
    dbus: LinuxDbusHost,
    window: Option<Arc<Window>>,
    instance: wgpu::Instance,
    renderer: Option<Renderer>,
    decode_pool: DecodePool,
    res_rx: mpsc::Receiver<DecodedImage>,
    /// Tracks in-flight image decodes and whether they have already been promoted to `urgent`.
    pending_decodes: BTreeMap<AssetId, bool>,
    last_frame: Instant,
    last_reload_check: Instant,
    last_battery_check: Instant,
    last_cursor_activity: Instant,
    last_back_press: Option<Instant>,
    next_wakeup_at: Option<Instant>,
    was_idle_sleeping: bool,
    watch_hot_reload: bool,
    cursor_logical: [f32; 2],
    left_mouse_down: bool,
    shift_held: bool,
    ctrl_held: bool,
    cursor_hidden: bool,
    icon_applied: bool,
    prev_story_reading: bool,
    clean_story_thumb: Option<String>,
    clean_story_thumb_epoch: u64,
    thumbs_uploaded: bool,
    auto_copy_hook: bool,
    last_dialogue_epoch: u64,
    last_mpris_auto: bool,
    scale_factor: f32,
    window_size: [u32; 2],
    should_quit: bool,
    init_error: Option<anyhow::Error>,
    #[cfg(not(target_os = "android"))]
    clipboard: desktop::DesktopClipboard,
    #[cfg(not(target_os = "android"))]
    video: desktop::DesktopVideoPlayer,
    #[cfg(target_os = "android")]
    android_video: android_video::AndroidVideoPlayer,
    #[cfg(target_os = "android")]
    android_app: Option<android_activity::AndroidApp>,
    #[cfg(target_os = "android")]
    keep_screen_on: bool,
    #[cfg(target_os = "android")]
    last_low_power_hint: bool,
    #[cfg(target_os = "android")]
    last_gpu_budget: u64,
}

impl NativeApp {
    fn sync_android_insets(&mut self) {
        #[cfg(target_os = "android")]
        if let Some(app) = &self.android_app {
            let scale = self.scale_factor.max(0.5);
            let pw = self.window_size[0] as i32;
            let ph = self.window_size[1] as i32;
            let mut left_px = 0i32;
            let mut top_px = 0i32;
            let mut right_px = 0i32;
            let mut bottom_px = 0i32;

            let rect = app.content_rect();
            if rect.right > rect.left && rect.bottom > rect.top && pw > 0 && ph > 0 {
                left_px = rect.left.max(0);
                top_px = rect.top.max(0);
                right_px = (pw - rect.right).max(0);
                bottom_px = (ph - rect.bottom).max(0);
            }
            if let Some([jl, jt, jr, jb]) = android_host::query_window_insets_px(app) {
                left_px = left_px.max(jl);
                top_px = top_px.max(jt);
                right_px = right_px.max(jr);
                bottom_px = bottom_px.max(jb);
            }
            let insets = nir_ui::SafeInsets {
                left: (left_px as f32) / scale,
                top: (top_px as f32) / scale,
                right: (right_px as f32) / scale,
                bottom: (bottom_px as f32) / scale,
            };
            self.runtime.set_safe_insets(insets);
            if let Some(shelf) = &mut self.shelf {
                shelf.safe_insets = insets;
                shelf.dirty = true;
            }
        }
    }

    fn sync_android_gpu_budget(&mut self) {
        #[cfg(target_os = "android")]
        if let Some(renderer) = &mut self.renderer {
            let eco_or_low = self.runtime.is_eco_mode() || self.last_low_power_hint;
            let budget = android_host::recommended_android_gpu_budget_bytes(
                renderer.backend.is_gl(),
                eco_or_low,
            );
            if budget != self.last_gpu_budget {
                self.last_gpu_budget = budget;
                renderer.set_texture_cache_limit(budget);
            }
        }
    }

    #[cfg(target_os = "android")]
    fn export_android_external_backup(&self) {
        if self.showing_shelf {
            return;
        }
        if let (Some(storage), Some(ext_dir)) = (
            self.storage.as_ref(),
            self.android_app
                .as_ref()
                .and_then(|a| a.external_data_path()),
        ) {
            let dest = ext_dir.join("saves-backup.json");
            let _ = storage.export_backup_json(
                &self.pak.program.meta.game_id,
                self.pak.program.meta.save_slots,
                &dest,
            );
        }
    }

    fn sync_wake_lock(&mut self) {
        let want = self.runtime.is_auto_or_skip_active();
        self.dbus.set_inhibit(want);
        #[cfg(target_os = "android")]
        if let Some(app) = &self.android_app {
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

    fn drain_gamepad_and_media_events(&mut self) -> bool {
        let mut any = false;
        while let Some(ev) = self.gamepad.try_recv() {
            any = true;
            match ev {
                GamepadEvent::Connected(name) => {
                    let short: String = name.chars().take(28).collect();
                    self.runtime
                        .show_toast(format!("已连接手柄：{short}"), 2400);
                }
                GamepadEvent::Disconnected => {
                    self.runtime.show_toast("手柄已断开", 1800);
                }
                GamepadEvent::Advance => {
                    #[cfg(not(target_os = "android"))]
                    if self.video.is_playing() {
                        if self.video.is_skippable() {
                            self.video.stop();
                            self.audio.set_host_paused(false);
                            self.runtime.notify_video_finished();
                        }
                        continue;
                    }
                    self.runtime.dispatch(UiAction::Advance);
                }
                GamepadEvent::CancelOrMenu => self.handle_back_action(),
                GamepadEvent::OpenHistory => self.runtime.dispatch(UiAction::OpenHistory),
                GamepadEvent::ToggleHideUi => self.runtime.dispatch(UiAction::ToggleHideUi),
                GamepadEvent::ScrollUp => self.runtime.scroll_wheel(-1.0),
                GamepadEvent::ScrollDown => self.runtime.scroll_wheel(1.0),
                GamepadEvent::ToggleAuto => self.runtime.dispatch(UiAction::ToggleAuto),
                GamepadEvent::QuickSave => self.runtime.dispatch(UiAction::QuickSave),
                GamepadEvent::HoldSkip(down) => self.runtime.dispatch(UiAction::HoldSkip(down)),
            }
        }
        while let Some(key) = self.dbus.try_recv_media_key() {
            any = true;
            match key {
                MediaKeyAction::ToggleAuto => self.runtime.dispatch(UiAction::ToggleAuto),
                MediaKeyAction::Advance => self.runtime.dispatch(UiAction::Advance),
                MediaKeyAction::Rollback => self.runtime.dispatch(UiAction::Rollback),
            }
        }
        any
    }

    fn sync_mpris_metadata(&mut self) {
        let (speaker, text) = self
            .runtime
            .current_dialogue_info()
            .map(|(_, s, t)| (s.to_owned(), t.trim().to_owned()))
            .unwrap_or_default();
        self.dbus.update_metadata(
            &self.pak.program.meta.title,
            &speaker,
            &text,
            self.runtime.auto,
        );
        self.last_mpris_auto = self.runtime.auto;
    }

    fn wake_cursor(&mut self) {
        self.last_cursor_activity = Instant::now();
        if self.cursor_hidden {
            self.cursor_hidden = false;
            if let Some(win) = &self.window {
                win.set_cursor_visible(true);
            }
        }
    }

    fn update_idle_cursor(&mut self, now: Instant) {
        if cfg!(target_os = "android") {
            return;
        }
        let allow_hide = self.runtime.is_story_reading()
            && !self.runtime.has_hovered_hit()
            && !self.left_mouse_down;
        if !allow_hide {
            if self.cursor_hidden {
                self.cursor_hidden = false;
                if let Some(win) = &self.window {
                    win.set_cursor_visible(true);
                }
            }
            return;
        }
        if !self.cursor_hidden
            && now.saturating_duration_since(self.last_cursor_activity) >= IDLE_CURSOR_HIDE_DELAY
        {
            self.cursor_hidden = true;
            if let Some(win) = &self.window {
                win.set_cursor_visible(false);
            }
        }
    }

    fn idle_cursor_wakeup_us(&self, now: Instant) -> Option<u64> {
        if cfg!(target_os = "android")
            || self.cursor_hidden
            || !self.runtime.is_story_reading()
            || self.runtime.has_hovered_hit()
            || self.left_mouse_down
        {
            return None;
        }
        let elapsed = now.saturating_duration_since(self.last_cursor_activity);
        let rem = IDLE_CURSOR_HIDE_DELAY.saturating_sub(elapsed);
        Some(rem.as_micros().max(16_000) as u64)
    }

    fn capture_stage_thumb(&mut self) -> Option<String> {
        let renderer = self.renderer.as_mut()?;
        let packet = self.runtime.last_packet();
        if packet.quads.is_empty() {
            return None;
        }
        // Render directly into a <= 320px offscreen target instead of full QHD+/4K swapchain resolution!
        let (w, h, rgba) = renderer
            .capture_stage_thumbnail_rgba(packet, self.scale_factor, 320)
            .ok()?;
        desktop::encode_thumb_data_url(&rgba, w, h)
    }

    fn upload_thumb_data_url(&mut self, key: &str, data_url: &str) {
        let Some((w, h, rgba)) = desktop::decode_thumb_data_url(data_url) else {
            return;
        };
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        if key == "autosave" {
            let id = nir_ui::AUTOSAVE_THUMB_ASSET_ID;
            if renderer.upload_rgba_premultiplied(id, w, h, &rgba).is_ok() {
                self.runtime.mark_autosave_has_thumb(true);
                self.runtime.mark_asset_loaded(id);
            }
        } else if key == "quicksave" {
            let id = nir_ui::QUICKSAVE_THUMB_ASSET_ID;
            if renderer.upload_rgba_premultiplied(id, w, h, &rgba).is_ok() {
                self.runtime.mark_quicksave_has_thumb(true);
                self.runtime.mark_asset_loaded(id);
            }
        } else if let Some(slot_str) = key.strip_prefix("slot_") {
            if let Ok(slot) = slot_str.parse::<u32>() {
                let id = AssetId(nir_ui::SAVE_THUMB_ASSET_BASE + slot);
                if renderer.upload_rgba_premultiplied(id, w, h, &rgba).is_ok() {
                    self.runtime.mark_slot_has_thumb(slot, true);
                    self.runtime.mark_asset_loaded(id);
                }
            }
        }
    }

    fn ensure_thumbs_uploaded(&mut self) {
        if self.thumbs_uploaded {
            return;
        }
        self.thumbs_uploaded = true;
        let Some(storage) = &self.storage else {
            return;
        };
        let thumbs = storage.collect_all_thumbnails(self.pak.program.meta.save_slots);
        for (key, url) in thumbs {
            self.upload_thumb_data_url(&key, &url);
        }
    }

    fn save_and_flush_on_exit(&mut self) {
        #[cfg(not(target_os = "android"))]
        self.video.stop();
        #[cfg(target_os = "android")]
        self.android_video.stop(&mut self.audio);
        if self.showing_shelf || self.storage.is_none() {
            return;
        }
        if self.runtime.is_story_reading() && self.clean_story_thumb.is_none() {
            if let Some(t) = self.capture_stage_thumb() {
                self.clean_story_thumb = Some(t);
            }
        }
        self.runtime.emit_autosave();
        self.runtime.flush_pending_saves();
        self.process_commands();
        if let Some(st) = &self.storage {
            st.flush();
        }
        #[cfg(target_os = "android")]
        self.export_android_external_backup();
    }

    fn copy_current_dialogue(&mut self, explicit_toast: bool) {
        let text = self
            .runtime
            .current_dialogue_info()
            .map(|(_, _, t)| t.trim().to_owned())
            .unwrap_or_default();
        if text.is_empty() {
            if explicit_toast {
                self.runtime.show_toast("当前无台词可复制", 1800);
            }
            return;
        }
        #[cfg(not(target_os = "android"))]
        let ok = self.clipboard.copy_text(&text);
        #[cfg(target_os = "android")]
        let ok = self
            .android_app
            .as_ref()
            .is_some_and(|app| android_host::copy_to_clipboard(app, &text));
        if ok {
            if explicit_toast {
                self.runtime.show_toast("已复制当前台词", 1600);
            }
        } else if explicit_toast {
            self.runtime.show_toast("剪贴板不可用", 2200);
        }
    }

    fn toggle_auto_copy_hook(&mut self) {
        self.auto_copy_hook = !self.auto_copy_hook;
        let msg = if self.auto_copy_hook {
            "已开启台词自动复制 (Texthooker)"
        } else {
            "已关闭台词自动复制"
        };
        self.runtime.show_toast(msg, 2200);
        if self.auto_copy_hook {
            self.copy_current_dialogue(false);
        }
    }

    fn capture_and_save_screenshot(&mut self) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let packet = self.runtime.last_packet();
        let Ok((w, h, rgba)) = renderer.capture_stage_rgba(packet, self.scale_factor) else {
            self.runtime.show_toast("截图失败", 2400);
            return;
        };
        let stem = desktop::safe_filename_stem(&self.pak.program.meta.title);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let filename = format!("{stem}-{stamp}.png");

        #[cfg(target_os = "android")]
        {
            let app_clone = self.android_app.clone();
            let fallback_dir = self
                .android_app
                .as_ref()
                .and_then(|a| a.external_data_path())
                .map(|d| d.join("screenshots"))
                .or_else(|| self.storage.as_ref().map(LocalStorage::screenshots_dir))
                .unwrap_or_else(|| PathBuf::from("screenshots"));
            let fname = filename.clone();
            let _ = std::thread::Builder::new()
                .name("nir-screenshot".into())
                .spawn(move || {
                    if let Ok(png) = encode_png_rgba(&rgba, w, h) {
                        let saved_gallery = app_clone.as_ref().is_some_and(|app| {
                            android_host::save_screenshot_mediastore(app, &fname, &png)
                        });
                        if !saved_gallery && std::fs::create_dir_all(&fallback_dir).is_ok() {
                            let _ = std::fs::write(fallback_dir.join(&fname), png);
                        }
                    }
                });
        }
        #[cfg(not(target_os = "android"))]
        {
            let Some(dir) = self.storage.as_ref().map(LocalStorage::screenshots_dir) else {
                return;
            };
            let path = dir.join(&filename);
            let _ = std::thread::Builder::new()
                .name("nir-screenshot".into())
                .spawn(move || {
                    if std::fs::create_dir_all(&dir).is_ok() {
                        if let Ok(png) = encode_png_rgba(&rgba, w, h) {
                            let _ = std::fs::write(&path, png);
                        }
                    }
                });
        }
        self.runtime
            .show_toast(format!("已保存无黑边截图：{filename}"), 2400);
    }

    #[cfg(not(target_os = "android"))]
    fn handle_dropped_file(&mut self, path: PathBuf) {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext == "nirpak" {
            self.switch_pak_file(&path);
            return;
        }
        if ext == "json" && !self.showing_shelf {
            // Detect portable `nir-backup-v1` save backup JSON before falling back to translation patch!
            if let Ok(text) = std::fs::read_to_string(&path) {
                if text.contains("nir-backup-v1") {
                    self.import_backup_from_path(&path);
                    return;
                }
                let locale = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("en")
                    .to_owned();
                match self.runtime.load_translation_patch(&locale, &text) {
                    Ok(count) => {
                        self.runtime
                            .show_toast(format!("已加载翻译补丁 [{locale}]：{count} 条"), 2800);
                    }
                    Err(e) => {
                        self.runtime
                            .show_toast(format!("翻译补丁解析失败：{e}"), 3200);
                    }
                }
            } else {
                self.runtime.show_toast("无法读取拖入的 JSON 文件", 2800);
            }
        }
    }

    fn switch_pak_file(&mut self, path: &Path) {
        let new_pak = match PakArchive::open_file(path) {
            Ok(p) => Arc::new(p),
            Err(e) => {
                if self.showing_shelf {
                    if let Some(shelf) = &mut self.shelf {
                        shelf.show_toast(format!("无法打开 .nirpak：{e}"), 3500);
                    }
                } else {
                    self.runtime
                        .show_toast(format!("无法打开 .nirpak：{e}"), 3500);
                }
                return;
            }
        };
        let new_storage =
            match LocalStorage::open(&new_pak.program.meta.game_id, self.storage_base.clone()) {
                Ok(s) => s,
                Err(e) => {
                    if self.showing_shelf {
                        if let Some(shelf) = &mut self.shelf {
                            shelf.show_toast(format!("无法初始化存档目录：{e}"), 3500);
                        }
                    } else {
                        self.runtime
                            .show_toast(format!("无法初始化存档目录：{e}"), 3500);
                    }
                    return;
                }
            };

        // Persist current game before switching
        self.save_and_flush_on_exit();
        self.audio.stop_all();

        if let Some(shelf) = &self.shelf {
            shelf.record_recent(path);
        } else {
            let recent = self
                .storage_base
                .clone()
                .unwrap_or_else(storage::default_data_dir)
                .join("nir")
                .join("recent_games.json");
            shelf::record_recent_game(&recent, path);
        }

        #[cfg(target_os = "android")]
        let mut imported_count = 0usize;
        #[cfg(target_os = "android")]
        if let Some(ext_dir) = self
            .android_app
            .as_ref()
            .and_then(|a| a.external_data_path())
        {
            let import_path = ext_dir.join("saves-import.json");
            if import_path.is_file() {
                if let Ok(n) =
                    new_storage.import_backup_json(&new_pak.program.meta.game_id, &import_path)
                {
                    imported_count = n;
                    let _ = std::fs::rename(&import_path, ext_dir.join("saves-imported.json"));
                }
            }
        }

        let prefs = new_storage.load_preferences();
        let profile = new_storage.load_profile();
        let slots = new_storage.list_slot_summaries(new_pak.program.meta.save_slots);
        let mut new_rt = match Runtime::new(new_pak.program.clone(), prefs.clone(), profile, slots)
        {
            Ok(rt) => rt,
            Err(e) => {
                if self.showing_shelf {
                    if let Some(shelf) = &mut self.shelf {
                        shelf.show_toast(format!("无法启动作品：{e}"), 3500);
                    }
                } else {
                    self.runtime.show_toast(format!("无法启动作品：{e}"), 3500);
                }
                return;
            }
        };
        new_rt.set_tz_offset_mins(local_tz_offset_mins());
        new_rt.set_host_has_shelf(self.shelf.is_some());
        new_rt.set_autosave_summary(new_storage.named_summary("autosave", u32::MAX));
        new_rt.set_quicksave_summary(new_storage.named_summary("quicksave", u32::MAX - 1));
        load_startup_patches(&mut new_rt, &new_pak, &new_storage, &[]);
        #[cfg(target_os = "android")]
        {
            new_rt.set_input_mode(nir_ui::InputMode::Touch);
            if let Some(app) = &self.android_app {
                new_rt.set_low_power_hint(android_host::read_low_power_hint(app));
                if let Some(ext_dir) = app.external_data_path() {
                    let _ = load_patch_dir(&mut new_rt, &ext_dir.join("patches"));
                }
            }
            if imported_count > 0 {
                new_rt.show_toast(format!("已导入 {imported_count} 个外部存档"), 2800);
            }
        }
        #[cfg(not(target_os = "android"))]
        {
            new_rt.set_low_power_hint(desktop::read_low_battery_hint());
            if new_storage.is_concurrent_instance() {
                new_rt.show_toast("检测到另一窗口正在运行本作，请注意避免存档覆盖", 4200);
            } else {
                new_rt.show_toast(format!("已切换作品：{}", new_pak.program.meta.title), 2600);
            }
        }

        self.audio.apply_preferences(&prefs);
        self.audio.clear_cache();
        self.pending_decodes.clear();
        self.decode_pool.clear_pending();
        if let Some(renderer) = &mut self.renderer {
            renderer.clear_textures();
        }
        self.delta_bases = collect_delta_bases(&new_pak);
        self.pak = new_pak;
        self.storage = Some(new_storage);
        self.runtime = new_rt;
        self.showing_shelf = false;
        self.icon_applied = false;
        self.prev_story_reading = false;
        self.clean_story_thumb = None;
        self.clean_story_thumb_epoch = u64::MAX;
        self.thumbs_uploaded = false;
        self.last_dialogue_epoch = u64::MAX;
        if let Some(win) = &self.window {
            win.set_title(&self.pak.program.meta.title);
        }
        self.sync_android_insets();
        self.upload_all_fonts();
        self.process_commands();
        if let Some(win) = &self.window {
            win.request_redraw();
        }
    }

    fn enter_shelf_mode(&mut self) {
        if self.shelf.is_none() {
            self.should_quit = true;
            return;
        }
        self.save_and_flush_on_exit();
        self.audio.stop_all();
        self.audio.clear_cache();
        self.pending_decodes.clear();
        self.decode_pool.clear_pending();
        self.storage = None;
        self.showing_shelf = true;
        self.thumbs_uploaded = false;
        self.wake_cursor();
        if let Some(renderer) = &mut self.renderer {
            renderer.clear_textures();
            renderer.ensure_system_fallback_fonts();
        }
        if let Some(shelf) = &mut self.shelf {
            shelf.rescan();
        }
        if let Some(win) = &self.window {
            win.set_title("NIR 书架");
            win.request_redraw();
        }
    }

    fn handle_shelf_action(&mut self, action: shelf::ShelfAction) {
        match action {
            shelf::ShelfAction::Launch(path) => {
                self.switch_pak_file(&path);
            }
            shelf::ShelfAction::Rescan => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.clear_textures();
                }
                if let Some(shelf) = &mut self.shelf {
                    shelf.rescan();
                    let n = shelf.entries.len();
                    shelf.show_toast(format!("已扫描完成：发现 {n} 部 .nirpak 作品"), 2200);
                }
                if let Some(win) = &self.window {
                    win.request_redraw();
                }
            }
            shelf::ShelfAction::OpenFilePicker => {
                #[cfg(not(target_os = "android"))]
                {
                    if let Some(picked) = rfd::FileDialog::new()
                        .set_title("Open NIR Game Package (.nirpak)")
                        .add_filter("NIR Game Package", &["nirpak"])
                        .pick_file()
                    {
                        self.switch_pak_file(&picked);
                    }
                }
            }
        }
    }

    fn export_saves_via_host(&mut self) {
        let Some(storage) = &self.storage else {
            return;
        };
        let game_id = self.pak.program.meta.game_id.clone();
        let slot_count = self.pak.program.meta.save_slots;

        #[cfg(target_os = "android")]
        let dest = self
            .android_app
            .as_ref()
            .and_then(|a| a.external_data_path())
            .map(|d| d.join("saves-backup.json"))
            .unwrap_or_else(|| storage.screenshots_dir().join("saves-backup.json"));

        #[cfg(not(target_os = "android"))]
        let dest = {
            let stem = desktop::safe_filename_stem(&game_id);
            let default_name = format!("{stem}-saves-backup.json");
            match rfd::FileDialog::new()
                .set_title("导出跨端存档备份 (nir-backup-v1)")
                .set_file_name(&default_name)
                .add_filter("NIR Save Backup", &["json"])
                .save_file()
            {
                Some(p) => p,
                None => return,
            }
        };

        match storage.export_backup_json(&game_id, slot_count, &dest) {
            Ok(n) => {
                let name = dest
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("saves-backup.json");
                self.runtime
                    .show_toast(format!("已导出 {n} 个存档至 {name}"), 3000);
            }
            Err(e) => {
                self.runtime.show_toast(format!("导出失败：{e}"), 3200);
            }
        }
    }

    fn import_saves_via_host(&mut self) {
        #[cfg(target_os = "android")]
        let src = {
            let Some(ext_dir) = self
                .android_app
                .as_ref()
                .and_then(|a| a.external_data_path())
            else {
                self.runtime.show_toast("外部存储目录不可用", 2600);
                return;
            };
            let import_file = ext_dir.join("saves-import.json");
            let backup_file = ext_dir.join("saves-backup.json");
            if import_file.is_file() {
                import_file
            } else if backup_file.is_file() {
                backup_file
            } else {
                self.runtime.show_toast(
                    "请将 nir-backup-v1 放入外部目录 saves-import.json 或 saves-backup.json",
                    3600,
                );
                return;
            }
        };

        #[cfg(not(target_os = "android"))]
        let src = match rfd::FileDialog::new()
            .set_title("导入跨端存档备份 (nir-backup-v1)")
            .add_filter("NIR Save Backup", &["json"])
            .pick_file()
        {
            Some(p) => p,
            None => return,
        };

        self.import_backup_from_path(&src);
    }

    fn import_backup_from_path(&mut self, src: &Path) {
        let Some(storage) = &self.storage else {
            return;
        };
        let game_id = self.pak.program.meta.game_id.clone();
        let slot_count = self.pak.program.meta.save_slots;
        match storage.import_backup_json(&game_id, src) {
            Ok(n) => {
                let prefs = storage.load_preferences();
                let profile = storage.load_profile();
                let slots = storage.list_slot_summaries(slot_count);
                let auto = storage.named_summary("autosave", u32::MAX);
                let quick = storage.named_summary("quicksave", u32::MAX - 1);
                self.audio.apply_preferences(&prefs);
                self.runtime
                    .reload_storage_state(Some(prefs), Some(profile), slots, auto, quick);
                self.thumbs_uploaded = false;
                self.ensure_thumbs_uploaded();
                self.runtime
                    .show_toast(format!("已导入并合并 {n} 个存档备份"), 3000);
            }
            Err(e) => {
                self.runtime.show_toast(format!("导入失败：{e}"), 3400);
            }
        }
    }

    #[cfg(target_os = "android")]
    fn render_android_video_frame(&mut self, lw: f32, lh: f32) {
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let [vw, vh] = self.android_video.last_frame_dims();
        let aspect = (vw as f32 / vh.max(1) as f32).clamp(0.25, 4.0);
        let view_aspect = lw / lh.max(1.0);
        let (dw, dh) = if view_aspect > aspect {
            (lh * aspect, lh)
        } else {
            (lw, lw / aspect)
        };
        let dx = (lw - dw) * 0.5;
        let dy = (lh - dh) * 0.5;
        let mut pkt = nir_ui::DrawPacket {
            width: lw,
            height: lh,
            stage_rect: [dx, dy, dw, dh],
            ..Default::default()
        };
        pkt.quads
            .push(nir_ui::Quad::solid([0.0, 0.0, lw, lh], [0.0, 0.0, 0.0, 1.0]));
        pkt.quads.push(nir_ui::Quad::image(
            [dx, dy, dw, dh],
            android_video::VIDEO_FRAME_ASSET_ID,
        ));
        if self.android_video.is_skippable() {
            let safe = self.runtime.safe_insets.rect(lw, lh);
            let pw = 112.0;
            let ph = 32.0;
            let px = safe[0] + safe[2] - pw - 16.0;
            let py = safe[1] + safe[3] - ph - 14.0;
            pkt.quads.push(nir_ui::Quad::panel(
                [px, py, pw, ph],
                [0.05, 0.07, 0.12, 0.72],
                None,
                [0.65, 0.78, 0.96, 0.45],
                1.0,
                16.0,
            ));
            let mut tr = nir_ui::TextRun {
                text: "点击跳过".into(),
                visible_bytes: None,
                x: px + 8.0,
                y: py,
                width: pw - 16.0,
                height: ph,
                size: 13.0,
                line_height: 17.0,
                color: [0.92, 0.96, 1.0, 0.90],
                emphasis: Vec::new(),
                color_spans: Vec::new(),
                ruby_spans: Vec::new(),
                shadow: None,
                outline: None,
                scroll: 0.0,
                clip: Some([px, py, pw, ph]),
                font_assets: Vec::new(),
                align: nir_ir::TextAlign::Center,
                valign_center: true,
                auto_fit: true,
                inline_images: Vec::new(),
                cache_key: 0,
            };
            tr.compute_key();
            pkt.texts.push(tr);
        }
        let _ = renderer.render(&pkt, self.scale_factor);
    }

    fn maybe_apply_window_icon(&mut self, id: AssetId, width: u32, height: u32, rgba: &[u8]) {
        if self.icon_applied || cfg!(target_os = "android") {
            return;
        }
        let title_bg = self
            .pak
            .program
            .ui
            .title_screen
            .as_ref()
            .and_then(|s| s.background)
            .or(self.pak.program.meta.title_bg);
        if title_bg != Some(id) {
            return;
        }
        if let Some(win) = &self.window {
            if let Ok(icon_rgba) = resize_rgba_icon(rgba, width, height, 64) {
                if let Ok(icon) = Icon::from_rgba(icon_rgba, 64, 64) {
                    win.set_window_icon(Some(icon));
                    self.icon_applied = true;
                }
            }
        }
    }

    fn handle_back_action(&mut self) {
        if self.showing_shelf {
            if cfg!(target_os = "android") {
                let now = Instant::now();
                if self
                    .last_back_press
                    .is_some_and(|prev| now.duration_since(prev) <= Duration::from_millis(2500))
                {
                    self.should_quit = true;
                } else {
                    self.last_back_press = Some(now);
                    if let Some(shelf) = &mut self.shelf {
                        shelf.show_toast("再按一次返回键退出", 2200);
                    }
                }
            }
            return;
        }
        #[cfg(not(target_os = "android"))]
        if self.video.is_playing() {
            if self.video.is_skippable() {
                self.video.stop();
                self.audio.set_host_paused(false);
                self.runtime.notify_video_finished();
            }
            return;
        }
        #[cfg(target_os = "android")]
        if self.android_video.is_playing() {
            if self.android_video.is_skippable() {
                self.android_video.stop(&mut self.audio);
                self.audio.set_game_paused_for_video(false);
                self.runtime.notify_video_finished();
                self.runtime.dirty = true;
            }
            return;
        }
        if self.runtime.screen == nir_ui::Screen::Title && cfg!(target_os = "android") {
            if self.shelf.is_some() {
                self.enter_shelf_mode();
                return;
            }
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
                    self.maybe_apply_window_icon(id, width, height, &rgba);
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
        let has_save_cmd = cmds.iter().any(|c| {
            matches!(
                c,
                HostCommand::SaveSlot { .. }
                    | HostCommand::SaveAuto { .. }
                    | HostCommand::SaveQuick { .. }
            )
        });
        let captured_thumb = if has_save_cmd {
            if self.runtime.is_story_reading() {
                self.capture_stage_thumb()
                    .or_else(|| self.clean_story_thumb.clone())
            } else {
                self.clean_story_thumb
                    .clone()
                    .or_else(|| self.capture_stage_thumb())
            }
        } else {
            None
        };

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
                HostCommand::PlayVideo { asset, skippable } => {
                    #[cfg(not(target_os = "android"))]
                    {
                        let played = if let (Some(meta), Some(bytes)) =
                            (self.pak.program.asset(asset), self.pak.asset_bytes(asset))
                        {
                            self.video.play(
                                bytes,
                                meta.codec.ext(),
                                &self.pak.program.meta.title,
                                skippable,
                            )
                        } else {
                            Err("video asset missing from nirpak".into())
                        };
                        match played {
                            Ok(()) => {
                                self.audio.set_host_paused(true);
                            }
                            Err(_) => {
                                self.runtime.show_toast(
                                    "未检测到系统播放器 (mpv/ffplay)，已跳过影片",
                                    2600,
                                );
                                self.runtime.notify_video_finished();
                            }
                        }
                    }
                    #[cfg(target_os = "android")]
                    {
                        let temp_dir = self
                            .android_app
                            .as_ref()
                            .and_then(|a| a.internal_data_path())
                            .unwrap_or_else(|| PathBuf::from("/data/local/tmp"));
                        let proxy = self.event_proxy.clone();
                        let played = if let (Some(meta), Some(bytes)) =
                            (self.pak.program.asset(asset), self.pak.asset_bytes(asset))
                        {
                            self.android_video.play(
                                bytes,
                                meta.codec.ext(),
                                skippable,
                                &temp_dir,
                                &mut self.audio,
                                move || {
                                    let _ = proxy.send_event(UserEvent::VideoFrame);
                                },
                            )
                        } else {
                            Err("video asset missing from nirpak".into())
                        };
                        match played {
                            Ok(()) => {
                                self.audio.set_game_paused_for_video(true);
                            }
                            Err(err) => {
                                android_host::log_warn(&format!(
                                    "Android video playback fallback: {err}"
                                ));
                                self.runtime.show_toast(
                                    "设备不支持该影片编码，已自动跳过",
                                    2400,
                                );
                                self.runtime.notify_video_finished();
                            }
                        }
                    }
                }
                HostCommand::StopVideo => {
                    #[cfg(not(target_os = "android"))]
                    if self.video.is_playing() {
                        self.video.stop();
                        self.audio.set_host_paused(false);
                    }
                    #[cfg(target_os = "android")]
                    if self.android_video.is_playing() {
                        self.android_video.stop(&mut self.audio);
                        self.audio.set_game_paused_for_video(false);
                    }
                }
                HostCommand::SaveSlot { slot, mut snapshot } => {
                    if snapshot.thumbnail.is_none() {
                        snapshot.thumbnail = captured_thumb.clone();
                    }
                    if let Some(storage) = &self.storage {
                        if let Err(e) = storage.save_slot(slot, &snapshot) {
                            self.runtime.show_toast(format!("Save failed: {e}"), 3000);
                        } else if let Some(t) = &snapshot.thumbnail {
                            self.upload_thumb_data_url(&format!("slot_{slot}"), t);
                        }
                    }
                }
                HostCommand::RequestLoadSlot(slot) => {
                    if let Some(storage) = &self.storage {
                        match storage.load_slot(slot) {
                            Ok(snap) => self.runtime.apply_loaded_snapshot(snap),
                            Err(e) => self.runtime.show_toast(format!("Load failed: {e}"), 3000),
                        }
                    }
                }
                HostCommand::DeleteSlot(slot) => {
                    if let Some(storage) = &self.storage {
                        let _ = storage.delete_slot(slot);
                    }
                }
                HostCommand::SaveAuto { mut snapshot } => {
                    if snapshot.thumbnail.is_none() {
                        snapshot.thumbnail = captured_thumb.clone();
                    }
                    if let Some(storage) = &self.storage {
                        let _ = storage.save_named("autosave", &snapshot);
                    }
                    if let Some(t) = &snapshot.thumbnail {
                        self.upload_thumb_data_url("autosave", t);
                    }
                }
                HostCommand::RequestLoadAuto => {
                    if let Some(storage) = &self.storage {
                        match storage.load_named("autosave") {
                            Ok(snap) => self.runtime.apply_loaded_snapshot(snap),
                            Err(e) => self.runtime.show_toast(format!("Load failed: {e}"), 3000),
                        }
                    }
                }
                HostCommand::SaveQuick { mut snapshot } => {
                    if snapshot.thumbnail.is_none() {
                        snapshot.thumbnail = captured_thumb.clone();
                    }
                    if let Some(storage) = &self.storage {
                        let _ = storage.save_named("quicksave", &snapshot);
                    }
                    if let Some(t) = &snapshot.thumbnail {
                        self.upload_thumb_data_url("quicksave", t);
                    }
                }
                HostCommand::RequestLoadQuick => {
                    if let Some(storage) = &self.storage {
                        match storage.load_named("quicksave") {
                            Ok(snap) => self.runtime.apply_loaded_snapshot(snap),
                            Err(e) => self.runtime.show_toast(format!("Load failed: {e}"), 3000),
                        }
                    }
                }
                HostCommand::SaveProfile(profile) => {
                    if let Some(storage) = &self.storage {
                        let _ = storage.save_profile(&profile);
                    }
                }
                HostCommand::SavePreferences(prefs) => {
                    self.audio.apply_preferences(&prefs);
                    if let Some(storage) = &self.storage {
                        let _ = storage.save_preferences(&prefs);
                    }
                }
                HostCommand::ExportSaves => {
                    self.export_saves_via_host();
                }
                HostCommand::ImportSaves => {
                    self.import_saves_via_host();
                }
                HostCommand::ReturnToShelf => {
                    self.enter_shelf_mode();
                    break;
                }
                HostCommand::Quit => {
                    self.should_quit = true;
                }
            }
        }
    }

    fn tick_and_render(&mut self) {
        if self.showing_shelf {
            let lw = (self.window_size[0] as f32 / self.scale_factor).max(1.0);
            let lh = (self.window_size[1] as f32 / self.scale_factor).max(1.0);
            if let (Some(shelf), Some(renderer)) = (&mut self.shelf, &mut self.renderer) {
                let _ = renderer.ensure_system_fallback_fonts();
                shelf.ensure_covers_uploaded(renderer);
                let packet = shelf.project_draw_packet(lw, lh);
                let _ = renderer.render(&packet, self.scale_factor);
            }
            return;
        }

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

        #[cfg(target_os = "android")]
        {
            match self.android_video.poll_frame() {
                android_video::VideoPoll::Playing(frame_opt) => {
                    if let (Some(frame), Some(renderer)) = (frame_opt, &mut self.renderer) {
                        let _ = renderer.upload_rgba_premultiplied(
                            android_video::VIDEO_FRAME_ASSET_ID,
                            frame.width,
                            frame.height,
                            &frame.rgba,
                        );
                    }
                    let lw = (self.window_size[0] as f32 / self.scale_factor).max(1.0);
                    let lh = (self.window_size[1] as f32 / self.scale_factor).max(1.0);
                    self.render_android_video_frame(lw, lh);
                    return;
                }
                android_video::VideoPoll::Finished => {
                    self.audio.stop_video_audio();
                    self.audio.set_game_paused_for_video(false);
                    self.runtime.notify_video_finished();
                    self.runtime.dirty = true;
                }
                android_video::VideoPoll::Idle => {}
            }
        }

        // Capture a clean (overlay-free) Story thumbnail RIGHT BEFORE rendering a newly opened Menu/Saves overlay!
        let pre_story_reading = self.runtime.is_story_reading();
        let cur_dlg_epoch = self.runtime.dialogue_epoch();
        if self.prev_story_reading
            && !pre_story_reading
            && self.clean_story_thumb_epoch != cur_dlg_epoch
        {
            if let Some(t) = self.capture_stage_thumb() {
                self.clean_story_thumb = Some(t);
                self.clean_story_thumb_epoch = cur_dlg_epoch;
            }
        }

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

        // Periodic low-battery / power-saver check (every 60s on Linux laptops & Android!)
        if now.duration_since(self.last_battery_check) >= BATTERY_POLL_INTERVAL {
            self.last_battery_check = now;
            #[cfg(target_os = "android")]
            if let Some(app) = &self.android_app {
                let low = android_host::read_low_power_hint(app);
                self.last_low_power_hint = low;
                self.runtime.set_low_power_hint(low);
            }
            #[cfg(not(target_os = "android"))]
            self.runtime
                .set_low_power_hint(desktop::read_low_battery_hint());
        }

        #[cfg(not(target_os = "android"))]
        if self.video.poll_finished() {
            self.audio.set_host_paused(false);
            self.runtime.notify_video_finished();
        }

        self.drain_decoded_images();
        self.drain_gamepad_and_media_events();
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
        self.sync_android_gpu_budget();
        let haptic_ms = self.runtime.take_haptic_ms();
        if haptic_ms > 0 {
            #[cfg(target_os = "android")]
            if let Some(app) = &self.android_app {
                android_host::vibrate(app, haptic_ms);
            }
            #[cfg(not(target_os = "android"))]
            self.gamepad.rumble(haptic_ms.saturating_mul(3).max(40));
        }

        // Lazy-upload stored save thumbnails ONLY when user opens the Saves screen!
        if matches!(self.runtime.screen, nir_ui::Screen::Saves { .. }) && !self.thumbs_uploaded {
            self.ensure_thumbs_uploaded();
        }

        // Auto-copy new dialogue lines when Texthooker mode (`Shift+C`) is active, and update MPRIS2.
        let post_dlg_epoch = self.runtime.dialogue_epoch();
        if post_dlg_epoch != self.last_dialogue_epoch {
            self.last_dialogue_epoch = post_dlg_epoch;
            self.sync_mpris_metadata();
            if self.auto_copy_hook {
                self.copy_current_dialogue(false);
            }
        } else if self.runtime.auto != self.last_mpris_auto {
            self.sync_mpris_metadata();
        }

        self.prev_story_reading = self.runtime.is_story_reading();
        self.update_idle_cursor(now);

        if self.renderer.as_ref().is_some_and(Renderer::is_lost) {
            if let (Some(win), Some(renderer)) = (&self.window, &mut self.renderer) {
                if let Ok(surface) = self.instance.create_surface(win.clone()) {
                    renderer.rebind_surface(surface);
                    renderer.clear_textures();
                    self.pending_decodes.clear();
                    self.thumbs_uploaded = false;
                    self.runtime.invalidate_all_textures();
                    self.process_commands();
                }
            }
        }

        let lw = (self.window_size[0] as f32 / self.scale_factor).max(1.0);
        let lh = (self.window_size[1] as f32 / self.scale_factor).max(1.0);
        if let Some(packet) = self.runtime.prepare_frame(lw, lh) {
            let mut oom_or_lost = false;
            if let Some(renderer) = &mut self.renderer {
                if let Err(err) = renderer.render(packet, self.scale_factor) {
                    let msg = err.to_string();
                    android_host::log_warn(&format!("render error: {msg}"));
                    if msg.contains("OutOfMemory") || msg.contains("Lost") {
                        oom_or_lost = true;
                    }
                } else if renderer.has_missing_glyphs() && renderer.ensure_system_fallback_fonts() {
                    self.runtime.mark_fonts_changed();
                }
            }
            if oom_or_lost {
                self.flush_caches_for_memory_pressure("render-oom");
            }
        }
    }

    fn flush_caches_for_memory_pressure(&mut self, reason: &str) {
        android_host::log_warn(&format!(
            "flushing GPU/audio caches under memory pressure ({reason})"
        ));
        self.pending_decodes.clear();
        self.decode_pool.clear_pending();
        self.audio.clear_cache();
        self.thumbs_uploaded = false;
        if let Some(renderer) = &mut self.renderer {
            renderer.clear_textures();
        }
        self.runtime.invalidate_all_textures();
        self.runtime.dirty = true;
    }
}

impl ApplicationHandler<UserEvent> for NativeApp {
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        let should_redraw = match event {
            UserEvent::ImageDecoded => self.drain_decoded_images(),
            UserEvent::GamepadInput | UserEvent::MediaKey => self.drain_gamepad_and_media_events(),
            #[cfg(target_os = "android")]
            UserEvent::VideoFrame => true,
        };
        if should_redraw {
            if let Some(win) = &self.window {
                win.request_redraw();
            }
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.runtime.set_host_paused(false);
        self.audio.reset_stream_backoff();
        self.audio.set_host_paused(false);
        #[cfg(target_os = "android")]
        self.android_video.set_paused(false);
        if self.window.is_none() {
            let (title, sw, sh) = if self.showing_shelf {
                ("NIR 书架", 1080u32, 680u32)
            } else {
                (
                    self.pak.program.meta.title.as_str(),
                    self.pak.program.meta.stage_width.max(640),
                    self.pak.program.meta.stage_height.max(360),
                )
            };
            let attrs = Window::default_attributes()
                .with_title(title)
                .with_inner_size(LogicalSize::new(sw as f64, sh as f64));
            let win = match event_loop.create_window(attrs) {
                Ok(w) => Arc::new(w),
                Err(e) => {
                    let err = anyhow::anyhow!("creating window failed: {e}");
                    android_host::log_error(&format!("{err:#}"));
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
                    android_host::log_info(&format!(
                        "GPU initialized: {} ({}x{} @ {:.2}x)",
                        renderer.adapter_info(),
                        self.window_size[0],
                        self.window_size[1],
                        self.scale_factor
                    ));
                    self.instance = instance;
                    self.renderer = Some(renderer);
                    self.sync_android_gpu_budget();
                    if self.showing_shelf {
                        if let Some(r) = &mut self.renderer {
                            let _ = r.ensure_system_fallback_fonts();
                        }
                    } else {
                        self.upload_all_fonts();
                    }
                }
                Err(err) => {
                    android_host::log_error(&format!("GPU initialization failed: {err:#}"));
                    self.init_error = Some(err);
                    event_loop.exit();
                    return;
                }
            }
            self.window = Some(win);
            self.sync_android_insets();
            if let Some(shelf) = &mut self.shelf {
                shelf.dirty = true;
            }
            self.runtime.dirty = true;
            if !self.showing_shelf {
                self.process_commands();
            }
        } else if let Some(win) = self.window.clone() {
            let phys = win.inner_size();
            self.window_size = [phys.width.max(1), phys.height.max(1)];
            self.scale_factor = win.scale_factor() as f32;
            if let Some(renderer) = &mut self.renderer {
                if let Ok(surface) = self.instance.create_surface(win.clone()) {
                    renderer.rebind_surface(surface);
                    renderer.resize(self.window_size[0], self.window_size[1]);
                } else {
                    android_host::log_warn("failed to recreate wgpu surface on resume");
                }
            }
            self.sync_android_gpu_budget();
            self.sync_android_insets();
            if let Some(shelf) = &mut self.shelf {
                shelf.dirty = true;
            }
            self.runtime.dirty = true;
            if !self.showing_shelf {
                self.process_commands();
            }
            win.request_redraw();
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        self.runtime.set_host_paused(true);
        self.audio.set_host_paused(true);
        #[cfg(target_os = "android")]
        self.android_video.set_paused(true);
        self.sync_wake_lock();
        if !self.showing_shelf {
            self.runtime.emit_autosave();
            self.runtime.flush_pending_saves();
            self.process_commands();
            if let Some(st) = &self.storage {
                st.flush();
            }
            #[cfg(target_os = "android")]
            self.export_android_external_backup();
        }
        if let Some(renderer) = &mut self.renderer {
            renderer.release_surface();
        }
    }

    fn memory_warning(&mut self, _event_loop: &ActiveEventLoop) {
        self.flush_caches_for_memory_pressure("os-memory-warning");
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                self.save_and_flush_on_exit();
                event_loop.exit();
            }
            WindowEvent::Focused(focused) => {
                if !focused {
                    self.left_mouse_down = false;
                    self.shift_held = false;
                    self.ctrl_held = false;
                    self.wake_cursor();
                    self.runtime.pointer_cancel();
                    self.runtime.dispatch(UiAction::HoldSkip(false));
                }
                if cfg!(target_os = "android") {
                    self.runtime.set_host_paused(!focused);
                    self.audio.set_host_paused(!focused);
                    #[cfg(target_os = "android")]
                    self.android_video.set_paused(!focused);
                    self.sync_wake_lock();
                    if !focused && !self.showing_shelf {
                        self.runtime.emit_autosave();
                        self.runtime.flush_pending_saves();
                        self.process_commands();
                    }
                }
            }
            WindowEvent::DroppedFile(path) => {
                #[cfg(not(target_os = "android"))]
                self.handle_dropped_file(path);
                #[cfg(target_os = "android")]
                let _ = path;
            }
            WindowEvent::ModifiersChanged(mods) => {
                let st = mods.state();
                self.shift_held = st.shift_key();
                self.ctrl_held = st.control_key();
            }
            WindowEvent::Resized(size) => {
                self.window_size = [size.width.max(1), size.height.max(1)];
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(self.window_size[0], self.window_size[1]);
                }
                self.sync_android_insets();
                if let Some(shelf) = &mut self.shelf {
                    shelf.dirty = true;
                }
                self.runtime.dirty = true;
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale_factor = scale_factor as f32;
                self.sync_android_insets();
                if let Some(shelf) = &mut self.shelf {
                    shelf.dirty = true;
                }
                self.runtime.dirty = true;
            }
            WindowEvent::Touch(touch) => {
                let lx = touch.location.x as f32 / self.scale_factor;
                let ly = touch.location.y as f32 / self.scale_factor;
                if self.showing_shelf {
                    match touch.phase {
                        TouchPhase::Started => {
                            self.shelf_touch_id = Some(touch.id);
                            self.shelf_touch_start = [lx, ly];
                            self.shelf_touch_last_y = ly;
                            self.shelf_touch_dragged = false;
                        }
                        TouchPhase::Moved if self.shelf_touch_id == Some(touch.id) => {
                            let dy = self.shelf_touch_last_y - ly;
                            self.shelf_touch_last_y = ly;
                            if (lx - self.shelf_touch_start[0]).abs() > 10.0
                                || (ly - self.shelf_touch_start[1]).abs() > 10.0
                            {
                                self.shelf_touch_dragged = true;
                            }
                            if let Some(shelf) = &mut self.shelf {
                                shelf.scroll(dy);
                            }
                        }
                        TouchPhase::Ended if self.shelf_touch_id == Some(touch.id) => {
                            self.shelf_touch_id = None;
                            if !self.shelf_touch_dragged {
                                if let Some(action) =
                                    self.shelf.as_ref().and_then(|s| s.hit_test(lx, ly))
                                {
                                    self.handle_shelf_action(action);
                                }
                            }
                        }
                        TouchPhase::Cancelled => {
                            self.shelf_touch_id = None;
                        }
                        _ => {}
                    }
                    return;
                }
                #[cfg(target_os = "android")]
                if self.android_video.is_playing() {
                    if touch.phase == TouchPhase::Ended && self.android_video.is_skippable() {
                        self.android_video.stop(&mut self.audio);
                        self.audio.set_game_paused_for_video(false);
                        self.runtime.notify_video_finished();
                        self.runtime.dirty = true;
                    }
                    return;
                }
                match touch.phase {
                    TouchPhase::Started => self.runtime.touch_start(touch.id, lx, ly),
                    TouchPhase::Moved => self.runtime.touch_move(touch.id, lx, ly),
                    TouchPhase::Ended => self.runtime.touch_end(touch.id, lx, ly),
                    TouchPhase::Cancelled => self.runtime.touch_cancel(touch.id),
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.wake_cursor();
                self.cursor_logical = [
                    position.x as f32 / self.scale_factor,
                    position.y as f32 / self.scale_factor,
                ];
                if self.showing_shelf {
                    let over = self.shelf.as_mut().is_some_and(|s| {
                        s.set_hover(self.cursor_logical[0], self.cursor_logical[1])
                    });
                    if let Some(win) = &self.window {
                        win.set_cursor(if over {
                            winit::window::CursorIcon::Pointer
                        } else {
                            winit::window::CursorIcon::Default
                        });
                    }
                    return;
                }
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
                self.wake_cursor();
                if self.showing_shelf {
                    if let Some(shelf) = &mut self.shelf {
                        shelf.set_hover(-1.0, -1.0);
                    }
                    return;
                }
                if !self.left_mouse_down {
                    self.runtime.set_hover(-1.0, -1.0);
                    if let Some(win) = &self.window {
                        win.set_cursor(winit::window::CursorIcon::Default);
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.wake_cursor();
                if self.showing_shelf {
                    if button == MouseButton::Left && state == ElementState::Released {
                        if let Some(action) = self
                            .shelf
                            .as_ref()
                            .and_then(|s| s.hit_test(self.cursor_logical[0], self.cursor_logical[1]))
                        {
                            self.handle_shelf_action(action);
                        }
                    }
                    return;
                }
                match (button, state) {
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
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => (p.y as f32) / 30.0,
                };
                if self.showing_shelf {
                    if let Some(shelf) = &mut self.shelf {
                        shelf.scroll(-dy * 48.0);
                    }
                    return;
                }
                if dy.abs() > 0.2 {
                    // winit +y = scroll up (matches browser -deltaY)
                    self.runtime.scroll_wheel(-dy);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                if self.showing_shelf {
                    if pressed && !event.repeat {
                        if matches!(
                            event.logical_key,
                            Key::Named(NamedKey::GoBack | NamedKey::BrowserBack)
                        ) {
                            self.handle_back_action();
                        } else if let PhysicalKey::Code(code) = event.physical_key {
                            match code {
                                KeyCode::ArrowLeft | KeyCode::ArrowUp => {
                                    if let Some(s) = &mut self.shelf {
                                        s.move_selection(-1);
                                    }
                                }
                                KeyCode::ArrowRight | KeyCode::ArrowDown => {
                                    if let Some(s) = &mut self.shelf {
                                        s.move_selection(1);
                                    }
                                }
                                KeyCode::Enter | KeyCode::Space => {
                                    if let Some(path) =
                                        self.shelf.as_ref().and_then(|s| s.selected_path())
                                    {
                                        self.switch_pak_file(&path);
                                    }
                                }
                                KeyCode::KeyR | KeyCode::F5 => {
                                    self.handle_shelf_action(shelf::ShelfAction::Rescan);
                                }
                                KeyCode::KeyO => {
                                    self.handle_shelf_action(shelf::ShelfAction::OpenFilePicker);
                                }
                                KeyCode::Escape | KeyCode::BrowserBack => {
                                    self.handle_back_action();
                                }
                                _ => {}
                            }
                        }
                    }
                    return;
                }
                if pressed
                    && !event.repeat
                    && matches!(
                        event.logical_key,
                        Key::Named(NamedKey::GoBack | NamedKey::BrowserBack)
                    )
                {
                    self.handle_back_action();
                } else if let PhysicalKey::Code(code) = event.physical_key {
                    match code {
                        KeyCode::ShiftLeft | KeyCode::ShiftRight => {
                            self.shift_held = pressed;
                        }
                        KeyCode::ControlLeft | KeyCode::ControlRight => {
                            self.ctrl_held = pressed;
                            self.runtime.dispatch(UiAction::HoldSkip(pressed));
                        }
                        KeyCode::Space | KeyCode::Enter if pressed && !event.repeat => {
                            #[cfg(not(target_os = "android"))]
                            if self.video.is_playing() {
                                if self.video.is_skippable() {
                                    self.video.stop();
                                    self.audio.set_host_paused(false);
                                    self.runtime.notify_video_finished();
                                }
                                return;
                            }
                            #[cfg(target_os = "android")]
                            if self.android_video.is_playing() {
                                if self.android_video.is_skippable() {
                                    self.android_video.stop(&mut self.audio);
                                    self.audio.set_game_paused_for_video(false);
                                    self.runtime.notify_video_finished();
                                    self.runtime.dirty = true;
                                }
                                return;
                            }
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
                        KeyCode::ArrowUp if pressed => {
                            self.runtime.scroll_wheel(-1.0);
                        }
                        KeyCode::ArrowDown if pressed => {
                            self.runtime.scroll_wheel(1.0);
                        }
                        KeyCode::KeyB if pressed && !event.repeat => {
                            self.runtime.dispatch(UiAction::ToggleBilingual);
                        }
                        KeyCode::KeyC if pressed && !event.repeat => {
                            if self.shift_held {
                                self.toggle_auto_copy_hook();
                            } else if !self.ctrl_held {
                                self.copy_current_dialogue(true);
                            }
                        }
                        KeyCode::KeyP if pressed && !event.repeat => {
                            self.capture_and_save_screenshot();
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
                } else if pressed {
                    // Android Bluetooth gamepads / TV remotes / handhelds often emit `Unidentified`
                    // physical keys with semantic `NamedKey` values (D-Pad, Select/A, Back/B, Media).
                    match event.logical_key {
                        Key::Named(NamedKey::Select | NamedKey::Enter | NamedKey::Execute)
                            if !event.repeat =>
                        {
                            #[cfg(target_os = "android")]
                            if self.android_video.is_playing() {
                                if self.android_video.is_skippable() {
                                    self.android_video.stop(&mut self.audio);
                                    self.audio.set_game_paused_for_video(false);
                                    self.runtime.notify_video_finished();
                                    self.runtime.dirty = true;
                                }
                                return;
                            }
                            self.runtime.dispatch(UiAction::Advance);
                        }
                        Key::Named(NamedKey::Escape | NamedKey::Cancel) if !event.repeat => {
                            self.handle_back_action();
                        }
                        Key::Named(NamedKey::ArrowUp) => {
                            self.runtime.scroll_wheel(-1.0);
                        }
                        Key::Named(NamedKey::ArrowDown) => {
                            self.runtime.scroll_wheel(1.0);
                        }
                        Key::Named(NamedKey::MediaPlayPause) if !event.repeat => {
                            self.runtime.dispatch(UiAction::ToggleAuto);
                        }
                        Key::Named(NamedKey::MediaFastForward) if !event.repeat => {
                            self.runtime.dispatch(UiAction::ToggleSkip);
                        }
                        Key::Named(NamedKey::MediaRewind) if !event.repeat => {
                            self.runtime.dispatch(UiAction::Rollback);
                        }
                        _ => {}
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                self.tick_and_render();
                if self.should_quit {
                    self.save_and_flush_on_exit();
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.showing_shelf {
            let wake_us = self
                .shelf
                .as_mut()
                .and_then(|s| s.next_wakeup_delay_us());
            let dirty = self.shelf.as_ref().is_some_and(|s| s.dirty);
            if dirty {
                if let Some(win) = &self.window {
                    win.request_redraw();
                }
            }
            if let Some(us) = wake_us {
                event_loop.set_control_flow(ControlFlow::WaitUntil(
                    Instant::now() + Duration::from_micros(us.max(16_000)),
                ));
            } else {
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            return;
        }

        self.drain_decoded_images();
        for ended_ch in self.audio.poll_ended() {
            self.runtime.notify_audio_ended(ended_ch);
        }
        #[cfg(not(target_os = "android"))]
        if self.video.poll_finished() {
            self.audio.set_host_paused(false);
            self.runtime.notify_video_finished();
        }
        let now = Instant::now();
        self.update_idle_cursor(now);

        // NOTE: Do NOT poll while waiting on `pending_decodes` — background workers wake us
        // immediately via `EventLoopProxy::send_event(UserEvent::ImageDecoded)`!
        let mut next_delay_us = self.runtime.next_wakeup_delay_us();
        let merge_delay = |cur: Option<u64>, add: Option<u64>| match (cur, add) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        next_delay_us = merge_delay(next_delay_us, self.audio.next_wakeup_delay_us());
        next_delay_us = merge_delay(next_delay_us, self.idle_cursor_wakeup_us(now));
        #[cfg(not(target_os = "android"))]
        if self.video.is_playing() {
            next_delay_us = merge_delay(next_delay_us, Some(150_000));
        }
        #[cfg(target_os = "android")]
        if self.android_video.is_playing() {
            next_delay_us = merge_delay(next_delay_us, Some(33_000));
        }

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
                // Sleep precisely until next Auto / Toast / Typewriter / Audio / Cursor / Timer event
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
