use crate::audio_envelope::{Envelope, EnvelopeSamples, Ramp};
use crate::audio_output::{OutputHandle, OutputReply, OutputWorker};
use crate::audio_output_state::OutputState;
use crate::audio_recovery_input::{
    Gesture as RecoveryGesture, Key as RecoveryKey, Keyboard as RecoveryKeyboard,
    Phase as RecoveryPhase, Pointer as RecoveryPointer,
};
use crate::audio_source::{AudioBuffer, SharedVoiceQueue};
use crate::close::{Close, CloseAction};
#[cfg(any(windows, target_os = "linux"))]
use crate::dialog::{self, DialogOutcome, DialogTask, PendingDialog};
use crate::io_worker::{IoReply, IoRequest, IoWorker};
use crate::lifecycle::{Lifecycle, Signal};
use crate::loader::{AssetData, Job, Loaded, Loader};
use crate::{atomic_write, Bundle, Storage};
use anyhow::{anyhow, bail, ensure, Context, Result};
use nir_engine::Engine;
use nir_format::*;
use nir_player::{AppCommand, AppEvent};
use nir_render_wgpu::{Renderer, RendererBackend};
use rodio::Sink;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
#[cfg(any(windows, target_os = "linux"))]
use winit::event_loop::EventLoop;
#[cfg(target_os = "android")]
use winit::platform::android::EventLoopBuilderExtAndroid;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow},
    keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey},
    window::{Fullscreen, Window, WindowId},
};

