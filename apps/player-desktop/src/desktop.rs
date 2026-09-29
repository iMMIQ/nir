use crate::audio_envelope::{Envelope, EnvelopeSamples, Ramp};
use crate::audio_source::AudioBuffer;
use crate::dialog::{self, DialogOutcome, DialogTask};
use crate::io_worker::{IoReply, IoRequest, IoWorker};
use crate::loader::{AssetData, Job, Loaded, Loader};
use crate::{atomic_write, Bundle, Storage};
use anyhow::{anyhow, bail, ensure, Context, Result};
use nir_engine::Engine;
use nir_format::*;
use nir_player::{AppCommand, AppEvent};
use nir_render_wgpu::{Renderer, RendererBackend};
use rodio::{OutputStream, OutputStreamBuilder, Sink};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey},
    window::{Fullscreen, Window, WindowId},
};

fn engine_result<T>(result: std::result::Result<T, String>) -> Result<T> {
    result.map_err(|e| anyhow!(e))
}
/// A worker result waiting for owner-thread admission into the engine.
enum PendingUpload {
    Bytes {
        request: u32,
        id: String,
        bytes: Arc<[u8]>,
    },
    Decoded {
        request: u32,
        id: String,
        width: u32,
        height: u32,
        pixels: Arc<[u8]>,
    },
}
impl PendingUpload {
    fn request(&self) -> u32 {
        match self {
            Self::Bytes { request, .. } | Self::Decoded { request, .. } => *request,
        }
    }
}
/// A pointer press waiting for release on the same target: button, action,
/// press position and the input identity observed at press time.
type PendingPointer = (MouseButton, UiAction, (f32, f32), (u32, u32));
struct Voice {
    sink: Sink,
    position_base: u64,
    bus: AudioBus,
    gain: f32,
    envelope: Envelope,
    asset: String,
}
struct Runtime {
    window: Arc<Window>,
    engine: Engine,
    storage: Arc<Storage>,
    io: IoWorker,
    loader: Loader,
    jobs: VecDeque<Job>,
    upload: Option<PendingUpload>,
    audio: Option<OutputStream>,
    /// A running file dialog's outcome channel; input is gated while set.
    dialog: Option<std::sync::mpsc::Receiver<DialogOutcome>>,
    buffers: BTreeMap<String, AudioBuffer>,
    voices: BTreeMap<(TimeDomain, u32, u32), Voice>,
    audio_paused: BTreeMap<TimeDomain, bool>,
    sequence: u32,
    last: Instant,
    cursor: (f32, f32),
    hidden: bool,
    held_controls: [bool; 2],
    modifiers: ModifiersState,
    pointer_down: Option<PendingPointer>,
    bar_pointer: bool,
    focused: bool,
    occluded: bool,
    audio_starts: usize,
}
impl Runtime {
    fn new(
        window: Arc<Window>,
        bundle: Arc<Bundle>,
        data: PathBuf,
        exe_check: Option<std::thread::JoinHandle<Result<()>>>,
    ) -> Result<Self> {
        let storage = Arc::new(Storage::open(
            &data,
            &bundle.manifest.game_id,
            &bundle.manifest.profile,
            &bundle.release,
        )?);
        let renderer = create_renderer(window.clone())?;
        // The whole-executable digest was hashed on a startup thread while the
        // renderer initialized; gate runtime construction on its verdict here.
        if let Some(handle) = exe_check {
            match handle.join() {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    return Err(
                        e.context("E_NATIVE_PLAYER: use the executable packaged with this release")
                    )
                }
                Err(_) => bail!("E_NATIVE_PLAYER: verification thread panicked"),
            }
        }
        let mut engine = engine_result(Engine::new(
            bundle.executable()?,
            bundle.release.clone(),
            bundle.manifest.title.clone(),
            storage.preferences()?,
            renderer,
        ))?;
        engine_result(engine.event(AppEvent::Profile(storage.profile()?)))?;
        let loader = Loader::new(bundle.clone());
        let mut runtime = Self {
            window,
            engine,
            io: IoWorker::new(storage.clone()),
            storage,
            loader,
            jobs: VecDeque::new(),
            upload: None,
            audio: OutputStreamBuilder::open_default_stream().ok(),
            dialog: None,
            buffers: BTreeMap::new(),
            voices: BTreeMap::new(),
            audio_paused: BTreeMap::from([
                (TimeDomain::Story, true),
                (TimeDomain::ForegroundUi, true),
            ]),
            sequence: 0,
            last: Instant::now(),
            cursor: (0., 0.),
            hidden: false,
            held_controls: [false; 2],
            modifiers: ModifiersState::default(),
            pointer_down: None,
            bar_pointer: false,
            focused: true,
            occluded: false,
            audio_starts: 0,
        };
        runtime.commands()?;
        Ok(runtime)
    }
    fn sample_audio_positions(&mut self) -> Result<()> {
        let mut groups: std::collections::BTreeMap<u32, Vec<nir_format::AudioPosition>> =
            std::collections::BTreeMap::new();
        for ((domain, session, task), voice) in &self.voices {
            if *domain == TimeDomain::Story && !voice.sink.empty() {
                let elapsed = voice.sink.get_pos().as_micros().min(u64::MAX as u128) as u64;
                groups
                    .entry(*session)
                    .or_default()
                    .push(nir_format::AudioPosition {
                        envelope: voice
                            .envelope
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .observation(),
                        task: *task,
                        position_us: nir_format::Micros(
                            voice.position_base.saturating_add(elapsed),
                        ),
                    });
            }
        }
        for (session, positions) in groups {
            engine_result(
                self.engine
                    .audio_positions_in(TimeDomain::Story, session, positions),
            )?;
        }
        Ok(())
    }
    fn input(&mut self, action: UiAction) -> Result<()> {
        self.sequence = self.sequence.checked_add(1).context("E_INPUT_SEQUENCE")?;
        self.engine.begin_turn();
        self.sample_audio_positions()?;
        engine_result(self.engine.input(action, self.sequence))?;
        self.commands()?;
        self.window.request_redraw();
        Ok(())
    }
    /// Opens a file dialog on its own thread; one dialog at a time, and
    /// while it runs the owner gates input and pauses the story clock.
    fn open_dialog(&mut self, task: DialogTask) -> Result<()> {
        ensure!(
            self.dialog.is_none(),
            "E_DIALOG_BUSY: a file dialog is already open"
        );
        self.dialog = Some(dialog::open(task)?);
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
                        if self.upload.as_ref().is_some_and(|u| u.request() == request) {
                            self.upload = None;
                        }
                        engine_result(self.engine.host_event(
                            "assets_cancelled".into(),
                            serde_json::json!({"request":request}).to_string(),
                        ))?;
                    }
                    AppCommand::AudioStart {
                        domain,
                        task,
                        asset,
                        bus,
                        looped,
                        position_us,
                        gain,
                        envelope: initial_envelope,
                        session,
                    } => {
                        let key = (domain, session, task);
                        self.voices.remove(&key);
                        let envelope = Arc::new(Mutex::new(Ramp::default()));
                        envelope
                            .lock()
                            .unwrap()
                            .set(initial_envelope, initial_envelope, 0);
                        let result = (|| -> Result<Sink> {
                            let stream = self
                                .audio
                                .as_ref()
                                .context("E_AUDIO_DEVICE: no output device")?;
                            let buffer = self.buffers.get(&asset).context("E_AUDIO_BUFFER")?;
                            let sink = Sink::connect_new(stream.mixer());
                            sink.set_volume(gain * self.volume(bus));
                            // The offset seek is a frame-index computation on
                            // shared samples; no per-sample skip pull happens
                            // on the owner thread.
                            sink.append(EnvelopeSamples::new(
                                buffer.source(position_us.0, looped),
                                envelope.clone(),
                                buffer.channels(),
                                buffer.rate(),
                            ));
                            if self.audio_paused.get(&domain).copied().unwrap_or(true) {
                                sink.pause();
                            }
                            Ok(sink)
                        })();
                        match result {
                            Ok(sink) => {
                                self.voices.insert(
                                    key,
                                    Voice {
                                        sink,
                                        position_base: position_us.0,
                                        bus,
                                        gain,
                                        envelope,
                                        asset,
                                    },
                                );
                                self.audio_starts += 1;
                            }
                            Err(e) => engine_result(self.engine.audio_failed_in(
                                domain,
                                task,
                                session,
                                e.to_string(),
                            ))?,
                        }
                    }
                    AppCommand::AudioEnvelope {
                        owner,
                        elapsed_us,
                        domain,
                        session,
                        task,
                        from,
                        to,
                        duration_us,
                    } => {
                        if let Some(voice) = self.voices.get(&(domain, session, task)) {
                            voice
                                .envelope
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .set_owned(owner, elapsed_us.0, from, to, duration_us.0);
                        }
                    }
                    AppCommand::AudioStop {
                        domain,
                        session,
                        task,
                    } => {
                        self.voices.remove(&(domain, session, task));
                    }
                    AppCommand::AudioReset { domain } => {
                        self.voices.retain(|(d, _, _), _| *d != domain)
                    }
                    AppCommand::AudioPause { domain, paused } => {
                        self.audio_paused.insert(domain, paused);
                        for ((d, _, _), voice) in &self.voices {
                            if *d == domain {
                                if paused {
                                    voice.sink.pause();
                                } else {
                                    voice.sink.play();
                                }
                            }
                        }
                    }
                    AppCommand::ApplyPreferences { .. } => {
                        for voice in self.voices.values() {
                            voice.sink.set_volume(voice.gain * self.volume(voice.bus));
                        }
                    }
                    AppCommand::PersistPreferences { preferences } => {
                        self.io.submit(IoRequest::WritePreferences(preferences))?
                    }
                    AppCommand::PersistProfile { keys } => {
                        self.io.submit(IoRequest::MergeProfile(keys))?
                    }
                    AppCommand::Save {
                        slot,
                        expected_revision,
                        job,
                        envelope,
                    } => self.io.submit(IoRequest::Save {
                        slot,
                        expected_revision,
                        job,
                        envelope,
                    })?,
                    AppCommand::Load { slot, job } => {
                        self.io.submit(IoRequest::Load { slot, job })?
                    }
                    AppCommand::ListSaves => self.io.submit(IoRequest::List)?,
                    AppCommand::Export { json } => {
                        self.open_dialog(DialogTask::Export { json })?;
                    }
                    AppCommand::Import => {
                        self.open_dialog(DialogTask::Import)?;
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
        // One storage reply per turn, before new commands: the events land
        // in the engine the turn after the command that issued them.
        if let Some(reply) = self.io.drain() {
            match reply {
                IoReply::Event(event) => engine_result(self.engine.event(event))?,
                IoReply::Done => {}
                IoReply::Fatal(message) => bail!(message),
            }
        }
        if let Some(dialog) = &mut self.dialog {
            match dialog.try_recv() {
                Ok(DialogOutcome::Exported(Ok(()))) | Ok(DialogOutcome::ImportCancelled) => {
                    self.dialog = None;
                }
                Ok(DialogOutcome::Exported(Err(message))) => {
                    self.dialog = None;
                    bail!("{message}");
                }
                Ok(DialogOutcome::Imported(Ok(envelope))) => {
                    self.dialog = None;
                    engine_result(self.engine.event(AppEvent::Loaded { envelope }))?;
                }
                Ok(DialogOutcome::Imported(Err(message))) => {
                    self.dialog = None;
                    engine_result(self.engine.event(AppEvent::LoadFailed(message)))?;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    bail!("E_DIALOG_THREAD: dialog ended without a reply")
                }
            }
        }
        if let Some(loaded) = self.loader.drain() {
            match loaded {
                Loaded::Content { request, data } if self.engine.accepts_content(request) => {
                    match data {
                        Ok(data) => engine_result(self.engine.content_ready(request, data))?,
                        Err(message) => {
                            engine_result(self.engine.content_failed(request, message))?
                        }
                    }
                }
                Loaded::Asset {
                    request, id, data, ..
                } if self.engine.accepts_resource(request) => match data {
                    Ok(AssetData::Bytes(bytes)) => {
                        self.upload = Some(PendingUpload::Bytes { request, id, bytes });
                    }
                    Ok(AssetData::Decoded {
                        width,
                        height,
                        pixels,
                    }) => {
                        self.upload = Some(PendingUpload::Decoded {
                            request,
                            id,
                            width,
                            height,
                            pixels,
                        });
                    }
                    Ok(AssetData::Audio {
                        samples,
                        channels,
                        rate,
                        bytes,
                    }) => {
                        self.buffers
                            .insert(id.clone(), AudioBuffer::from_parts(samples, channels, rate));
                        self.upload = Some(PendingUpload::Bytes { request, id, bytes });
                    }
                    Err(message) => engine_result(self.engine.resource_failed(request, message))?,
                },
                _ => {}
            }
        }
        if let Some(pending) = self.upload.take() {
            let admitted = match &pending {
                PendingUpload::Bytes { request, id, bytes } => {
                    self.engine.resource(*request, id.clone(), bytes)
                }
                PendingUpload::Decoded {
                    request,
                    id,
                    width,
                    height,
                    pixels,
                } => self
                    .engine
                    .resource_decoded(*request, id.clone(), *width, *height, pixels),
            };
            match admitted {
                Ok(false) => self.upload = Some(pending),
                Ok(true) => {}
                Err(error) => engine_result(self.engine.resource_failed(pending.request(), error))?,
            }
        }
        let ended: Vec<_> = self
            .voices
            .iter()
            .filter(|(_, v)| v.sink.empty())
            .map(|(id, _)| *id)
            .collect();
        for (domain, session, task) in ended {
            self.voices.remove(&(domain, session, task));
            engine_result(self.engine.audio_ended_in(domain, task, session))?;
        }
        let now = Instant::now();
        // A file dialog pauses the story clock for the same reason the
        // blocking dialog froze it: nothing behind the picker may advance.
        if !self.hidden && self.dialog.is_none() && self.engine.needs_clock() {
            let elapsed = now
                .duration_since(self.last)
                .as_micros()
                .min(u32::MAX as u128) as u32;
            self.last += Duration::from_micros(elapsed as u64);
            self.sample_audio_positions()?;
            engine_result(self.engine.tick(elapsed))?;
        } else {
            self.last = now;
        }
        engine_result(self.engine.continue_turn())?;
        self.commands()?;
        // Engine admission stays one delivery per turn; dispatching ahead
        // only pipelines object reads and decode on the worker pool.
        while self.loader.has_capacity() {
            let Some(job) = self.jobs.pop_front() else {
                break;
            };
            let valid = match &job {
                Job::Content { request, .. } => self.engine.accepts_content(*request),
                Job::Asset { request, .. } => self.engine.accepts_resource(*request),
            };
            if !valid {
                continue;
            }
            self.loader.dispatch(job)?;
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
        self.dialog.is_some()
            || self.io.outstanding() > 0
            || self.loader.outstanding() > 0
            || self.upload.is_some()
            || !self.jobs.is_empty()
            || self.engine.pending_events() > 0
            || (!self.hidden && self.engine.needs_clock())
            || !self.voices.is_empty()
    }
}
fn create_renderer(window: Arc<Window>) -> Result<Renderer> {
    let size = window.inner_size();
    #[cfg(windows)]
    let (backends, backend) = (wgpu::Backends::DX12, RendererBackend::Dx12);
    #[cfg(target_os = "linux")]
    let (backends, backend) = (wgpu::Backends::VULKAN, RendererBackend::Vulkan);
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends,
        ..Default::default()
    });
    let surface = instance.create_surface(window)?;
    Ok(pollster::block_on(Renderer::new(
        &instance,
        surface,
        size.width.max(1),
        size.height.max(1),
        backend,
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
    exe_check: Option<std::thread::JoinHandle<Result<()>>>,
    /// Set when a return-to-title ending restarted the story: the save cycle
    /// must run at the first dialogue because saves from Title are refused.
    save_at_dialogue: bool,
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
                    if self.save_at_dialogue || self.advances >= 30 {
                        runtime.input(UiAction::Saves)?;
                        self.smoke_step = 2;
                    } else {
                        runtime.input(UiAction::Advance)?;
                        self.advances += 1;
                        self.smoke_advance = Instant::now();
                    }
                }
                // A pending choice is a mid-story checkpoint: run the save
                // cycle here so the load restores an interactive state.
                // (Advance cannot settle a choice, and saving after the
                // ending would restore a dead end.)
                1 if !state["choice"].is_null() => {
                    runtime.input(UiAction::Saves)?;
                    self.smoke_step = 2;
                }
                // A finished story parks on Ended (or returns to Title); a
                // choice-free story never offers the mid-story checkpoint.
                // Ended still accepts saves; Title refuses them, so restart
                // and save at the first dialogue instead.
                1 if matches!(state["screen"].as_str(), Some("Ended") | Some("Title")) => {
                    if state["screen"] == "Title" {
                        runtime.input(UiAction::NewGame)?;
                        self.save_at_dialogue = true;
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
                4 if state["loading"] == false
                    && state["screen"] == "Story"
                    && (!state["dialogue"].is_null()
                        || !state["choice"].is_null()
                        || !state["outcome"].is_null()) =>
                {
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
            Runtime::new(
                window,
                self.bundle.clone(),
                self.data.clone(),
                self.exe_check.take(),
            )
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
        // While a file dialog is open the game behind it is inert: input is
        // gated so engine state cannot change behind the modal picker.
        let dialog_open = runtime.dialog.is_some();
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
                    match event {
                        WindowEvent::Focused(focused) => runtime.focused = focused,
                        _ => runtime.occluded = hidden,
                    }
                    let hidden = !runtime.focused || runtime.occluded;
                    if hidden {
                        runtime.engine.focus_control(None);
                        runtime.held_controls = [false; 2];
                        runtime.pointer_down = None;
                        runtime.bar_pointer = false;
                        engine_result(runtime.engine.pointer_gesture(3, 0., 0., 0))?;
                    }
                    runtime.hidden = hidden;
                    runtime.last = Instant::now();
                    runtime.engine.begin_turn();
                    engine_result(runtime.engine.hidden(hidden))?;
                    runtime.commands()?;
                }
                WindowEvent::CursorMoved { position, .. } if !dialog_open => {
                    let scale = runtime.window.scale_factor().clamp(1., 2.) as f32;
                    runtime.cursor = (position.x as f32 / scale, position.y as f32 / scale);
                    if runtime.bar_pointer {
                        engine_result(runtime.engine.pointer_gesture(
                            1,
                            runtime.cursor.0,
                            runtime.cursor.1,
                            0,
                        ))?;
                    }
                    engine_result(runtime.engine.hover(runtime.cursor.0, runtime.cursor.1))?;
                }
                WindowEvent::ModifiersChanged(modifiers) => runtime.modifiers = modifiers.state(),
                WindowEvent::CursorLeft { .. } => {
                    runtime.pointer_down = None;
                    runtime.bar_pointer = false;
                    engine_result(runtime.engine.pointer_gesture(3, 0., 0., 0))?;
                    engine_result(runtime.engine.hover(-1., -1.))?;
                }
                WindowEvent::MouseInput { state, button, .. }
                    if !dialog_open && matches!(button, MouseButton::Left | MouseButton::Right) =>
                {
                    let code = if button == MouseButton::Left { 0 } else { 2 };
                    if button == MouseButton::Left {
                        if state == ElementState::Pressed {
                            runtime.engine.focus_control(None);
                            if engine_result(runtime.engine.pointer_gesture(
                                0,
                                runtime.cursor.0,
                                runtime.cursor.1,
                                0,
                            ))? {
                                runtime.bar_pointer = true;
                                runtime.pointer_down = None;
                                return Ok(());
                            }
                        } else if runtime.bar_pointer {
                            runtime.bar_pointer = false;
                            engine_result(runtime.engine.pointer_gesture(
                                2,
                                runtime.cursor.0,
                                runtime.cursor.1,
                                0,
                            ))?;
                            return Ok(());
                        }
                    }
                    let hit =
                        runtime
                            .engine
                            .pointer_action(runtime.cursor.0, runtime.cursor.1, code);
                    if state == ElementState::Pressed {
                        runtime.engine.focus_control(None);
                        runtime.pointer_down = hit.map(|action| {
                            (
                                button,
                                action,
                                runtime.cursor,
                                runtime.engine.input_identity(),
                            )
                        });
                    } else if let Some((pressed, action, origin, identity)) =
                        runtime.pointer_down.take()
                    {
                        if button == pressed
                            && hit
                                .as_ref()
                                .is_some_and(|hit| action.same_pointer_target(hit))
                            && runtime.engine.input_identity() == identity
                            && (matches!(
                                action,
                                UiAction::MenuValue {
                                    value: nir_format::MenuValueInput::Number(_),
                                    ..
                                }
                            ) || (runtime.cursor.0 - origin.0)
                                .hypot(runtime.cursor.1 - origin.1)
                                < 20.)
                        {
                            if let Some(hit) = hit {
                                runtime.input(hit)?;
                            }
                        }
                    }
                }
                WindowEvent::MouseWheel { delta, .. } if !dialog_open => {
                    let y = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y,
                        MouseScrollDelta::PixelDelta(p) => p.y as f32,
                    };
                    if y != 0. {
                        if let Some(action) = runtime.engine.scroll_action(
                            Some(runtime.cursor),
                            if y > 0. { -1 } else { 1 },
                            false,
                        ) {
                            runtime.input(action)?;
                        }
                    }
                }
                WindowEvent::KeyboardInput { event, .. }
                    if !dialog_open
                        && event.logical_key == Key::Named(NamedKey::Control)
                        && !event.repeat =>
                {
                    let index = match event.physical_key {
                        PhysicalKey::Code(KeyCode::ControlRight) => 1,
                        _ => 0,
                    };
                    runtime.held_controls[index] = event.state == ElementState::Pressed;
                    let pressed = runtime.held_controls.iter().any(|v| *v);
                    if event.state == ElementState::Pressed || !pressed {
                        runtime.input(UiAction::HoldSkip { pressed })?;
                    }
                }
                WindowEvent::KeyboardInput { event, .. }
                    if !dialog_open
                        && event.state == ElementState::Pressed
                        && !event.repeat
                        && !runtime.modifiers.control_key()
                        && !runtime.modifiers.alt_key()
                        && !runtime.modifiers.super_key() =>
                {
                    let value_direction = match event.logical_key {
                        Key::Named(NamedKey::ArrowLeft) => Some(0),
                        Key::Named(NamedKey::ArrowRight) => Some(1),
                        Key::Named(NamedKey::ArrowUp) => Some(4),
                        Key::Named(NamedKey::ArrowDown) => Some(5),
                        Key::Named(NamedKey::Home) => Some(2),
                        Key::Named(NamedKey::End) => Some(3),
                        _ => None,
                    };
                    if let Some(action) =
                        value_direction.and_then(|d| runtime.engine.focus_value_action(d))
                    {
                        runtime.input(action)?;
                        return Ok(());
                    }
                    match event.logical_key {
                        Key::Named(NamedKey::PageUp | NamedKey::PageDown) => {
                            let delta = if event.logical_key == Key::Named(NamedKey::PageUp) {
                                -1
                            } else {
                                1
                            };
                            if let Some(action) = runtime.engine.scroll_action(None, delta, true) {
                                runtime.input(action)?;
                            }
                        }
                        Key::Named(NamedKey::Tab)
                        | Key::Named(NamedKey::ArrowLeft)
                        | Key::Named(NamedKey::ArrowRight)
                        | Key::Named(NamedKey::ArrowUp)
                        | Key::Named(NamedKey::ArrowDown) => {
                            let direction = match event.logical_key {
                                Key::Named(NamedKey::Tab) => {
                                    if runtime.modifiers.shift_key() {
                                        0
                                    } else {
                                        1
                                    }
                                }
                                Key::Named(NamedKey::ArrowLeft) => 2,
                                Key::Named(NamedKey::ArrowRight) => 3,
                                Key::Named(NamedKey::ArrowUp) => 4,
                                _ => 5,
                            };
                            runtime.engine.navigate_focus(direction);
                            if let Some((x, y)) = runtime.engine.focused_center() {
                                engine_result(runtime.engine.hover(x, y))?;
                                runtime.commands()?;
                            }
                            runtime.window.request_redraw();
                        }
                        Key::Named(NamedKey::Space) | Key::Named(NamedKey::Enter) => {
                            if let Some(action) = runtime.engine.primary_action() {
                                runtime.input(action)?;
                            }
                        }
                        Key::Named(NamedKey::Escape) => {
                            // Only the Escape arm needs the state report; parse
                            // it here instead of on every qualifying keypress.
                            let state: serde_json::Value =
                                serde_json::from_str(&runtime.engine.state())?;
                            runtime.input(
                                if state["menu_depth"].as_u64().is_some_and(|depth| depth > 0) {
                                    UiAction::Close
                                } else if state["screen"] == "Story" || state["screen"] == "Title" {
                                    UiAction::Menu
                                } else {
                                    UiAction::Close
                                },
                            )?
                        }
                        Key::Character(ref c) if c.eq_ignore_ascii_case("h") => {
                            runtime.input(UiAction::ToggleInterface)?
                        }
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
/// Save/preferences root. Windows follows %LOCALAPPDATA%; Linux follows the XDG
/// data dir ($XDG_DATA_HOME when absolute, else ~/.local/share). `--data-dir`
/// overrides this in `run`.
#[cfg(windows)]
fn default_data_root() -> Result<PathBuf> {
    Ok(std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .context("E_STORAGE_ROOT: LOCALAPPDATA missing")?
        .join("NIR/games"))
}
#[cfg(target_os = "linux")]
fn default_data_root() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME") {
        let dir = PathBuf::from(dir);
        if dir.is_absolute() {
            return Ok(dir.join("NIR/games"));
        }
    }
    Ok(std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("E_STORAGE_ROOT: HOME missing to locate ~/.local/share")?
        .join(".local/share/NIR/games"))
}

pub fn run() -> Result<()> {
    let exe = std::env::current_exe()?;
    let mut root = exe.parent().context("E_PACKAGE_ROOT")?.join("data");
    let mut data = default_data_root()?;
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
    // Hash the whole executable on a helper thread so startup overlaps the
    // digest (~14-22 ms warm, more cold) with event-loop and GPU init; the
    // verdict gates Runtime construction in resumed().
    let expected_player = bundle.manifest.player.clone();
    let exe_check = std::thread::spawn(move || -> Result<()> {
        nir_content::verify(&fs::read(&exe)?, &expected_player)?;
        Ok(())
    });
    if verify {
        match exe_check.join() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                return Err(
                    e.context("E_NATIVE_PLAYER: use the executable packaged with this release")
                )
            }
            Err(_) => bail!("E_NATIVE_PLAYER: verification thread panicked"),
        }
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
        save_at_dialogue: false,
        hidden,
        exe_check: Some(exe_check),
    };
    event_loop.run_app(&mut app)?;
    if let Some(error) = app.error {
        return Err(error);
    }
    Ok(())
}
