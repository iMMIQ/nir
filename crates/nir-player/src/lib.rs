//! Single-owner application coordination. Host completions enter through pump.
#![forbid(unsafe_code)]
use nir_assets::{BudgetLedger, Generation, PrepareJob, Reservation};
use nir_core::*;
use nir_format::*;
use nir_presentation::{ChoiceView, DialogueView, Screen, SlotView, UiModel, WindowTransition};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
mod checkpoints;
mod clock;
use checkpoints::Checkpoints;
mod menu;
use menu::{MenuSession, SaveConfirmation};
mod content;
mod effects;
use effects::{DeferredExitKind, MenuEffectsState};
mod replay;
use replay::{ReplayPhase, ReplayWork};
mod character_voice;
mod preference_recovery;
use preference_recovery::PreferenceRecovery;
mod history_voice;
use history_voice::{HistoryPlayback, HistoryVoiceStage};
mod pause;
use clock::ForegroundClockDemand;
pub use clock::ForegroundClockToken;
pub use content::ContentRequest;
use content::{ContentPreparation, ContentPurpose, RestoreWork};
pub use pause::PauseToken;
use pause::Pauses;

pub const EVENT_CAPACITY: usize = 256;
const INPUT_CAPACITY: usize = 128;
/// Ceiling for the sum of all decoded-asset leases (stage media, fonts,
/// audio); the render-surface reservation is carved out of the same ledger.
const MEMORY_LEDGER_LIMIT: u64 = 256 * 1024 * 1024;
fn render_surface_cost(stage: &Stage, width: u32, height: u32) -> u64 {
    // RGBA8: two stage freeze targets, the physical window root and two
    // physical menu roots. Fixed slack covers glyph/vertex working storage.
    stage.width as u64 * stage.height as u64 * 8
        + width as u64 * height as u64 * 12
        + 8 * 1024 * 1024
}

