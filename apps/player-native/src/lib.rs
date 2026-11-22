//! Unified Native Player for Windows, Linux, macOS, and Android.
//!
//! Combines `nir-runtime` + `nir-render` + zero-copy `memmap2` `.nirpak` +
//! background image decoding + `rodio`/`symphonia` gapless audio + atomic local saves.
//! Zero self-hashing (`exe_check`), zero `recovery_ui_probe`, zero OS-split crates!

pub mod audio;
pub mod pak;
pub mod storage;

use anyhow::{Context, Result};
use audio::AudioHost;
use nir_ir::{AssetId, AssetKind, UiAction};
use nir_render::{decode_image, wgpu, Renderer, RendererBackend};
use nir_runtime::{HostCommand, Runtime};
use pak::PakArchive;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use storage::LocalStorage;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Fullscreen, Window, WindowId};

enum DecodedImage {
    Ok {
        id: AssetId,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    Err {
        id: AssetId,
        error: String,
    },
}

struct DecodeRequest {
    pak: Arc<PakArchive>,
    id: AssetId,
}

/// Runs the unified native player on Desktop (Windows / Linux / macOS).
pub fn run_desktop(pak_path: &Path, watch_hot_reload: bool) -> Result<()> {
    let pak = Arc::new(PakArchive::open_file(pak_path)?);
    let event_loop = EventLoop::new().context("creating winit event loop")?;
    run_with_event_loop(pak, None, watch_hot_reload, event_loop)
}

/// Runs the unified native player on Android (`NativeActivity`), reading
/// STORED `assets/game.nirpak` directly from Android `AssetManager`.
#[cfg(target_os = "android")]
pub fn run_android(android_app: android_activity::AndroidApp) -> Result<()> {
    use std::ffi::CString;
    use winit::platform::android::EventLoopBuilderExtAndroid;

    let name = CString::new("game.nirpak").unwrap();
    let mut asset = android_app
        .asset_manager()
        .open(&name)
        .context("opening assets/game.nirpak from APK AssetManager")?;
    let bytes = asset.buffer()?.to_vec();
    let pak = Arc::new(PakArchive::from_bytes(bytes)?);
    let data_dir = android_app.internal_data_path();
    let event_loop = EventLoop::builder()
        .with_android_app(android_app)
        .build()
        .context("creating Android event loop")?;
    run_with_event_loop(pak, data_dir, false, event_loop)
}

fn run_with_event_loop(
    pak: Arc<PakArchive>,
    storage_base: Option<PathBuf>,
    watch_hot_reload: bool,
    event_loop: EventLoop<()>,
) -> Result<()> {
    let storage = LocalStorage::open(&pak.program.meta.game_id, storage_base)?;
    let prefs = storage.load_preferences();
    let profile = storage.load_profile();
    let slots = storage.list_slot_summaries(pak.program.meta.save_slots);

    let audio = AudioHost::new(&prefs);
    let runtime = Runtime::new(pak.program.clone(), prefs, profile, slots)
        .map_err(|e| anyhow::anyhow!("initializing runtime: {e}"))?;

    // Background image decoding worker thread (keeps 60fps UI/render loop hitch-free!)
    let (req_tx, req_rx) = mpsc::channel::<DecodeRequest>();
    let (res_tx, res_rx) = mpsc::channel::<DecodedImage>();
    std::thread::Builder::new()
        .name("nir-img-decode".into())
        .spawn(move || {
            while let Ok(req) = req_rx.recv() {
                let msg = match req.pak.asset_bytes(req.id) {
                    Some(bytes) => match decode_image(bytes) {
                        Ok((width, height, rgba)) => DecodedImage::Ok {
                            id: req.id,
                            width,
                            height,
                            rgba,
                        },
                        Err(e) => DecodedImage::Err {
                            id: req.id,
                            error: e.to_string(),
                        },
                    },
                    None => DecodedImage::Err {
                        id: req.id,
                        error: "asset missing from nirpak".into(),
                    },
                };
                if res_tx.send(msg).is_err() {
                    break;
                }
            }
        })
        .context("spawning image decode worker")?;

    let mut app = NativeApp {
        pak,
        runtime,
        storage,
        audio,
        window: None,
        instance: wgpu::Instance::new(&wgpu::InstanceDescriptor::default()),
        renderer: None,
        req_tx,
        res_rx,
        pending_decodes: BTreeSet::new(),
        last_frame: Instant::now(),
        last_reload_check: Instant::now(),
        watch_hot_reload,
        cursor_logical: [0.0, 0.0],
        scale_factor: 1.0,
        window_size: [1280, 720],
        should_quit: false,
    };
    event_loop.run_app(&mut app).context("winit event loop")?;
    Ok(())
}

struct NativeApp {
    pak: Arc<PakArchive>,
    runtime: Runtime,
    storage: LocalStorage,
    audio: AudioHost,
    window: Option<Arc<Window>>,
    instance: wgpu::Instance,
    renderer: Option<Renderer>,
    req_tx: mpsc::Sender<DecodeRequest>,
    res_rx: mpsc::Receiver<DecodedImage>,
    pending_decodes: BTreeSet<AssetId>,
    last_frame: Instant,
    last_reload_check: Instant,
    watch_hot_reload: bool,
    cursor_logical: [f32; 2],
    scale_factor: f32,
    window_size: [u32; 2],
    should_quit: bool,
}

impl NativeApp {
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
            if let Some(bytes) = self.pak.asset_bytes(id) {
                if renderer.load_font(id, bytes.to_vec()).is_ok() {
                    self.runtime.mark_asset_loaded(id);
                }
            }
        }
    }

    fn drain_decoded_images(&mut self) {
        while let Ok(msg) = self.res_rx.try_recv() {
            match msg {
                DecodedImage::Ok {
                    id,
                    width,
                    height,
                    rgba,
                } => {
                    self.pending_decodes.remove(&id);
                    if let Some(renderer) = &mut self.renderer {
                        let _ = renderer.upload_rgba_premultiplied(id, width, height, &rgba);
                    }
                    self.runtime.mark_asset_loaded(id);
                }
                DecodedImage::Err { id, error } => {
                    self.pending_decodes.remove(&id);
                    self.runtime.mark_asset_failed(id, &error);
                }
            }
        }
    }

    fn process_commands(&mut self) {
        let cmds = self.runtime.take_commands();
        for cmd in cmds {
            match cmd {
                HostCommand::LoadAsset { id, .. } => {
                    let Some(meta) = self.pak.program.asset(id) else {
                        continue;
                    };
                    match meta.kind {
                        AssetKind::Image => {
                            if self.renderer.as_ref().is_some_and(|r| r.has_texture(id)) {
                                self.runtime.mark_asset_loaded(id);
                            } else if self.pending_decodes.insert(id) {
                                let _ = self.req_tx.send(DecodeRequest {
                                    pak: self.pak.clone(),
                                    id,
                                });
                            }
                        }
                        AssetKind::Font => {
                            if let (Some(renderer), Some(bytes)) =
                                (&mut self.renderer, self.pak.asset_bytes(id))
                            {
                                let _ = renderer.load_font(id, bytes.to_vec());
                            }
                            self.runtime.mark_asset_loaded(id);
                        }
                        AssetKind::Audio | AssetKind::Video | AssetKind::Raw => {
                            // Audio is decoded on PlayAudio from zero-copy mmap slice.
                            self.runtime.mark_asset_loaded(id);
                        }
                    }
                }
                HostCommand::HintPinnedAssets(pinned) => {
                    if let Some(renderer) = &mut self.renderer {
                        renderer.evict_lru(&pinned);
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
                    if let (Some(meta), Some(bytes)) =
                        (self.pak.program.asset(asset), self.pak.asset_bytes(asset))
                    {
                        self.audio.play(
                            channel,
                            bus,
                            bytes,
                            meta.codec.ext(),
                            gain,
                            looped,
                            fade_in_us,
                            loop_region,
                        );
                    }
                }
                HostCommand::StopAudio { channel, .. } => {
                    self.audio.stop(channel);
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
                HostCommand::SaveProfile(profile) => {
                    let _ = self.storage.save_profile(&profile);
                }
                HostCommand::SavePreferences(prefs) => {
                    self.audio.apply_preferences(&prefs);
                    let _ = self.storage.save_preferences(&prefs);
                }
                HostCommand::Quit => {
                    self.should_quit = true;
                }
            }
        }
    }

    fn tick_and_render(&mut self) {
        let now = Instant::now();
        let delta_us = now
            .saturating_duration_since(self.last_frame)
            .as_micros()
            .min(100_000) as u64;
        self.last_frame = now;

        // Optional live hot-reload watch on `game.nirpak`
        if self.watch_hot_reload && now.duration_since(self.last_reload_check) >= Duration::from_millis(250) {
            self.last_reload_check = now;
            if let Some(new_pak) = self.pak.check_reloaded() {
                self.pak = new_pak;
                self.pending_decodes.clear();
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

        let lw = (self.window_size[0] as f32 / self.scale_factor).max(1.0);
        let lh = (self.window_size[1] as f32 / self.scale_factor).max(1.0);
        if let Some(packet) = self.runtime.prepare_frame(lw, lh) {
            if let Some(renderer) = &mut self.renderer {
                let _ = renderer.render(packet, self.scale_factor);
            }
        }
    }
}

impl ApplicationHandler for NativeApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let sw = self.pak.program.meta.stage_width;
            let sh = self.pak.program.meta.stage_height;
            let attrs = Window::default_attributes()
                .with_title(&self.pak.program.meta.title)
                .with_inner_size(LogicalSize::new(sw.max(640) as f64, sh.max(360) as f64));
            let Ok(win) = event_loop.create_window(attrs) else {
                event_loop.exit();
                return;
            };
            let win = Arc::new(win);
            let phys = win.inner_size();
            self.window_size = [phys.width.max(1), phys.height.max(1)];
            self.scale_factor = win.scale_factor() as f32;

            let backend = if cfg!(target_os = "windows") {
                RendererBackend::Dx12
            } else if cfg!(target_os = "macos") {
                RendererBackend::Metal
            } else {
                RendererBackend::Vulkan
            };
            if let Ok(surface) = self.instance.create_surface(win.clone()) {
                if let Ok(renderer) = pollster::block_on(Renderer::new(
                    self.instance.clone(),
                    surface,
                    self.window_size[0],
                    self.window_size[1],
                    backend,
                )) {
                    self.renderer = Some(renderer);
                    self.upload_all_fonts();
                }
            }
            self.window = Some(win);
            self.runtime.dirty = true;
            self.process_commands();
        } else if let (Some(win), Some(renderer)) = (&self.window, &mut self.renderer) {
            if let Ok(surface) = self.instance.create_surface(win.clone()) {
                renderer.rebind_surface(surface);
                self.runtime.dirty = true;
            }
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
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
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                self.window_size = [size.width.max(1), size.height.max(1)];
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(self.window_size[0], self.window_size[1]);
                }
                self.runtime.dirty = true;
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale_factor = scale_factor as f32;
                self.runtime.dirty = true;
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_logical = [
                    position.x as f32 / self.scale_factor,
                    position.y as f32 / self.scale_factor,
                ];
                self.runtime
                    .set_hover(self.cursor_logical[0], self.cursor_logical[1]);
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } => match button {
                MouseButton::Left => {
                    self.runtime
                        .pointer_click(self.cursor_logical[0], self.cursor_logical[1]);
                }
                MouseButton::Right => {
                    self.runtime.cancel_or_menu();
                }
                _ => {}
            },
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => (p.y as f32) / 30.0,
                };
                if dy > 0.2 {
                    if self.runtime.screen == nir_ui::Screen::History {
                        self.runtime.dispatch(UiAction::ScrollHistory(-2));
                    } else {
                        self.runtime.dispatch(UiAction::Rollback);
                    }
                } else if dy < -0.2 {
                    if self.runtime.screen == nir_ui::Screen::History {
                        self.runtime.dispatch(UiAction::ScrollHistory(2));
                    } else {
                        self.runtime.dispatch(UiAction::Advance);
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    let pressed = event.state == ElementState::Pressed;
                    match code {
                        KeyCode::ControlLeft | KeyCode::ControlRight => {
                            self.runtime.dispatch(UiAction::HoldSkip(pressed));
                        }
                        KeyCode::Space | KeyCode::Enter if pressed && !event.repeat => {
                            self.runtime.dispatch(UiAction::Advance);
                        }
                        KeyCode::Escape if pressed && !event.repeat => {
                            self.runtime.cancel_or_menu();
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
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.drain_decoded_images();
        if self.runtime.dirty || self.runtime.needs_clock() || !self.pending_decodes.is_empty() {
            if let Some(win) = &self.window {
                win.request_redraw();
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(16),
            ));
        } else if self.watch_hot_reload {
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(250),
            ));
            if let Some(win) = &self.window {
                win.request_redraw();
            }
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}
