use anyhow::{anyhow, bail, ensure, Context, Result};
use nir_engine::Engine;
use nir_format::*;
use nir_player::{AppCommand, AppEvent};
use nir_presentation::SlotView;
use nir_render_wgpu::{Renderer, RendererBackend};
use player_windows::{atomic_write, Bundle, Storage};
use rodio::{buffer::SamplesBuffer, Decoder, OutputStream, OutputStreamBuilder, Sink, Source};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    io::Cursor,
    path::PathBuf,
    sync::{mpsc, Arc},
    time::{Duration, Instant},
};
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, NamedKey},
    window::{Fullscreen, Window, WindowId},
};

fn engine_result<T>(result: std::result::Result<T, String>) -> Result<T> {
    result.map_err(|e| anyhow!(e))
}
enum Job {
    Content {
        request: u32,
        hashes: Vec<String>,
        limit: usize,
    },
    Asset {
        request: u32,
        id: String,
        descriptor: Asset,
    },
}
enum Loaded {
    Content {
        request: u32,
        data: std::result::Result<Vec<Vec<u8>>, String>,
    },
    Asset {
        request: u32,
        id: String,
        data: std::result::Result<(Vec<u8>, Option<SamplesBuffer>), String>,
    },
}
fn worker(bundle: Arc<Bundle>) -> (mpsc::SyncSender<Job>, mpsc::Receiver<Loaded>) {
    let (send, jobs) = mpsc::sync_channel::<Job>(1);
    let (done, receive) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        while let Ok(job) = jobs.recv() {
            let loaded = match job {
                Job::Content {
                    request,
                    hashes,
                    limit,
                } => {
                    let data = (|| -> Result<_> {
                        let mut used = 0usize;
                        let mut data = Vec::new();
                        ensure!(hashes.len() <= 128, "E_CONTENT_LIMIT");
                        for hash in hashes {
                            let size = bundle
                                .manifest
                                .objects
                                .get(&hash)
                                .context("E_OBJECT_REFERENCE")?
                                .bytes;
                            ensure!(size <= limit.saturating_sub(used) as u64, "E_CONTENT_LIMIT");
                            let bytes = bundle.object(&hash)?;
                            used += bytes.len();
                            data.push(bytes);
                        }
                        Ok(data)
                    })()
                    .map_err(|e| e.to_string());
                    Loaded::Content { request, data }
                }
                Job::Asset {
                    request,
                    id,
                    descriptor,
                } => {
                    let data = (|| -> Result<_> {
                        let bytes = bundle.object(&descriptor.object)?;
                        let audio = if descriptor.kind == AssetKind::Audio {
                            let decoder = Decoder::try_from(Cursor::new(bytes.clone()))?;
                            let channels = decoder.channels();
                            let rate = decoder.sample_rate();
                            let samples: Vec<f32> = decoder.collect();
                            ensure!(
                                samples.len() as u64 * 4 <= descriptor.decoded_bytes,
                                "E_AUDIO_SIZE"
                            );
                            Some(SamplesBuffer::new(channels, rate, samples))
                        } else {
                            None
                        };
                        Ok((bytes, audio))
                    })()
                    .map_err(|e| e.to_string());
                    Loaded::Asset { request, id, data }
                }
            };
            if done.send(loaded).is_err() {
                break;
            }
        }
    });
    (send, receive)
}
struct Voice {
    sink: Sink,
    session: u32,
    bus: AudioBus,
    asset: String,
}
struct Runtime {
    window: Arc<Window>,
    engine: Engine,
    storage: Storage,
    send: mpsc::SyncSender<Job>,
    receive: mpsc::Receiver<Loaded>,
    jobs: VecDeque<Job>,
    working: bool,
    upload: Option<(u32, String, Vec<u8>)>,
    audio: Option<OutputStream>,
    buffers: BTreeMap<String, SamplesBuffer>,
    voices: BTreeMap<u32, Voice>,
    audio_paused: bool,
    sequence: u32,
    last: Instant,
    cursor: (f32, f32),
    hidden: bool,
    audio_starts: usize,
}
impl Runtime {
    fn new(window: Arc<Window>, bundle: Arc<Bundle>, data: PathBuf) -> Result<Self> {
        let storage = Storage::open(
            &data,
            &bundle.manifest.game_id,
            &bundle.manifest.profile,
            &bundle.release,
        )?;
        let renderer = create_renderer(window.clone())?;
        let mut engine = engine_result(Engine::new(
            bundle.executable()?,
            bundle.release.clone(),
            bundle.manifest.title.clone(),
            storage.preferences()?,
            renderer,
        ))?;
        engine_result(engine.event(AppEvent::Profile(storage.profile()?)))?;
        let (send, receive) = worker(bundle.clone());
        let mut runtime = Self {
            window,
            engine,
            storage,
            send,
            receive,
            jobs: VecDeque::new(),
            working: false,
            upload: None,
            audio: OutputStreamBuilder::open_default_stream().ok(),
            buffers: BTreeMap::new(),
            voices: BTreeMap::new(),
            audio_paused: true,
            sequence: 0,
            last: Instant::now(),
            cursor: (0., 0.),
            hidden: false,
            audio_starts: 0,
        };
        runtime.commands()?;
        Ok(runtime)
    }
    fn input(&mut self, action: UiAction) -> Result<()> {
        self.sequence = self.sequence.checked_add(1).context("E_INPUT_SEQUENCE")?;
        self.engine.begin_turn();
        engine_result(self.engine.input(action, self.sequence))?;
        self.commands()?;
        self.window.request_redraw();
        Ok(())
    }
    fn volume(&self, bus: AudioBus) -> f32 {
        let p = self.engine.preferences();
        match bus {
            AudioBus::Bgm => p.bgm_volume,
            AudioBus::Voice => p.voice_volume,
            AudioBus::Sfx => p.sfx_volume,
        }
    }
    fn commands(&mut self) -> Result<()> {
        for _ in 0..32 {
            let commands = self.engine.take_commands();
            if commands.is_empty() {
                return Ok(());
            }
            for command in commands {
                match command {
                    AppCommand::GetContent {
                        request,
                        objects,
                        max_bytes,
                        ..
                    } => {
                        self.jobs.push_back(Job::Content {
                            request,
                            hashes: objects.into_iter().map(|o| o.hash).collect(),
                            limit: max_bytes
                                .unwrap_or(MAX_INPUT_BYTES as u64)
                                .min(MAX_INPUT_BYTES as u64)
                                as usize,
                        });
                    }
                    AppCommand::GetAssets {
                        request,
                        assets,
                        descriptors,
                        ..
                    } => {
                        for id in assets {
                            let descriptor =
                                descriptors.get(&id).context("E_ASSET_DESCRIPTOR")?.clone();
                            self.jobs.push_back(Job::Asset {
                                request,
                                id,
                                descriptor,
                            });
                        }
                    }
                    AppCommand::CancelContent { request } => self
                        .jobs
                        .retain(|j| !matches!(j, Job::Content { request: r, .. } if *r == request)),
                    AppCommand::CancelAssets { request } => {
                        self.jobs.retain(
                            |j| !matches!(j, Job::Asset { request: r, .. } if *r == request),
                        );
                        if self.upload.as_ref().is_some_and(|u| u.0 == request) {
                            self.upload = None;
                        }
                        engine_result(self.engine.host_event(
                            "assets_cancelled".into(),
                            serde_json::json!({"request":request}).to_string(),
                        ))?;
                    }
                    AppCommand::AudioStart {
                        task,
                        asset,
                        bus,
                        looped,
                        position_us,
                        session,
                    } => {
                        self.voices.remove(&task);
                        let result = (|| -> Result<Sink> {
                            let stream = self
                                .audio
                                .as_ref()
                                .context("E_AUDIO_DEVICE: no output device")?;
                            let source =
                                self.buffers.get(&asset).context("E_AUDIO_BUFFER")?.clone();
                            let sink = Sink::connect_new(stream.mixer());
                            sink.set_volume(self.volume(bus));
                            let duration = source.total_duration().unwrap_or_default();
                            let offset = if looped && !duration.is_zero() {
                                position_us.0 % duration.as_micros() as u64
                            } else {
                                position_us.0
                            };
                            if looped {
                                sink.append(
                                    source
                                        .repeat_infinite()
                                        .skip_duration(Duration::from_micros(offset)),
                                );
                            } else {
                                sink.append(source.skip_duration(Duration::from_micros(offset)));
                            }
                            if self.audio_paused {
                                sink.pause();
                            }
                            Ok(sink)
                        })();
                        match result {
                            Ok(sink) => {
                                self.voices.insert(
                                    task,
                                    Voice {
                                        sink,
                                        session,
                                        bus,
                                        asset,
                                    },
                                );
                                self.audio_starts += 1;
                            }
                            Err(e) => engine_result(self.engine.audio_failed(
                                task,
                                session,
                                e.to_string(),
                            ))?,
                        }
                    }
                    AppCommand::AudioStop { task } => {
                        self.voices.remove(&task);
                    }
                    AppCommand::AudioReset => self.voices.clear(),
                    AppCommand::AudioPause { paused } => {
                        self.audio_paused = paused;
                        for voice in self.voices.values() {
                            if paused {
                                voice.sink.pause();
                            } else {
                                voice.sink.play();
                            }
                        }
                    }
                    AppCommand::ApplyPreferences { .. } => {
                        for voice in self.voices.values() {
                            voice.sink.set_volume(self.volume(voice.bus));
                        }
                    }
                    AppCommand::PersistPreferences { preferences } => {
                        self.storage.write_preferences(&preferences)?
                    }
                    AppCommand::PersistProfile { keys } => self.storage.merge_profile(keys)?,
                    AppCommand::Save {
                        slot,
                        expected_revision,
                        job,
                        envelope,
                    } => {
                        let event = match self.storage.save(slot, expected_revision, &envelope) {
                            Ok(()) => AppEvent::Saved {
                                job,
                                slot,
                                revision: envelope.revision,
                            },
                            Err(e) => AppEvent::SaveFailed {
                                job,
                                message: e.to_string(),
                            },
                        };
                        engine_result(self.engine.event(event))?;
                    }
                    AppCommand::Load { slot } => {
                        let event = match self.storage.load(slot) {
                            Ok(Some(envelope)) => AppEvent::Loaded {
                                envelope: Box::new(envelope),
                            },
                            Ok(None) => AppEvent::LoadFailed("E_SAVE_MISSING".into()),
                            Err(e) => AppEvent::LoadFailed(e.to_string()),
                        };
                        engine_result(self.engine.event(event))?;
                    }
                    AppCommand::ListSaves => {
                        let mut rows = Vec::new();
                        let mut revisions = BTreeMap::new();
                        for slot in 0..3 {
                            let value = self.storage.load(slot)?;
                            if let Some(ref envelope) = value {
                                revisions.insert(slot, envelope.revision);
                            }
                            rows.push(SlotView {
                                slot,
                                label: value
                                    .as_ref()
                                    .map(|s| format!("#{}", s.revision))
                                    .unwrap_or_default(),
                                exists: value.is_some(),
                            });
                        }
                        engine_result(self.engine.event(AppEvent::Slots(rows, revisions)))?;
                    }
                    AppCommand::Export { json } => {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_file_name("save.nir-save.json")
                            .save_file()
                        {
                            atomic_write(&path, json.as_bytes())?;
                        }
                        self.last = Instant::now();
                    }
                    AppCommand::Import => {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("NIR save", &["json"])
                            .pick_file()
                        {
                            let result = fs::read(path)
                                .map_err(anyhow::Error::from)
                                .and_then(|b| Ok(nir_content::parse(&b, "imported save")?));
                            let event = match result {
                                Ok(envelope) => AppEvent::Loaded {
                                    envelope: Box::new(envelope),
                                },
                                Err(e) => AppEvent::LoadFailed(e.to_string()),
                            };
                            engine_result(self.engine.event(event))?;
                        }
                        self.last = Instant::now();
                    }
                    AppCommand::PromoteContent { request, .. } => {
                        if let Some(at) = self
                            .jobs
                            .iter()
                            .position(|j| matches!(j, Job::Content {request:r,..} if *r==request))
                        {
                            let job = self.jobs.remove(at).unwrap();
                            self.jobs.push_front(job);
                        }
                    }
                    AppCommand::PromoteAssets { .. }
                    | AppCommand::ResourceStage { .. }
                    | AppCommand::Observation { .. }
                    | AppCommand::Diagnostic { .. }
                    | AppCommand::Trace { .. } => {}
                    AppCommand::PreparePresentation { .. } | AppCommand::PrepareLocale { .. } => {
                        bail!("E_NATIVE_HOST: unhandled presentation command")
                    }
                }
                ensure!(self.jobs.len() <= 256, "E_REQUEST_CAPACITY");
            }
        }
        bail!("E_HOST_BUDGET: command cycle")
    }
    fn turn(&mut self) -> Result<()> {
        self.engine.begin_turn();
        if self.engine.device_lost() {
            engine_result(self.engine.begin_recovery())?;
            engine_result(
                self.engine
                    .replace_gpu(create_renderer(self.window.clone())?),
            )?;
        }
        if let Ok(loaded) = self.receive.try_recv() {
            self.working = false;
            match loaded {
                Loaded::Content { request, data } if self.engine.accepts_content(request) => {
                    match data {
                        Ok(data) => engine_result(self.engine.content_ready(request, data))?,
                        Err(message) => {
                            engine_result(self.engine.content_failed(request, message))?
                        }
                    }
                }
                Loaded::Asset { request, id, data } if self.engine.accepts_resource(request) => {
                    match data {
                        Ok((bytes, audio)) => {
                            if let Some(audio) = audio {
                                self.buffers.insert(id.clone(), audio);
                            }
                            self.upload = Some((request, id, bytes));
                        }
                        Err(message) => {
                            engine_result(self.engine.resource_failed(request, message))?
                        }
                    }
                }
                _ => {}
            }
        }
        if let Some((request, id, bytes)) = self.upload.take() {
            match self.engine.resource(request, id.clone(), &bytes) {
                Ok(false) => self.upload = Some((request, id, bytes)),
                Ok(true) => {}
                Err(error) => engine_result(self.engine.resource_failed(request, error))?,
            }
        }
        let ended: Vec<_> = self
            .voices
            .iter()
            .filter(|(_, v)| v.sink.empty())
            .map(|(id, v)| (*id, v.session))
            .collect();
        for (task, session) in ended {
            self.voices.remove(&task);
            engine_result(self.engine.audio_ended(task, session))?;
        }
        let now = Instant::now();
        let elapsed = now.duration_since(self.last).as_micros().min(250_000) as u32;
        self.last = now;
        if !self.hidden && self.engine.needs_clock() {
            engine_result(self.engine.tick(elapsed))?;
        }
        engine_result(self.engine.continue_turn())?;
        self.commands()?;
        if !self.working && self.upload.is_none() {
            while let Some(job) = self.jobs.pop_front() {
                let valid = match &job {
                    Job::Content { request, .. } => self.engine.accepts_content(*request),
                    Job::Asset { request, .. } => self.engine.accepts_resource(*request),
                };
                if !valid {
                    continue;
                }
                self.send
                    .try_send(job)
                    .map_err(|_| anyhow!("E_WORKER_UNAVAILABLE"))?;
                self.working = true;
                break;
            }
        }
        let mut retained: BTreeSet<String> = serde_json::from_str(&self.engine.retained())?;
        retained.extend(self.voices.values().map(|v| v.asset.clone()));
        self.buffers.retain(|id, _| retained.contains(id));
        let size = self.window.inner_size();
        if !self.hidden && size.width > 0 && size.height > 0 {
            let scale = self.window.scale_factor().clamp(1., 2.) as f32;
            engine_result(self.engine.draw(
                size.width as f32 / scale,
                size.height as f32 / scale,
                scale,
            ))?;
        }
        if let Some(error) = self.engine.gpu_error() {
            bail!("E_GPU: {error}");
        }
        Ok(())
    }
    fn busy(&self) -> bool {
        self.working
            || self.upload.is_some()
            || !self.jobs.is_empty()
            || self.engine.pending_events() > 0
            || (!self.hidden && self.engine.needs_clock())
            || !self.voices.is_empty()
    }
}
fn create_renderer(window: Arc<Window>) -> Result<Renderer> {
    let size = window.inner_size();
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::DX12,
        ..Default::default()
    });
    let surface = instance.create_surface(window)?;
    Ok(pollster::block_on(Renderer::new(
        &instance,
        surface,
        size.width.max(1),
        size.height.max(1),
        RendererBackend::Dx12,
    ))?)
}
struct App {
    bundle: Arc<Bundle>,
    data: PathBuf,
    runtime: Option<Runtime>,
    error: Option<anyhow::Error>,
    smoke: Option<PathBuf>,
    smoke_step: u32,
    started: Instant,
    smoke_advance: Instant,
    advances: u32,
    hidden: bool,
}
impl App {
    fn update(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let Some(runtime) = &mut self.runtime else {
            return Ok(());
        };
        runtime.turn()?;
        if let Some(path) = &self.smoke {
            ensure!(
                self.started.elapsed() < Duration::from_secs(90),
                "E_SMOKE_TIMEOUT"
            );
            let state: serde_json::Value = serde_json::from_str(&runtime.engine.state())?;
            ensure!(
                state["error"].is_null(),
                "E_SMOKE_PLAYER: {}",
                state["error"]
            );
            match self.smoke_step {
                0 if runtime.engine.is_ready() => {
                    runtime.input(UiAction::NewGame)?;
                    self.smoke_step = 1;
                }
                1 if !state["dialogue"].is_null()
                    && self.smoke_advance.elapsed() > Duration::from_millis(120) =>
                {
                    if self.advances < 30 {
                        runtime.input(UiAction::Advance)?;
                        self.advances += 1;
                        self.smoke_advance = Instant::now();
                    } else {
                        runtime.input(UiAction::Saves)?;
                        self.smoke_step = 2;
                    }
                }
                2 if state["screen"] == "Saves" => {
                    runtime.input(UiAction::Save { slot: 0 })?;
                    self.smoke_step = 3;
                }
                3 if runtime.storage.load(0)?.is_some() => {
                    runtime.input(UiAction::Title)?;
                    runtime.input(UiAction::Saves)?;
                    runtime.input(UiAction::Load { slot: 0 })?;
                    self.smoke_step = 4;
                }
                4 if !state["dialogue"].is_null() && state["loading"] == false => {
                    atomic_write(
                        path,
                        &serde_json::to_vec_pretty(
                            &serde_json::json!({"ok":true,"elapsed_ms":self.started.elapsed().as_millis(),"audio_starts":runtime.audio_starts,"state":state,"save_revision":runtime.storage.load(0)?.map(|s|s.revision)}),
                        )?,
                    )?;
                    event_loop.exit();
                }
                _ => {}
            }
        }
        event_loop.set_control_flow(if runtime.busy() || self.smoke.is_some() {
            ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(8))
        } else {
            ControlFlow::Wait
        });
        Ok(())
    }
    fn failed(&mut self, event_loop: &ActiveEventLoop, error: anyhow::Error) {
        let state = self
            .runtime
            .as_ref()
            .map(|r| r.engine.state())
            .unwrap_or_default();
        self.error = Some(error.context(format!(
            "native step {} after {} advances; state={state}",
            self.smoke_step, self.advances
        )));
        event_loop.exit();
    }
}
impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.runtime.is_some() {
            return;
        }
        let result = (|| -> Result<Runtime> {
            let window = Arc::new(
                event_loop.create_window(
                    Window::default_attributes()
                        .with_title(&self.bundle.manifest.title)
                        .with_inner_size(LogicalSize::new(1024., 768.))
                        .with_visible(!self.hidden),
                )?,
            );
            Runtime::new(window, self.bundle.clone(), self.data.clone())
        })();
        match result {
            Ok(runtime) => self.runtime = Some(runtime),
            Err(error) => self.failed(event_loop, error),
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(runtime) = &mut self.runtime else {
            return;
        };
        let result = (|| -> Result<()> {
            match event {
                WindowEvent::CloseRequested => event_loop.exit(),
                WindowEvent::Resized(_)
                | WindowEvent::ScaleFactorChanged { .. }
                | WindowEvent::RedrawRequested => {
                    runtime.turn()?;
                }
                WindowEvent::Occluded(hidden) | WindowEvent::Focused(hidden) => {
                    // Focus loss also pauses audio and the story clock.
                    let hidden = match event {
                        WindowEvent::Focused(focused) => !focused,
                        _ => hidden,
                    };
                    runtime.hidden = hidden;
                    runtime.last = Instant::now();
                    runtime.engine.begin_turn();
                    engine_result(runtime.engine.hidden(hidden))?;
                    runtime.commands()?;
                }
                WindowEvent::CursorMoved { position, .. } => {
                    let scale = runtime.window.scale_factor().clamp(1., 2.) as f32;
                    runtime.cursor = (position.x as f32 / scale, position.y as f32 / scale);
                    engine_result(runtime.engine.hover(runtime.cursor.0, runtime.cursor.1))?;
                }
                WindowEvent::MouseInput {
                    state: ElementState::Released,
                    button: MouseButton::Left,
                    ..
                } => {
                    if let Some(action) = runtime
                        .engine
                        .hit_action(runtime.cursor.0, runtime.cursor.1)
                    {
                        runtime.input(action)?;
                    }
                }
                WindowEvent::MouseInput {
                    state: ElementState::Released,
                    button: MouseButton::Right,
                    ..
                } => runtime.input(UiAction::Menu)?,
                WindowEvent::MouseWheel { delta, .. } => {
                    let y = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y,
                        MouseScrollDelta::PixelDelta(p) => p.y as f32,
                    };
                    let state: serde_json::Value = serde_json::from_str(&runtime.engine.state())?;
                    let region = if state["screen"] == "History" {
                        ScrollRegion::History
                    } else if !state["choice"].is_null() {
                        ScrollRegion::Choices
                    } else {
                        ScrollRegion::Dialogue
                    };
                    runtime.input(UiAction::Scroll {
                        region,
                        delta: if y > 0. { -1 } else { 1 },
                    })?;
                }
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == ElementState::Pressed && !event.repeat =>
                {
                    let state: serde_json::Value = serde_json::from_str(&runtime.engine.state())?;
                    match event.logical_key {
                        Key::Named(NamedKey::Space) | Key::Named(NamedKey::Enter) => runtime
                            .input(if state["screen"] == "Title" {
                                UiAction::NewGame
                            } else if state["paused"] == true {
                                UiAction::Continue
                            } else {
                                UiAction::Advance
                            })?,
                        Key::Named(NamedKey::Escape) => runtime.input(
                            if state["screen"] == "Story" || state["screen"] == "Title" {
                                UiAction::Menu
                            } else {
                                UiAction::Close
                            },
                        )?,
                        Key::Named(NamedKey::F11) => runtime.window.set_fullscreen(
                            if runtime.window.fullscreen().is_some() {
                                None
                            } else {
                                Some(Fullscreen::Borderless(None))
                            },
                        ),
                        Key::Character(c) if c == "1" || c == "2" || c == "3" => {
                            let index = c.parse::<usize>().unwrap() - 1;
                            let actions: Vec<_> = runtime
                                .engine
                                .focus_actions()
                                .into_iter()
                                .filter(|a| matches!(a, UiAction::Choose { .. }))
                                .collect();
                            if let Some(action) = actions.get(index) {
                                runtime.input(action.clone())?;
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.failed(event_loop, error);
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Err(error) = self.update(event_loop) {
            self.failed(event_loop, error);
        }
    }
}
pub fn run() -> Result<()> {
    let exe = std::env::current_exe()?;
    let mut root = exe.parent().context("E_PACKAGE_ROOT")?.join("data");
    let mut data = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .context("E_STORAGE_ROOT: LOCALAPPDATA missing")?
        .join("NIR/games");
    let mut smoke = None;
    let mut hidden = false;
    let mut verify = false;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--content") => root = args.next().context("--content needs a path")?.into(),
            Some("--data-dir") => data = args.next().context("--data-dir needs a path")?.into(),
            Some("--smoke-report") => {
                smoke = Some(PathBuf::from(
                    args.next().context("--smoke-report needs a path")?,
                ))
            }
            Some("--hidden") => hidden = true,
            Some("--verify") => verify = true,
            _ => bail!("Unknown argument: {}", arg.to_string_lossy()),
        }
    }
    let bundle = Arc::new(Bundle::open(&root)?);
    nir_content::verify(&fs::read(&exe)?, &bundle.manifest.player)
        .context("E_NATIVE_PLAYER: use the executable packaged with this release")?;
    if verify {
        return bundle.verify_all();
    }
    let event_loop = EventLoop::new()?;
    let mut app = App {
        bundle,
        data,
        runtime: None,
        error: None,
        smoke,
        smoke_step: 0,
        started: Instant::now(),
        smoke_advance: Instant::now(),
        advances: 0,
        hidden,
    };
    event_loop.run_app(&mut app)?;
    if let Some(error) = app.error {
        return Err(error);
    }
    Ok(())
}