fn engine_result<T>(result: std::result::Result<T, String>) -> Result<T> {
    result.map_err(|e| anyhow!(e))
}
#[cfg(any(target_os = "android", test))]
fn android_export_path(root: &std::path::Path, stamp: u128, job: u32) -> PathBuf {
    root.join(format!("nir-export-{stamp}-{job}.json"))
}
#[cfg(test)]
mod export_path_tests {
    use super::*;
    #[test]
    fn android_export_names_are_distinct_even_with_the_same_clock_stamp() {
        let root = std::path::Path::new("exports");
        assert_ne!(
            android_export_path(root, 42, 1),
            android_export_path(root, 42, 2)
        );
        assert_eq!(
            android_export_path(root, 42, 1),
            root.join("nir-export-42-1.json")
        );
    }
}
/// A worker result waiting for owner-thread admission into the engine.
/// Large images admit over several turns (2 MiB row budget per turn), so
/// drained results queue behind the one currently uploading.
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
    queue: SharedVoiceQueue,
    started: bool,
    position_base: u64,
    bus: AudioBus,
    gain: f32,
    character: String,
    envelope: Envelope,
    asset: String,
}
struct Runtime {
    /// None only while the window is destroyed (Android suspend); desktop
    /// hosts keep the same window for the process lifetime.
    window: Option<Arc<Window>>,
    engine: Engine,
    /// The instance the renderer's device lives in; it must also create any
    /// replacement surface (a fresh instance cannot configure against the
    /// live device). Kept for the process lifetime.
    instance: wgpu::Instance,
    storage: Arc<Storage>,
    io: IoWorker,
    close: Close,
    loader: Loader,
    jobs: VecDeque<Job>,
    uploads: VecDeque<PendingUpload>,
    audio: Option<OutputHandle>,
    output_worker: OutputWorker,
    output: OutputState,
    output_auto_retry_used: bool,
    recovery_gesture: RecoveryGesture,
    metadata_gesture: RecoveryGesture,
    recovery_keyboard: RecoveryKeyboard,
    title: String,
    output_caption: Option<String>,
    /// A running file dialog's outcome channel; input is gated while set.
    #[cfg(any(windows, target_os = "linux"))]
    dialog: Option<PendingDialog>,
    buffers: BTreeMap<String, AudioBuffer>,
    voices: BTreeMap<(TimeDomain, u32, u32), Voice>,
    audio_paused: BTreeMap<TimeDomain, bool>,
    audio_bus_paused: BTreeMap<(TimeDomain, AudioBus), bool>,
    sequence: u32,
    lifecycle: Lifecycle,
    cursor: (f32, f32),
    held_controls: [bool; 2],
    modifiers: ModifiersState,
    pointer_down: Option<PendingPointer>,
    bar_pointer: bool,
    /// The finger driving the current touch sequence; secondary contacts are
    /// ignored because the engine tracks a single pointer.
    touch_id: Option<u64>,
    audio_starts: usize,
    /// Where `Export` replies land on Android, which has no file picker.
    #[cfg(target_os = "android")]
    exports: PathBuf,
}
impl Runtime {
    fn new(
        window: Arc<Window>,
        bundle: Arc<Bundle>,
        data: PathBuf,
        exe_check: Option<std::thread::JoinHandle<Result<()>>>,
        #[cfg(target_os = "android")] exports: PathBuf,
    ) -> Result<Self> {
        let storage = Arc::new(Storage::open(
            &data,
            &bundle.manifest.game_id,
            &bundle.manifest.profile,
            &bundle.release,
        )?);
        let (renderer, instance) = create_renderer(window.clone())?;
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
        let startup = storage.startup();
        let mut engine = engine_result(Engine::new(
            bundle.executable()?,
            bundle.release.clone(),
            bundle.manifest.title.clone(),
            startup.preferences,
            renderer,
        ))?;
        engine_result(engine.event(AppEvent::Profile(startup.profile)))?;
        for failure in startup.failures {
            engine_result(engine.event(failure))?;
        }
        let loader = Loader::new(bundle.clone());
        let mut runtime = Self {
            window: Some(window),
            engine,
            instance,
            io: IoWorker::new(storage.clone()),
            close: Close::default(),
            storage,
            loader,
            jobs: VecDeque::new(),
            uploads: VecDeque::new(),
            audio: None,
            output_worker: OutputWorker::new(),
            output: OutputState::default(),
            output_auto_retry_used: false,
            recovery_gesture: RecoveryGesture::default(),
            metadata_gesture: RecoveryGesture::default(),
            recovery_keyboard: RecoveryKeyboard::default(),
            title: bundle.manifest.title.clone(),
            output_caption: None,
            #[cfg(any(windows, target_os = "linux"))]
            dialog: None,
            buffers: BTreeMap::new(),
            voices: BTreeMap::new(),
            audio_paused: BTreeMap::from([
                (TimeDomain::Story, true),
                (TimeDomain::ForegroundUi, true),
            ]),
            audio_bus_paused: BTreeMap::new(),
            sequence: 0,
            lifecycle: Lifecycle::new(Instant::now()),
            cursor: (0., 0.),
            held_controls: [false; 2],
            modifiers: ModifiersState::default(),
            pointer_down: None,
            bar_pointer: false,
            touch_id: None,
            audio_starts: 0,
            #[cfg(target_os = "android")]
            exports,
        };
        runtime.engine.begin_turn();
        runtime.retry_output(false)?;
        runtime.commands()?;
        Ok(runtime)
    }
    fn output_title(&mut self) {
        use nir_presentation::audio_recovery::Status;
        self.engine.set_native_audio_recovery(
            (self.output.blocked() && !self.voices.is_empty()).then_some(
                if self.output.pending() {
                    Status::Opening
                } else {
                    Status::Failed
                },
            ),
        );
        self.recovery_keyboard.sync(
            self.engine
                .native_audio_recovery_layout()
                .map(|layout| layout.status),
        );
        self.engine
            .set_native_audio_recovery_focus(self.recovery_keyboard.focused());
        if self.close.pending() {
            self.output_caption = None;
            return;
        }
        if let Some(window) = &self.window {
            let status = if self.output.pending() {
                "正在恢复声音… / Reconnecting sound… — "
            } else if self.output.blocked() {
                "声音不可用（F8 重试） / Sound unavailable (F8 to retry) — "
            } else {
                ""
            };
            let caption = format!("{status}{}", self.title);
            if self.output_caption.as_ref() != Some(&caption) {
                window.set_title(&caption);
                self.output_caption = Some(caption);
            }
        }
    }
    fn recovery_pointer(&mut self, pointer: RecoveryPointer, phase: RecoveryPhase) -> Result<bool> {
        let layout = self.engine.native_audio_recovery_layout();
        let button = match pointer {
            RecoveryPointer::Mouse(button) => button,
            RecoveryPointer::Touch(_) => 0,
        };
        let target = layout.and_then(|layout| layout.hit(self.cursor.0, self.cursor.1, button));
        let (consumed, action) =
            self.recovery_gesture
                .event(pointer, phase, self.cursor, layout.is_some(), target);
        if consumed {
            self.pointer_down = None;
            self.bar_pointer = false;
            match action {
                Some(nir_presentation::audio_recovery::Action::Retry) => {
                    self.engine.begin_turn();
                    self.retry_output(true)?;
                    self.commands()?;
                }
                Some(nir_presentation::audio_recovery::Action::Menu) => {
                    self.input(UiAction::Menu)?
                }
                None => {}
            }
        }
        Ok(consumed)
    }
    fn recovery_key(&mut self, key: &Key) -> Result<bool> {
        let Some(layout) = self.engine.native_audio_recovery_layout() else {
            return Ok(false);
        };
        let key = match key {
            // Fullscreen remains a window shortcut rather than a story input.
            Key::Named(NamedKey::F11) => return Ok(false),
            Key::Named(NamedKey::Tab) if self.modifiers.shift_key() => RecoveryKey::Previous,
            Key::Named(NamedKey::Tab | NamedKey::ArrowRight | NamedKey::ArrowDown) => {
                RecoveryKey::Next
            }
            Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowUp) => RecoveryKey::Previous,
            Key::Named(NamedKey::Enter | NamedKey::Space) => RecoveryKey::Activate,
            Key::Named(NamedKey::Escape | NamedKey::BrowserBack) => RecoveryKey::Menu,
            Key::Named(NamedKey::F8) => RecoveryKey::Retry,
            _ => RecoveryKey::Other,
        };
        let action = self.recovery_keyboard.event(layout.status, key);
        self.engine
            .set_native_audio_recovery_focus(self.recovery_keyboard.focused());
        match action {
            Some(nir_presentation::audio_recovery::Action::Retry) => {
                self.engine.begin_turn();
                self.retry_output(true)?;
                self.commands()?;
            }
            Some(nir_presentation::audio_recovery::Action::Menu) => self.input(UiAction::Menu)?,
            None => {}
        }
        Ok(true)
    }
    fn sync_metadata_recovery(&mut self) {
        let failed = self.engine.native_metadata_failure_kinds();
        let save_pending = self.io.has_unconfirmed_saves();
        self.engine
            .set_native_storage_recovery((save_pending || !failed.is_empty()).then(|| {
                nir_presentation::storage_recovery::Status {
                    pending: self.io.confirmation_in_flight()
                        || failed.iter().any(|kind| self.io.metadata_pending(*kind)),
                    retry_enabled: self.io.confirmation_retry_enabled()
                        || failed.iter().any(|kind| !self.io.metadata_pending(*kind)),
                    save_pending,
                }
            }));
    }
    fn retry_metadata(&mut self) -> Result<()> {
        let Some(layout) = self.engine.native_storage_recovery_layout() else {
            return Ok(());
        };
        if !layout.status.retry_enabled {
            return Ok(());
        }
        let kinds = self
            .engine
            .native_metadata_failure_kinds()
            .into_iter()
            .filter(|kind| !self.io.metadata_pending(*kind))
            .collect::<Vec<_>>();
        if kinds.is_empty() && !self.io.confirmation_retry_enabled() {
            return Ok(());
        }
        // Pointer and shortcut retries keep the same activation owner as a
        // keyboard retry. Disabling it must not transfer Enter to New Game.
        engine_result(
            self.engine
                .focus_control(Some(nir_presentation::storage_recovery::CONTROL_ID)),
        )?;
        self.engine.begin_turn();
        self.io.retry_unconfirmed_saves();
        for kind in kinds {
            self.submit_storage(IoRequest::ReadMetadata(kind))?;
        }
        self.commands()?;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        Ok(())
    }
    fn metadata_pointer(&mut self, pointer: RecoveryPointer, phase: RecoveryPhase) -> Result<bool> {
        let layout = self.engine.native_storage_recovery_layout();
        let button = match pointer {
            RecoveryPointer::Mouse(button) => button,
            RecoveryPointer::Touch(_) => 0,
        };
        let visible = layout.is_some_and(|l| l.contains(self.cursor.0, self.cursor.1));
        let target = layout
            .filter(|l| l.hit_retry(self.cursor.0, self.cursor.1, button))
            .map(|_| nir_presentation::audio_recovery::Action::Retry);
        let (consumed, action) =
            self.metadata_gesture
                .event(pointer, phase, self.cursor, visible, target);
        if consumed {
            self.pointer_down = None;
            self.bar_pointer = false;
            if action.is_some() {
                self.retry_metadata()?;
            }
        }
        Ok(consumed)
    }
    fn pause_output(&mut self) -> Result<()> {
        for voice in self.voices.values() {
            voice.sink.pause();
        }
        self.sample_audio_positions()?;
        self.audio = None;
        if !self.voices.is_empty() {
            engine_result(self.engine.audio_blocked(true))?;
        }
        // Device waiting/open/teardown time never belongs to Story.
        self.lifecycle.elapsed(Instant::now(), false);
        self.sequence = self.sequence.checked_add(1).context("E_INPUT_SEQUENCE")?;
        engine_result(
            self.engine
                .input(UiAction::HoldSkip { pressed: false }, self.sequence),
        )?;
        self.held_controls = [false; 2];
        self.modifiers = ModifiersState::default();
        self.touch_id = None;
        self.pointer_down = None;
        self.bar_pointer = false;
        engine_result(self.engine.focus_control(None))?;
        engine_result(self.engine.pointer_gesture(3, 0., 0., 0))?;
        self.output_title();
        Ok(())
    }
    fn retry_output(&mut self, manual: bool) -> Result<()> {
        if self.close.pending() || !self.output.blocked() || self.output.pending() {
            return Ok(());
        }
        if manual {
            self.output_auto_retry_used = false;
        }
        let Some(generation) = self.output.request() else {
            return Ok(());
        };
        if self.output_worker.closed() {
            self.output_worker = OutputWorker::new();
        }
        if !self.output_worker.request(generation) {
            self.output.failed(generation);
        }
        self.pause_output()
    }
    fn output_turn(&mut self) -> Result<()> {
        let fault = self.output_worker.take_fault();
        let was_ready = !self.output.blocked();
        if fault != 0 && self.output.failed(fault) {
            self.pause_output()?;
            if was_ready && !self.output_auto_retry_used {
                self.output_auto_retry_used = true;
                self.retry_output(false)?;
            }
        }
        if let Some(reply) = self.output_worker.drain() {
            match reply {
                OutputReply::Ready { generation, handle } if self.output.ready(generation) => {
                    // Settle genuinely finished sources under the output
                    // pause before admitting a fresh reading input.
                    let ended: Vec<_> = self
                        .voices
                        .iter()
                        .filter(|(_, voice)| voice.sink.empty())
                        .map(|(key, _)| *key)
                        .collect();
                    for (domain, session, task) in ended {
                        self.voices.remove(&(domain, session, task));
                        engine_result(self.engine.audio_ended_in(domain, task, session))?;
                    }
                    // The worker has dropped the old stream. Reattach the same
                    // live queue, including its sample cursor and envelope;
                    // no AudioStart/AudioEnded or replayed intro is invented.
                    for voice in self.voices.values_mut() {
                        if !voice.sink.empty() {
                            handle.mixer.add(voice.queue.clone());
                            if !voice.started {
                                self.audio_starts += 1;
                                voice.started = true;
                            }
                        }
                    }
                    self.audio = Some(handle);
                    self.lifecycle.elapsed(Instant::now(), false);
                    engine_result(self.engine.audio_blocked(false))?;
                    self.output_title();
                }
                OutputReply::Failed {
                    generation,
                    message,
                } if self.output.failed(generation) => {
                    eprintln!("{message}");
                    self.pause_output()?;
                }
                _ => {} // Stale opens/errors cannot replace a newer output.
            }
        }
        Ok(())
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
        if action == UiAction::Retry && self.engine.native_storage_recovery_focused() {
            return self.retry_metadata();
        }
        // The output can become ready in output_turn. A reading action that
        // arrived while its notice was up must not exploit that race.
        let recovery_input = self.output.blocked()
            && !self.voices.is_empty()
            && matches!(
                action,
                UiAction::Advance
                    | UiAction::Continue
                    | UiAction::Choose { .. }
                    | UiAction::SelectChoice { .. }
                    | UiAction::CancelChoice
                    | UiAction::ToggleAuto
                    | UiAction::ToggleSkip
                    | UiAction::ToggleInterface
                    | UiAction::HoldSkip { pressed: true }
            );
        self.engine.begin_turn();
        self.output_turn()?;
        if recovery_input {
            return self.commands();
        }
        self.sequence = self.sequence.checked_add(1).context("E_INPUT_SEQUENCE")?;
        self.sample_audio_positions()?;
        engine_result(self.engine.input(action, self.sequence))?;
        self.commands()?;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        Ok(())
    }
    fn dialog_open(&self) -> bool {
        #[cfg(any(windows, target_os = "linux"))]
        {
            self.dialog.is_some()
        }
        #[cfg(target_os = "android")]
        {
            false
        }
    }
    /// Detaches a window that the system is about to destroy (Android
    /// suspend): the presentation surface goes first so the renderer never
    /// outlives the raw window handle, then the window reference itself.
    fn suspend_window(&mut self) -> Result<()> {
        let paused = self.visibility(Signal::Suspended(true));
        // Even a failed engine notification must not retain a raw window
        // handle after the operating system's Suspended callback returns.
        self.engine.release_surface();
        self.window = None;
        paused
    }
    /// Attaches the window recreated after a resume and rebinds its surface
    /// onto the live device; no asset replay happens, so the story, textures
    /// and audio continue where they left off.
    fn resume_window(&mut self, window: Arc<Window>) -> Result<()> {
        // The surface must come from the instance that owns the live device;
        // a fresh instance would configure against a foreign device id.
        let surface = create_surface_on(&self.instance, window.clone())?;
        engine_result(self.engine.rebind_surface(surface))?;
        self.window = Some(window);
        // A caption cached for the destroyed window does not belong to this
        // replacement, even when the audio status itself stayed unchanged.
        self.output_caption = None;
        self.visibility(Signal::Suspended(false))?;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        Ok(())
    }
    /// Apply lifecycle ownership before admitting storage or resource replies.
    /// Pause existing sinks first, without depending on a later Focused event
    /// or on engine notification succeeding. Unpause only through Player's
    /// domain/bus commands, which retain menu and restored-session policies.
    fn visibility(&mut self, signal: Signal) -> Result<()> {
        self.lifecycle.signal(signal, Instant::now());
        let hidden = self.lifecycle.hidden();
        if hidden {
            self.recovery_gesture.clear();
            self.metadata_gesture.clear();
            for voice in self.voices.values() {
                voice.sink.pause();
            }
            self.held_controls = [false; 2];
            self.modifiers = ModifiersState::default();
            self.touch_id = None;
            self.pointer_down = None;
            self.bar_pointer = false;
        }
        self.engine.begin_turn();
        self.sample_audio_positions()?;
        engine_result(self.engine.hidden(hidden))?;
        if hidden {
            engine_result(self.engine.focus_control(None))?;
            engine_result(self.engine.pointer_gesture(3, 0., 0., 0))?;
        }
        if !hidden && self.output.blocked() && !self.output.pending() && !self.close.pending() {
            self.retry_output(true)?;
        }
        self.commands()
    }
    fn voice_paused(&self, domain: TimeDomain, bus: AudioBus) -> bool {
        self.output.blocked()
            || self.lifecycle.audio_paused(
                self.audio_paused.get(&domain).copied().unwrap_or(true),
                self.audio_bus_paused
                    .get(&(domain, bus))
                    .copied()
                    .unwrap_or(false),
            )
    }
    /// Opens a file dialog on its own thread; one dialog at a time, and
    /// while it runs the owner gates input and pauses the story clock.
    #[cfg(any(windows, target_os = "linux"))]
    fn open_dialog(&mut self, task: DialogTask) -> Result<()> {
        let failure = task.failure();
        let result = if self.dialog.is_some() {
            Err(anyhow!("E_DIALOG_BUSY: a file dialog is already open"))
        } else {
            dialog::open(task)
        };
        match result {
            Ok(dialog) => self.dialog = Some(dialog),
            Err(error) => {
                self.close.save_failed();
                engine_result(self.engine.event(failure.event(error.to_string())))?;
            }
        }
        Ok(())
    }
    fn character_volume(&self, bus: AudioBus, character: &str) -> f32 {
        if bus == AudioBus::Voice {
            self.engine.preferences().character_voice_gain(character)
        } else {
            1.
        }
    }
    fn update_voice_volumes(&self) {
        for voice in self.voices.values() {
            voice.sink.set_volume(
                voice.gain
                    * self.volume(voice.bus)
                    * self.character_volume(voice.bus, &voice.character),
            );
        }
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
                self.output_title();
                // Output ready may release no domain/bus command: its pause
                // is an independent logical barrier. Apply all owners here.
                for ((domain, _, _), voice) in &self.voices {
                    if self.voice_paused(*domain, voice.bus) {
                        voice.sink.pause();
                    } else {
                        voice.sink.play();
                    }
                }
                self.sync_metadata_recovery();
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
                        self.uploads.retain(|u| u.request() != request);
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
                        loop_region,
                        position_us,
                        gain,
                        envelope: initial_envelope,
                        character,
                        session,
                    } => {
                        let key = (domain, session, task);
                        self.voices.remove(&key);
                        let envelope = Arc::new(Mutex::new(Ramp::default()));
                        envelope
                            .lock()
                            .unwrap()
                            .set(initial_envelope, initial_envelope, 0);
                        let result = (|| -> Result<(Sink, SharedVoiceQueue)> {
                            let buffer = self.buffers.get(&asset).context("E_AUDIO_BUFFER")?;
                            let rate = self
                                .audio
                                .as_ref()
                                .map_or(buffer.rate(), |output| output.rate);
                            let (sink, queue) = Sink::new();
                            sink.set_volume(
                                gain * self.volume(bus) * self.character_volume(bus, &character),
                            );
                            sink.append(EnvelopeSamples::new(
                                buffer
                                    .source_with_region(position_us.0, looped, loop_region)
                                    .map_err(|error| anyhow::anyhow!(error))?
                                    .at_device_rate(rate),
                                envelope.clone(),
                                buffer.channels(),
                                rate,
                            ));
                            if self.voice_paused(domain, bus) {
                                sink.pause();
                            }
                            let queue = SharedVoiceQueue::new(queue, buffer.channels(), rate);
                            if let Some(output) = &self.audio {
                                output.mixer.add(queue.clone());
                            }
                            Ok((sink, queue))
                        })();
                        match result {
                            Ok((sink, queue)) => {
                                self.voices.insert(
                                    key,
                                    Voice {
                                        sink,
                                        queue,
                                        started: self.audio.is_some(),
                                        position_base: position_us.0,
                                        bus,
                                        gain,
                                        character,
                                        envelope,
                                        asset,
                                    },
                                );
                                if self.audio.is_some() {
                                    self.audio_starts += 1;
                                }
                                if self.output.blocked() {
                                    engine_result(self.engine.audio_blocked(true))?;
                                    self.lifecycle.elapsed(Instant::now(), false);
                                }
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
                        if self.output.blocked() && self.voices.is_empty() {
                            engine_result(self.engine.audio_blocked(false))?;
                        }
                    }
                    AppCommand::AudioReset { domain } => {
                        self.voices.retain(|(d, _, _), _| *d != domain);
                        if self.output.blocked() && self.voices.is_empty() {
                            engine_result(self.engine.audio_blocked(false))?;
                        }
                    }
                    AppCommand::AudioPause { domain, paused } => {
                        self.audio_paused.insert(domain, paused);
                        for ((d, _, _), voice) in &self.voices {
                            if *d == domain {
                                if self.voice_paused(domain, voice.bus) {
                                    voice.sink.pause();
                                } else {
                                    voice.sink.play();
                                }
                            }
                        }
                    }
                    AppCommand::AudioBusPause {
                        domain,
                        bus,
                        paused,
                    } => {
                        self.audio_bus_paused.insert((domain, bus), paused);
                        for ((d, _, _), voice) in &self.voices {
                            if *d == domain && voice.bus == bus {
                                if self.voice_paused(domain, bus) {
                                    voice.sink.pause();
                                } else {
                                    voice.sink.play();
                                }
                            }
                        }
                    }
                    AppCommand::AudioCharacter {
                        domain,
                        task,
                        character,
                        session,
                    } => {
                        if let Some(voice) = self.voices.get_mut(&(domain, session, task)) {
                            voice.character = character;
                        }
                        self.update_voice_volumes();
                    }
                    AppCommand::ApplyPreferences { .. } => {
                        self.update_voice_volumes();
                    }
                    AppCommand::PersistPreferences { preferences } => {
                        self.update_voice_volumes();
                        self.submit_storage(IoRequest::WritePreferences(preferences))?
                    }
                    AppCommand::PersistProfile { keys } => {
                        self.submit_storage(IoRequest::MergeProfile(keys))?
                    }
                    AppCommand::Save {
                        slot,
                        expected_revision,
                        job,
                        envelope,
                    } => self.submit_storage(IoRequest::Save {
                        slot,
                        expected_revision,
                        job,
                        envelope,
                    })?,
                    AppCommand::Load { slot, job } => {
                        self.submit_storage(IoRequest::Load { slot, job })?
                    }
                    AppCommand::ListSaves => self.submit_storage(IoRequest::List)?,
                    #[cfg(any(windows, target_os = "linux"))]
                    AppCommand::Export { job, json } => {
                        self.open_dialog(DialogTask::Export { job, json })?;
                    }
                    #[cfg(any(windows, target_os = "linux"))]
                    AppCommand::Import => {
                        self.open_dialog(DialogTask::Import)?;
                    }
                    #[cfg(target_os = "android")]
                    AppCommand::Export { job, json } => {
                        // No system picker on Android: the export lands in the
                        // app's external files dir, reachable over USB/adb and
                        // from desktop file managers while the device is
                        // connected. The timestamp keeps repeated exports.
                        let stamp = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_nanos())
                            .unwrap_or(0);
                        self.submit_storage(IoRequest::Export {
                            job,
                            path: android_export_path(&self.exports, stamp, job),
                            json,
                        })?;
                    }
                    #[cfg(target_os = "android")]
                    AppCommand::Import => {
                        engine_result(self.engine.event(AppEvent::LoadFailed(
                            "E_DIALOG: import is unavailable on this platform".into(),
                        )))?;
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
    fn submit_storage(&mut self, request: IoRequest) -> Result<()> {
        let failure = request.failure();
        if let Err(error) = self.io.submit(request) {
            let event = failure.event(error.to_string());
            if matches!(
                &event,
                AppEvent::SaveFailed { .. }
                    | AppEvent::PersistenceFailed { .. }
                    | AppEvent::ExportFailed { .. }
            ) {
                self.close.save_failed();
            }
            engine_result(self.engine.event(event))?;
        }
        Ok(())
    }
    fn storage_turn(&mut self) -> Result<()> {
        // One storage reply per turn, before new commands: the events land
        // in the engine the turn after the command that issued them.
        if let Some(reply) = self.io.drain() {
            let refresh_conflict = matches!(
                &reply,
                IoReply::Event(AppEvent::SaveFault { diagnostic, .. })
                    if diagnostic.code == "E_SAVE_CONFLICT"
            );
            if matches!(&reply, IoReply::Event(AppEvent::SavePending { .. })) {
                // Cancel a waiting close so the pending status is visible and
                // the player can keep reading while its slot stays busy.
                self.close.save_pending();
            }
            if matches!(
                &reply,
                IoReply::Event(
                    AppEvent::SaveFailed { .. }
                        | AppEvent::SaveFault { .. }
                        | AppEvent::PersistenceFailed { .. }
                        | AppEvent::ExportFailed { .. }
                )
            ) {
                self.close.save_failed();
            }
            match reply {
                IoReply::Event(event) => {
                    let profile_recovered = matches!(&event, AppEvent::ProfileRecovered(_));
                    engine_result(self.engine.event(event))?;
                    if profile_recovered {
                        // Admission/worker failures retain deltas until an
                        // explicit recovery. Re-read first, then merge those
                        // keys without replacing the disk's existing profile.
                        self.submit_storage(IoRequest::MergeProfile(BTreeSet::new()))?;
                    }
                }
                #[cfg(test)]
                IoReply::Done => {}
            }
            if refresh_conflict {
                // A read confirmed another record, not this write. Refresh
                // slot revisions so the next explicit save can use it; never
                // replay the uncertain write or clear the conflict warning.
                self.submit_storage(IoRequest::List)?;
            }
        }
        #[cfg(any(windows, target_os = "linux"))]
        if let Some(dialog) = &mut self.dialog {
            match dialog.replies.try_recv() {
                Ok(DialogOutcome::ImportCancelled) => {
                    self.dialog = None;
                }
                Ok(DialogOutcome::ExportSelected { job, path, json }) => {
                    self.dialog = None;
                    if let Some(path) = path {
                        self.submit_storage(IoRequest::Export { job, path, json })?;
                    } else {
                        engine_result(self.engine.event(AppEvent::ExportCancelled { job }))?;
                    }
                }
                Ok(DialogOutcome::Imported(Ok(envelope))) => {
                    self.dialog = None;
                    engine_result(self.engine.event(AppEvent::Loaded { envelope }))?;
                }
                Ok(DialogOutcome::Imported(Err(message))) => {
                    self.dialog = None;
                    self.close.save_failed();
                    engine_result(self.engine.event(AppEvent::LoadFailed(message)))?;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    let failure = dialog.failure;
                    self.dialog = None;
                    self.close.save_failed();
                    engine_result(self.engine.event(
                        failure.event("E_DIALOG_THREAD: dialog ended without a reply".into()),
                    ))?;
                }
            }
        }
        Ok(())
    }
    fn turn(&mut self) -> Result<()> {
        self.engine.begin_turn();
        if self.close.pending() {
            // A close drains durable I/O and dialog outcomes, not media or
            // renderer work. GPU recovery and queued uploads must not delay
            // an exit or fail it before a pending save can complete.
            self.storage_turn()?;
            engine_result(self.engine.continue_turn())?;
            return self.commands();
        }
        self.output_turn()?;
        if self.engine.device_lost() {
            engine_result(self.engine.begin_recovery())?;
            let window = self
                .window
                .clone()
                .context("E_WINDOW: no window to recover")?;
            let (renderer, instance) = create_renderer(window)?;
            engine_result(self.engine.replace_gpu(renderer))?;
            self.instance = instance;
        }
        self.storage_turn()?;
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
                        self.uploads
                            .push_back(PendingUpload::Bytes { request, id, bytes });
                    }
                    Ok(AssetData::Decoded {
                        width,
                        height,
                        pixels,
                    }) => {
                        self.uploads.push_back(PendingUpload::Decoded {
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
                        self.uploads
                            .push_back(PendingUpload::Bytes { request, id, bytes });
                    }
                    Err(message) => engine_result(self.engine.resource_failed(request, message))?,
                },
                _ => {}
            }
        }
        // One admission per turn. A partial image upload stays at the front
        // until its row budget completes; results drained meanwhile queue
        // behind it instead of replacing it.
        if let Some(pending) = self.uploads.pop_front() {
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
                Ok(false) => self.uploads.push_front(pending),
                Ok(true) => {}
                Err(error) => engine_result(self.engine.resource_failed(pending.request(), error))?,
            }
        }
        let ended: Vec<_> = self
            .voices
            .iter()
            .filter(|(_, v)| !self.output.blocked() && v.sink.empty())
            .map(|(id, _)| *id)
            .collect();
        for (domain, session, task) in ended {
            self.voices.remove(&(domain, session, task));
            engine_result(self.engine.audio_ended_in(domain, task, session))?;
        }
        let now = Instant::now();
        // A file dialog pauses the story clock for the same reason the
        // blocking dialog froze it: nothing behind the picker may advance.
        if let Some(elapsed) = self
            .lifecycle
            .elapsed(now, !self.dialog_open() && self.engine.needs_clock())
        {
            self.sample_audio_positions()?;
            engine_result(self.engine.tick(elapsed))?;
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
        if let Some(window) = &self.window {
            let size = window.inner_size();
            if !self.lifecycle.hidden() && size.width > 0 && size.height > 0 {
                let scale = window.scale_factor().clamp(1., 2.) as f32;
                engine_result(self.engine.draw(
                    size.width as f32 / scale,
                    size.height as f32 / scale,
                    scale,
                ))?;
            }
        }
        if let Some(error) = self.engine.gpu_error() {
            bail!("E_GPU: {error}");
        }
        Ok(())
    }
    fn request_close(&mut self, title: &str) -> Result<()> {
        if self.close.request() {
            // Reuse visibility handling to pause sinks immediately and clear
            // held/touch input. Closing is an independent pause owner, so a
            // focus gain cannot resume the story while storage is pending.
            self.visibility(Signal::Closing(true))?;
            if let Some(window) = &self.window {
                window.set_title(&format!(
                    "正在关闭…（Esc 取消） / Closing… (Esc to cancel) — {title}"
                ));
            }
        }
        Ok(())
    }
    fn cancel_close(&mut self, _title: &str) -> Result<()> {
        self.close.cancel();
        self.visibility(Signal::Closing(false))?;
        self.output_title();
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        Ok(())
    }
    fn close_action(&mut self) -> CloseAction {
        if self.io.delayed_save_pending() {
            // A later close request must not hide an already-issued notice.
            self.close.save_pending();
        }
        // Accepted owner input and an open dialog can still submit storage
        // work. Wait for their admission as well as physical confirmations.
        let owner_pending = self.dialog_open() || self.engine.pending_events() > 0;
        self.close.poll(self.io.outstanding(), owner_pending)
    }
    fn busy(&self) -> bool {
        self.close.pending()
            || self.dialog_open()
            || self.io.outstanding() > 0
            || self.loader.outstanding() > 0
            || !self.uploads.is_empty()
            || !self.jobs.is_empty()
            || self.engine.pending_events() > 0
            || self.output.pending()
            || (!self.lifecycle.hidden() && self.engine.needs_clock())
            || (!self.output.blocked() && !self.voices.is_empty())
    }
}
/// Vulkan on Android; the driver coverage matches the desktop native ports
/// (no GLES fallback backend exists yet).
#[cfg(target_os = "android")]
fn surface_backends() -> wgpu::Backends {
    wgpu::Backends::VULKAN
}
#[cfg(windows)]
fn surface_backends() -> wgpu::Backends {
    wgpu::Backends::DX12
}
#[cfg(target_os = "linux")]
fn surface_backends() -> wgpu::Backends {
    wgpu::Backends::VULKAN
}
/// Creates the presentation surface on `instance`; the instance outlives the
/// call so the surface and the device it is configured with always share one
/// wgpu instance.
fn create_surface_on(
    instance: &wgpu::Instance,
    window: Arc<Window>,
) -> Result<nir_render_wgpu::wgpu::Surface<'static>> {
    Ok(instance.create_surface(window)?)
}
fn create_renderer(window: Arc<Window>) -> Result<(Renderer, wgpu::Instance)> {
    let size = window.inner_size();
    #[cfg(windows)]
    let backend = RendererBackend::Dx12;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let backend = RendererBackend::Vulkan;
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: surface_backends(),
        ..Default::default()
    });
    let surface = create_surface_on(&instance, window)?;
    let renderer = pollster::block_on(Renderer::new(
        &instance,
        surface,
        size.width.max(1),
        size.height.max(1),
        backend,
    ))?;
    Ok((renderer, instance))
}
struct App {
    bundle: Arc<Bundle>,
    data: PathBuf,
    /// Android export target (the app's external files dir).
    #[cfg(target_os = "android")]
    exports: PathBuf,
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
/// Desktop sets a sensible logical size; Android ignores size/title and gets
/// the fullscreen system window either way.
fn window_attributes(title: &str, hidden: bool) -> winit::window::WindowAttributes {
    Window::default_attributes()
        .with_title(title)
        .with_inner_size(LogicalSize::new(1024., 768.))
        .with_visible(!hidden)
}
impl App {
    fn update(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let Some(runtime) = &mut self.runtime else {
            return Ok(());
        };
        if runtime.window.is_none() {
            // Android must not run owner admission against a destroyed window.
            // Workers finish independently; their replies wait for resume.
            event_loop.set_control_flow(ControlFlow::Wait);
            return Ok(());
        }
        runtime.turn()?;
        match runtime.close_action() {
            CloseAction::Exit => {
                event_loop.exit();
                return Ok(());
            }
            CloseAction::Cancel => {
                // The save failure or pending notice has reached Player.
                // Keep the window open so it can be read and acted on.
                runtime.cancel_close(&self.bundle.manifest.title)?;
            }
            CloseAction::Continue | CloseAction::Wait => {}
        }
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
        if let Some(runtime) = &mut self.runtime {
            // Android resume: the window was destroyed at suspend, so a fresh
            // one is created and its surface rebound onto the live device.
            let result =
                (|| -> Result<()> {
                    runtime.resume_window(Arc::new(event_loop.create_window(window_attributes(
                        &self.bundle.manifest.title,
                        self.hidden,
                    ))?))
                })();
            if let Err(error) = result {
                self.failed(event_loop, error);
            }
            return;
        }
        let result = (|| -> Result<Runtime> {
            let window = Arc::new(
                event_loop
                    .create_window(window_attributes(&self.bundle.manifest.title, self.hidden))?,
            );
            let data = self.data.clone();
            Runtime::new(
                window,
                self.bundle.clone(),
                data,
                self.exe_check.take(),
                #[cfg(target_os = "android")]
                self.exports.clone(),
            )
        })();
        match result {
            Ok(runtime) => self.runtime = Some(runtime),
            Err(error) => self.failed(event_loop, error),
        }
    }
    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        let Some(runtime) = &mut self.runtime else {
            return;
        };
        // Pause now and release the surface before returning to the OS.
        // Storage keeps running off-thread; acknowledge it on a resumed turn,
        // never block this callback or run a late load into a destroyed window.
        if let Err(error) = runtime.suspend_window() {
            self.failed(event_loop, error);
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(runtime) = &mut self.runtime else {
            return;
        };
        if runtime.window.is_none() {
            // Focus may change between TerminateWindow and InitWindow. Record
            // it even without a drawable window; the suspension owner still
            // prevents audio/clock release until resume has rebound the surface.
            match event {
                WindowEvent::Focused(focused) => runtime
                    .lifecycle
                    .signal(Signal::Focused(focused), Instant::now()),
                WindowEvent::Occluded(occluded) => runtime
                    .lifecycle
                    .signal(Signal::Occluded(occluded), Instant::now()),
                _ => {}
            }
            return;
        }
        let Some(window) = runtime.window.clone() else {
            return;
        };
        // While a file dialog is open the game behind it is inert: input is
        // gated so engine state cannot change behind the modal picker.
        let dialog_open = runtime.dialog_open() || runtime.close.pending();
        let result = (|| -> Result<()> {
            match event {
                WindowEvent::CloseRequested => {
                    runtime.request_close(&self.bundle.manifest.title)?;
                    event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now()));
                }
                WindowEvent::Resized(_)
                | WindowEvent::ScaleFactorChanged { .. }
                | WindowEvent::RedrawRequested => {
                    runtime.turn()?;
                }
                WindowEvent::Occluded(occluded) => {
                    runtime.visibility(Signal::Occluded(occluded))?
                }
                WindowEvent::Focused(focused) => runtime.visibility(Signal::Focused(focused))?,
                WindowEvent::CursorMoved { position, .. } if !dialog_open => {
                    let scale = window.scale_factor().clamp(1., 2.) as f32;
                    runtime.cursor = (position.x as f32 / scale, position.y as f32 / scale);
                    if runtime.recovery_pointer(RecoveryPointer::Mouse(0), RecoveryPhase::Move)? {
                        return Ok(());
                    }
                    if runtime.metadata_pointer(RecoveryPointer::Mouse(0), RecoveryPhase::Move)? {
                        return Ok(());
                    }
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
                WindowEvent::ModifiersChanged(modifiers) if !dialog_open => {
                    runtime.modifiers = modifiers.state()
                }
                WindowEvent::CursorLeft { .. } => {
                    runtime.recovery_gesture.clear();
                    runtime.metadata_gesture.clear();
                    runtime.pointer_down = None;
                    runtime.bar_pointer = false;
                    engine_result(runtime.engine.pointer_gesture(3, 0., 0., 0))?;
                    engine_result(runtime.engine.hover(-1., -1.))?;
                }
                WindowEvent::MouseInput { state, button, .. }
                    if !dialog_open && matches!(button, MouseButton::Left | MouseButton::Right) =>
                {
                    let code = if button == MouseButton::Left { 0 } else { 2 };
                    if runtime.recovery_pointer(
                        RecoveryPointer::Mouse(code),
                        if state == ElementState::Pressed {
                            RecoveryPhase::Down
                        } else {
                            RecoveryPhase::Up
                        },
                    )? {
                        return Ok(());
                    }
                    if runtime.metadata_pointer(
                        RecoveryPointer::Mouse(code),
                        if state == ElementState::Pressed {
                            RecoveryPhase::Down
                        } else {
                            RecoveryPhase::Up
                        },
                    )? {
                        return Ok(());
                    }
                    if button == MouseButton::Left {
                        if state == ElementState::Pressed {
                            engine_result(runtime.engine.focus_control(None))?;
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
                        engine_result(runtime.engine.focus_control(None))?;
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
                    if runtime.engine.native_audio_recovery_layout().is_some() {
                        return Ok(());
                    }
                    if runtime
                        .engine
                        .native_storage_recovery_layout()
                        .is_some_and(|l| l.contains(runtime.cursor.0, runtime.cursor.1))
                    {
                        return Ok(());
                    }
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
                // Touch drives the same pointer semantics as the mouse arms:
                // Started/Ended replay the press/release pairing (including
                // the movement slop and input-identity checks), Moved feeds
                // the scroll-bar drag or hover, Cancelled aborts. Android
                // delivers only Touch events (physical pixels); desktop
                // touchscreens route here as well.
                WindowEvent::Touch(touch) if !dialog_open => {
                    let scale = window.scale_factor().clamp(1., 2.) as f32;
                    runtime.cursor = (
                        touch.location.x as f32 / scale,
                        touch.location.y as f32 / scale,
                    );
                    if runtime.recovery_pointer(
                        RecoveryPointer::Touch(touch.id),
                        match touch.phase {
                            TouchPhase::Started => RecoveryPhase::Down,
                            TouchPhase::Moved => RecoveryPhase::Move,
                            TouchPhase::Ended => RecoveryPhase::Up,
                            TouchPhase::Cancelled => RecoveryPhase::Cancel,
                        },
                    )? {
                        return Ok(());
                    }
                    if runtime.metadata_pointer(
                        RecoveryPointer::Touch(touch.id),
                        match touch.phase {
                            TouchPhase::Started => RecoveryPhase::Down,
                            TouchPhase::Moved => RecoveryPhase::Move,
                            TouchPhase::Ended => RecoveryPhase::Up,
                            TouchPhase::Cancelled => RecoveryPhase::Cancel,
                        },
                    )? {
                        return Ok(());
                    }
                    // Single-pointer engine: the first finger owns the
                    // gesture until it ends; later contacts are ignored.
                    match touch.phase {
                        TouchPhase::Started => {
                            if runtime.touch_id.is_some() {
                                return Ok(());
                            }
                            runtime.touch_id = Some(touch.id);
                            engine_result(runtime.engine.focus_control(None))?;
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
                            let hit = runtime.engine.pointer_action(
                                runtime.cursor.0,
                                runtime.cursor.1,
                                0,
                            );
                            runtime.pointer_down = hit.map(|action| {
                                (
                                    MouseButton::Left,
                                    action,
                                    runtime.cursor,
                                    runtime.engine.input_identity(),
                                )
                            });
                        }
                        TouchPhase::Moved => {
                            if runtime.touch_id != Some(touch.id) {
                                return Ok(());
                            }
                            if runtime.bar_pointer {
                                engine_result(runtime.engine.pointer_gesture(
                                    1,
                                    runtime.cursor.0,
                                    runtime.cursor.1,
                                    0,
                                ))?;
                            } else {
                                engine_result(
                                    runtime.engine.hover(runtime.cursor.0, runtime.cursor.1),
                                )?;
                            }
                        }
                        TouchPhase::Ended => {
                            if runtime.touch_id != Some(touch.id) {
                                return Ok(());
                            }
                            runtime.touch_id = None;
                            if runtime.bar_pointer {
                                runtime.bar_pointer = false;
                                engine_result(runtime.engine.pointer_gesture(
                                    2,
                                    runtime.cursor.0,
                                    runtime.cursor.1,
                                    0,
                                ))?;
                                return Ok(());
                            }
                            let hit = runtime.engine.pointer_action(
                                runtime.cursor.0,
                                runtime.cursor.1,
                                0,
                            );
                            if let Some((pressed, action, origin, identity)) =
                                runtime.pointer_down.take()
                            {
                                if pressed == MouseButton::Left
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
                        TouchPhase::Cancelled => {
                            if runtime.touch_id != Some(touch.id) {
                                return Ok(());
                            }
                            runtime.touch_id = None;
                            runtime.pointer_down = None;
                            runtime.bar_pointer = false;
                            engine_result(runtime.engine.pointer_gesture(3, 0., 0., 0))?;
                            engine_result(runtime.engine.hover(-1., -1.))?;
                        }
                    }
                }
                WindowEvent::KeyboardInput { event, .. }
                    if runtime.close.pending()
                        && event.state == ElementState::Pressed
                        && event.logical_key == Key::Named(NamedKey::Escape) =>
                {
                    runtime.cancel_close(&self.bundle.manifest.title)?;
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
                    if runtime.engine.native_audio_recovery_layout().is_some() {
                        runtime.held_controls = [false; 2];
                        return Ok(());
                    }
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
                    if runtime.recovery_key(&event.logical_key)? {
                        window.request_redraw();
                        return Ok(());
                    }
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
                        Key::Named(NamedKey::F8) => {
                            runtime.engine.begin_turn();
                            runtime.retry_output(true)?;
                        }
                        Key::Named(NamedKey::F9) => {
                            if runtime.engine.native_storage_recovery_layout().is_some() {
                                runtime.retry_metadata()?;
                            }
                        }
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
                            engine_result(runtime.engine.navigate_focus(direction))?;
                            if let Some((x, y)) = runtime.engine.focused_center() {
                                engine_result(runtime.engine.hover(x, y))?;
                                runtime.commands()?;
                            }
                            window.request_redraw();
                        }
                        Key::Named(NamedKey::Space) | Key::Named(NamedKey::Enter) => {
                            if let Some(action) = runtime.engine.primary_action() {
                                runtime.input(action)?;
                            }
                        }
                        // Android's back button surfaces as BrowserBack; it
                        // shares Escape's menu/close depth logic.
                        Key::Named(NamedKey::Escape | NamedKey::BrowserBack) => {
                            // Only this arm needs the state report; parse
                            // it here instead of on every qualifying keypress.
                            let state: serde_json::Value =
                                serde_json::from_str(&runtime.engine.state())?;
                            runtime.input(if state["choice"]["on_cancel"].is_string() {
                                // A cancellable interaction owns Escape.
                                UiAction::CancelChoice
                            } else if state["menu_depth"].as_u64().is_some_and(|depth| depth > 0) {
                                UiAction::Close
                            } else if state["screen"] == "Story" || state["screen"] == "Title" {
                                UiAction::Menu
                            } else {
                                UiAction::Close
                            })?
                        }
                        Key::Character(ref c) if c.eq_ignore_ascii_case("h") => {
                            runtime.input(UiAction::ToggleInterface)?
                        }
                        Key::Named(NamedKey::F11) => {
                            window.set_fullscreen(if window.fullscreen().is_some() {
                                None
                            } else {
                                Some(Fullscreen::Borderless(None))
                            })
                        }
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

#[cfg(any(windows, target_os = "linux"))]
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

#[cfg(all(test, target_os = "linux"))]
mod recovery_ui_probe {
    use super::*;
    use winit::platform::x11::EventLoopBuilderExtX11;

    fn state(runtime: &Runtime) -> serde_json::Value {
        serde_json::from_str(&runtime.engine.state()).unwrap()
    }
    fn export_request(runtime: &mut Runtime) -> (u32, String) {
        runtime.engine.begin_turn();
        runtime.sequence += 1;
        runtime
            .engine
            .input(UiAction::Export, runtime.sequence)
            .unwrap();
        runtime
            .engine
            .take_commands()
            .into_iter()
            .find_map(|command| {
                if let AppCommand::Export { job, json } = command {
                    Some((job, json))
                } else {
                    None
                }
            })
            .expect("export snapshot command")
    }
    fn until(runtime: &mut Runtime, condition: impl Fn(&Runtime) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !condition(runtime) {
            runtime.turn().unwrap();
            assert!(
                Instant::now() < deadline,
                "native recovery probe: {}",
                runtime.engine.state()
            );
            std::thread::yield_now();
        }
    }
    fn title_pending_metadata_keyboard(
        runtime: &mut Runtime,
        activation: &str,
    ) -> serde_json::Value {
        until(runtime, |r| {
            r.engine.focus_actions().contains(&UiAction::Retry)
        });
        let focus =
            (activation == "keyboard").then_some(nir_presentation::storage_recovery::CONTROL_ID);
        engine_result(runtime.engine.focus_control(focus)).unwrap();
        assert_eq!(
            runtime.engine.native_storage_recovery_focused(),
            activation == "keyboard"
        );
        let before = state(runtime);
        let backend = runtime.storage.clone();
        let (started, ready) = std::sync::mpsc::channel();
        let (release, gate) = std::sync::mpsc::channel();
        let (driver, worker) = IoWorker::with_driver(runtime.storage.clone(), move |request| {
            assert!(matches!(
                request,
                IoRequest::ReadMetadata(nir_player::PersistenceKind::Preferences)
            ));
            started.send(()).unwrap();
            gate.recv().unwrap();
            IoReply::Event(match backend.preferences() {
                Ok(value) => AppEvent::PreferencesRecovered(value.unwrap_or_default()),
                Err(error) => AppEvent::PersistenceReadFailed {
                    kind: nir_player::PersistenceKind::Preferences,
                    message: error.to_string(),
                },
            })
        });
        runtime.io = driver;
        match activation {
            "keyboard" => runtime.input(UiAction::Retry).unwrap(),
            "touch" => {
                let layout = runtime.engine.native_storage_recovery_layout().unwrap();
                runtime.cursor = (layout.retry[0] + 20., layout.retry[1] + 20.);
                for phase in [RecoveryPhase::Down, RecoveryPhase::Up] {
                    assert!(runtime
                        .metadata_pointer(RecoveryPointer::Touch(63), phase)
                        .unwrap());
                }
            }
            "shortcut" => runtime.retry_metadata().unwrap(),
            _ => panic!("unknown metadata activation"),
        }
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        runtime.turn().unwrap(); // Observe the actual disabled-control projection.
        let pending = state(runtime);
        let next = runtime.engine.primary_action();
        runtime.retry_metadata().unwrap();
        assert_eq!(
            runtime.io.outstanding(),
            1,
            "a repeated retry must not queue another read"
        );
        release.send(()).unwrap();
        until(runtime, |r| r.io.outstanding() == 0);
        runtime.io = IoWorker::new(runtime.storage.clone());
        worker.join().unwrap();
        let report = serde_json::json!({"activation":activation,"before":before,"pending":pending,"primaryWhileBusy":next,"after":state(runtime)});
        eprintln!("NIR_METADATA_TITLE_KEYBOARD {}", report);
        assert!(
            next.is_none() || next == Some(UiAction::Retry),
            "a disabled metadata retry must not fall through to Begin reading"
        );
        assert_eq!(pending["screen"], "Title");
        assert_eq!(pending["sequence"], before["sequence"]);
        report
    }
    fn pending_saves(
        runtime: &mut Runtime,
        source: &mut rodio::mixer::MixerSource,
    ) -> Vec<serde_json::Value> {
        let mut pending_save_checks = Vec::new();
        for outcome in ["success", "failure"] {
            until(runtime, |r| r.io.outstanding() == 0);
            let slot_file = runtime.storage.slot(0).unwrap();
            let old_record = fs::read(&slot_file).unwrap();
            let old_revision = runtime.storage.load(0).unwrap().unwrap().revision;
            let blocked = slot_file.with_extension(format!("{}.tmp", std::process::id()));
            if outcome == "failure" {
                fs::create_dir(&blocked).unwrap();
            }
            let backend = runtime.storage.clone();
            let (started, ready) = std::sync::mpsc::channel();
            let (release, gate) = std::sync::mpsc::channel();
            let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let observed_calls = calls.clone();
            let (driver, worker) = IoWorker::with_driver(runtime.storage.clone(), move |request| {
                match request {
                    IoRequest::Save {
                        slot,
                        expected_revision,
                        job,
                        envelope,
                    } => {
                        observed_calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        // The success case has already committed on disk;
                        // the failure case has not attempted its blocked write.
                        // Neither case publishes a terminal result before release.
                        let committed = if outcome == "success" {
                            Some(backend.save(slot, expected_revision, &envelope))
                        } else {
                            None
                        };
                        started.send(()).unwrap();
                        gate.recv().unwrap();
                        IoReply::Event(
                            match committed
                                .unwrap_or_else(|| backend.save(slot, expected_revision, &envelope))
                            {
                                Ok(()) => AppEvent::Saved {
                                    job,
                                    slot,
                                    revision: envelope.revision,
                                },
                                Err(error) => AppEvent::SaveFailed {
                                    job,
                                    message: error.to_string(),
                                },
                            },
                        )
                    }
                    IoRequest::List => {
                        let mut rows = Vec::new();
                        let mut revisions = BTreeMap::new();
                        for slot in 0..3 {
                            let record = backend.load(slot).unwrap();
                            if let Some(record) = &record {
                                revisions.insert(slot, record.revision);
                            }
                            rows.push(nir_presentation::SlotView {
                                slot,
                                exists: record.is_some(),
                                label: record
                                    .map(|r| format!("#{}", r.revision))
                                    .unwrap_or_default(),
                                ..Default::default()
                            });
                        }
                        IoReply::Event(AppEvent::Slots(rows, revisions))
                    }
                    IoRequest::MergeProfile(keys) => {
                        IoReply::Event(match backend.merge_profile(keys) {
                            Ok(()) => {
                                AppEvent::PersistenceStored(nir_player::PersistenceKind::Profile)
                            }
                            Err(error) => AppEvent::PersistenceFailed {
                                kind: nir_player::PersistenceKind::Profile,
                                message: error.to_string(),
                            },
                        })
                    }
                    IoRequest::WritePreferences(p) => {
                        IoReply::Event(match backend.write_preferences(&p) {
                            Ok(()) => AppEvent::PersistenceStored(
                                nir_player::PersistenceKind::Preferences,
                            ),
                            Err(error) => AppEvent::PersistenceFailed {
                                kind: nir_player::PersistenceKind::Preferences,
                                message: error.to_string(),
                            },
                        })
                    }
                    _ => panic!("unexpected pending-save probe request"),
                }
            });
            runtime.io = driver;
            let before = state(runtime);
            let bgm_keys = runtime
                .voices
                .iter()
                .filter(|(_, voice)| voice.bus == AudioBus::Bgm)
                .map(|(key, _)| *key)
                .collect::<Vec<_>>();
            assert!(!bgm_keys.is_empty());
            runtime.input(UiAction::Save { slot: 0 }).unwrap();
            ready.recv_timeout(Duration::from_secs(5)).unwrap();
            runtime.request_close("NIR pending save probe").unwrap();
            until(runtime, |r| r.io.delayed_save_pending());
            let pending = state(runtime);
            assert_eq!(runtime.io.outstanding(), 1);
            assert_eq!(pending["diagnostic"]["code"], "E_STORAGE_UNCERTAIN");
            assert_eq!(pending["error"], serde_json::Value::Null);
            assert!(pending["status"].as_str().unwrap().contains("确认"));
            assert_eq!(runtime.close_action(), CloseAction::Cancel);
            runtime.cancel_close("NIR pending save probe").unwrap();
            // A later close after the once-only notice also stays open.
            runtime.request_close("NIR pending save probe").unwrap();
            assert_eq!(runtime.close_action(), CloseAction::Cancel);
            runtime.cancel_close("NIR pending save probe").unwrap();
            // Duplicate save input is handled by the actual Player busy-slot
            // policy, rather than a stub refusing a second filesystem write.
            for _ in 0..8 {
                runtime.input(UiAction::Save { slot: 0 }).unwrap();
                runtime.turn().unwrap();
                assert_eq!(runtime.io.outstanding(), 1);
                assert_eq!(state(runtime)["diagnostic"], pending["diagnostic"]);
            }
            assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
            let visible = state(runtime);
            for key in [
                "session",
                "interaction",
                "position",
                "tick_us",
                "history_count",
                "sequence",
            ] {
                assert_eq!(visible[key], before[key]);
            }
            let pending_samples = source.by_ref().take(9600).collect::<Vec<_>>();
            assert!(pending_samples.iter().any(|s| s.abs() > 0.00001));
            assert!(runtime
                .voices
                .values()
                .filter(|v| v.bus == AudioBus::Bgm)
                .all(|v| !v.sink.is_paused()));
            assert_eq!(
                runtime
                    .voices
                    .iter()
                    .filter(|(_, voice)| voice.bus == AudioBus::Bgm)
                    .map(|(key, _)| *key)
                    .collect::<Vec<_>>(),
                bgm_keys
            );
            runtime.input(UiAction::Close).unwrap();
            until(runtime, |r| {
                let s = state(r);
                s["screen"] == "Story"
                    && s["paused"] == false
                    && s["loading"] == false
                    && !s["dialogue"].is_null()
            });
            let reading_before = state(runtime);
            runtime.input(UiAction::Advance).unwrap();
            until(runtime, |r| {
                let s = state(r);
                if s["screen"] != "Story" || s["paused"] != false || s["loading"] != false {
                    return false;
                }
                if reading_before["dialogue"]["ready"] == false {
                    s["dialogue"]["ready"] == true
                        && s["dialogue"]["visible"] != reading_before["dialogue"]["visible"]
                } else {
                    s["interaction"] != reading_before["interaction"]
                        || s["position"] != reading_before["position"]
                }
            });
            let reading_after = state(runtime);
            assert!(
                runtime.io.delayed_save_pending(),
                "reading did not settle or replace the original save"
            );
            assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
            runtime.input(UiAction::Saves).unwrap();
            until(runtime, |r| state(r)["screen"] == "Saves");
            release.send(()).unwrap();
            until(runtime, |r| r.io.outstanding() == 0);
            let settled = state(runtime);
            for key in [
                "session",
                "interaction",
                "position",
                "tick_us",
                "history_count",
            ] {
                assert_eq!(
                    settled[key], reading_after[key],
                    "confirmation must not restore the earlier save point"
                );
            }
            assert!(!runtime.io.delayed_save_pending());
            assert_eq!(settled["error"], serde_json::Value::Null);
            if outcome == "success" {
                assert_eq!(settled["diagnostic"], serde_json::Value::Null);
                assert_eq!(
                    runtime.storage.load(0).unwrap().unwrap().revision,
                    old_revision + 1
                );
            } else {
                assert_eq!(settled["diagnostic"]["code"], "E_STORAGE");
                assert_eq!(fs::read(&slot_file).unwrap(), old_record);
                fs::remove_dir(blocked).unwrap();
            }
            assert_eq!(runtime.close_action(), CloseAction::Continue);
            runtime.request_close("NIR pending save probe").unwrap();
            until(runtime, |r| r.engine.pending_events() == 0);
            assert_eq!(runtime.close_action(), CloseAction::Exit);
            runtime.cancel_close("NIR pending save probe").unwrap();
            pending_save_checks.push(serde_json::json!({
                "outcome":outcome,"before":before,"pending":pending,"visibleAfterCancel":visible,"settled":settled,
                "saveCalls":calls.load(std::sync::atomic::Ordering::Relaxed),"outstandingDuringNotice":1,
                "pendingCloseCancelled":true,"laterCloseCancelled":true,"freshCloseAfterSettlement":true,
                "readingBefore":reading_before,"readingAfter":reading_after,"bgmKeysDuringMenu":bgm_keys,
                "softwareNonzeroSamples":pending_samples.iter().filter(|s| s.abs()>0.00001).count()
            }));
            // All physical requests and owner replies have settled before
            // replacing this controlled worker with the ordinary backend.
            runtime.io = IoWorker::new(runtime.storage.clone());
            worker.join().unwrap();
        }
        pending_save_checks
    }

    #[test]
    #[ignore = "requires NIR_RECOVERY_UI_PACKAGE, local X11/Vulkan and isolated ALSA config"]
    #[allow(deprecated)]
    fn lost_native_save_confirmation_recovers_without_replay_or_rewinding_reading() {
        let package = PathBuf::from(std::env::var_os("NIR_RECOVERY_UI_PACKAGE").unwrap());
        let bundle = Arc::new(Bundle::open(&package.join("data")).unwrap());
        let mut builder = EventLoop::builder();
        builder.with_x11().with_any_thread(true);
        let event_loop = builder.build().unwrap();
        let window = Arc::new(
            event_loop
                .create_window(window_attributes("NIR lost save probe", true))
                .unwrap(),
        );
        let data = tempfile::tempdir().unwrap();
        let mut runtime = Runtime::new(window, bundle, data.path().to_owned(), None).unwrap();
        let (source_tx, sources) = std::sync::mpsc::channel();
        runtime.output_worker = OutputWorker::with_opener(move |_, _| {
            let (mixer, source) = rodio::mixer::mixer(2, 48_000);
            source_tx.send(source).unwrap();
            Ok(OutputHandle {
                mixer,
                rate: 48_000,
            })
        });
        runtime.output = OutputState::default();
        runtime.audio = None;
        runtime.engine.begin_turn();
        runtime.retry_output(true).unwrap();
        until(&mut runtime, |r| r.engine.is_ready() && !r.output.blocked());
        runtime.input(UiAction::NewGame).unwrap();
        until(&mut runtime, |r| {
            state(r)["screen"] == "Story"
                && state(r)["loading"] == false
                && r.voices.values().any(|v| v.bus == AudioBus::Bgm)
        });
        let mut source = sources.recv_timeout(Duration::from_secs(5)).unwrap();
        runtime.input(UiAction::Saves).unwrap();
        until(&mut runtime, |r| {
            r.io.outstanding() == 0 && state(r)["screen"] == "Saves"
        });
        runtime.input(UiAction::Save { slot: 0 }).unwrap();
        until(&mut runtime, |r| {
            r.io.outstanding() == 0 && state(r)["status"] == "已保存"
        });
        let mut checks = Vec::new();
        for outcome in ["matching", "not-written", "conflict", "unreadable"] {
            until(&mut runtime, |r| r.io.outstanding() == 0);
            let before = state(&runtime);
            let path = runtime.storage.slot(0).unwrap();
            let old_record = fs::read(&path).unwrap();
            let old_revision = runtime.storage.load(0).unwrap().unwrap().revision;
            let bgm_keys = runtime
                .voices
                .iter()
                .filter(|(_, v)| v.bus == AudioBus::Bgm)
                .map(|(key, _)| *key)
                .collect::<Vec<_>>();
            assert!(!bgm_keys.is_empty());
            let backend = runtime.storage.clone();
            let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let count = calls.clone();
            let (sent, records) = std::sync::mpsc::channel();
            let (driver, worker) = IoWorker::with_driver(runtime.storage.clone(), move |request| {
                let IoRequest::Save {
                    slot,
                    expected_revision,
                    job,
                    mut envelope,
                } = request
                else {
                    panic!("only the original save may execute on the dying worker")
                };
                count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if outcome != "not-written" {
                    backend.save(slot, expected_revision, &envelope).unwrap();
                }
                let written = fs::read(backend.slot(slot).unwrap()).unwrap();
                if outcome == "unreadable" {
                    fs::write(backend.slot(slot).unwrap(), b"{unreadable save").unwrap();
                } else if outcome == "conflict" {
                    envelope.snapshot.tick_us.0 += 1;
                    envelope.digest =
                        nir_content::digest(&serde_json::to_vec(&envelope.snapshot).unwrap());
                    crate::atomic_write(
                        &backend.slot(slot).unwrap(),
                        &serde_json::to_vec(&envelope).unwrap(),
                    )
                    .unwrap();
                }
                sent.send((job, written)).unwrap();
                panic!("controlled native save thread death before owner confirmation");
            });
            runtime.io = driver;
            runtime.input(UiAction::Save { slot: 0 }).unwrap();
            assert!(worker.join().is_err());
            let (job, committed) = records.recv().unwrap();
            let record_after_loss = fs::read(&path).unwrap();
            let modified = fs::metadata(&path).unwrap().modified().unwrap();
            runtime.turn().unwrap(); // Admit the original uncertain event, not the subsequent read.
            let pending = state(&runtime);
            assert!(runtime.io.has_unconfirmed_saves());
            assert!(
                runtime
                    .engine
                    .native_storage_recovery_layout()
                    .unwrap()
                    .status
                    .save_pending
            );
            assert_eq!(pending["diagnostic"]["code"], "E_STORAGE_UNCERTAIN");
            assert_eq!(pending["error"], serde_json::Value::Null);
            for _ in 0..8 {
                runtime.input(UiAction::Save { slot: 0 }).unwrap();
            }
            assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
            runtime.request_close("NIR lost save probe").unwrap();
            assert_eq!(runtime.close_action(), CloseAction::Cancel);
            runtime.cancel_close("NIR lost save probe").unwrap();
            let mut continued_reading = None;
            let mut touch_release = None;
            if outcome == "unreadable" {
                until(&mut runtime, |r| r.io.outstanding() == 0);
                let layout = runtime.engine.native_storage_recovery_layout().unwrap();
                assert!(layout.status.retry_enabled && !layout.status.pending);
                for _ in 0..100 {
                    runtime.turn().unwrap();
                    assert_eq!(runtime.io.outstanding(), 0);
                }
                assert_eq!(fs::read(&path).unwrap(), record_after_loss);
                // Reading remains usable even while a stopped worker's result
                // cannot be determined. A later confirmation cannot restore it.
                runtime.input(UiAction::Close).unwrap();
                until(&mut runtime, |r| {
                    state(r)["screen"] == "Story"
                        && state(r)["loading"] == false
                        && state(r)["paused"] == false
                });
                let reading_before = state(&runtime);
                runtime.input(UiAction::Advance).unwrap();
                until(&mut runtime, |r| {
                    let s = state(r);
                    s["screen"] == "Story"
                        && s["loading"] == false
                        && s["paused"] == false
                        && if reading_before["dialogue"]["ready"] == false {
                            s["dialogue"]["ready"] == true
                        } else {
                            s["interaction"] != reading_before["interaction"]
                                || s["position"] != reading_before["position"]
                        }
                });
                continued_reading = Some(state(&runtime));
                assert!(runtime.io.has_unconfirmed_saves());
                runtime.input(UiAction::Saves).unwrap();
                until(&mut runtime, |r| {
                    r.io.outstanding() == 0 && state(r)["screen"] == "Saves"
                });
                let layout = runtime.engine.native_storage_recovery_layout().unwrap();
                runtime.cursor = (layout.retry[0] + 20., layout.retry[1] + 20.);
                for phase in [RecoveryPhase::Down, RecoveryPhase::Up] {
                    assert!(runtime
                        .metadata_pointer(RecoveryPointer::Touch(81), phase)
                        .unwrap());
                }
                assert_eq!(runtime.io.outstanding(), 1);
                runtime.retry_metadata().unwrap();
                assert_eq!(
                    runtime.io.outstanding(),
                    1,
                    "duplicate retries must not replay a write or queue reads"
                );
                until(&mut runtime, |r| r.io.outstanding() == 0);
                assert!(runtime.io.has_unconfirmed_saves());
                assert_eq!(fs::read(&path).unwrap(), record_after_loss);
                // External repair belongs only to this fixture.
                crate::atomic_write(&path, &committed).unwrap();
                let repaired_modified = fs::metadata(&path).unwrap().modified().unwrap();
                let before_retry = state(&runtime);
                for phase in [RecoveryPhase::Down, RecoveryPhase::Up] {
                    assert!(runtime
                        .metadata_pointer(RecoveryPointer::Touch(82), phase)
                        .unwrap());
                }
                assert!(runtime
                    .metadata_pointer(RecoveryPointer::Touch(83), RecoveryPhase::Down)
                    .unwrap());
                until(&mut runtime, |r| {
                    r.io.outstanding() == 0 && !r.io.has_unconfirmed_saves()
                });
                assert!(runtime.engine.native_storage_recovery_layout().is_none());
                let settled = state(&runtime);
                assert!(runtime
                    .metadata_pointer(RecoveryPointer::Touch(83), RecoveryPhase::Up)
                    .unwrap());
                let released = state(&runtime);
                for key in [
                    "session",
                    "interaction",
                    "position",
                    "tick_us",
                    "history_count",
                    "variables",
                ] {
                    assert_eq!(
                        settled[key], before_retry[key],
                        "late read confirmation must not rewind reading"
                    );
                    assert_eq!(
                        released[key], settled[key],
                        "touch release must not hit the newly uncovered menu"
                    );
                }
                assert_eq!(fs::read(&path).unwrap(), committed);
                assert_eq!(
                    fs::metadata(&path).unwrap().modified().unwrap(),
                    repaired_modified
                );
                touch_release = Some(
                    serde_json::json!({"before":before_retry,"settled":settled,"released":released}),
                );
            } else {
                until(&mut runtime, |r| {
                    r.io.outstanding() == 0 && !r.io.has_unconfirmed_saves()
                });
                let after_read = state(&runtime);
                for key in [
                    "session",
                    "interaction",
                    "position",
                    "tick_us",
                    "history_count",
                    "variables",
                ] {
                    assert_eq!(
                        after_read[key], before[key],
                        "read-only confirmation must preserve the current session"
                    );
                }
                assert_eq!(fs::read(&path).unwrap(), record_after_loss);
                assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
            }
            let settled = state(&runtime);
            assert_eq!(settled["error"], serde_json::Value::Null);
            match outcome {
                "matching" | "unreadable" => {
                    assert_eq!(settled["diagnostic"], serde_json::Value::Null);
                    assert_eq!(
                        runtime.storage.load(0).unwrap().unwrap().revision,
                        old_revision + 1
                    );
                }
                "not-written" => {
                    assert_eq!(settled["diagnostic"]["code"], "E_STORAGE");
                    assert_eq!(fs::read(&path).unwrap(), old_record);
                }
                "conflict" => assert_eq!(settled["diagnostic"]["code"], "E_SAVE_CONFLICT"),
                _ => unreachable!(),
            }
            assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
            assert_eq!(
                runtime
                    .voices
                    .iter()
                    .filter(|(_, v)| v.bus == AudioBus::Bgm)
                    .map(|(key, _)| *key)
                    .collect::<Vec<_>>(),
                bgm_keys
            );
            let samples = source.by_ref().take(9600).collect::<Vec<_>>();
            assert!(samples.iter().any(|v| v.abs() > 0.00001));
            checks.push(serde_json::json!({"outcome":outcome,"originalJob":job,"saveCalls":1,"before":before,"pending":pending,"settled":settled,"continuedReading":continued_reading,"touchRelease":touch_release,"bgmKeys":bgm_keys,"softwareNonzeroSamples":samples.iter().filter(|v| v.abs()>0.00001).count(),"oldRevision":old_revision,"recordAfterLossSha256":nir_content::digest(&record_after_loss),"originalWrittenRecordSha256":nir_content::digest(&committed)}));
            eprintln!("NIR_LOST_SAVE_CASE {}", checks.last().unwrap());
            // Only a fresh explicit user save is allowed after settlement.
            runtime.input(UiAction::Save { slot: 0 }).unwrap();
            until(&mut runtime, |r| {
                r.io.outstanding() == 0 && state(r)["status"] == "已保存"
            });
        }
        if let Some(path) = std::env::var_os("NIR_RECOVERY_UI_REPORT") {
            fs::write(path, serde_json::to_vec_pretty(&serde_json::json!({"lostSaveChecks":checks,"after":state(&runtime),"limits":["Controlled thread death with actual durable Storage writes, not physical power loss","Hidden X11/Vulkan window and native handlers, not Android touch events","Actual rodio software mixer, not device or acoustic output"]})).unwrap()).unwrap();
        }
    }

    #[test]
    #[ignore = "requires NIR_RECOVERY_UI_PACKAGE, local X11/Vulkan and isolated ALSA config"]
    #[allow(deprecated)]
    fn pending_native_save_keeps_reading_and_busy_slot_until_original_reply() {
        let package = PathBuf::from(std::env::var_os("NIR_RECOVERY_UI_PACKAGE").unwrap());
        let bundle = Arc::new(Bundle::open(&package.join("data")).unwrap());
        let mut builder = EventLoop::builder();
        builder.with_x11().with_any_thread(true);
        let event_loop = builder.build().unwrap();
        let window = Arc::new(
            event_loop
                .create_window(window_attributes("NIR pending save probe", true))
                .unwrap(),
        );
        let data = tempfile::tempdir().unwrap();
        let mut runtime = Runtime::new(window, bundle, data.path().to_owned(), None).unwrap();
        let (source_tx, sources) = std::sync::mpsc::channel();
        runtime.output_worker = OutputWorker::with_opener(move |_, _| {
            let (mixer, source) = rodio::mixer::mixer(2, 48_000);
            source_tx.send(source).unwrap();
            Ok(OutputHandle {
                mixer,
                rate: 48_000,
            })
        });
        runtime.output = OutputState::default();
        runtime.audio = None;
        runtime.engine.begin_turn();
        runtime.retry_output(true).unwrap();
        until(&mut runtime, |r| r.engine.is_ready() && !r.output.blocked());
        runtime.input(UiAction::NewGame).unwrap();
        until(&mut runtime, |r| {
            state(r)["screen"] == "Story"
                && state(r)["loading"] == false
                && r.voices.values().any(|v| v.bus == AudioBus::Bgm)
        });
        let mut source = sources.recv_timeout(Duration::from_secs(5)).unwrap();
        runtime.input(UiAction::Saves).unwrap();
        until(&mut runtime, |r| {
            r.io.outstanding() == 0 && state(r)["screen"] == "Saves"
        });
        runtime.input(UiAction::Save { slot: 0 }).unwrap();
        until(&mut runtime, |r| {
            r.io.outstanding() == 0 && state(r)["status"] == "已保存"
        });
        let checks = pending_saves(&mut runtime, &mut source);
        if let Some(path) = std::env::var_os("NIR_RECOVERY_UI_REPORT") {
            fs::write(path, serde_json::to_vec_pretty(&serde_json::json!({
                "pendingSaveChecks":checks,"after":state(&runtime),
                "limits":["Hidden actual X11/Vulkan window and renderer submissions; no independent pixel comparison",
                    "Actual rodio software mixer and MP3 queues; no CPAL/device/acoustic listening",
                    "Native handler inputs, not physical Android touch/lifecycle events"]
            })).unwrap()).unwrap();
        }
    }

    #[test]
    #[ignore = "requires NIR_RECOVERY_UI_PACKAGE, local X11/Vulkan and isolated ALSA config"]
    #[allow(deprecated)]
    fn window_menu_save_load_and_touch_retry_keep_restore_pause_without_click_through() {
        use nir_presentation::audio_recovery::Status;
        let package = PathBuf::from(std::env::var_os("NIR_RECOVERY_UI_PACKAGE").unwrap());
        let bundle = Arc::new(Bundle::open(&package.join("data")).unwrap());
        let mut builder = EventLoop::builder();
        builder.with_x11().with_any_thread(true);
        let event_loop = builder.build().unwrap();
        let window = Arc::new(
            event_loop
                .create_window(window_attributes("NIR recovery probe", true))
                .unwrap(),
        );
        let data = tempfile::tempdir().unwrap();
        let mut runtime = Runtime::new(window, bundle, data.path().to_owned(), None).unwrap();
        // Only the error/open result is controlled. The actual host, loader,
        // Winit window, renderer, Player, MP3 queues and durable storage run.
        let (source_tx, sources) = std::sync::mpsc::channel();
        let mut attempts = 0;
        runtime.output_worker = OutputWorker::with_opener(move |_, _| {
            attempts += 1;
            if attempts == 1 {
                return Err("controlled output unavailable".into());
            }
            let (mixer, source) = rodio::mixer::mixer(2, 48_000);
            source_tx.send(source).unwrap();
            Ok(OutputHandle {
                mixer,
                rate: 48_000,
            })
        });
        runtime.output = OutputState::default();
        runtime.audio = None;
        runtime.engine.begin_turn();
        runtime.retry_output(true).unwrap();
        until(&mut runtime, |r| r.engine.is_ready());
        runtime.input(UiAction::NewGame).unwrap();
        until(&mut runtime, |r| {
            r.engine
                .native_audio_recovery_layout()
                .is_some_and(|l| l.status == Status::Failed)
        });
        let initial = state(&runtime);
        let layout = runtime.engine.native_audio_recovery_layout().unwrap();
        assert!(initial["paused"].as_bool().unwrap());
        runtime.cursor = (layout.menu[0] + 20., layout.menu[1] + 20.);
        assert!(runtime
            .recovery_pointer(RecoveryPointer::Touch(7), RecoveryPhase::Down)
            .unwrap());
        assert!(runtime
            .recovery_pointer(RecoveryPointer::Touch(7), RecoveryPhase::Up)
            .unwrap());
        assert_eq!(state(&runtime)["screen"], "Menu");
        assert!(runtime.engine.native_audio_recovery_layout().is_none());
        assert!(runtime.output.blocked());
        assert!(!runtime.recovery_key(&Key::Named(NamedKey::Tab)).unwrap());
        runtime.input(UiAction::Close).unwrap();
        assert!(runtime.recovery_key(&Key::Named(NamedKey::Tab)).unwrap());
        runtime.turn().unwrap();
        assert_eq!(state(&runtime)["native_audio_recovery_focus"], "retry");
        assert!(runtime.recovery_key(&Key::Named(NamedKey::Tab)).unwrap());
        runtime.turn().unwrap();
        let keyboard_menu = state(&runtime);
        assert_eq!(keyboard_menu["native_audio_recovery_focus"], "menu");
        assert_eq!(keyboard_menu["position"], initial["position"]);
        assert_eq!(keyboard_menu["tick_us"], initial["tick_us"]);
        assert!(runtime.recovery_key(&Key::Named(NamedKey::Enter)).unwrap());
        assert_eq!(state(&runtime)["screen"], "Menu");
        runtime.input(UiAction::Saves).unwrap();
        runtime.input(UiAction::Save { slot: 0 }).unwrap();
        until(&mut runtime, |r| r.storage.load(0).unwrap().is_some());
        assert_eq!(runtime.storage.load(0).unwrap().unwrap().revision, 1);
        until(&mut runtime, |r| state(r)["status"] == "已保存");
        let saved_status = state(&runtime)["status"].clone();
        runtime.input(UiAction::Load { slot: 0 }).unwrap();
        until(&mut runtime, |r| {
            state(r)["screen"] == "Story"
                && state(r)["loading"] == false
                && r.engine.native_audio_recovery_layout().is_some()
        });
        let restored = state(&runtime);
        runtime.input(UiAction::Continue).unwrap();
        assert_eq!(state(&runtime)["paused"], true);
        assert_eq!(state(&runtime)["position"], restored["position"]);
        assert_eq!(state(&runtime)["sequence"], restored["sequence"]);
        let keys: Vec<_> = runtime.voices.keys().copied().collect();
        assert!(!keys.is_empty());
        let layout = runtime.engine.native_audio_recovery_layout().unwrap();
        runtime.cursor = (layout.retry[0] + 20., layout.retry[1] + 20.);
        runtime
            .recovery_pointer(RecoveryPointer::Touch(7), RecoveryPhase::Down)
            .unwrap();
        runtime
            .recovery_pointer(RecoveryPointer::Touch(7), RecoveryPhase::Up)
            .unwrap();
        assert!(runtime.output.pending());
        assert!(runtime.recovery_key(&Key::Named(NamedKey::Enter)).unwrap());
        // A second touch starts on the disabled retry while opening. Finish
        // it only after the notice is gone and Continue is now visible.
        runtime
            .recovery_pointer(RecoveryPointer::Touch(8), RecoveryPhase::Down)
            .unwrap();
        until(&mut runtime, |r| !r.output.blocked());
        let ready = state(&runtime);
        assert!(runtime
            .recovery_pointer(RecoveryPointer::Touch(8), RecoveryPhase::Up)
            .unwrap());
        let released = state(&runtime);
        assert!(runtime.engine.native_audio_recovery_layout().is_none());
        assert_eq!(
            released["native_audio_recovery_focus"],
            serde_json::Value::Null
        );
        assert_eq!(runtime.voices.keys().copied().collect::<Vec<_>>(), keys);
        assert_eq!(released["session"], restored["session"]);
        assert_eq!(released["position"], restored["position"]);
        assert_eq!(released["sequence"], restored["sequence"]);
        assert_eq!(released["tick_us"], restored["tick_us"]);
        assert_eq!(
            released["paused"], true,
            "output readiness must retain load's Continue pause"
        );
        let mut source = sources.recv_timeout(Duration::from_secs(5)).unwrap();
        // Continue is deliberately separate from the recovery gesture.
        runtime.input(UiAction::Continue).unwrap();
        // This snapshot was taken at a cue preparation boundary. Continue
        // admits that cue's resources; its separate loading pause can remain
        // until owner admission completes, even though restore is released.
        until(&mut runtime, |r| {
            state(r)["loading"] == false && state(r)["paused"] == false
        });
        assert_eq!(state(&runtime)["paused"], false);
        let samples: Vec<_> = source.by_ref().take(9600).collect();
        assert!(samples.iter().any(|s| s.abs() > 0.00001));
        until(&mut runtime, |r| r.io.outstanding() == 0);
        runtime
            .storage
            .write_preferences(runtime.engine.preferences())
            .unwrap();
        runtime.storage.merge_profile(BTreeSet::new()).unwrap();
        let prefs_file = runtime.storage.root.join("preferences.json");
        let profile_file = runtime.storage.root.join("profile.json");
        let old_prefs = fs::read(&prefs_file).unwrap();
        let old_profile = fs::read(&profile_file).unwrap();
        let prefs_block = prefs_file.with_extension(format!("{}.tmp", std::process::id()));
        let profile_block = profile_file.with_extension(format!("{}.tmp", std::process::id()));
        fs::create_dir(&prefs_block).unwrap();
        fs::create_dir(&profile_block).unwrap();
        runtime.input(UiAction::Settings).unwrap();
        let persistence_before = state(&runtime);
        let bgm_before = runtime
            .voices
            .iter()
            .filter(|(_, v)| v.bus == AudioBus::Bgm)
            .map(|(key, _)| *key)
            .collect::<Vec<_>>();
        assert!(!bgm_before.is_empty());
        runtime
            .input(UiAction::Volume {
                bus: AudioBus::Bgm,
                delta: -0.1,
            })
            .unwrap();
        runtime
            .submit_storage(IoRequest::MergeProfile(BTreeSet::from([
                "probe.failed".into()
            ])))
            .unwrap();
        until(&mut runtime, |r| r.io.outstanding() == 0);
        let persistence_failed = state(&runtime);
        assert_eq!(persistence_failed["error"], serde_json::Value::Null);
        for key in ["session", "tick_us", "position", "interaction"] {
            assert_eq!(persistence_failed[key], persistence_before[key]);
        }
        assert_eq!(fs::read(&prefs_file).unwrap(), old_prefs);
        assert_eq!(fs::read(&profile_file).unwrap(), old_profile);
        assert_eq!(
            runtime
                .voices
                .iter()
                .filter(|(_, v)| v.bus == AudioBus::Bgm)
                .map(|(key, _)| *key)
                .collect::<Vec<_>>(),
            bgm_before
        );
        assert!(runtime
            .voices
            .values()
            .filter(|v| v.bus == AudioBus::Bgm)
            .all(|v| !v.sink.is_paused()));
        let during_failure = source.by_ref().take(9600).collect::<Vec<_>>();
        assert!(during_failure.iter().any(|v| v.abs() > 0.00001));
        // A failed preference write during close cancels exit after the owner
        // admits the failure, just as a failed slot save already did.
        runtime.request_close("NIR persistence probe").unwrap();
        runtime
            .submit_storage(IoRequest::WritePreferences(
                runtime.engine.preferences().clone(),
            ))
            .unwrap();
        until(&mut runtime, |r| r.io.outstanding() == 0);
        assert_eq!(runtime.close.poll(0, false), CloseAction::Cancel);
        runtime.cancel_close("NIR persistence probe").unwrap();
        fs::remove_dir(&prefs_block).unwrap();
        fs::remove_dir(&profile_block).unwrap();
        runtime
            .input(UiAction::Volume {
                bus: AudioBus::Bgm,
                delta: 0.1,
            })
            .unwrap();
        runtime
            .submit_storage(IoRequest::MergeProfile(BTreeSet::from([
                "probe.later".into()
            ])))
            .unwrap();
        until(&mut runtime, |r| r.io.outstanding() == 0);
        assert!(runtime
            .storage
            .profile()
            .unwrap()
            .is_superset(&BTreeSet::from([
                "probe.failed".into(),
                "probe.later".into()
            ])));
        assert_eq!(
            runtime.storage.preferences().unwrap().unwrap(),
            *runtime.engine.preferences()
        );
        let persistence_recovered = state(&runtime);
        assert_eq!(persistence_recovered["error"], serde_json::Value::Null);
        for key in ["session", "tick_us", "position", "interaction"] {
            assert_eq!(persistence_recovered[key], persistence_before[key]);
        }
        let export_file = runtime.storage.root.join("probe-export.json");
        fs::write(&export_file, b"old export stays intact").unwrap();
        let export_block = export_file.with_extension(format!("{}.tmp", std::process::id()));
        fs::create_dir(&export_block).unwrap();
        let export_before = state(&runtime);
        let (job, json) = export_request(&mut runtime);
        let (send, replies) = std::sync::mpsc::channel();
        runtime.dialog = Some(PendingDialog {
            replies,
            failure: dialog::DialogFailure::Export(job),
        });
        send.send(DialogOutcome::ExportSelected {
            job,
            path: Some(export_file.clone()),
            json,
        })
        .unwrap();
        runtime.request_close("NIR export probe").unwrap();
        runtime.storage_turn().unwrap();
        assert_eq!(runtime.io.outstanding(), 1);
        until(&mut runtime, |r| r.io.outstanding() == 0);
        let export_failed = state(&runtime);
        assert_eq!(export_failed["diagnostic"]["location"], "export");
        assert_eq!(export_failed["error"], serde_json::Value::Null);
        assert_eq!(runtime.close.poll(0, false), CloseAction::Cancel);
        runtime.cancel_close("NIR export probe").unwrap();
        assert_eq!(fs::read(&export_file).unwrap(), b"old export stays intact");
        let export_samples = source.by_ref().take(9600).collect::<Vec<_>>();
        assert!(export_samples.iter().any(|s| s.abs() > 0.00001));
        let (job, json) = export_request(&mut runtime);
        let (send, replies) = std::sync::mpsc::channel();
        runtime.dialog = Some(PendingDialog {
            replies,
            failure: dialog::DialogFailure::Export(job),
        });
        send.send(DialogOutcome::ExportSelected {
            job,
            path: None,
            json,
        })
        .unwrap();
        runtime.storage_turn().unwrap();
        assert_eq!(runtime.io.outstanding(), 0);
        assert_eq!(fs::read(&export_file).unwrap(), b"old export stays intact");
        fs::remove_dir(&export_block).unwrap();
        let (job, json) = export_request(&mut runtime);
        let (send, replies) = std::sync::mpsc::channel();
        runtime.dialog = Some(PendingDialog {
            replies,
            failure: dialog::DialogFailure::Export(job),
        });
        send.send(DialogOutcome::ExportSelected {
            job,
            path: Some(export_file.clone()),
            json,
        })
        .unwrap();
        runtime.storage_turn().unwrap();
        until(&mut runtime, |r| r.io.outstanding() == 0);
        let exported: nir_player::SaveEnvelope =
            nir_content::parse(&fs::read(&export_file).unwrap(), "export probe").unwrap();
        assert_eq!(
            exported.digest,
            nir_content::digest(&serde_json::to_vec(&exported.snapshot).unwrap())
        );
        let export_recovered = state(&runtime);
        assert_ne!(export_recovered["diagnostic"]["location"], "export");
        // A picker thread exiting without an outcome also keeps the window.
        let (job, _) = export_request(&mut runtime);
        let (send, replies) = std::sync::mpsc::channel();
        runtime.dialog = Some(PendingDialog {
            replies,
            failure: dialog::DialogFailure::Export(job),
        });
        drop(send);
        runtime.storage_turn().unwrap();
        let export_disconnected = state(&runtime);
        assert_eq!(export_disconnected["diagnostic"]["location"], "export");
        assert!(export_disconnected["diagnostic"]["message"]
            .as_str()
            .unwrap()
            .contains("E_DIALOG_THREAD"));
        for current in [&export_failed, &export_recovered, &export_disconnected] {
            for key in ["session", "tick_us", "position", "interaction"] {
                assert_eq!(current[key], export_before[key]);
            }
            assert_eq!(current["error"], serde_json::Value::Null);
        }
        assert_eq!(
            runtime
                .voices
                .iter()
                .filter(|(_, v)| v.bus == AudioBus::Bgm)
                .map(|(key, _)| *key)
                .collect::<Vec<_>>(),
            bgm_before
        );
        assert!(runtime
            .voices
            .values()
            .filter(|v| v.bus == AudioBus::Bgm)
            .all(|v| !v.sink.is_paused()));
        let export_checks = serde_json::json!({
            "before":export_before,"failed":export_failed,"recovered":export_recovered,"disconnected":export_disconnected,
            "oldFilePreserved":true,"cancelSubmittedNoWrite":true,"exportDigestValid":true,
            "closeCancelledOnFailure":true,"softwareNonzeroSamples":export_samples.iter().filter(|s|s.abs()>0.00001).count(),
        });
        let healthy_slot = runtime.storage.load(0).unwrap().unwrap();
        let mut startup_checks = vec![];
        for kind in ["preferences", "profile", "both"] {
            let data = tempfile::tempdir().unwrap();
            let bundle = Arc::new(Bundle::open(&package.join("data")).unwrap());
            let storage = Storage::open(
                data.path(),
                &bundle.manifest.game_id,
                &bundle.manifest.profile,
                &bundle.release,
            )
            .unwrap();
            storage.save(0, 0, &healthy_slot).unwrap();
            let preferences = Preferences {
                font_scale: 1.4,
                bgm_volume: 0.2,
                ..Default::default()
            };
            storage.write_preferences(&preferences).unwrap();
            storage
                .merge_profile(BTreeSet::from(["fixture.prior".into()]))
                .unwrap();
            let prefs_file = storage.root.join("preferences.json");
            let profile_file = storage.root.join("profile.json");
            let healthy_prefs = fs::read(&prefs_file).unwrap();
            let healthy_profile = fs::read(&profile_file).unwrap();
            if kind != "profile" {
                fs::write(&prefs_file, b"{broken preferences").unwrap();
            }
            if kind != "preferences" {
                fs::write(&profile_file, b"[1]").unwrap();
            }
            let old_prefs = fs::read(&prefs_file).unwrap();
            let old_profile = fs::read(&profile_file).unwrap();
            drop(storage);
            let window = Arc::new(
                event_loop
                    .create_window(window_attributes("NIR startup metadata probe", true))
                    .unwrap(),
            );
            let mut probe = Runtime::new(window, bundle, data.path().to_owned(), None).unwrap();
            until(&mut probe, |r| {
                r.engine.is_ready() && state(r)["loading"] == false && r.io.outstanding() == 0
            });
            let startup = state(&probe);
            assert_eq!(startup["screen"], "Title");
            assert_eq!(startup["error"], serde_json::Value::Null);
            assert_eq!(startup["diagnostic"]["code"], "E_STORAGE");
            assert_eq!(
                startup["diagnostic"]["details"]["operation"],
                "load_metadata"
            );
            assert!(!startup["status"].as_str().unwrap().is_empty());
            if kind == "profile" {
                assert_eq!(*probe.engine.preferences(), preferences);
            }
            assert_eq!(fs::read(&prefs_file).unwrap(), old_prefs);
            assert_eq!(fs::read(&profile_file).unwrap(), old_profile);
            let title_keyboard_check = if kind == "preferences" {
                ["keyboard", "touch", "shortcut"]
                    .into_iter()
                    .map(|activation| title_pending_metadata_keyboard(&mut probe, activation))
                    .collect::<Vec<_>>()
            } else {
                vec![]
            };
            probe.input(UiAction::Saves).unwrap();
            until(&mut probe, |r| {
                state(r)["screen"] == "Saves" && state(r)["loading"] == false
            });
            probe.input(UiAction::Load { slot: 0 }).unwrap();
            until(&mut probe, |r| {
                state(r)["screen"] == "Story"
                    && state(r)["loading"] == false
                    && r.io.outstanding() == 0
            });
            let loaded = state(&probe);
            assert_eq!(loaded["error"], serde_json::Value::Null);
            assert_eq!(loaded["paused"], true);
            assert_eq!(loaded["position"], restored["position"]);
            assert_eq!(loaded["diagnostic"]["code"], "E_STORAGE");
            let before = state(&probe);
            if kind != "profile" {
                probe
                    .input(UiAction::Volume {
                        bus: AudioBus::Bgm,
                        delta: -0.15,
                    })
                    .unwrap();
            }
            if kind != "preferences" {
                probe
                    .submit_storage(IoRequest::MergeProfile(BTreeSet::from([
                        "probe.startup.failed".into(),
                    ])))
                    .unwrap();
            }
            until(&mut probe, |r| r.io.outstanding() == 0);
            let failed = state(&probe);
            for key in ["session", "tick_us", "position", "interaction", "paused"] {
                assert_eq!(failed[key], before[key]);
            }
            assert_eq!(failed["error"], serde_json::Value::Null);
            assert_eq!(fs::read(&prefs_file).unwrap(), old_prefs);
            assert_eq!(fs::read(&profile_file).unwrap(), old_profile);
            assert!(
                probe.engine.native_storage_recovery_layout().is_none(),
                "reading is not covered by metadata recovery"
            );
            probe.input(UiAction::Settings).unwrap();
            until(&mut probe, |r| {
                r.engine.native_storage_recovery_layout().is_some()
                    && r.engine.focus_actions().contains(&UiAction::Retry)
            });
            // Visit the actual semantic host control through the existing
            // keyboard focus cycle. Its Retry is consumed by Runtime, not VM.
            for _ in 0..512 {
                engine_result(probe.engine.navigate_focus(1)).unwrap();
                if probe.engine.native_storage_recovery_focused() {
                    break;
                }
            }
            assert!(
                probe.engine.native_storage_recovery_focused(),
                "metadata recovery lost from keyboard cycle: actions={:?}, center={:?}, state={}",
                probe.engine.focus_actions(),
                probe.engine.focused_center(),
                probe.engine.state()
            );
            let action = probe.engine.primary_action().unwrap();
            assert_eq!(action, UiAction::Retry);
            probe.input(action).unwrap();
            until(&mut probe, |r| r.io.outstanding() == 0);
            let bad_retry = state(&probe);
            assert_eq!(bad_retry["diagnostic"]["code"], "E_STORAGE");
            assert_eq!(fs::read(&prefs_file).unwrap(), old_prefs);
            assert_eq!(fs::read(&profile_file).unwrap(), old_profile);
            // Only the fixture repairs originals, then ordinary worker writes recover.
            if kind != "profile" {
                fs::write(&prefs_file, &healthy_prefs).unwrap();
            }
            if kind != "preferences" {
                fs::write(&profile_file, &healthy_profile).unwrap();
            }
            let layout = probe.engine.native_storage_recovery_layout().unwrap();
            probe.cursor = (layout.retry[0] + 20., layout.retry[1] + 20.);
            assert!(probe
                .metadata_pointer(RecoveryPointer::Touch(41), RecoveryPhase::Down)
                .unwrap());
            assert!(probe
                .metadata_pointer(RecoveryPointer::Touch(41), RecoveryPhase::Up)
                .unwrap());
            let working = probe.engine.native_storage_recovery_layout().unwrap();
            assert!(working.status.pending && !working.status.retry_enabled);
            assert!(probe
                .metadata_pointer(RecoveryPointer::Touch(42), RecoveryPhase::Down)
                .unwrap());
            let partial_recovery = if kind == "both" {
                until(&mut probe, |r| {
                    r.engine.native_metadata_failure_kinds()
                        == BTreeSet::from([nir_player::PersistenceKind::Profile])
                });
                let partial = state(&probe);
                assert_eq!(partial["diagnostic"]["location"], "profile");
                Some(partial)
            } else {
                None
            };
            until(&mut probe, |r| {
                r.io.outstanding() == 0 && r.engine.native_metadata_failure_kinds().is_empty()
            });
            let read_recovered = state(&probe);
            assert!(probe.engine.native_storage_recovery_layout().is_none());
            assert!(probe
                .metadata_pointer(RecoveryPointer::Touch(42), RecoveryPhase::Up)
                .unwrap());
            assert_eq!(state(&probe)["position"], read_recovered["position"]);
            assert_eq!(
                probe.engine.preferences().font_scale,
                1.4,
                "untouched preference is recovered from disk"
            );
            if kind != "profile" {
                assert!(
                    (probe.engine.preferences().bgm_volume - 0.15).abs() < 0.00001,
                    "current-session edit must win over recovered disk value 0.2"
                );
            }
            probe.input(UiAction::Close).unwrap();
            until(&mut probe, |r| state(r)["screen"] == "Story");
            if kind != "profile" {
                probe
                    .input(UiAction::Volume {
                        bus: AudioBus::Bgm,
                        delta: 0.1,
                    })
                    .unwrap();
                until(&mut probe, |r| r.io.outstanding() == 0);
                assert_eq!(
                    probe.storage.preferences().unwrap().unwrap(),
                    *probe.engine.preferences()
                );
                if kind == "both" {
                    assert_eq!(
                        state(&probe)["diagnostic"],
                        serde_json::Value::Null,
                        "both metadata reads and failed writes were explicitly recovered"
                    );
                }
            }
            if kind != "preferences" {
                probe
                    .submit_storage(IoRequest::MergeProfile(BTreeSet::from([
                        "probe.startup.next".into(),
                    ])))
                    .unwrap();
                until(&mut probe, |r| r.io.outstanding() == 0);
                let profile = probe.storage.profile().unwrap();
                for key in [
                    "fixture.prior",
                    "probe.startup.failed",
                    "probe.startup.next",
                ] {
                    assert!(profile.contains(key));
                }
            }
            let recovered = state(&probe);
            assert_eq!(recovered["diagnostic"], serde_json::Value::Null);
            assert_eq!(recovered["error"], serde_json::Value::Null);
            assert_eq!(recovered["paused"], true);
            startup_checks.push(serde_json::json!({"kind":kind,"startup":startup,"loaded":loaded,"failedWrite":failed,"titleKeyboard":title_keyboard_check,"badRetry":bad_retry,"partialRecovery":partial_recovery,"readRecovered":read_recovered,"recovered":recovered,"originalBadRecordsPreserved":true,"healthySlotLoaded":true,"failedProgressRetriedAfterFixtureRepair":kind!="preferences","touchUpConsumedAfterRecovery":true,"keyboardRetryUsedActualSemanticControl":true}));
        }
        // A real thread unwinds outside the mailbox lock with an accepted
        // Player save and other requests queued behind it. Keep the actual
        // window/Player/renderer/audio while identities are settled one turn
        // at a time, then explicitly submit fresh operations.
        runtime.input(UiAction::Saves).unwrap();
        until(&mut runtime, |r| r.io.outstanding() == 0);
        let death_before = state(&runtime);
        let old_prefs = fs::read(&prefs_file).unwrap();
        let old_profile = fs::read(&profile_file).unwrap();
        assert!(runtime.storage.load(2).unwrap().is_none());
        let (death_started, death_ready) = std::sync::mpsc::channel();
        let (resume, blocked) = std::sync::mpsc::channel();
        let (driver, worker) = IoWorker::with_driver(runtime.storage.clone(), move |request| {
            assert!(matches!(request, IoRequest::Save { slot: 2, .. }));
            death_started.send(()).unwrap();
            blocked.recv().unwrap();
            panic!("controlled native storage thread death before save write");
        });
        runtime.io = driver;
        runtime.input(UiAction::Save { slot: 2 }).unwrap();
        death_ready.recv_timeout(Duration::from_secs(5)).unwrap();
        runtime
            .submit_storage(IoRequest::MergeProfile(BTreeSet::from([
                "probe.thread.failed".into(),
            ])))
            .unwrap();
        runtime
            .submit_storage(IoRequest::WritePreferences(
                runtime.engine.preferences().clone(),
            ))
            .unwrap();
        let (job, json) = export_request(&mut runtime);
        runtime
            .submit_storage(IoRequest::Export {
                job,
                path: export_file.clone(),
                json,
            })
            .unwrap();
        let old_export = fs::read(&export_file).unwrap();
        runtime.request_close("NIR storage death probe").unwrap();
        resume.send(()).unwrap();
        assert!(worker.join().is_err());
        until(&mut runtime, |r| r.io.outstanding() == 0);
        let death_failed = state(&runtime);
        for key in [
            "session",
            "interaction",
            "position",
            "tick_us",
            "history_count",
            "sequence",
        ] {
            assert_eq!(death_failed[key], death_before[key]);
        }
        assert_eq!(death_failed["error"], serde_json::Value::Null);
        assert_eq!(death_failed["diagnostic"]["code"], "E_STORAGE");
        assert!(runtime.window.is_some());
        assert_eq!(runtime.close.poll(0, false), CloseAction::Cancel);
        runtime.cancel_close("NIR storage death probe").unwrap();
        assert!(runtime.storage.load(2).unwrap().is_none());
        assert_eq!(fs::read(&prefs_file).unwrap(), old_prefs);
        assert_eq!(fs::read(&profile_file).unwrap(), old_profile);
        assert_eq!(fs::read(&export_file).unwrap(), old_export);
        let death_samples = source.by_ref().take(9600).collect::<Vec<_>>();
        assert!(death_samples.iter().any(|s| s.abs() > 0.00001));
        runtime.input(UiAction::Save { slot: 2 }).unwrap();
        until(&mut runtime, |r| r.io.outstanding() == 0);
        assert_eq!(runtime.storage.load(2).unwrap().unwrap().revision, 1);
        runtime
            .submit_storage(IoRequest::MergeProfile(BTreeSet::from([
                "probe.thread.next".into(),
            ])))
            .unwrap();
        runtime
            .submit_storage(IoRequest::WritePreferences(
                runtime.engine.preferences().clone(),
            ))
            .unwrap();
        let (job, json) = export_request(&mut runtime);
        runtime
            .submit_storage(IoRequest::Export {
                job,
                path: export_file.clone(),
                json,
            })
            .unwrap();
        until(&mut runtime, |r| r.io.outstanding() == 0);
        let death_recovered = state(&runtime);
        for key in [
            "session",
            "interaction",
            "position",
            "tick_us",
            "history_count",
            "sequence",
        ] {
            assert_eq!(death_recovered[key], death_before[key]);
        }
        assert_eq!(death_recovered["error"], serde_json::Value::Null);
        let profile = runtime.storage.profile().unwrap();
        assert!(profile.contains("probe.thread.failed") && profile.contains("probe.thread.next"));
        assert_eq!(
            runtime
                .voices
                .iter()
                .filter(|(_, v)| v.bus == AudioBus::Bgm)
                .map(|(key, _)| *key)
                .collect::<Vec<_>>(),
            bgm_before
        );
        assert!(runtime
            .voices
            .values()
            .filter(|v| v.bus == AudioBus::Bgm)
            .all(|v| !v.sink.is_paused()));
        let storage_death_checks = serde_json::json!({"before":death_before,"failed":death_failed,"recovered":death_recovered,
            "acceptedSaveSettled":true,"closeCancelled":true,"oldFilesPreserved":true,"explicitNewSaveRevision":1,
            "failedProgressKeysRecovered":true,"bgmKeys":bgm_before,"softwareNonzeroSamples":death_samples.iter().filter(|s|s.abs()>0.00001).count()});
        if let Some(path) = std::env::var_os("NIR_RECOVERY_UI_REPORT") {
            std::fs::write(path, serde_json::to_vec_pretty(&serde_json::json!({
                "initial":initial,"keyboardMenu":keyboard_menu,"savedStatus":saved_status,
                "restored":restored,"ready":ready,"released":released,
                "persistenceBefore":persistence_before,"persistenceFailed":persistence_failed,"persistenceRecovered":persistence_recovered,
                "persistenceBgmKeys":bgm_before,"persistenceSoftwareNonzeroSamples":during_failure.iter().filter(|s|s.abs()>0.00001).count(),
                "oldPersistenceFilesKeptOnFailure":true,"failedProgressRetried":true,"closeCancelledOnPersistenceFailure":true,
                "exportChecks":export_checks,
                "startupChecks":startup_checks,
                "storageDeathChecks":storage_death_checks,
                "afterContinue":state(&runtime),"renderedFrames":state(&runtime)["frames"],
                "sourceNonzeroSamples":samples.iter().filter(|s|s.abs()>0.00001).count(),
                "limits":["Controlled output worker and software mixer; no CPAL/device/hardware listening.",
                    "Hidden actual X11/Vulkan window; states and frame submissions, not independent pixel comparison.",
                    "Native pointer handler invoked with Touch identity; no physical Android events."]
            })).unwrap()).unwrap();
        }
    }
}

/// Android entry. The AndroidApp handle supplies the storage roots and APK
/// assets; `native_library` is the installed libplayer.so path used for the
/// whole-player digest attestation (None skips the check).
#[cfg(target_os = "android")]
pub fn run_android(
    app: android_activity::AndroidApp,
    native_library: Option<PathBuf>,
) -> Result<()> {
    let files = app
        .internal_data_path()
        .context("E_STORAGE_ROOT: internal data path unavailable")?;
    let root = extract_content(&app, &files)?;
    // Saves and exports live in the external files dir (Android/data/<pkg>/
    // files, visible over USB) so the documented backup path works; the rare
    // device without one falls back to internal storage.
    let external = app.external_data_path().unwrap_or_else(|| files.clone());
    let data = external.join("NIR/games");
    let exports = external.join("exports");
    fs::create_dir_all(&exports).ok();
    let bundle = Arc::new(Bundle::open(&root)?);
    // Same helper-thread attestation as the desktop: the installed native
    // library is hashed while the event loop and GPU come up, and the verdict
    // gates runtime construction in resumed().
    let expected_player = bundle.manifest.player.clone();
    let exe_check = native_library.map(|path| {
        std::thread::spawn(move || -> Result<()> {
            nir_content::verify(&fs::read(path)?, &expected_player)?;
            Ok(())
        })
    });
    let event_loop = winit::event_loop::EventLoop::builder()
        .with_android_app(app)
        .build()?;
    let mut app = App {
        bundle,
        data,
        exports,
        runtime: None,
        error: None,
        smoke: None,
        smoke_step: 0,
        started: Instant::now(),
        smoke_advance: Instant::now(),
        advances: 0,
        save_at_dialogue: false,
        hidden: false,
        exe_check,
    };
    event_loop.run_app(&mut app)?;
    if let Some(error) = app.error {
        return Err(error);
    }
    Ok(())
}

/// Streams the packaged `data/` tree out of the APK into filesDir once.
/// Unchanged content-addressed files (same name and size) are skipped, so an
/// app update only copies content that actually changed; the digests
/// Bundle::open re-verifies still cover the extracted copies. `release.txt`
/// is the one fixed-name file whose content changes every release (its size
/// never does), so it is always rewritten.
#[cfg(target_os = "android")]
fn extract_content(app: &android_activity::AndroidApp, files: &std::path::Path) -> Result<PathBuf> {
    use std::{ffi::CString, io::Read};
    fn walk(
        manager: &ndk::asset::AssetManager,
        apk_dir: &str,
        out_dir: &std::path::Path,
    ) -> Result<()> {
        let dir_c = CString::new(apk_dir.to_owned())
            .with_context(|| format!("E_PACKAGE_PATH: {apk_dir}"))?;
        let Some(mut dir) = manager.open_dir(&dir_c) else {
            bail!("E_PACKAGE_ROOT: {apk_dir} missing from APK assets");
        };
        let mut names = Vec::new();
        while let Some(name) = dir.with_next(|c| c.to_bytes().to_vec()) {
            names.push(name);
        }
        for name in names {
            let name = String::from_utf8_lossy(&name).into_owned();
            if name.is_empty() || name.contains(['\\', ':']) || name.split('/').any(|s| s == "..") {
                bail!("E_PACKAGE_PATH: invalid asset name {name:?}");
            }
            let apk_path = format!("{apk_dir}/{name}");
            let out_path = out_dir.join(&name);
            let asset_c = CString::new(apk_path.clone())
                .with_context(|| format!("E_PACKAGE_PATH: {apk_path}"))?;
            if let Some(mut asset) = manager.open(&asset_c) {
                let len = asset.length();
                // Only the content-addressed trees are immutable; the release
                // pointer is fixed-name and fixed-size, so skipping it on a
                // size match would pin every update to the old release.
                let addressable = apk_path != "data/release.txt";
                if addressable
                    && matches!(fs::metadata(&out_path), Ok(m) if m.len() as usize == len)
                {
                    continue;
                }
                let mut bytes = Vec::with_capacity(len);
                asset
                    .read_to_end(&mut bytes)
                    .with_context(|| format!("E_PACKAGE_READ: {apk_path}"))?;
                atomic_write(&out_path, &bytes)?;
            } else {
                fs::create_dir_all(&out_path)
                    .with_context(|| format!("E_PACKAGE_PATH: {out_path:?}"))?;
                walk(manager, &apk_path, &out_path)?;
            }
        }
        Ok(())
    }
    let root = files.join("data");
    fs::create_dir_all(&root).context("E_PACKAGE_ROOT")?;
    walk(&app.asset_manager(), "data", &root)?;
    Ok(root)
}