mod actions;
mod events;
#[cfg(test)]
mod media_tests;
mod model;
mod prepare;
mod types;
use types::PersistencePhase;
pub use types::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Purpose {
    Boot,
    Menu,
    Activation,
    Restore,
    Rollback,
    Device,
    /// Isolated replay entry: the frozen session stays live until this
    /// candidate prepares.
    Replay,
    /// Mid-story media top-up (an operation-committed reveal mask): swaps the
    /// active lease only, never advances the VM.
    TopUp,
}
struct Preparation {
    request: u32,
    promoted_from: Option<u32>,
    assets: BTreeSet<String>,
    purpose: Purpose,
    job: PrepareJob,
    preflight: bool,
    failed: bool,
}
struct MediaLookahead {
    request: u32,
    fingerprint: String,
    assets: BTreeSet<String>,
    job: PrepareJob,
}
#[derive(Debug, Clone)]
struct LocaleCandidate {
    request: u32,
    ui_locale: String,
    text_locale: String,
    preflight: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct PrefetchAttempt {
    wait: String,
    function: String,
    module: String,
    locale: String,
    session: u32,
}
pub struct Player {
    messages: nir_presentation::Messages,
    core: Core,
    validated: ValidatedProgram,
    audio_decode_sample_rate: Option<u32>,
    pub title: String,
    pub release: String,
    pub screen: Screen,
    return_screen: Screen,
    pub preferences: Preferences,
    pub effective_ui_locale: String,
    pub effective_text_locale: String,
    locale_candidate: Option<LocaleCandidate>,
    locale_job: Option<PrepareJob>,
    locale_error: Option<String>,
    pub generation: Generation,
    pub status: String,
    pub error: Option<String>,
    pub diagnostic: Option<Diagnostic>,
    persistence_failures: BTreeMap<(PersistenceKind, PersistencePhase), Diagnostic>,
    preference_recovery: PreferenceRecovery,
    export_job: Option<u32>,
    pub auto: bool,
    pub skip: bool,
    held_skip: bool,
    interface_hidden: bool,
    menu_peek: bool,
    pub profile: BTreeSet<String>,
    profile_values: BTreeMap<String, nir_format::Value>,
    profile_value_edits: BTreeMap<String, nir_format::Value>,
    image_menu: String,
    menu_session: MenuSession,
    overlay_menu: Option<String>,
    pending_menu: Option<menu::MenuNavigation>,
    prepared_menu: Option<(String, u32, u32, u32, u32)>,
    hovered_image: Option<String>,
    pub slots: Vec<SlotView>,
    pauses: Pauses,
    ui_pauses: Pauses,
    ui_clock_us: Micros,
    ui_clock_demand: ForegroundClockDemand,
    menu_effects: MenuEffectsState,
    menu_effects_clock: Option<ForegroundClockToken>,
    /// A foreground-owned visual state changed discretely (a menu page fade
    /// completed). Clock-only ticks must still re-project for it: the clock
    /// token releases at completion and can stop the frame loop before the
    /// engine's safety valve would have refreshed the view.
    ui_visual_pulse: bool,
    /// One mask fetch per in-flight window reveal, keyed by its start.
    window_mask_attempt: Option<(Micros, String)>,
    audio_paused: BTreeMap<TimeDomain, bool>,
    audio_bus_paused: BTreeMap<AudioBus, bool>,
    /// One terminal callback per current sound, held across logical pauses.
    deferred_audio_events: BTreeMap<u32, CoreInput>,
    inbox: VecDeque<(u32, AppEvent)>,
    work_used: u32,
    prepare: Option<Preparation>,
    failed_admission: Option<(Purpose, u32, BTreeSet<String>)>,
    media_lookahead: Option<MediaLookahead>,
    media_retired: BTreeMap<u32, PrepareJob>,
    deferred_prepare: Option<(Generation, Purpose, u32, BTreeSet<String>)>,
    deferred_locale: Option<u32>,
    media_attempted: Option<String>,
    content: BTreeMap<u32, ContentPreparation>,
    content_leases: Vec<nir_core::ContentLease>,
    restore_work: Option<RestoreWork>,
    prefetch_attempted: Option<PrefetchAttempt>,
    candidate: Option<Core>,
    device_resume: Option<Purpose>,
    replay_work: Option<ReplayWork>,
    ledger: BudgetLedger,
    _surface_budget: Reservation,
    active: Option<Reservation>,
    request: u32,
    commands: Vec<AppCommand>,
    checkpoints: Checkpoints,
    slot_revisions: BTreeMap<u32, u32>,
    save_confirmation: Option<SaveConfirmation>,
    slot_restore: bool,
    slot_load: Option<(u32, u32, u32, u32)>,
    save_jobs: BTreeMap<u32, (u32, u32)>,
    unconfirmed_saves: BTreeMap<u32, Diagnostic>,
    /// At most one failed write per supported slot. Saving another slot or
    /// recovering metadata does not confirm that this snapshot was saved.
    failed_saves: BTreeMap<u32, Diagnostic>,
    auto_elapsed: u64,
    auto_wait_delay: Option<u64>,
    auto_voice_wait: Option<bool>,
    auto_anchor: Option<(u32, u32, u32)>,
    history_offset: usize,
    history_voice: Option<HistoryPlayback>,
    tracing: bool,
}
impl Player {
    pub fn new(program: Program, release: String, title: String) -> Result<Self> {
        Self::from_validated(ValidatedProgram::new(program)?, release, title, None, None)
    }
    pub fn new_runtime(
        root: RuntimeProgram,
        release: String,
        title: String,
        preferences: Option<Preferences>,
    ) -> Result<Self> {
        Self::new_runtime_with_audio_sample_rate(root, release, title, preferences, None)
    }
    pub fn new_runtime_with_audio_sample_rate(
        root: RuntimeProgram,
        release: String,
        title: String,
        preferences: Option<Preferences>,
        audio_decode_sample_rate: Option<u32>,
    ) -> Result<Self> {
        Self::from_validated(
            ValidatedProgram::from_runtime(root)?,
            release,
            title,
            preferences,
            audio_decode_sample_rate,
        )
    }
    fn from_validated(
        validated: ValidatedProgram,
        release: String,
        title: String,
        initial: Option<Preferences>,
        audio_decode_sample_rate: Option<u32>,
    ) -> Result<Self> {
        if audio_decode_sample_rate == Some(0) {
            return Err(Diagnostic::new(
                "E_AUDIO_RATE",
                "player",
                "decoder sample rate must be positive",
            ));
        }
        let config = &validated.program().locale_config;
        let mut preferences = initial.unwrap_or_else(|| {
            validated
                .program()
                .player
                .preferences(config.default_ui.clone(), config.default_text.clone())
        });
        if !config.ui.contains_key(&preferences.ui_locale) {
            preferences.ui_locale = config.default_ui.clone();
        }
        if !config.text.contains_key(&preferences.text_locale) {
            preferences.text_locale = config.default_text.clone();
        }
        preferences.text_speed = finite_clamp(preferences.text_speed, 0.25, 4., 1.);
        preferences.auto_wait_scale = finite_clamp(preferences.auto_wait_scale, 0.25, 4., 1.);
        preferences.font_scale = finite_clamp(preferences.font_scale, 0.8, 1.5, 1.);
        preferences.bgm_volume = finite_clamp(preferences.bgm_volume, 0., 1., 0.3);
        preferences.voice_volume = finite_clamp(preferences.voice_volume, 0., 1., 0.8);
        preferences.sfx_volume = finite_clamp(preferences.sfx_volume, 0., 1., 0.5);
        preferences
            .character_voices
            .retain(|id, _| validated.program().text_identity(id).is_some());
        preferences.normalize_character_voices();
        let locale = preferences.text_locale.clone();
        let ui_locale = preferences.ui_locale.clone();
        let mut core = Core::new(validated.clone(), release.clone(), locale.clone())?;
        core.set_tracing(false);
        // The decoded-asset ledger bounds worst-case residency, not a target.
        // A real image-heavy route legitimately peaks past 128 MiB once scene
        // crossfades admit the outgoing and incoming cue together with the
        // always-resident fonts (certified against a full LiveNovel import);
        // 256 MiB keeps that headroom while still bounding runaway leases.
        let ledger = BudgetLedger::new(MEMORY_LEDGER_LIMIT);
        let _surface_budget = ledger.reserve(&BTreeMap::from([(
            "@render-surfaces".into(),
            render_surface_cost(
                &validated.program().stage,
                validated.program().stage.width,
                validated.program().stage.height,
            ),
        )]))?;
        let save_slots = core.program().player.save_slots;
        let menu_session = MenuSession::new(
            core.program().theme.image_menus.get("title"),
            &preferences,
            save_slots,
        );
        let mut p = Self {
            messages: nir_presentation::Messages::default(),
            core,
            validated,
            audio_decode_sample_rate,
            title,
            release,
            screen: Screen::Title,
            return_screen: Screen::Title,
            preference_recovery: PreferenceRecovery::new(&preferences),
            preferences,
            effective_ui_locale: ui_locale,
            effective_text_locale: locale,
            locale_candidate: None,
            locale_job: None,
            locale_error: None,
            generation: Generation {
                session: 1,
                device: 1,
                surface: 1,
                typography: 1,
                language: 1,
            },
            status: String::new(),
            error: None,
            diagnostic: None,
            persistence_failures: BTreeMap::new(),
            export_job: None,
            auto: false,
            skip: false,
            held_skip: false,
            interface_hidden: false,
            menu_peek: false,
            profile: BTreeSet::new(),
            profile_values: BTreeMap::new(),
            profile_value_edits: BTreeMap::new(),
            image_menu: "title".into(),
            menu_session,
            overlay_menu: None,
            pending_menu: None,
            prepared_menu: None,
            hovered_image: None,
            slots: (0..save_slots)
                .map(|slot| SlotView {
                    slot,
                    ..Default::default()
                })
                .collect(),
            pauses: Pauses::default(),
            ui_pauses: Pauses::default(),
            ui_clock_us: Micros(0),
            ui_clock_demand: ForegroundClockDemand::default(),
            menu_effects: MenuEffectsState::new(1),
            menu_effects_clock: None,
            ui_visual_pulse: false,
            window_mask_attempt: None,
            audio_paused: BTreeMap::from([
                (TimeDomain::Story, true),
                (TimeDomain::ForegroundUi, true),
            ]),
            audio_bus_paused: BTreeMap::new(),
            deferred_audio_events: BTreeMap::new(),
            inbox: VecDeque::new(),
            work_used: 0,
            prepare: None,
            failed_admission: None,
            media_lookahead: None,
            media_retired: BTreeMap::new(),
            deferred_prepare: None,
            deferred_locale: None,
            media_attempted: None,
            candidate: None,
            device_resume: None,
            replay_work: None,
            ledger,
            _surface_budget,
            active: None,
            request: 0,
            commands: vec![AppCommand::ListSaves],
            checkpoints: Checkpoints::default(),
            slot_revisions: BTreeMap::new(),
            save_confirmation: None,
            slot_restore: false,
            slot_load: None,
            save_jobs: BTreeMap::new(),
            unconfirmed_saves: BTreeMap::new(),
            failed_saves: BTreeMap::new(),
            auto_elapsed: 0,
            auto_wait_delay: None,
            auto_voice_wait: None,
            auto_anchor: None,
            history_offset: 0,
            history_voice: None,
            content: BTreeMap::new(),
            content_leases: vec![],
            restore_work: None,
            prefetch_attempted: None,
            tracing: false,
        };
        let assets = p.title_assets();
        p.begin_prepare(Purpose::Boot, 0, assets)?;
        Ok(p)
    }
    pub fn acquire_pause(&self, reason: impl Into<String>) -> PauseToken {
        self.pauses.acquire(reason.into())
    }
    /// Freeze logical Story progress while its output waits for a device
    /// gesture. Do not suspend other working audio routes or prevent the
    /// blocked route from resuming. The host owns and releases this token.
    pub fn acquire_audio_output_wait(&self) -> PauseToken {
        self.pauses.acquire_barrier("audio-output".into())
    }
    pub fn work_used(&self) -> u32 {
        self.work_used
    }
    /// Enable or disable per-op `AppCommand::Trace` emission (disabled by default
    /// in production; enabled only when browser/test tracing requests it).
    pub fn set_tracing(&mut self, enabled: bool) {
        self.tracing = enabled;
        self.core.set_tracing(enabled);
    }
    /// Lightweight hover/input queries that avoid constructing a full `UiModel`.
    pub fn has_authored_menu(&self) -> bool {
        matches!(self.screen, Screen::Title | Screen::Menu)
            && self
                .active_menu_id()
                .is_some_and(|id| self.core.program().theme.image_menus.contains_key(id))
    }
    pub fn has_image_choice(&self) -> bool {
        if self.screen != Screen::Story {
            return false;
        }
        self.core
            .state()
            .choice
            .as_ref()
            .and_then(|c| self.core.program().choices.get(&c.id))
            .is_some_and(|choice| choice.options.iter().any(|o| o.image.is_some()))
    }
    pub fn hovered_image(&self) -> Option<&str> {
        self.hovered_image.as_deref()
    }
    pub fn save_slots(&self) -> u32 {
        self.core.program().player.save_slots
    }
    pub fn escape_action(&self) -> UiAction {
        if self
            .core
            .state()
            .choice
            .as_ref()
            .is_some_and(|c| c.on_cancel.is_some())
        {
            UiAction::CancelChoice
        } else if self.menu_session.depth() > 0 {
            UiAction::Close
        } else if matches!(self.screen, Screen::Story | Screen::Title) {
            UiAction::Menu
        } else {
            UiAction::Close
        }
    }
    /// Takes whether a foreground-owned visual state changed discretely since
    /// the last check; the engine re-projects on the next draw when set.
    pub fn take_ui_visual_pulse(&mut self) -> bool {
        std::mem::take(&mut self.ui_visual_pulse)
    }
    pub fn pending_events(&self) -> usize {
        self.inbox.len()
            + if self.paused() {
                0
            } else {
                self.deferred_audio_events.len()
            }
    }
    pub fn skipping(&self) -> bool {
        self.skip || self.held_skip
    }
    pub fn core(&self) -> &Core {
        &self.core
    }
    /// Native hosts recover each failed metadata kind independently. A
    /// successful write does not settle a preceding failed read of that kind.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn metadata_failure_kinds(&self) -> BTreeSet<PersistenceKind> {
        self.persistence_failures
            .keys()
            .map(|(kind, _)| *kind)
            .collect()
    }
    pub fn locale_preview(&self, request: u32) -> Option<UiModel> {
        let candidate = self
            .locale_candidate
            .as_ref()
            .filter(|c| c.request == request)?;
        let mut model = self.model_for_locale(&self.core, self.screen, &candidate.ui_locale);
        model.interface_hidden = false;
        model.locale_pending = true;
        let ui_plan = &self.core.program().locale_config.ui[&candidate.ui_locale];
        let text_plan = &self.core.program().locale_config.text[&candidate.text_locale];
        model.text_locale = candidate.text_locale.clone();
        model.text_fonts = text_plan.fonts.clone();
        model.text_font_plan_digest = text_plan.digest.clone();
        let mut preflight_texts = Vec::new();
        let mut append = |text: String, locale: &str, fonts: &[String], digest: &str| {
            preflight_texts.push(nir_presentation::TextRun {
                text,
                visible: None,
                x: -10_000.,
                y: -10_000.,
                width: 4096.,
                height: 64.,
                size: 18.,
                line_height: 27.,
                color: [0., 0., 0., 0.],
                emphasis: vec![],
                images: vec![],
                image_scale: 1.,
                scroll: 0.,
                clip: None,
                region: None,
                locale: locale.into(),
                font_assets: fonts.to_vec(),
                font_plan_digest: digest.into(),
                preflight_only: true,
                shadow: None,
                monochrome: false,
            });
        };
        for text in self.messages.preflight(&candidate.ui_locale) {
            append(text, &candidate.ui_locale, &ui_plan.fonts, &ui_plan.digest);
        }
        if let Some(docs) = self.core.program().locales.get(&candidate.text_locale) {
            for doc in docs.values() {
                let mut text = String::new();
                for span in &doc.spans {
                    match span {
                        Span::Text { text: value, .. } => text.push_str(value),
                        Span::Ruby {
                            text: value,
                            reading,
                            ..
                        } => {
                            text.push_str(value);
                            append(
                                reading.clone(),
                                &candidate.text_locale,
                                &text_plan.fonts,
                                &text_plan.digest,
                            );
                        }
                        Span::Image { .. } => {}
                        Span::Break { .. } => text.push('\n'),
                        Span::Gate { .. } | Span::Pause { .. } => {}
                        Span::Param { name, .. } => {
                            if let Some(value) = self
                                .core
                                .state()
                                .variables
                                .get(name)
                                .or_else(|| self.core.program().variables.get(name))
                            {
                                match value {
                                    Value::Bool(value) => {
                                        text.push_str(if *value { "true" } else { "false" })
                                    }
                                    Value::I32(value) => text.push_str(&value.to_string()),
                                    Value::String(value) => text.push_str(value),
                                    Value::F80(value) => text.push_str(&value.to_string()),
                                }
                            }
                        }
                    }
                }
                append(
                    text,
                    &candidate.text_locale,
                    &text_plan.fonts,
                    &text_plan.digest,
                );
            }
        }
        model.preflight_texts = preflight_texts;
        Some(model)
    }
    pub fn accepts_locale(&self, request: u32) -> bool {
        self.locale_error.is_none()
            && self
                .locale_candidate
                .as_ref()
                .is_some_and(|c| c.request == request)
    }
    pub fn accepts_resource(&self, request: u32) -> bool {
        self.accepts_history_voice_resource(request)
            || self.accepts(request)
            || self
                .prepare
                .as_ref()
                .is_some_and(|p| p.promoted_from == Some(request) && !p.failed)
            || (self.accepts_locale(request) && self.locale_job.is_some())
            || self
                .media_lookahead
                .as_ref()
                .is_some_and(|p| p.request == request)
    }
    pub fn locale_pending(&self) -> bool {
        (self.locale_candidate.is_some()
            || self
                .content
                .values()
                .any(|p| matches!(p.purpose, ContentPurpose::Locale)))
            && self.locale_error.is_none()
    }
    pub fn locale_error(&self) -> Option<&str> {
        self.locale_error.as_deref()
    }
    fn invalidate_locale_candidate(&mut self) {
        self.deferred_locale = None;
        self.cancel_content(true);
        if let Some(candidate) = self.locale_candidate.take() {
            self.commands.push(AppCommand::CancelAssets {
                request: candidate.request,
            });
        }
        self.locale_job = None;
    }
    fn locale_failed(&mut self, request: u32, message: String) {
        if !self.accepts_locale(request) {
            return;
        }
        self.deferred_locale = None;
        self.locale_job = None;
        self.locale_error = Some(message);
        self.pauses.remove("locale");
        self.commands.push(AppCommand::CancelAssets { request });
        self.status = self
            .messages
            .text(&self.effective_ui_locale, "language-failed");
    }
    fn start_locale_switch(&mut self) -> Result<()> {
        self.deferred_locale = None;
        let config = &self.core.program().locale_config;
        if !config.ui.contains_key(&self.preferences.ui_locale)
            || !config.text.contains_key(&self.preferences.text_locale)
        {
            return Err(Diagnostic::new(
                "E_LOCALE",
                "preferences",
                "unsupported UI or text locale",
            ));
        }
        if self.preferences.ui_locale == self.effective_ui_locale
            && self.preferences.text_locale == self.effective_text_locale
        {
            self.invalidate_locale_candidate();
            self.locale_error = None;
            self.pauses.remove("locale");
            return Ok(());
        }
        let fonts: BTreeSet<_> = config.ui[&self.preferences.ui_locale]
            .fonts
            .iter()
            .chain(&config.text[&self.preferences.text_locale].fonts)
            .cloned()
            .collect();
        self.invalidate_locale_candidate();
        let mut needs = vec![];
        if self.screen != Screen::Title && self.return_screen != Screen::Title {
            if let Some(frame) = self.core.state().frames.last() {
                if let Some(module) = self.core.program().function_module(&frame.function) {
                    needs = self.content_requirements(
                        module,
                        Some(&self.preferences.text_locale),
                        false,
                    )?;
                }
            }
        }
        for requirement in self.asset_content_requirements(&fonts)? {
            if !needs.contains(&requirement) {
                needs.push(requirement);
            }
        }
        if !needs.is_empty() {
            self.locale_error = None;
            return self.begin_content(ContentPurpose::Locale, needs);
        }
        self.request = self.request.checked_add(1).ok_or_else(|| {
            Diagnostic::new("E_LIMIT", "locale", "locale request counter overflow")
        })?;
        let candidate = LocaleCandidate {
            request: self.request,
            ui_locale: self.preferences.ui_locale.clone(),
            text_locale: self.preferences.text_locale.clone(),
            preflight: false,
        };
        self.locale_candidate = Some(candidate.clone());
        self.locale_error = None;
        self.pauses.insert_barrier("locale".into());
        let costs = self.costs(&fonts)?;
        match PrepareJob::new(candidate.request, self.generation, costs, &self.ledger) {
            Ok(job) => {
                self.locale_job = Some(job);
                self.commands.push(AppCommand::GetAssets {
                    request: candidate.request,
                    session: self.generation.session,
                    device: self.generation.device,
                    descriptors: self.describe_assets(&fonts)?,
                    assets: fonts.into_iter().collect(),
                    priority: PreparePriority::Required,
                });
            }
            Err(error) => {
                if self.media_lookahead.is_some() || !self.media_retired.is_empty() {
                    self.cancel_media_lookahead();
                    self.deferred_locale = Some(candidate.request);
                    self.observe("locale_waiting_for_media_cancel", Some(candidate.request));
                } else {
                    self.locale_failed(candidate.request, error.to_string());
                }
            }
        }
        Ok(())
    }
    fn cancel_locale_switch(&mut self) {
        self.invalidate_locale_candidate();
        self.locale_error = None;
        self.pauses.remove("locale");
        self.preferences.ui_locale = self.effective_ui_locale.clone();
        self.preferences.text_locale = self.effective_text_locale.clone();
        self.persist_preferences();
    }
    pub fn current_interaction(&self) -> u32 {
        self.core
            .state()
            .choice
            .as_ref()
            .map(|c| c.interaction)
            .or_else(|| self.core.advance_wait())
            .or_else(|| self.core.dialogue().map(|(_, d)| d.interaction))
            .unwrap_or(0)
    }
    pub fn accepts(&self, request: u32) -> bool {
        self.prepare
            .as_ref()
            .is_some_and(|p| p.request == request && !p.failed)
    }
    pub fn memory_used(&self) -> u64 {
        self.ledger.used()
    }
    /// Account for actual physical compositor roots before their owner resizes.
    /// The global media ledger limit is unchanged; a failed resize is atomic.
    pub fn reserve_render_surfaces(&mut self, width: u32, height: u32) -> Result<()> {
        if width == 0 || height == 0 || width > 16_384 || height > 16_384 {
            return Err(Diagnostic::new(
                "E_VIEWPORT",
                "resize",
                "invalid physical extent",
            ));
        }
        self._surface_budget.resize_single(render_surface_cost(
            &self.validated.program().stage,
            width,
            height,
        ))
    }
    pub fn audio_decode_sample_rate(&self) -> Option<u32> {
        self.audio_decode_sample_rate
    }
    pub fn observe_audio_positions(
        &mut self,
        domain: TimeDomain,
        session: u32,
        positions: &[AudioPosition],
    ) -> Result<()> {
        if domain == TimeDomain::Story && session == self.generation.session {
            self.core.observe_audio_positions(positions)?;
        }
        Ok(())
    }
    pub fn interface_hidden(&self) -> bool {
        self.interface_hidden
    }
    pub fn presentation_screen(&self) -> Screen {
        if self.menu_peek {
            Screen::Story
        } else {
            self.screen
        }
    }
    fn set_interface_hidden(&mut self, hidden: bool) {
        if !hidden {
            self.menu_peek = false;
        }
        self.interface_hidden = hidden;
        if hidden && self.core.program().player.hide_policy == HidePolicy::PauseStory {
            self.pauses.insert("interface-hidden".into());
        } else {
            self.pauses.remove("interface-hidden");
        }
        if hidden {
            self.auto = false;
            self.skip = false;
            self.held_skip = false;
            self.auto_elapsed = 0;
            self.auto_wait_delay = None;
            self.auto_voice_wait = None;
        }
    }
    pub fn needs_clock(&self) -> bool {
        self.needs_story_clock()
            || (self.ui_clock_demand.active() && !self.domain_paused(TimeDomain::ForegroundUi))
    }
    /// Clock-driven picture changes, distinct from audio position accounting.
    pub fn needs_visual_clock(&self) -> bool {
        (self.screen == Screen::Story
            && self.pauses.is_empty()
            && (self.core.needs_visual_clock() || self.auto || self.skip || self.held_skip))
            || (self.ui_clock_demand.active() && !self.domain_paused(TimeDomain::ForegroundUi))
    }
    fn needs_story_clock(&self) -> bool {
        self.screen == Screen::Story
            && self.pauses.is_empty()
            && (self.core.needs_clock() || self.auto || self.skip || self.held_skip)
    }
    fn story_context_active(&self) -> bool {
        self.screen != Screen::Title && self.return_screen != Screen::Title
    }
    pub fn paused(&self) -> bool {
        !self.pauses.is_empty()
    }
    /// Device playback pause; resource barriers still freeze Story logic.
    pub fn domain_paused(&self, domain: TimeDomain) -> bool {
        match domain {
            TimeDomain::Story => self.pauses.audio_paused(),
            TimeDomain::ForegroundUi => !self.ui_pauses.is_empty(),
        }
    }
    fn overlay_bus_paused(&self, bus: AudioBus) -> bool {
        self.core.state().audio_paused.contains(&bus)
            || (self.pauses.contains("menu")
                && (bus != AudioBus::Bgm || self.menu_effects.music.is_some()))
    }
    /// Actual playback policy; menu ownership cannot release a device/background pause.
    pub fn bus_paused(&self, domain: TimeDomain, bus: AudioBus) -> bool {
        self.domain_paused(domain) || (domain == TimeDomain::Story && self.overlay_bus_paused(bus))
    }
    pub fn foreground_clock(&self) -> Micros {
        self.ui_clock_us
    }
    /// Opacity of the active menu page while a page effect fade runs
    /// (1.0 otherwise, including under reduced motion).
    pub fn menu_opacity(&self) -> f32 {
        self.menu_effects
            .opacity(self.ui_clock_us.0, self.preferences.reduced_motion)
    }
    /// The in-flight spatial page reveal: (style, to_visible, progress).
    /// `None` when no reveal runs (no style, dissolve, or reduced motion).
    pub fn menu_transition(&self) -> Option<(StageTransition, bool, f32)> {
        self.menu_effects
            .transition(self.ui_clock_us.0, self.preferences.reduced_motion)
    }
    /// The furthest-along element enter animation's normalized progress,
    /// or `None` when no element is animating (reduced motion included).
    pub fn menu_element_progress(&self) -> Option<f32> {
        if self.preferences.reduced_motion {
            None
        } else {
            self.menu_effects.element_progress(self.ui_clock_us.0)
        }
    }
    pub fn acquire_foreground_clock(&self) -> Option<ForegroundClockToken> {
        self.ui_clock_demand.acquire()
    }
    pub fn acquire_domain_pause(
        &self,
        domain: TimeDomain,
        reason: impl Into<String>,
    ) -> PauseToken {
        match domain {
            TimeDomain::Story => self.pauses.acquire(reason.into()),
            TimeDomain::ForegroundUi => self.ui_pauses.acquire(reason.into()),
        }
    }
    fn defer_audio_event(&mut self, task: u32, input: CoreInput) -> Result<()> {
        if self.deferred_audio_events.len() >= MAX_TASKS
            && !self.deferred_audio_events.contains_key(&task)
        {
            return Err(Diagnostic::new(
                "E_EVENT_QUEUE",
                "audio",
                "deferred completion limit",
            ));
        }
        // The first valid terminal event wins; duplicate or conflicting late
        // callbacks cannot turn a natural end into a failure, or vice versa.
        self.deferred_audio_events.entry(task).or_insert(input);
        Ok(())
    }
    fn reset_audio(&mut self) {
        self.stop_history_voice();
        self.deferred_audio_events.clear();
        for domain in [TimeDomain::Story, TimeDomain::ForegroundUi] {
            self.commands.push(AppCommand::AudioReset { domain });
        }
    }
    pub fn is_loading(&self) -> bool {
        self.prepare.is_some()
            || self.content.values().any(|p| {
                !p.failed
                    && !matches!(
                        p.purpose,
                        ContentPurpose::Locale
                            | ContentPurpose::Prefetch
                            | ContentPurpose::HistoryVoice
                    )
            })
    }
    /// A failed barrier has been replaced by live work. Further Retry input
    /// must not cancel that work; a new terminal failure enables retry again.
    pub fn retrying(&self) -> bool {
        self.error.is_some() && self.recovery_work_pending()
    }
    fn recovery_work_pending(&self) -> bool {
        let required = |p: &ContentPreparation| {
            !matches!(
                p.purpose,
                ContentPurpose::Locale | ContentPurpose::Prefetch | ContentPurpose::HistoryVoice
            )
        };
        self.failed_admission.is_none()
            && !self.prepare.as_ref().is_some_and(|p| p.failed)
            && !self.content.values().any(|p| p.failed && required(p))
            && (self.prepare.is_some()
                || self.deferred_prepare.is_some()
                || self.content.values().any(|p| !p.failed && required(p)))
    }
    fn title_nodes(&self) -> Vec<Node> {
        self.validated.title_nodes().to_vec()
    }
    fn font_assets(&self, ui: &str, text: &str) -> BTreeSet<String> {
        let config = &self.validated.program().locale_config;
        config
            .ui
            .get(ui)
            .into_iter()
            .flat_map(|p| p.fonts.iter())
            .chain(
                config
                    .text
                    .get(text)
                    .into_iter()
                    .flat_map(|p| p.fonts.iter()),
            )
            .cloned()
            .collect()
    }
    fn title_assets(&self) -> BTreeSet<String> {
        let mut a: BTreeSet<_> = self
            .title_nodes()
            .iter()
            .filter_map(|n| n.asset.clone())
            .collect();
        a.extend(
            self.core
                .program()
                .theme
                .dialogue
                .background
                .iter()
                .cloned(),
        );
        if let Some(menu) = self.core.program().theme.image_menus.get(&self.image_menu) {
            a.extend(menu.prepared_assets());
        }
        a.extend(self.font_assets(&self.effective_ui_locale, &self.effective_text_locale));
        a
    }
    fn state_assets(&self, core: &Core) -> BTreeSet<String> {
        let s = core.state();
        let mut a: BTreeSet<_> = s
            .scene
            .iter()
            .chain(s.draft.iter())
            .filter_map(|n| n.asset.clone())
            .collect();
        a.extend(
            s.dialogue_decorations
                .values()
                .map(|image| image.asset.clone()),
        );
        for images in s
            .tasks
            .values()
            .filter_map(|t| t.dialogue.as_ref())
            .map(|d| d.inline_images())
            .chain(
                s.pending
                    .iter()
                    .flat_map(|p| p.dialogues.values())
                    .map(|d| d.inline_images()),
            )
            .chain(s.history.iter().map(|h| h.images.clone()))
        {
            a.extend(images.into_iter().map(|i| i.image.asset));
        }
        for t in s.tasks.values().filter(|t| t.state == TaskState::Running) {
            a.extend(
                t.source
                    .iter()
                    .chain(t.target.iter())
                    .filter_map(|n| n.asset.clone()),
            );
            if let Effect::StagePresent { transition, .. } = &t.effect {
                a.extend(transition.asset().map(str::to_owned));
            }
            if let Effect::Audio { asset, .. } = &t.effect {
                a.insert(asset.clone());
            }
        }
        if let Some(reveal) = &s.window_reveal {
            a.extend(reveal.style.asset().map(str::to_owned));
        }
        if let Some(pending) = &s.pending {
            a.extend(self.validated.cue_assets(&pending.cue));
        }
        // Keep only the active overlay's media; hidden pages pin nothing. The
        if let Some(choice) = &s.choice {
            if let Some(definition) = core.program().choices.get(&choice.id) {
                for image in definition.options.iter().filter_map(|o| o.image.as_ref()) {
                    a.insert(image.asset.clone());
                    a.extend(image.hover_asset.iter().cloned());
                    a.extend(image.disabled_asset.iter().cloned());
                }
            }
        }
        // page's effect sounds and music stay resident with its images, or the
        // host prunes the decoded buffers the moment the page needs them.
        if self.screen == Screen::Menu {
            if let Some(menu) = self
                .active_menu_id()
                .and_then(|id| core.program().theme.image_menus.get(id))
            {
                a.extend(menu.prepared_assets());
            }
        }
        let dialogue = s
            .dialogue_style
            .as_ref()
            .and_then(|id| core.program().theme.dialogue_styles.get(id))
            .map(|style| &style.dialogue)
            .unwrap_or(&core.program().theme.dialogue);
        a.extend(dialogue.background.iter().cloned());
        // Operation-committed masks belong to no cue recipe; the loaded-body
        // set keeps authored reveals from stalling on their first encounter.
        a.extend(self.validated.window_transition_assets());
        a.extend(self.font_assets(&self.effective_ui_locale, &self.effective_text_locale));
        let config = &core.program().locale_config;
        for locale in s
            .tasks
            .values()
            .filter_map(|t| t.dialogue.as_ref().map(|d| &d.locale))
            .chain(
                s.pending
                    .iter()
                    .flat_map(|p| p.dialogues.values().map(|d| &d.locale)),
            )
            .chain(s.choice.iter().map(|c| &c.locale))
            .chain(s.history.iter().map(|h| &h.locale))
        {
            if let Some(plan) = config.text.get(locale) {
                a.extend(plan.fonts.iter().cloned());
            }
        }
        a
    }
    pub fn retained_assets(&self) -> BTreeSet<String> {
        let mut a = if !self.story_context_active() {
            self.title_assets()
        } else {
            self.state_assets(&self.core)
        };
        a.extend(self.pending_menu_assets());
        if let Some(c) = &self.candidate {
            a.extend(self.state_assets(c));
        }
        if let Some(prep) = &self.prepare {
            a.extend(prep.assets.iter().cloned());
        }
        if let Some(spec) = &self.media_lookahead {
            a.extend(spec.assets.iter().cloned());
        }
        if let Some(voice) = &self.history_voice {
            if matches!(
                voice.stage,
                Some(HistoryVoiceStage::Media { .. } | HistoryVoiceStage::Playing { .. })
            ) {
                a.insert(voice.current.asset.clone());
            }
        }
        a
    }
    pub fn content_residency(&self) -> nir_core::ResidencyReport {
        self.validated.residency()
    }
    pub fn asset_descriptor(&self, id: &str) -> Option<&Asset> {
        self.validated.asset(id)
    }
    pub fn retained_descriptors(&self) -> BTreeMap<String, Asset> {
        self.retained_assets()
            .into_iter()
            .filter_map(|id| self.asset_descriptor(&id).cloned().map(|asset| (id, asset)))
            .collect()
    }
    fn describe_assets(&self, ids: &BTreeSet<String>) -> Result<BTreeMap<String, Asset>> {
        ids.iter()
            .map(|id| {
                self.asset_descriptor(id)
                    .cloned()
                    .map(|asset| (id.clone(), asset))
                    .ok_or_else(|| {
                        Diagnostic::new("E_ASSET", id, "resource catalog is not resident")
                    })
            })
            .collect()
    }
    fn costs(&self, ids: &BTreeSet<String>) -> Result<BTreeMap<String, u64>> {
        ids.iter()
            .map(|id| {
                let a = self
                    .validated
                    .asset(id)
                    .ok_or_else(|| Diagnostic::new("E_ASSET", id, "missing asset"))?;
                let cost = match a.kind {
                    AssetKind::Image => {
                        (a.width as u64 * a.height as u64 * 8).saturating_add(a.bytes)
                    }
                    AssetKind::Audio => self
                        .audio_decode_sample_rate
                        .map_or(a.decoded_bytes, |rate| a.resampled_pcm_budget(rate))
                        .saturating_add(a.bytes),
                    AssetKind::Font => a.bytes * 4,
                };
                Ok((id.clone(), cost.max(1)))
            })
            .collect()
    }
    fn observe(&mut self, stage: &str, request: Option<u32>) {
        self.observe_from(stage, request, None, None, None);
    }
    fn observe_from(
        &mut self,
        stage: &str,
        request: Option<u32>,
        origin_session: Option<u32>,
        task: Option<u32>,
        sequence: Option<u32>,
    ) {
        self.commands.push(AppCommand::Observation {
            origin_session,
            task,
            sequence,
            stage: stage.into(),
            session: self.generation.session,
            device: self.generation.device,
            request,
            location: self.core.location(),
            cue: self.core.state().pending.as_ref().map(|p| p.cue.clone()),
        });
    }
    fn clear_fault_status(&mut self) {
        if self.error.as_ref() == Some(&self.status) {
            self.status.clear();
        }
    }
    fn clear_storage_warning(&mut self, location: &str) {
        if self.error.is_some()
            || !self.diagnostic.as_ref().is_some_and(|d| {
                d.details
                    .as_ref()
                    .is_some_and(|details| details.domain == ErrorDomain::Storage)
                    && d.location == location
            })
        {
            return;
        }
        let d = self.diagnostic.take().unwrap();
        let owns_status = self.status == self.messages.diagnostic(&d, &self.effective_ui_locale)
            || ["en", "zh-Hans"]
                .iter()
                .any(|locale| self.status == self.messages.diagnostic(&d, locale));
        if let Some(other) = self.storage_warning().cloned() {
            let status = self.status.clone();
            self.report(other, false);
            if !owns_status {
                self.status = status;
            }
        } else if owns_status {
            self.status.clear();
        }
    }
    fn persistence_failure(
        &mut self,
        kind: PersistenceKind,
        message: String,
        operation: &str,
        stage: &str,
    ) {
        let d = Diagnostic::new("E_STORAGE", kind.location(), message).classified(
            ErrorDomain::Storage,
            operation,
            stage,
            vec![Recovery::KeepCurrent],
        );
        let phase = if operation == "load_metadata" {
            PersistencePhase::Read
        } else {
            PersistencePhase::Write
        };
        self.persistence_failures.insert((kind, phase), d.clone());
        self.report(d, false);
    }
    fn storage_warning(&self) -> Option<&Diagnostic> {
        self.unconfirmed_saves
            .values()
            .next()
            .or_else(|| self.failed_saves.values().next())
            .or_else(|| self.persistence_failures.values().next())
    }
    fn restore_storage_warning(&mut self) {
        if self.error.is_some() || self.diagnostic.is_some() {
            return;
        }
        let Some(d) = self.storage_warning().cloned() else {
            return;
        };
        let owns_status = self.status.is_empty()
            || self
                .unconfirmed_saves
                .values()
                .chain(self.failed_saves.values())
                .chain(self.persistence_failures.values())
                .any(|d| {
                    self.status == self.messages.diagnostic(d, &self.effective_ui_locale)
                        || ["en", "zh-Hans"]
                            .iter()
                            .any(|locale| self.status == self.messages.diagnostic(d, locale))
                });
        let status = self.status.clone();
        self.report(d, false);
        if !owns_status {
            self.status = status;
        }
    }
    fn report(&mut self, mut d: Diagnostic, blocking: bool) {
        if blocking {
            self.set_interface_hidden(false);
        }
        if d.details.is_none() {
            d = d.classified(
                ErrorDomain::Core,
                "execute",
                "core",
                vec![Recovery::KeepCurrent, Recovery::Exit],
            );
        }
        let details = d.details.as_mut().unwrap();
        details.release = Some(self.release.clone());
        details.session.get_or_insert(self.generation.session);
        details.device = Some(self.generation.device);
        let message = self.messages.diagnostic(&d, &self.effective_ui_locale);
        self.status = message.clone();
        if blocking {
            self.error = Some(message);
        }
        if self.diagnostic.as_ref() != Some(&d) {
            self.commands.push(AppCommand::Diagnostic {
                diagnostic: Box::new(d.clone()),
            });
        }
        if blocking || self.error.is_none() {
            self.diagnostic = Some(d);
        }
    }
}

fn finite_clamp(v: f32, min: f32, max: f32, default: f32) -> f32 {
    if v.is_finite() {
        v.clamp(min, max)
    } else {
        default
    }
}
impl Player {
    pub fn viewport_changed(&mut self) -> Result<()> {
        self.generation.surface += 1;
        self.restart_preparation()?;
        if self.locale_pending() {
            self.start_locale_switch()?;
        }
        Ok(())
    }
}
