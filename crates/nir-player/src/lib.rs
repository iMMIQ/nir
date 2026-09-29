//! Single-owner application coordination. Host completions enter through pump.
#![forbid(unsafe_code)]
use nir_assets::{BudgetLedger, Generation, PrepareJob, Reservation};
use nir_core::*;
use nir_format::*;
use nir_presentation::{ChoiceView, DialogueView, Screen, SlotView, UiModel};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
mod clock;
mod menu;
use menu::{MenuSession, SaveConfirmation};
mod content;
mod effects;
use effects::{DeferredExitKind, MenuEffectsState};
mod pause;
use clock::ForegroundClockDemand;
pub use clock::ForegroundClockToken;
pub use content::ContentRequest;
use content::{ContentPreparation, ContentPurpose, RestoreWork};
pub use pause::PauseToken;
use pause::Pauses;

pub const EVENT_CAPACITY: usize = 256;
const INPUT_CAPACITY: usize = 128;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContentPriority {
    Required,
    Prefetch,
}
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PreparePriority {
    Required,
    Near,
    Speculative,
    Background,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveEnvelope {
    pub format: u32,
    pub slot: u32,
    pub revision: u32,
    pub snapshot: Snapshot,
    pub digest: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppCommand {
    GetContent {
        request: u32,
        session: u32,
        objects: Vec<ContentRequest>,
        priority: ContentPriority,
        #[serde(skip_serializing_if = "Option::is_none")]
        max_bytes: Option<u64>,
    },
    PromoteContent {
        request: u32,
        session: u32,
    },
    CancelContent {
        request: u32,
    },
    ResourceStage {
        stage: String,
        request: u32,
        asset: String,
        object: Option<String>,
        session: u32,
        device: u32,
        start_us: Micros,
        end_us: Micros,
        bytes: usize,
    },
    Observation {
        origin_session: Option<u32>,
        task: Option<u32>,
        sequence: Option<u32>,
        stage: String,
        session: u32,
        device: u32,
        request: Option<u32>,
        location: String,
        cue: Option<String>,
    },
    Diagnostic {
        diagnostic: Box<Diagnostic>,
    },
    GetAssets {
        request: u32,
        session: u32,
        device: u32,
        assets: Vec<String>,
        descriptors: BTreeMap<String, Asset>,
        priority: PreparePriority,
    },
    PromoteAssets {
        request: u32,
        session: u32,
    },
    CancelAssets {
        request: u32,
    },
    PreparePresentation {
        request: u32,
    },
    PrepareLocale {
        request: u32,
        ui_locale: String,
        text_locale: String,
    },
    AudioStart {
        domain: TimeDomain,
        task: u32,
        asset: String,
        bus: AudioBus,
        looped: bool,
        position_us: Micros,
        gain: f32,
        envelope: f32,
        session: u32,
    },
    AudioEnvelope {
        owner: Option<u32>,
        elapsed_us: Micros,
        session: u32,
        domain: TimeDomain,
        task: u32,
        from: f32,
        to: f32,
        duration_us: Micros,
    },
    AudioStop {
        session: u32,
        domain: TimeDomain,
        task: u32,
    },
    AudioPause {
        domain: TimeDomain,
        paused: bool,
    },
    AudioReset {
        domain: TimeDomain,
    },
    Save {
        slot: u32,
        expected_revision: u32,
        job: u32,
        envelope: Box<SaveEnvelope>,
    },
    Load {
        slot: u32,
        job: u32,
    },
    Export {
        json: String,
    },
    Import,
    ApplyPreferences {
        preferences: Preferences,
    },
    PersistPreferences {
        preferences: Preferences,
    },
    PersistProfile {
        keys: BTreeSet<String>,
    },
    ListSaves,
    Trace {
        event: String,
        at: String,
    },
}
#[derive(Debug, Clone)]
pub enum AppEvent {
    AudioPositions {
        domain: TimeDomain,
        session: u32,
        positions: Vec<AudioPosition>,
    },
    ContentReady {
        request: u32,
        objects: Vec<Vec<u8>>,
    },
    ContentFailed {
        request: u32,
        message: String,
    },
    ContentSkipped {
        request: u32,
        code: String,
        message: String,
    },
    Action {
        action: UiAction,
        interaction: u32,
        sequence: u32,
        session: u32,
    },
    Tick {
        delta_us: u64,
    },
    /// Host elapsed time after independently applying domain boundary policies.
    TickDomains {
        story_us: u64,
        foreground_us: u64,
    },
    /// Internal continuation: this elapsed time was already charged to the UI clock.
    #[doc(hidden)]
    ContinueStoryTime {
        delta_us: u64,
    },
    AssetReady {
        request: u32,
        asset: String,
    },
    AssetFailed {
        request: u32,
        message: String,
    },
    AssetFault {
        request: u32,
        diagnostic: Box<Diagnostic>,
    },
    AssetsCancelled {
        request: u32,
    },
    PresentationReady {
        request: u32,
    },
    LocaleReady {
        request: u32,
    },
    LocaleFailed {
        request: u32,
        message: String,
    },
    AudioEnded {
        domain: TimeDomain,
        task: u32,
        session: u32,
    },
    AudioFailed {
        domain: TimeDomain,
        task: u32,
        session: u32,
        message: String,
    },
    Hidden(bool),
    SlotLoaded {
        job: u32,
        envelope: Box<SaveEnvelope>,
    },
    SlotLoadFailed {
        job: u32,
        message: String,
    },
    Loaded {
        envelope: Box<SaveEnvelope>,
    },
    LoadFailed(String),
    HostFailed(String),
    Saved {
        job: u32,
        slot: u32,
        revision: u32,
    },
    SaveFault {
        job: u32,
        diagnostic: Box<Diagnostic>,
    },
    SaveFailed {
        job: u32,
        message: String,
    },
    Slots(Vec<SlotView>, BTreeMap<u32, u32>),
    Preferences(Preferences),
    Profile(BTreeSet<String>),
    DeviceLost,
    DeviceReady,
}
#[derive(Debug, Clone, Copy)]
enum Purpose {
    Boot,
    Menu,
    Activation,
    Restore,
    Rollback,
    Device,
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
    pub auto: bool,
    pub skip: bool,
    held_skip: bool,
    interface_hidden: bool,
    menu_peek: bool,
    pub profile: BTreeSet<String>,
    image_menu: String,
    menu_session: MenuSession,
    overlay_menu: Option<String>,
    prepared_menu: Option<(String, u32, u32, u32, u32)>,
    hovered_image: Option<String>,
    pub slots: Vec<SlotView>,
    pauses: Pauses,
    ui_pauses: Pauses,
    ui_clock_us: Micros,
    ui_clock_demand: ForegroundClockDemand,
    menu_effects: MenuEffectsState,
    menu_effects_clock: Option<ForegroundClockToken>,
    audio_paused: BTreeMap<TimeDomain, bool>,
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
    ledger: BudgetLedger,
    _surface_budget: Reservation,
    active: Option<Reservation>,
    request: u32,
    commands: Vec<AppCommand>,
    checkpoints: Vec<Snapshot>,
    slot_revisions: BTreeMap<u32, u32>,
    save_confirmation: Option<SaveConfirmation>,
    slot_restore: bool,
    slot_load: Option<(u32, u32, u32, u32)>,
    save_jobs: BTreeMap<u32, (u32, u32)>,
    auto_elapsed: u64,
    auto_wait_delay: Option<u64>,
    auto_anchor: Option<(u32, u32, u32)>,
    history_offset: usize,
}
impl Player {
    pub fn new(program: Program, release: String, title: String) -> Result<Self> {
        Self::from_validated(ValidatedProgram::new(program)?, release, title, None)
    }
    pub fn new_runtime(
        root: RuntimeProgram,
        release: String,
        title: String,
        preferences: Option<Preferences>,
    ) -> Result<Self> {
        Self::from_validated(
            ValidatedProgram::from_runtime(root)?,
            release,
            title,
            preferences,
        )
    }
    fn from_validated(
        validated: ValidatedProgram,
        release: String,
        title: String,
        initial: Option<Preferences>,
    ) -> Result<Self> {
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
        let locale = preferences.text_locale.clone();
        let ui_locale = preferences.ui_locale.clone();
        let core = Core::new(validated.clone(), release.clone(), locale.clone())?;
        let ledger = BudgetLedger::new(128 * 1024 * 1024);
        let _surface_budget = ledger.reserve(&BTreeMap::from([(
            "@render-surfaces".into(),
            validated.program().stage.width as u64 * validated.program().stage.height as u64 * 8
                + 8 * 1024 * 1024
                + 32 * 1024 * 1024,
        )]))?;
        let menu_session =
            MenuSession::new(core.program().theme.image_menus.get("title"), &preferences);
        let mut p = Self {
            messages: nir_presentation::Messages::default(),
            core,
            validated,
            title,
            release,
            screen: Screen::Title,
            return_screen: Screen::Title,
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
            auto: false,
            skip: false,
            held_skip: false,
            interface_hidden: false,
            menu_peek: false,
            profile: BTreeSet::new(),
            image_menu: "title".into(),
            menu_session,
            overlay_menu: None,
            prepared_menu: None,
            hovered_image: None,
            slots: (0..3)
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
            audio_paused: BTreeMap::from([
                (TimeDomain::Story, true),
                (TimeDomain::ForegroundUi, true),
            ]),
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
            ledger,
            _surface_budget,
            active: None,
            request: 0,
            commands: vec![AppCommand::ListSaves],
            checkpoints: vec![],
            slot_revisions: BTreeMap::new(),
            save_confirmation: None,
            slot_restore: false,
            slot_load: None,
            save_jobs: BTreeMap::new(),
            auto_elapsed: 0,
            auto_wait_delay: None,
            auto_anchor: None,
            history_offset: 0,
            content: BTreeMap::new(),
            content_leases: vec![],
            restore_work: None,
            prefetch_attempted: None,
        };
        let assets = p.title_assets();
        p.begin_prepare(Purpose::Boot, 0, assets)?;
        Ok(p)
    }
    pub fn acquire_pause(&self, reason: impl Into<String>) -> PauseToken {
        self.pauses.acquire(reason.into())
    }
    pub fn work_used(&self) -> u32 {
        self.work_used
    }
    pub fn pending_events(&self) -> usize {
        self.inbox.len()
    }
    pub fn core(&self) -> &Core {
        &self.core
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
                        Span::Break { .. } => text.push('\n'),
                        Span::Gate { .. } => {}
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
        self.accepts(request)
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
        self.pauses.insert("locale".into());
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
        }
    }
    pub fn needs_clock(&self) -> bool {
        self.needs_story_clock()
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
    pub fn domain_paused(&self, domain: TimeDomain) -> bool {
        match domain {
            TimeDomain::Story => self.paused(),
            TimeDomain::ForegroundUi => !self.ui_pauses.is_empty(),
        }
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
    fn reset_audio(&mut self) {
        for domain in [TimeDomain::Story, TimeDomain::ForegroundUi] {
            self.commands.push(AppCommand::AudioReset { domain });
        }
    }
    pub fn is_loading(&self) -> bool {
        self.prepare.is_some()
            || self.content.values().any(|p| {
                !p.failed && !matches!(p.purpose, ContentPurpose::Locale | ContentPurpose::Prefetch)
            })
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
        a.extend(self.core.program().theme.title_image_assets());
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
        if let Some(pending) = &s.pending {
            a.extend(self.validated.cue_assets(&pending.cue));
        }
        // Keep only the active overlay's media; hidden pages pin nothing. The
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
        a.extend(core.program().theme.dialogue.background.iter().cloned());
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
        if let Some(c) = &self.candidate {
            a.extend(self.state_assets(c));
        }
        if let Some(prep) = &self.prepare {
            a.extend(prep.assets.iter().cloned());
        }
        if let Some(spec) = &self.media_lookahead {
            a.extend(spec.assets.iter().cloned());
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
                    AssetKind::Audio => a.decoded_bytes.saturating_add(a.bytes),
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
    fn begin_prepare(
        &mut self,
        purpose: Purpose,
        activation: u32,
        assets: BTreeSet<String>,
    ) -> Result<()> {
        self.failed_admission = None;
        match self.admit_prepare(purpose, activation, assets.clone()) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.cancel_preparation();
                self.failed_admission = Some((purpose, activation, assets));
                self.pauses.insert("prepare".into());
                Err(error.classified(
                    ErrorDomain::Prepare,
                    "prepare",
                    "admission",
                    vec![Recovery::Retry, Recovery::KeepCurrent, Recovery::Exit],
                ))
            }
        }
    }
    fn admit_prepare(
        &mut self,
        purpose: Purpose,
        activation: u32,
        mut assets: BTreeSet<String>,
    ) -> Result<()> {
        if self.deferred_prepare.take().is_some() {
            self.pauses.remove("prepare");
        }
        // Old and candidate resources are admitted together; never pin half a cue.
        assets.extend(if !self.story_context_active() {
            self.title_assets()
        } else {
            self.state_assets(&self.core)
        });
        let needs = self.asset_content_requirements(&assets)?;
        if !needs.is_empty() {
            return self.begin_content(
                ContentPurpose::Media {
                    purpose,
                    activation,
                    assets,
                },
                needs,
            );
        }
        self.request = self
            .request
            .checked_add(1)
            .ok_or_else(|| Diagnostic::new("E_LIMIT", "request", "counter"))?;
        let costs = self.costs(&assets)?;
        // Stop/cancel intents precede this preparation in the owner's command
        // queue. Only current scene/tasks and the candidate need to stay pinned;
        // a cancelled voice/BGM must not occupy the previous active lease.
        if let Some(active) = &mut self.active {
            active.retain(&assets);
        }
        let job = match PrepareJob::new(activation, self.generation, costs.clone(), &self.ledger) {
            Ok(job) => job,
            Err(_) if self.media_lookahead.is_some() || !self.media_retired.is_empty() => {
                self.cancel_media_lookahead();
                self.deferred_prepare = Some((self.generation, purpose, activation, assets));
                self.pauses.insert("prepare".into());
                self.observe("prepare_waiting_for_media_cancel", None);
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let mut job = job;
        let promoted = matches!(purpose, Purpose::Activation)
            && self.media_lookahead.as_ref().is_some_and(|s| {
                s.job.generation == self.generation && s.assets.is_subset(&assets)
            });
        let request = self.request;
        let speculative_assets = self
            .media_lookahead
            .as_ref()
            .map(|s| s.assets.clone())
            .unwrap_or_default();
        let promoted_from = if promoted {
            let spec = self.media_lookahead.take().unwrap();
            for ready in spec.assets.difference(&spec.job.missing) {
                job.ready(ready, self.generation);
            }
            self.commands.push(AppCommand::PromoteAssets {
                request: spec.request,
                session: self.generation.session,
            });
            Some(spec.request)
        } else {
            self.cancel_media_lookahead();
            None
        };
        let required_fetch: BTreeSet<_> = if promoted_from.is_some() {
            assets.difference(&speculative_assets).cloned().collect()
        } else {
            assets.clone()
        };
        self.cancel_preparation();
        self.prepare = Some(Preparation {
            request,
            promoted_from,
            assets: assets.clone(),
            purpose,
            job,
            preflight: false,
            failed: false,
        });
        self.pauses.insert("prepare".into());
        self.observe("prepare_requested", Some(request));
        if !required_fetch.is_empty() {
            self.commands.push(AppCommand::GetAssets {
                request,
                session: self.generation.session,
                device: self.generation.device,
                descriptors: self.describe_assets(&required_fetch)?,
                assets: required_fetch.into_iter().collect(),
                priority: PreparePriority::Required,
            });
        }
        if self
            .prepare
            .as_ref()
            .is_some_and(|p| p.job.missing.is_empty())
        {
            self.prepare.as_mut().unwrap().preflight = true;
            self.commands
                .push(AppCommand::PreparePresentation { request });
        }
        Ok(())
    }
    fn cancel_preparation(&mut self) {
        self.failed_admission = None;
        if let Some(p) = self.prepare.take() {
            if !p.failed {
                self.observe("prepare_cancelled", Some(p.request));
                self.commands
                    .push(AppCommand::CancelAssets { request: p.request });
                if let Some(old) = p.promoted_from {
                    self.commands
                        .push(AppCommand::CancelAssets { request: old });
                }
            }
        }
    }
    fn cancel_media_lookahead(&mut self) {
        if let Some(spec) = self.media_lookahead.take() {
            self.commands.push(AppCommand::CancelAssets {
                request: spec.request,
            });
            self.observe("media_lookahead_cancelled", Some(spec.request));
            self.media_retired.insert(spec.request, spec.job);
        }
    }
    fn maybe_prefetch_media(&mut self) {
        let eligible = self.screen == Screen::Story
            && self.story_context_active()
            && !self.paused()
            && self.prepare.is_none()
            && self.candidate.is_none()
            && self.restore_work.is_none()
            && self.locale_candidate.is_none()
            && self.preferences.ui_locale == self.effective_ui_locale
            && self.preferences.text_locale == self.effective_text_locale
            && !self.content.values().any(|job| {
                !matches!(
                    job.purpose,
                    ContentPurpose::Locale | ContentPurpose::Prefetch
                )
            })
            && self.validated.program().player.prefetch_media;
        let cue = if eligible {
            self.core.predict_next_cue()
        } else {
            None
        };
        let fingerprint = cue.as_ref().map(|cue| {
            format!(
                "{}:{}:{}:{}:{}:{}",
                self.generation.session,
                self.generation.device,
                self.effective_text_locale,
                self.core.location(),
                self.core
                    .state()
                    .waiting
                    .as_ref()
                    .and_then(|w| serde_json::to_string(w).ok())
                    .unwrap_or_default(),
                cue
            )
        });
        if self
            .media_lookahead
            .as_ref()
            .is_some_and(|s| Some(&s.fingerprint) != fingerprint.as_ref())
        {
            self.cancel_media_lookahead();
        }
        if fingerprint
            .as_ref()
            .is_some_and(|f| self.media_attempted.as_ref() != Some(f))
        {
            self.media_attempted = None;
        }
        let Some(fingerprint) = fingerprint else {
            return;
        };
        if self.media_attempted.as_ref() == Some(&fingerprint)
            || self.media_lookahead.is_some()
            || !self.media_retired.is_empty()
        {
            return;
        }
        self.media_attempted = Some(fingerprint.clone());
        let cue = cue.unwrap();
        let recipe = self.validated.cue_assets(&cue);
        if recipe.iter().any(|id| self.validated.asset(id).is_none()) {
            return;
        }
        let resident = self.retained_assets();
        let assets: BTreeSet<_> = recipe
            .into_iter()
            .filter(|id| {
                !resident.contains(id)
                    && self
                        .validated
                        .asset(id)
                        .is_some_and(|a| matches!(a.kind, AssetKind::Image | AssetKind::Audio))
            })
            .collect();
        if assets.is_empty() {
            return;
        }
        // Only the incremental cost of media absent from the active group is
        // speculative. The ledger still jointly accounts for shared assets.
        let costs = match self.costs(&assets) {
            Ok(c) => c,
            Err(_) => return,
        };
        let incremental: u64 = costs
            .values()
            .fold(0u64, |total, cost| total.saturating_add(*cost));
        if incremental > 32 * 1024 * 1024 {
            return;
        }
        let Some(request) = self.request.checked_add(1) else {
            return;
        };
        let Ok(job) = PrepareJob::new(0, self.generation, costs, &self.ledger) else {
            return;
        };
        self.request = request;
        let descriptors = match self.describe_assets(&assets) {
            Ok(d) => d,
            Err(_) => return,
        };
        self.media_lookahead = Some(MediaLookahead {
            request,
            fingerprint,
            assets: assets.clone(),
            job,
        });
        self.commands.push(AppCommand::GetAssets {
            request,
            session: self.generation.session,
            device: self.generation.device,
            assets: assets.into_iter().collect(),
            descriptors,
            priority: PreparePriority::Near,
        });
        self.observe("media_lookahead_requested", Some(request));
    }
    fn step(&mut self, input: CoreInput, budget: &mut u32) -> Result<()> {
        self.core.set_text_speed(self.preferences.text_speed)?;
        let before_location = self.core.location();
        let output = self.core.step(input, *budget);
        *budget -= output.work_used;
        if before_location != output.location {
            self.touch_snapshot_content(self.core.state())?;
        }
        if output.remaining_time_us > 0 {
            self.inbox.push_front((
                self.generation.session,
                AppEvent::ContinueStoryTime {
                    delta_us: output.remaining_time_us,
                },
            ));
        }
        for intent in output.intents {
            match intent {
                CoreIntent::PrepareContent { module, locale } => {
                    let needs = self.content_requirements(&module, Some(&locale), true)?;
                    // Independent media completions may wake the VM while its
                    // PC is at the same barrier. They do not restart a download
                    // or implicitly retry a failed preparation.
                    if !self.content.values().any(|p| {
                        !matches!(p.purpose, ContentPurpose::Locale | ContentPurpose::Prefetch)
                    }) {
                        self.begin_content(ContentPurpose::Execution, needs)?;
                    }
                }
                CoreIntent::Prepare { activation, cue } => self.begin_prepare(
                    Purpose::Activation,
                    activation,
                    self.validated.cue_assets(&cue),
                )?,
                CoreIntent::AudioStart {
                    task,
                    asset,
                    bus,
                    looped,
                    position_us,
                    gain,
                } => self.commands.push(AppCommand::AudioStart {
                    domain: TimeDomain::Story,
                    task,
                    asset,
                    bus,
                    looped,
                    position_us,
                    gain,
                    envelope: 1.,
                    session: self.generation.session,
                }),
                CoreIntent::AudioEnvelope {
                    owner,
                    elapsed_us,
                    task,
                    from,
                    to,
                    duration_us,
                } => {
                    self.commands.push(AppCommand::AudioEnvelope {
                        owner,
                        elapsed_us,
                        domain: TimeDomain::Story,
                        session: self.generation.session,
                        task,
                        from,
                        to,
                        duration_us,
                    });
                }
                CoreIntent::AudioStop { task } => self.commands.push(AppCommand::AudioStop {
                    domain: TimeDomain::Story,
                    session: self.generation.session,
                    task,
                }),
                CoreIntent::ProfileMerge { key } => {
                    if self.profile.insert(key) {
                        self.commands.push(AppCommand::PersistProfile {
                            keys: self.profile.clone(),
                        });
                    }
                }
                CoreIntent::Checkpoint => {
                    self.checkpoints.push(self.core.snapshot());
                    // Bound checkpoint storage independently of count. The ledger
                    // reserves 32 MiB for at most 16 MiB of serialized snapshots.
                    while self.checkpoints.len() > 32
                        || self
                            .checkpoints
                            .iter()
                            .map(|s| serde_json::to_vec(s).unwrap().len())
                            .sum::<usize>()
                            > 16 * 1024 * 1024
                    {
                        self.checkpoints.remove(0);
                    }
                }
                CoreIntent::Trace { event, at } => {
                    self.commands.push(AppCommand::Trace { event, at })
                }
            }
        }
        if let Some(e) = &self.core.state().fault {
            self.report(e.clone(), true);
            self.pauses.insert("fault".into());
        }
        if self.core.state().fault.is_some()
            || (!self.menu_peek
                && (self.core.state().choice.is_some() || self.core.dialogue().is_none()))
        {
            self.set_interface_hidden(false);
        }
        if self.core.state().outcome.is_some() {
            if self.core.program().theme.return_to_title {
                let root = self
                    .core
                    .state()
                    .frames
                    .first()
                    .map(|frame| frame.function.as_str());
                let menu = self.core.program().theme.image_menus.iter().find_map(|(id, menu)|
                    menu.controls().any(|(_,action,_)| matches!(action, nir_format::ImageMenuAction::Entry { function } if Some(function.as_str()) == root)).then(|| id.clone()))
                    .unwrap_or_else(|| "title".into());
                self.action(UiAction::Title, 0, 0, budget)?;
                self.image_menu = menu;
            } else {
                self.screen = Screen::Ended;
            }
            self.auto = false;
            self.skip = false;
            self.held_skip = false;
        }
        Ok(())
    }
    fn cancel_prefetch_content(&mut self) -> bool {
        let requests: Vec<_> = self
            .content
            .iter()
            .filter(|(_, job)| matches!(job.purpose, ContentPurpose::Prefetch))
            .map(|(request, _)| *request)
            .collect();
        for request in &requests {
            self.content.remove(request);
            self.commands
                .push(AppCommand::CancelContent { request: *request });
        }
        !requests.is_empty()
    }
    fn maybe_prefetch_content(&mut self) {
        let eligible = self.screen == Screen::Story
            && self.story_context_active()
            && !self.paused()
            && self.prepare.is_none()
            && self.candidate.is_none()
            && self.restore_work.is_none()
            && self.validated.program().player.prefetch_content
            && !self.content.values().any(|job| {
                !matches!(
                    job.purpose,
                    ContentPurpose::Locale | ContentPurpose::Prefetch
                )
            });
        if !eligible {
            if self.cancel_prefetch_content() {
                self.prefetch_attempted = None;
            }
            return;
        }
        let Some(module) = self.core.prefetch_module() else {
            if self.cancel_prefetch_content() {
                self.prefetch_attempted = None;
            }
            self.prefetch_attempted = None;
            return;
        };
        let state = self.core.state();
        let Some(frame) = state.frames.last() else {
            self.prefetch_attempted = None;
            return;
        };
        let wait = state
            .waiting
            .as_ref()
            .and_then(|waiting| serde_json::to_string(waiting).ok())
            .unwrap_or_default();
        let attempt = PrefetchAttempt {
            wait: format!("{}:{wait}", self.core.location()),
            function: frame.function.clone(),
            module,
            locale: self.effective_text_locale.clone(),
            session: self.generation.session,
        };
        if self.prefetch_attempted.as_ref() != Some(&attempt) {
            self.cancel_prefetch_content();
            self.prefetch_attempted = None;
        }
        if self.prefetch_attempted.as_ref() == Some(&attempt) {
            return;
        }
        if self
            .content
            .values()
            .any(|job| matches!(job.purpose, ContentPurpose::Prefetch))
        {
            self.prefetch_attempted = Some(attempt);
            return;
        }
        let objects = match self.content_requirements(&attempt.module, Some(&attempt.locale), true)
        {
            Ok(objects) => objects,
            Err(_) => {
                self.prefetch_attempted = Some(attempt);
                return;
            }
        };
        if objects.is_empty() {
            self.prefetch_attempted = Some(attempt);
            return;
        }
        if self
            .begin_content(ContentPurpose::Prefetch, objects)
            .is_err()
        {
            // Speculation must not affect the running story on admission or
            // request-limit failures.
            self.prefetch_attempted = Some(attempt);
            return;
        }
        if self
            .content
            .values()
            .any(|job| matches!(job.purpose, ContentPurpose::Prefetch))
        {
            self.prefetch_attempted = Some(attempt);
        }
    }
    pub fn pump(&mut self, events: Vec<AppEvent>, budget: u32) -> Vec<AppCommand> {
        // Admission leaves room for resource, audio and storage terminal events.
        for event in events {
            let input = matches!(
                event,
                AppEvent::Action { .. }
                    | AppEvent::Tick { .. }
                    | AppEvent::TickDomains { .. }
                    | AppEvent::ContinueStoryTime { .. }
            );
            let limit = if input {
                INPUT_CAPACITY
            } else {
                EVENT_CAPACITY
            };
            if self.inbox.len() >= limit {
                self.report(
                    Diagnostic::new("E_EVENT_QUEUE", "pump", "event admission limit").classified(
                        ErrorDomain::Host,
                        "admit",
                        "queue",
                        vec![Recovery::Exit],
                    ),
                    true,
                );
                self.pauses.insert("queue-overflow".into());
                self.ui_pauses.insert("queue-overflow".into());
                continue;
            }
            self.inbox.push_back((self.generation.session, event));
        }
        // Stable partition across retained work, so a choice beats same-turn time.
        self.inbox.make_contiguous().sort_by_key(|(_, e)| {
            matches!(
                e,
                AppEvent::Tick { .. }
                    | AppEvent::TickDomains { .. }
                    | AppEvent::ContinueStoryTime { .. }
            )
        });
        let mut remaining = budget.min(100_000);
        let limit = remaining;
        while remaining > 0 {
            let Some((session, event)) = self.inbox.pop_front() else {
                break;
            };
            remaining -= 1;
            if matches!(
                event,
                AppEvent::Tick { .. }
                    | AppEvent::TickDomains { .. }
                    | AppEvent::ContinueStoryTime { .. }
            ) && session != self.generation.session
            {
                self.observe("stale_tick_discarded", None);
                continue;
            }
            if let Err(e) = self
                .event(event, &mut remaining)
                .and_then(|_| self.sync_menu_state())
            {
                self.report(e, true);
            }
        }
        if self.prepare.is_none() && self.screen == Screen::Story && self.pauses.is_empty() {
            if let Err(e) = self.step(CoreInput::None, &mut remaining) {
                self.report(e, true);
            }
        }
        // Advance finite UI effect fades, then commit any page that became
        // prepared. Runs after the event loop so prepare completions inside
        // this turn are visible without waiting for the next host frame.
        if let Err(e) = self.update_menu_effects(&mut remaining) {
            self.report(e, true);
        }
        if let Err(e) = self.sync_menu_state() {
            self.report(e, true);
        }
        if let Err(e) = self.prepare_active_menu() {
            self.report(e, true);
        }
        self.maybe_prefetch_content();
        self.maybe_prefetch_media();
        if let Err(error) = self.refresh_content_lease() {
            self.report(error, true);
        }
        self.work_used = limit - remaining;
        for domain in [TimeDomain::Story, TimeDomain::ForegroundUi] {
            let after = self.domain_paused(domain);
            if self.audio_paused.get(&domain) != Some(&after) {
                self.audio_paused.insert(domain, after);
                self.commands.push(AppCommand::AudioPause {
                    domain,
                    paused: after,
                });
            }
        }
        std::mem::take(&mut self.commands)
    }
    fn event(&mut self, e: AppEvent, budget: &mut u32) -> Result<()> {
        if let AppEvent::ContentReady { request, objects } = e {
            if let Err(error) = self.complete_content(request, objects) {
                self.fail_content(request, error.to_string());
            }
            return Ok(());
        }
        if let AppEvent::ContentFailed { request, message } = e {
            self.fail_content(request, message);
            return Ok(());
        }
        if let AppEvent::ContentSkipped {
            request,
            code,
            message,
        } = e
        {
            if self.accepts_content(request) {
                let job = self.content.remove(&request).unwrap();
                self.commands.push(AppCommand::CancelContent { request });
                if matches!(job.purpose, ContentPurpose::Prefetch) {
                    self.observe("prefetch_skipped", Some(request));
                } else {
                    // A prefetch can be promoted after the host's speculative
                    // size preflight has already rejected it. Retry the same
                    // exact objects under a fresh required request envelope.
                    self.observe("promoted_prefetch_skipped", Some(request));
                    if let Err(error) = self.begin_content(job.purpose, job.objects) {
                        self.pauses.insert("content".into());
                        self.report(
                            Diagnostic::new(
                                "E_MODULE_PREPARE",
                                "content",
                                format!("{code}: {message}; retry failed: {error}"),
                            ),
                            true,
                        );
                    }
                }
            }
            return Ok(());
        }
        match e {
            AppEvent::ContentReady { .. }
            | AppEvent::ContentFailed { .. }
            | AppEvent::ContentSkipped { .. } => unreachable!(),
            AppEvent::Action {
                action,
                interaction,
                sequence,
                session,
            } => {
                if session == self.generation.session {
                    self.observe_from(
                        "input_dispatched",
                        None,
                        Some(session),
                        None,
                        Some(sequence),
                    );
                    self.action(action, interaction, sequence, budget)?;
                } else {
                    self.observe_from(
                        "stale_input_discarded",
                        None,
                        Some(session),
                        None,
                        Some(sequence),
                    );
                }
            }
            AppEvent::Tick { delta_us } => {
                self.event(
                    AppEvent::TickDomains {
                        story_us: delta_us,
                        foreground_us: delta_us,
                    },
                    budget,
                )?;
            }
            AppEvent::TickDomains {
                story_us,
                foreground_us,
            } => {
                if !self.domain_paused(TimeDomain::ForegroundUi) {
                    self.ui_clock_us.0 =
                        self.ui_clock_us
                            .0
                            .checked_add(foreground_us)
                            .ok_or_else(|| {
                                Diagnostic::new("E_TIME", "clock", "foreground clock overflow")
                            })?;
                }
                self.advance_story_time(story_us, budget)?;
            }
            AppEvent::ContinueStoryTime { delta_us } => {
                self.advance_story_time(delta_us, budget)?;
            }
            AppEvent::AssetReady { request, asset } => {
                if !self.accepts_resource(request) {
                    self.observe("stale_asset_discarded", Some(request));
                }
                if let Some(spec) = self
                    .media_lookahead
                    .as_mut()
                    .filter(|s| s.request == request)
                {
                    spec.job.ready(&asset, self.generation);
                }
                if let Some(p) = self.prepare.as_mut().filter(|p| {
                    (p.request == request || p.promoted_from == Some(request)) && !p.failed
                }) {
                    p.job.ready(&asset, self.generation);
                    if p.job.missing.is_empty() && !p.preflight {
                        p.preflight = true;
                        self.commands
                            .push(AppCommand::PreparePresentation { request: p.request });
                    }
                }
                if let (Some(candidate), Some(job)) =
                    (self.locale_candidate.as_mut(), self.locale_job.as_mut())
                {
                    if candidate.request == request && self.locale_error.is_none() {
                        job.ready(&asset, self.generation);
                        if job.missing.is_empty() && !candidate.preflight {
                            candidate.preflight = true;
                            self.commands.push(AppCommand::PrepareLocale {
                                request,
                                ui_locale: candidate.ui_locale.clone(),
                                text_locale: candidate.text_locale.clone(),
                            });
                        }
                    }
                }
            }
            AppEvent::AssetsCancelled { request } => {
                self.media_retired.remove(&request);
                if self.media_retired.is_empty() {
                    if let Some((generation, purpose, activation, assets)) =
                        self.deferred_prepare.take()
                    {
                        self.pauses.remove("prepare");
                        if generation == self.generation {
                            self.begin_prepare(purpose, activation, assets)?;
                        }
                    }
                    if let Some(request) = self.deferred_locale.take() {
                        if self
                            .locale_candidate
                            .as_ref()
                            .is_some_and(|c| c.request == request)
                        {
                            self.start_locale_switch()?;
                        }
                    }
                }
            }
            AppEvent::AssetFailed { request, message } => {
                if self
                    .media_lookahead
                    .as_ref()
                    .is_some_and(|s| s.request == request)
                {
                    self.cancel_media_lookahead();
                    return Ok(());
                }
                if self.accepts_locale(request) {
                    self.locale_failed(request, message);
                } else {
                    self.asset_fault(
                        request,
                        Diagnostic::new("E_PREPARE", self.core.location(), message),
                    );
                }
            }
            AppEvent::AssetFault {
                request,
                diagnostic,
            } => {
                if self
                    .media_lookahead
                    .as_ref()
                    .is_some_and(|s| s.request == request)
                {
                    self.cancel_media_lookahead();
                    return Ok(());
                }
                if self.accepts_locale(request) {
                    self.locale_failed(request, diagnostic.to_string());
                } else {
                    self.asset_fault(request, *diagnostic);
                }
            }
            AppEvent::PresentationReady { request } => self.complete(request, budget)?,
            AppEvent::LocaleReady { request } => {
                if let Some(candidate) = self
                    .locale_candidate
                    .clone()
                    .filter(|c| c.request == request && c.preflight && self.locale_error.is_none())
                {
                    let lease = self.locale_job.take().and_then(|job| job.finish().ok());
                    if lease
                        .as_ref()
                        .is_some_and(|lease| lease.valid(request, self.generation))
                    {
                        self.core.set_locale(&candidate.text_locale)?;
                        self.effective_ui_locale = candidate.ui_locale;
                        self.effective_text_locale = candidate.text_locale;
                        self.locale_candidate = None;
                        self.locale_error = None;
                        self.pauses.remove("locale");
                        self.prefetch_attempted = None;
                        self.touch_snapshot_content(self.core.state())?;
                        self.restore_locale_changed()?;
                        self.status = self
                            .messages
                            .text(&self.effective_ui_locale, "language-applied");
                    } else {
                        self.locale_failed(request, "E_STALE_LEASE".into());
                    }
                }
            }
            AppEvent::LocaleFailed { request, message } => {
                self.locale_failed(request, message);
            }
            AppEvent::AudioEnded {
                domain,
                task,
                session,
            } => {
                if domain == TimeDomain::Story && session == self.generation.session {
                    self.step(CoreInput::AudioEnded { task }, budget)?;
                } else if domain == TimeDomain::ForegroundUi
                    && session == self.generation.session
                    && self.menu_effects.sounds.remove(&task).is_some()
                {
                    self.observe("ui_sound_ended", None);
                }
            }
            AppEvent::AudioFailed {
                domain,
                task,
                session,
                message,
            } => {
                if domain == TimeDomain::Story && session == self.generation.session {
                    let mut d = Diagnostic::new("E_AUDIO", self.core.location(), &message)
                        .classified(
                            ErrorDomain::Host,
                            "audio",
                            "playback",
                            vec![Recovery::KeepCurrent],
                        );
                    d.details.as_mut().unwrap().task = Some(task);
                    self.report(d, false);
                    self.step(CoreInput::TaskFailed { task, message }, budget)?;
                } else if domain == TimeDomain::ForegroundUi
                    && session == self.generation.session
                    && self.menu_effects.sounds.remove(&task).is_some()
                {
                    // Menu page effect voices are best effort; a failed one
                    // never faults the session or cancels the transition.
                    self.observe_from(
                        "ui_sound_failed",
                        None,
                        Some(session),
                        Some(task),
                        None,
                    );
                } else {
                    self.observe_from(
                        "stale_audio_discarded",
                        None,
                        Some(session),
                        Some(task),
                        None,
                    );
                }
            }
            AppEvent::AudioPositions {
                domain,
                session,
                positions,
            } => {
                self.observe_audio_positions(domain, session, &positions)?;
            }
            AppEvent::Hidden(hidden) => {
                if hidden {
                    self.held_skip = false;
                    self.pauses.insert("hidden".into());
                    self.ui_pauses.insert("hidden".into());
                } else {
                    self.pauses.remove("hidden");
                    self.ui_pauses.remove("hidden");
                }
            }
            AppEvent::SlotLoaded { job, envelope } => {
                if let Some(slot) = self.accept_slot_load(job) {
                    self.slot_restore = true;
                    if envelope.slot != slot {
                        return Err(Diagnostic::new(
                            "E_SAVE_SLOT",
                            "load",
                            "slot does not match request",
                        ));
                    }
                    self.event(AppEvent::Loaded { envelope }, budget)?;
                }
            }
            AppEvent::SlotLoadFailed { job, message } => {
                if self.accept_slot_load(job).is_some() {
                    self.event(AppEvent::LoadFailed(message), budget)?;
                }
            }
            AppEvent::Loaded { envelope } => {
                if envelope.format != 1
                    || nir_content::digest(&serde_json::to_vec(&envelope.snapshot).unwrap())
                        != envelope.digest
                {
                    return Err(Diagnostic::new(
                        "E_SAVE_DIGEST",
                        "load",
                        "snapshot checksum",
                    ));
                }
                self.restore(envelope.snapshot).map_err(|d| {
                    d.classified(
                        ErrorDomain::Storage,
                        "load",
                        "validate",
                        vec![Recovery::KeepCurrent],
                    )
                })?;
            }
            AppEvent::HostFailed(m) => self.report(
                Diagnostic::new("E_HOST", "dispatch", m).classified(
                    ErrorDomain::Host,
                    "dispatch",
                    "owner",
                    vec![Recovery::Reload],
                ),
                false,
            ),
            AppEvent::LoadFailed(m) => self.report(
                Diagnostic::new("E_STORAGE", "load", m).classified(
                    ErrorDomain::Storage,
                    "load",
                    "transaction",
                    vec![Recovery::Retry, Recovery::KeepCurrent],
                ),
                false,
            ),
            AppEvent::Saved {
                job,
                slot,
                revision,
            } => {
                if self
                    .save_jobs
                    .remove(&job)
                    .is_some_and(|(saved_slot, _)| saved_slot == slot)
                {
                    self.slot_revisions.insert(slot, revision);
                    self.status = if self.effective_ui_locale == "en" {
                        "Saved in this browser"
                    } else {
                        "浏览器已保存"
                    }
                    .into();
                    self.commands.push(AppCommand::ListSaves);
                }
            }
            AppEvent::SaveFailed { job, message } => {
                self.save_fault(job, Diagnostic::new("E_STORAGE", "save", message))
            }
            AppEvent::SaveFault { job, diagnostic } => self.save_fault(job, *diagnostic),
            AppEvent::Slots(slots, revisions) => {
                for slot in 0..3 {
                    let revision = revisions.get(&slot).copied().unwrap_or(0);
                    if revision < self.slot_revisions.get(&slot).copied().unwrap_or(0) {
                        continue;
                    }
                    let row = slots
                        .iter()
                        .find(|row| row.slot == slot)
                        .cloned()
                        .unwrap_or(SlotView {
                            slot,
                            ..Default::default()
                        });
                    if let Some(old) = self.slots.iter_mut().find(|row| row.slot == slot) {
                        *old = row;
                    }
                    self.slot_revisions.insert(slot, revision);
                }
            }
            AppEvent::Preferences(mut p) => {
                let locale_config = &self.core.program().locale_config;
                if !locale_config.ui.contains_key(&p.ui_locale) {
                    p.ui_locale = locale_config.default_ui.clone();
                }
                if !locale_config.text.contains_key(&p.text_locale) {
                    p.text_locale = locale_config.default_text.clone();
                }
                p.text_speed = finite_clamp(p.text_speed, 0.25, 4., 1.);
                p.auto_wait_scale = finite_clamp(p.auto_wait_scale, 0.25, 4., 1.);
                p.font_scale = finite_clamp(p.font_scale, 0.8, 1.5, 1.);
                p.bgm_volume = finite_clamp(p.bgm_volume, 0., 1., 0.3);
                p.voice_volume = finite_clamp(p.voice_volume, 0., 1., 0.8);
                p.sfx_volume = finite_clamp(p.sfx_volume, 0., 1., 0.5);
                self.preferences = p;
                self.commands.push(AppCommand::ApplyPreferences {
                    preferences: self.preferences.clone(),
                });
                self.start_locale_switch()?;
            }
            AppEvent::Profile(keys) => self.profile.extend(keys),
            AppEvent::DeviceLost => {
                self.observe("device_lost", None);
                if self.locale_pending() {
                    self.invalidate_locale_candidate();
                    self.pauses.remove("locale");
                }
                self.generation.device += 1;
                self.device_resume = self.prepare.as_ref().map(|p| p.purpose);
                self.cancel_preparation();
                self.pauses.insert("device".into());
                self.ui_pauses.insert("device".into());
                self.commands.push(AppCommand::AudioPause {
                    domain: TimeDomain::Story,
                    paused: true,
                });
            }
            AppEvent::DeviceReady => {
                self.observe("device_ready", None);
                if let Some(work) = &self.restore_work {
                    if let Some(candidate) = &self.candidate {
                        let purpose = if work.rollback {
                            Purpose::Rollback
                        } else {
                            Purpose::Restore
                        };
                        let assets = self.state_assets(candidate);
                        self.begin_prepare(purpose, 0, assets)?;
                    } else if self.content.values().any(|job| {
                        matches!(
                            job.purpose,
                            ContentPurpose::RestoreValidation | ContentPurpose::RestoreBodies(_)
                        )
                    }) {
                        // Keep the old device pause until staged restore has a
                        // complete candidate whose media can be prepared.
                    } else {
                        self.resume_restore_work()?;
                    }
                    if self.locale_error.is_none()
                        && (self.preferences.ui_locale != self.effective_ui_locale
                            || self.preferences.text_locale != self.effective_text_locale)
                    {
                        self.start_locale_switch()?;
                    }
                    return Ok(());
                }
                let purpose = match self.device_resume.take() {
                    Some(Purpose::Restore) => Purpose::Restore,
                    Some(Purpose::Rollback) => Purpose::Rollback,
                    _ => Purpose::Device,
                };
                self.begin_prepare(purpose, 0, self.retained_assets())?;
                if self.locale_error.is_none()
                    && (self.preferences.ui_locale != self.effective_ui_locale
                        || self.preferences.text_locale != self.effective_text_locale)
                {
                    self.start_locale_switch()?;
                }
            }
        }
        Ok(())
    }
    fn save_fault(&mut self, job: u32, d: Diagnostic) {
        let Some((_, session)) = self.save_jobs.remove(&job) else {
            self.observe("stale_save_failure_discarded", Some(job));
            return;
        };
        let mut d = d.classified(
            ErrorDomain::Storage,
            "save",
            "transaction",
            vec![Recovery::KeepCurrent],
        );
        d.details.as_mut().unwrap().request = Some(job);
        d.details.as_mut().unwrap().session = Some(session);
        self.report(d, false);
    }
    fn asset_fault(&mut self, request: u32, mut d: Diagnostic) {
        let request = self
            .prepare
            .as_ref()
            .and_then(|p| (p.promoted_from == Some(request)).then_some(p.request))
            .unwrap_or(request);
        if !self.accepts(request) {
            self.observe("stale_failure_discarded", Some(request));
            return;
        }
        self.prepare.as_mut().unwrap().failed = true;
        self.commands.push(AppCommand::CancelAssets { request });
        if let Some(old) = self.prepare.as_ref().and_then(|p| p.promoted_from) {
            self.commands
                .push(AppCommand::CancelAssets { request: old });
        }
        if d.details.is_none() {
            d = d.classified(
                ErrorDomain::Prepare,
                "prepare",
                "resource",
                vec![Recovery::Retry, Recovery::KeepCurrent, Recovery::Exit],
            );
        }
        d.details.as_mut().unwrap().request = Some(request);
        let message = self.messages.diagnostic(&d, &self.effective_ui_locale);
        self.report(d, true);
        self.observe("prepare_failed", Some(request));
        if let Some(p) = self.core.state().pending.as_ref().filter(|_| {
            self.prepare
                .as_ref()
                .is_some_and(|p| matches!(p.purpose, Purpose::Activation))
        }) {
            self.core.step(
                CoreInput::PreparationFailed {
                    activation: p.id,
                    message,
                },
                0,
            );
        }
    }
    fn complete(&mut self, request: u32, budget: &mut u32) -> Result<()> {
        if !self.accepts(request) {
            self.observe("stale_presentation_discarded", Some(request));
            return Ok(());
        }
        let prep = self.prepare.take().unwrap();
        if let Some(old) = prep.promoted_from {
            self.commands
                .push(AppCommand::CancelAssets { request: old });
        }
        let purpose = prep.purpose;
        let restart_locale =
            matches!(purpose, Purpose::Restore | Purpose::Rollback) && self.locale_pending();
        let lease = prep.job.finish()?;
        if !lease.valid(lease.activation, self.generation) {
            return Err(Diagnostic::new(
                "E_STALE_LEASE",
                "commit",
                "generation changed",
            ));
        }
        if self.screen == Screen::Menu {
            if let Some(id) = self.active_menu_id() {
                if self.core.program().theme.image_menus[id]
                    .prepared_assets()
                    .is_subset(&prep.assets)
                {
                    self.prepared_menu = Some(self.menu_asset_stamp(id));
                }
            }
        }
        self.observe("lease_ready", Some(request));
        self.pauses.remove("prepare");
        self.pauses.remove("device");
        self.ui_pauses.remove("device");
        self.error = None;
        self.diagnostic = None;
        let commit_location = self.core.location();
        let commit_cue = self.core.state().pending.as_ref().map(|p| p.cue.clone());
        let commit_generation = self.generation;
        self.observe("commit_started", Some(request));
        match purpose {
            Purpose::Boot | Purpose::Menu => {}
            Purpose::Activation => self.step(
                CoreInput::Prepared {
                    activation: lease.activation,
                },
                budget,
            )?,
            Purpose::Restore | Purpose::Rollback => {
                let mut candidate = self
                    .candidate
                    .take()
                    .ok_or_else(|| Diagnostic::new("E_RESTORE", "commit", "no candidate"))?;
                candidate.set_locale(&self.effective_text_locale)?;
                self.generation.session += 1;
                self.set_interface_hidden(false);
                self.held_skip = false;
                self.failed_admission = None;
                self.reset_audio();
                self.slot_restore = false;
                self.core = candidate;
                self.restore_work = None;
                self.touch_snapshot_content(self.core.state())?;
                self.screen = Screen::Story;
                self.return_screen = Screen::Story;
                self.pauses.remove("menu");
                self.pauses.remove("fault");
                self.pauses.insert("restored".into());
                if matches!(purpose, Purpose::Rollback) {
                    self.checkpoints.pop();
                } else {
                    self.checkpoints.clear();
                    self.checkpoints.push(self.core.snapshot());
                }
                self.auto = false;
                self.skip = false;
                self.held_skip = false;
                self.restart_audio();
            }
            Purpose::Device => {
                self.pauses.remove("device");
                self.ui_pauses.remove("device");
                self.pauses.insert("restored".into());
                self.restart_audio();
            }
        }
        let assets = self.retained_assets();
        let active = self.ledger.reserve(&self.costs(&assets)?)?;
        self.active = Some(active);
        drop(lease);
        self.commands.push(AppCommand::Observation {
            origin_session: None,
            task: None,
            sequence: None,
            stage: "prepare_commit".into(),
            request: Some(request),
            location: commit_location,
            cue: commit_cue,
            session: commit_generation.session,
            device: commit_generation.device,
        });
        if restart_locale {
            self.start_locale_switch()?;
        }
        Ok(())
    }
    fn restart_audio(&mut self) {
        for t in self
            .core
            .state()
            .tasks
            .values()
            .filter(|t| t.state == TaskState::Running)
        {
            if let Effect::Audio {
                asset,
                bus,
                looped,
                gain,
            } = &t.effect
            {
                self.commands.push(AppCommand::AudioStart {
                    domain: TimeDomain::Story,
                    task: t.id,
                    asset: asset.clone(),
                    bus: *bus,
                    looped: *looped,
                    gain: *gain,
                    envelope: self.core.audio_envelope(t.id).0,
                    position_us: t.audio_position_us.unwrap_or(t.elapsed_us),
                    session: self.generation.session,
                });
                let (from, to, duration_us) = self.core.audio_envelope(t.id);
                let (owner, elapsed_us) = self.core.audio_envelope_checkpoint(t.id);
                self.commands.push(AppCommand::AudioEnvelope {
                    owner,
                    elapsed_us,
                    domain: TimeDomain::Story,
                    session: self.generation.session,
                    task: t.id,
                    from,
                    to,
                    duration_us,
                });
            }
        }
        self.commands.push(AppCommand::AudioPause {
            domain: TimeDomain::Story,
            paused: true,
        });
    }
    fn cancel_slot_restore(&mut self) {
        if !std::mem::take(&mut self.slot_restore) {
            return;
        }
        let owns_error = self.diagnostic.as_ref().is_some_and(|d| {
            d.location == "load"
                || d.details.as_ref().is_some_and(|details| {
                    details.operation == "load"
                        || details.request.is_some_and(|request| {
                            self.prepare.as_ref().is_some_and(|p| {
                                p.request == request && matches!(p.purpose, Purpose::Restore)
                            }) || self.content.get(&request).is_some_and(|p| {
                                matches!(
                                    p.purpose,
                                    ContentPurpose::Restore(_, false)
                                        | ContentPurpose::RestoreValidation
                                        | ContentPurpose::RestoreBodies(false)
                                        | ContentPurpose::Media {
                                            purpose: Purpose::Restore,
                                            ..
                                        }
                                )
                            })
                        })
                        || (details.operation == "prepare"
                            && details.stage == "admission"
                            && self
                                .failed_admission
                                .as_ref()
                                .is_some_and(|(purpose, _, _)| matches!(purpose, Purpose::Restore)))
                })
        });
        if owns_error {
            self.error = None;
            self.diagnostic = None;
            self.status.clear();
        }
        // Invalid envelopes have not acquired a candidate lane: leave any
        // pre-existing Story or menu preparation intact in that case.
        if self.content.values().any(|p| {
            matches!(
                p.purpose,
                ContentPurpose::Restore(_, false)
                    | ContentPurpose::RestoreValidation
                    | ContentPurpose::RestoreBodies(false)
                    | ContentPurpose::Media {
                        purpose: Purpose::Restore,
                        ..
                    }
            )
        }) {
            self.cancel_content(false);
        }
        let owns_prepare = self
            .prepare
            .as_ref()
            .is_some_and(|p| matches!(p.purpose, Purpose::Restore))
            || self
                .failed_admission
                .as_ref()
                .is_some_and(|(p, _, _)| matches!(p, Purpose::Restore));
        let owns_deferred = self
            .deferred_prepare
            .as_ref()
            .is_some_and(|(_, p, _, _)| matches!(p, Purpose::Restore));
        if owns_prepare {
            self.cancel_preparation();
        }
        if owns_deferred {
            self.deferred_prepare = None;
        }
        if owns_prepare || owns_deferred {
            self.pauses.remove("prepare");
        }
        self.candidate = None;
        self.restore_work = None;
        if matches!(self.device_resume, Some(Purpose::Restore)) {
            self.device_resume = None;
        }
    }
    fn accept_slot_load(&mut self, job: u32) -> Option<u32> {
        let (expected, slot, session, instance) = self.slot_load?;
        if expected != job {
            return None;
        }
        self.slot_load = None;
        if session != self.generation.session || instance != self.menu_session.instance {
            self.observe("stale_slot_load_discarded", Some(job));
            return None;
        }
        Some(slot)
    }
    fn restore(&mut self, s: Snapshot) -> Result<()> {
        self.restore_with_purpose(s, false)
    }
    fn restore_with_purpose(&mut self, s: Snapshot, rollback: bool) -> Result<()> {
        if self.validated.runtime_root().is_some() {
            return self.start_staged_restore(s, rollback);
        }
        self.cancel_content(false);
        let needs = self.restore_content_requirements(&s)?;
        if !needs.is_empty() {
            return self.begin_content(ContentPurpose::Restore(Box::new(s), rollback), needs);
        }
        let mut candidate = Core::restore(self.validated.clone(), s, &self.release)?;
        // Preferences are independent of saves. Frozen current instances retain
        // their saved language; future instances use the current preference.
        candidate.set_locale(&self.effective_text_locale)?;
        let assets = self.state_assets(&candidate);
        self.begin_prepare(
            if rollback {
                Purpose::Rollback
            } else {
                Purpose::Restore
            },
            0,
            assets,
        )?;
        self.candidate = Some(candidate);
        Ok(())
    }
    fn action(
        &mut self,
        a: UiAction,
        interaction: u32,
        sequence: u32,
        budget: &mut u32,
    ) -> Result<()> {
        // A close fade locks the old page's input for its finite duration.
        if self.menu_effects.closing.is_some() {
            return Ok(());
        }
        self.sync_menu_state()?;
        let a = match a {
            UiAction::ConfirmSave { token } => {
                let Some(c) = self.save_confirmation.as_ref().filter(|c| c.token == token) else {
                    return Ok(());
                };
                if self.is_loading() || self.slot_load.is_some() {
                    return Ok(());
                }
                let slot = c.slot;
                self.save_confirmation = None;
                UiAction::Save { slot }
            }
            UiAction::CancelSave { token } => {
                if self
                    .save_confirmation
                    .as_ref()
                    .is_some_and(|c| c.token == token)
                {
                    self.save_confirmation = None;
                }
                return Ok(());
            }
            UiAction::Close if self.save_confirmation.is_some() => {
                self.save_confirmation = None;
                return Ok(());
            }
            UiAction::Title | UiAction::Load { .. } if self.save_confirmation.is_some() => {
                self.save_confirmation = None;
                a
            }
            _ if self.save_confirmation.is_some() => return Ok(()),
            _ => a,
        };
        let a = if let UiAction::MenuValue {
            instance,
            revision,
            control,
            value,
        } = a
        {
            let Some(action) = self.resolve_menu_value(instance, revision, &control, value)? else {
                return Ok(());
            };
            action
        } else {
            a
        };
        let resolved_menu = matches!(&a, UiAction::MenuControl { .. });
        let a = if let UiAction::MenuControl {
            instance,
            revision,
            control,
        } = a
        {
            let Some(action) =
                self.resolve_menu_control(instance, revision, &control, interaction, sequence)?
            else {
                return Ok(());
            };
            action
        } else {
            a
        };
        if self.menu_peek && matches!(a, UiAction::Close) {
            self.set_interface_hidden(false);
            return Ok(());
        }
        let cancelled_slot_restore = self.slot_restore;
        if matches!(
            &a,
            UiAction::Close
                | UiAction::Title
                | UiAction::NewGame
                | UiAction::ImageMenuEntry { .. }
                | UiAction::ImageMenu { .. }
                | UiAction::Menu
                | UiAction::Settings
                | UiAction::Saves
                | UiAction::History
                | UiAction::Load { .. }
                | UiAction::Import
                | UiAction::Rollback
        ) {
            self.cancel_slot_restore();
            self.slot_load = None;
        }
        if self.interface_hidden
            && matches!(
                a,
                UiAction::Advance
                    | UiAction::Continue
                    | UiAction::Menu
                    | UiAction::ToggleAuto
                    | UiAction::ToggleSkip
                    | UiAction::HoldSkip { pressed: true }
                    | UiAction::ToggleInterface
                    | UiAction::RestoreInterface
            )
        {
            self.set_interface_hidden(false);
            return Ok(());
        }
        match a {
            UiAction::MenuValue { .. }
            | UiAction::MenuControl { .. }
            | UiAction::ConfirmSave { .. }
            | UiAction::CancelSave { .. } => unreachable!("resolved above"),
            UiAction::ToggleInterface => {
                if self.screen == Screen::Story
                    && !self.paused()
                    && !self.is_loading()
                    && self.core.state().choice.is_none()
                    && self.core.dialogue().is_some()
                {
                    self.set_interface_hidden(true);
                }
            }
            UiAction::RestoreInterface => {
                self.set_interface_hidden(false);
            }
            UiAction::ImageMenu { menu } => {
                let allowed = self.active_menu_id().and_then(|id|self.core.program().theme.image_menus.get(id)).is_some_and(|current|
                    current.controls().any(|(id,action,requires)| (resolved_menu || (self.screen==Screen::Title && !current.uses_state() && !current.uses_services() && current.buttons.iter().any(|b|b.id==id))) && matches!(action, nir_format::ImageMenuAction::Menu { menu: target } if target == &menu)
                        && requires.is_none_or(|key| self.profile.contains(key))));
                if matches!(self.screen, Screen::Title | Screen::Menu)
                    && allowed
                    && self.core.program().theme.image_menus.contains_key(&menu)
                {
                    if self.core.program().theme.image_menus[&menu].uses_storage() {
                        self.commands.push(AppCommand::ListSaves);
                    }
                    if self.screen == Screen::Title {
                        self.image_menu = menu;
                    } else {
                        self.cancel_menu_preparation();
                        self.overlay_menu = Some(menu);
                    }
                    self.menu_session.menu.clear();
                    self.hovered_image = None;
                }
            }
            UiAction::HoverImage { id } => {
                self.hovered_image = id.filter(|id| {
                    self.active_menu_id()
                        .and_then(|menu| self.core.program().theme.image_menus.get(menu))
                        .is_some_and(|menu| menu.controls().any(|(control, _, _)| control == id))
                });
            }
            UiAction::NewGame | UiAction::ImageMenuEntry { .. } => {
                let function = match &a {
                    UiAction::ImageMenuEntry { function } => {
                        let allowed = self.screen == Screen::Title && self.core.program().theme.image_menus
                            .get(&self.image_menu).is_some_and(|menu| menu.controls().any(|(id,action,requires)|
                                (resolved_menu || (!menu.uses_state() && !menu.uses_services() && menu.buttons.iter().any(|b|b.id==id))) && matches!(action, nir_format::ImageMenuAction::Entry { function: target } if target == function)
                                && requires.is_none_or(|key| self.profile.contains(key))));
                        if !allowed {
                            return Ok(());
                        }
                        function.clone()
                    }
                    _ => {
                        self.image_menu = "title".into();
                        self.core.program().entry.clone()
                    }
                };
                if self.prepare.is_some() {
                    return Ok(());
                }
                let restart_locale = self.locale_pending();
                self.cancel_content(false);
                self.restore_work = None;
                self.candidate = None;
                self.prefetch_attempted = None;
                self.generation.session += 1;
                self.set_interface_hidden(false);
                self.held_skip = false;
                self.failed_admission = None;
                self.reset_audio();
                self.core = Core::new_at(
                    self.validated.clone(),
                    self.release.clone(),
                    self.effective_text_locale.clone(),
                    &function,
                )?;
                self.screen = Screen::Story;
                self.return_screen = Screen::Story;
                self.pauses.retain(|r| r == "hidden");
                self.ui_pauses.retain(|r| r == "hidden");
                self.checkpoints.clear();
                self.error = None;
                self.diagnostic = None;
                self.status.clear();
                if restart_locale {
                    self.start_locale_switch()?;
                }
                // Freeze the first dialogue only after a pending language
                // transaction commits its effective context for this session.
                if !self.locale_pending() {
                    self.step(CoreInput::None, budget)?;
                }
            }
            UiAction::MenuHistoryScroll { .. } => {}
            UiAction::Scroll {
                region: ScrollRegion::Settings,
                ..
            } => {}
            UiAction::Scroll { .. } => {
                if interaction != self.current_interaction() {
                    return Ok(());
                }
                // Browsing revealed text takes control back from automatic reading.
                self.auto = false;
                self.skip = false;
                self.held_skip = false;
                self.auto_elapsed = 0;
                self.auto_wait_delay = None;
            }
            UiAction::Advance => {
                if self.screen == Screen::Story && !self.paused() {
                    self.auto_elapsed = 0;
                    self.auto_wait_delay = None;
                    self.step(
                        CoreInput::Advance {
                            interaction,
                            sequence,
                        },
                        budget,
                    )?;
                }
            }
            UiAction::Choose { option } => {
                if self.screen == Screen::Story && !self.paused() {
                    self.skip = false;
                    self.held_skip = false;
                    self.step(
                        CoreInput::Choose {
                            interaction,
                            option,
                            sequence,
                        },
                        budget,
                    )?;
                }
            }
            UiAction::Continue => {
                self.pauses.remove("restored");
                if self.prepare.is_none() {
                    if let Some(pending) = &self.core.state().pending {
                        self.begin_prepare(
                            Purpose::Activation,
                            pending.id,
                            self.validated.cue_assets(&pending.cue),
                        )?;
                    }
                }
                self.screen = Screen::Story;
            }
            UiAction::Menu | UiAction::Settings | UiAction::History | UiAction::Saves => {
                if matches!(a, UiAction::Menu) && self.screen != Screen::Menu {
                    self.overlay_menu = None;
                    // Force a fresh overlay when reopening from the story.
                    if self.screen != Screen::Title {
                        self.menu_session.menu.clear();
                    }
                }
                if matches!(self.screen, Screen::Title | Screen::Story | Screen::Ended) {
                    self.return_screen = self.screen;
                }
                self.held_skip = false;
                self.set_interface_hidden(false);
                self.screen = match a {
                    UiAction::Settings => Screen::Settings,
                    UiAction::History => Screen::History,
                    UiAction::Saves => Screen::Saves,
                    _ => Screen::Menu,
                };
                self.pauses.insert("menu".into());
                if self.screen == Screen::Saves
                    || self
                        .active_menu_id()
                        .and_then(|id| self.core.program().theme.image_menus.get(id))
                        .is_some_and(ImageMenu::uses_storage)
                {
                    self.commands.push(AppCommand::ListSaves);
                }
            }
            UiAction::Close => {
                if self.pop_menu()? {
                    return Ok(());
                }
                if self.begin_menu_close(
                    DeferredExitKind::CloseScreen {
                        cancelled_slot_restore,
                    },
                    interaction,
                    sequence,
                ) {
                    return Ok(());
                }
                self.commit_close(cancelled_slot_restore)?;
            }
            UiAction::Title => {
                self.image_menu = "title".into();
                self.hovered_image = None;
                let restart_locale = self.locale_pending();
                self.cancel_content(false);
                self.reset_audio();
                self.cancel_preparation();
                self.candidate = None;
                self.restore_work = None;
                self.prefetch_attempted = None;
                self.device_resume = None;
                self.pauses.retain(|r| r == "hidden");
                self.ui_pauses.retain(|r| r == "hidden");
                self.screen = Screen::Title;
                self.return_screen = Screen::Title;
                self.generation.session += 1;
                self.set_interface_hidden(false);
                self.held_skip = false;
                self.error = None;
                self.diagnostic = None;
                self.status.clear();
                self.begin_prepare(Purpose::Boot, 0, self.title_assets())?;
                if restart_locale {
                    self.start_locale_switch()?;
                }
            }
            UiAction::ToggleAuto => {
                self.auto = !self.auto;
                self.skip = false;
                self.held_skip = false;
                self.auto_elapsed = 0;
                self.auto_wait_delay = None;
                if self.auto
                    && self.core.dialogue().is_some_and(|(_, d)| {
                        d.reading
                            .as_ref()
                            .is_some_and(|r| r.wait == VoiceWaitPolicy::SampledRemaining)
                    })
                {
                    self.read_policy(0, budget)?;
                }
            }
            UiAction::HoldSkip { pressed } => {
                self.held_skip = pressed
                    && self.screen == Screen::Story
                    && !self.paused()
                    && self.core.state().choice.is_none();
                if self.held_skip {
                    self.auto = false;
                }
            }
            UiAction::ToggleSkip => {
                self.held_skip = false;
                self.skip = !self.skip;
                self.auto = false;
            }
            UiAction::UiLocale { locale } => {
                if !self.core.program().locale_config.ui.contains_key(&locale) {
                    return Err(Diagnostic::new(
                        "E_LOCALE",
                        "ui_locale",
                        "unsupported UI locale",
                    ));
                }
                self.preferences.ui_locale = locale;
                self.start_locale_switch()?;
                self.persist_preferences();
            }
            UiAction::TextLocale { locale } => {
                if !self.core.program().locale_config.text.contains_key(&locale) {
                    return Err(Diagnostic::new(
                        "E_LOCALE",
                        "text_locale",
                        "unsupported text locale",
                    ));
                }
                self.preferences.text_locale = locale;
                self.start_locale_switch()?;
                self.persist_preferences();
            }
            UiAction::LocaleRetry => {
                self.start_locale_switch()?;
            }
            UiAction::LocaleCancel => {
                self.cancel_locale_switch();
            }
            UiAction::FontSize { delta } => {
                self.preferences.font_scale = (self.preferences.font_scale + delta).clamp(0.8, 1.5);
                self.generation.typography += 1;
                self.persist_preferences();
                self.restart_preparation()?;
                if self.locale_pending() {
                    self.start_locale_switch()?;
                }
            }
            UiAction::TextSpeed { delta } => {
                self.preferences.text_speed =
                    finite_clamp(self.preferences.text_speed + delta, 0.25, 4., 1.);
                self.persist_preferences();
            }
            UiAction::AutoWait { delta } => {
                self.preferences.auto_wait_scale =
                    finite_clamp(self.preferences.auto_wait_scale + delta, 0.25, 4., 1.);
                self.persist_preferences();
            }
            UiAction::Volume { bus, delta } => {
                let v = match bus {
                    AudioBus::Bgm => &mut self.preferences.bgm_volume,
                    AudioBus::Voice => &mut self.preferences.voice_volume,
                    AudioBus::Sfx => &mut self.preferences.sfx_volume,
                };
                *v = (*v + delta).clamp(0., 1.);
                self.persist_preferences();
            }
            UiAction::ReducedMotion => {
                self.preferences.reduced_motion = !self.preferences.reduced_motion;
                self.persist_preferences();
            }
            UiAction::Save { slot } => {
                if self.return_screen == Screen::Title
                    || slot > 2
                    || self
                        .save_jobs
                        .values()
                        .any(|(saved_slot, _)| *saved_slot == slot)
                {
                    return Ok(());
                }
                let s = self.core.snapshot();
                let bytes = serde_json::to_vec(&s).unwrap();
                if bytes.len() > MAX_INPUT_BYTES - 1024 {
                    return Err(Diagnostic::new(
                        "E_SAVE_LIMIT",
                        "save",
                        "snapshot exceeds import limit",
                    ));
                }
                let revision = self.slot_revisions.get(&slot).copied().unwrap_or(0);
                let next_revision = revision.checked_add(1).ok_or_else(|| {
                    Diagnostic::new("E_SAVE_LIMIT", "save", "slot revision exhausted")
                })?;
                self.request = self.request.checked_add(1).ok_or_else(|| {
                    Diagnostic::new("E_REQUEST_LIMIT", "save", "request identity exhausted")
                })?;
                let job = self.request;
                self.save_jobs.insert(job, (slot, self.generation.session));
                let envelope = SaveEnvelope {
                    format: 1,
                    slot,
                    revision: next_revision,
                    digest: nir_content::digest(&bytes),
                    snapshot: s,
                };
                self.commands.push(AppCommand::Save {
                    slot,
                    expected_revision: revision,
                    job,
                    envelope: Box::new(envelope),
                });
                self.status = if self.effective_ui_locale == "en" {
                    "Saving…"
                } else {
                    "正在保存…"
                }
                .into();
            }
            UiAction::Load { slot } => {
                if slot > 2 {
                    return Ok(());
                }
                self.request = self.request.checked_add(1).ok_or_else(|| {
                    Diagnostic::new("E_REQUEST_LIMIT", "load", "request identity exhausted")
                })?;
                let job = self.request;
                self.slot_load = Some((
                    job,
                    slot,
                    self.generation.session,
                    self.menu_session.instance,
                ));
                self.commands.push(AppCommand::Load { slot, job });
            }
            UiAction::Export => {
                let snapshot = self.core.snapshot();
                let envelope = SaveEnvelope {
                    format: 1,
                    slot: 0,
                    revision: 0,
                    digest: nir_content::digest(&serde_json::to_vec(&snapshot).unwrap()),
                    snapshot,
                };
                self.commands.push(AppCommand::Export {
                    json: serde_json::to_string(&envelope).unwrap(),
                });
            }
            UiAction::Import => self.commands.push(AppCommand::Import),
            UiAction::Rollback => {
                if self.checkpoints.len() > 1 {
                    let s = self.checkpoints[self.checkpoints.len() - 2].clone();
                    self.restore_with_purpose(s, true)?;
                }
            }
            UiAction::HistoryPage { delta } => {
                self.history_offset = (self.history_offset as i64 + delta as i64)
                    .clamp(0, self.core.state().history.len().saturating_sub(1) as i64)
                    as usize;
            }
            UiAction::Retry => {
                if let Some(job) = self
                    .content
                    .values()
                    .find(|p| p.failed && !matches!(p.purpose, ContentPurpose::Locale))
                    .cloned()
                {
                    self.begin_content(job.purpose, job.objects)?;
                } else if self.candidate.is_some() {
                    let rollback = self.restore_work.as_ref().is_some_and(|work| work.rollback);
                    let purpose = if rollback {
                        Purpose::Rollback
                    } else {
                        Purpose::Restore
                    };
                    let assets = self
                        .candidate
                        .as_ref()
                        .map(|candidate| self.state_assets(candidate))
                        .unwrap_or_default();
                    self.begin_prepare(purpose, 0, assets)?;
                } else {
                    self.restart_preparation()?;
                }
            }
        }
        self.sync_menu_state()?;
        Ok(())
    }
    fn persist_preferences(&mut self) {
        self.commands.push(AppCommand::PersistPreferences {
            preferences: self.preferences.clone(),
        });
    }
    fn restart_preparation(&mut self) -> Result<()> {
        if let Some((purpose, activation, assets)) = self.failed_admission.clone() {
            return self.begin_prepare(purpose, activation, assets);
        }
        if let Some(prep) = self.prepare.as_ref() {
            let (purpose, activation) = (prep.purpose, prep.job.activation);
            let assets = self.retained_assets();
            self.begin_prepare(purpose, activation, assets)?;
        }
        Ok(())
    }
    fn advance_story_time(&mut self, delta_us: u64, budget: &mut u32) -> Result<()> {
        if self.needs_story_clock() {
            let before = self.core.state().tick_us.0;
            self.step(CoreInput::Time { delta_us }, budget)?;
            self.read_policy(self.core.state().tick_us.0 - before, budget)?;
        }
        Ok(())
    }
    fn read_policy(&mut self, delta: u64, budget: &mut u32) -> Result<()> {
        if self.interface_hidden || self.paused() || self.core.state().choice.is_some() {
            self.skip = false;
            self.held_skip = false;
            return Ok(());
        }
        let Some((_, d)) = self.core.dialogue() else {
            return Ok(());
        };
        let anchor = (
            self.generation.session,
            d.interaction,
            d.reading.as_ref().map_or(0, |r| r.revision),
        );
        if self.auto_anchor != Some(anchor) {
            self.auto_elapsed = 0;
            self.auto_wait_delay = None;
            self.auto_anchor = Some(anchor);
        }
        let read = self
            .profile
            .contains(&format!("read:{}:{}", d.text_id, d.meaning_revision));
        if (self.skip || self.held_skip) && !read {
            self.skip = false;
            self.held_skip = false;
        }
        if (self.skip || self.held_skip) && read && !d.at_gate {
            let token = d.interaction;
            let sequence = self.core.state().last_input.saturating_add(1);
            self.step(
                CoreInput::Advance {
                    interaction: token,
                    sequence,
                },
                budget,
            )?;
        } else if self.auto && d.awaiting_advance {
            let voice = if let Some(reading) = &d.reading {
                reading.voice.is_some_and(|id| {
                    self.core
                        .state()
                        .tasks
                        .get(&id)
                        .is_some_and(|t| t.state == TaskState::Running)
                })
            } else {
                self.core.state().tasks.values().any(|t| {
                    t.state == TaskState::Running
                        && matches!(
                            t.effect,
                            Effect::Audio {
                                bus: AudioBus::Voice,
                                ..
                            }
                        )
                })
            };
            let sampled = d
                .reading
                .as_ref()
                .is_some_and(|r| r.wait == VoiceWaitPolicy::SampledRemaining);
            if voice
                && d.reading
                    .as_ref()
                    .is_some_and(|r| r.wait == VoiceWaitPolicy::AfterVoice)
            {
                self.auto_elapsed = 0;
                self.auto_wait_delay = None;
            } else {
                let starting = self.auto_wait_delay.is_none();
                self.auto_wait_delay.get_or_insert_with(|| {
                    let base = self.core.program().player.auto_delay(
                        d.full_text().chars().count(),
                        self.preferences.auto_wait_scale,
                    );
                    let remaining = if sampled && self.preferences.voice_volume > 0. {
                        d.reading
                            .as_ref()
                            .and_then(|r| r.voice)
                            .and_then(|id| self.core.state().tasks.get(&id))
                            .filter(|t| t.state == TaskState::Running)
                            .and_then(|t| {
                                if let Effect::Audio { asset, .. } = &t.effect {
                                    self.core.program().asset(asset).map(|a| {
                                        a.duration_us.0.saturating_sub(
                                            t.audio_position_us.unwrap_or(t.elapsed_us).0,
                                        )
                                    })
                                } else {
                                    None
                                }
                            })
                            .unwrap_or(0)
                    } else {
                        0
                    };
                    base.saturating_add(remaining)
                });
                // The sample is taken at the current boundary, so do not
                // charge time from before that boundary to a new sampled timer.
                if !sampled || !starting {
                    self.auto_elapsed = self.auto_elapsed.saturating_add(delta);
                }
            }
            if (!voice || sampled)
                && self
                    .auto_wait_delay
                    .is_some_and(|delay| self.auto_elapsed >= delay)
            {
                let token = d.interaction;
                self.auto_elapsed = 0;
                self.auto_wait_delay = None;
                self.step(
                    CoreInput::Advance {
                        interaction: token,
                        sequence: self.core.state().last_input.saturating_add(1),
                    },
                    budget,
                )?;
            }
        } else {
            self.auto_elapsed = 0;
            self.auto_wait_delay = None;
        }
        Ok(())
    }
    pub fn menu_depth(&self) -> usize {
        self.menu_session.depth()
    }
    pub fn model(&self) -> UiModel {
        self.model_for(&self.core, self.screen)
    }
    fn model_for(&self, c: &Core, screen: Screen) -> UiModel {
        self.model_for_locale(c, screen, &self.effective_ui_locale)
    }
    fn model_for_locale(&self, c: &Core, screen: Screen, ui_locale: &str) -> UiModel {
        let screen = if self.menu_peek && screen == Screen::Menu {
            Screen::Story
        } else {
            screen
        };
        let ui_plan = &c.program().locale_config.ui[ui_locale];
        let text_plan = &c.program().locale_config.text[&self.effective_text_locale];
        // Menus opened from the title use its retained background, even when
        // Core still contains the previous story or a restore is preparing.
        let title_context = screen == Screen::Title
            || (self.return_screen == Screen::Title
                && matches!(
                    screen,
                    Screen::Menu | Screen::Settings | Screen::Saves | Screen::History
                ));
        UiModel {
            transition_style: c.transition_style(),
            image_menu: self.active_menu_id().unwrap_or(&self.image_menu).to_owned(),
            authored_menu: self.active_menu_id().is_some()
                && !(screen == Screen::Menu && self.error.is_some()),
            menu_instance: self.menu_session.instance,
            menu_revision: self.menu_session.revision,
            menu_depth: self.menu_session.depth(),
            menu_locals: self.menu_session.locals.clone(),
            hovered_image: self.hovered_image.clone(),
            profile: self.profile.clone(),
            title: self.title.clone(),
            screen,
            nodes: if title_context {
                self.title_nodes()
            } else {
                c.sample_scene()
            },
            transition: if title_context || self.preferences.reduced_motion {
                None
            } else {
                c.transition().map(|(n, p)| (n.to_vec(), p))
            },
            stage: [
                c.program().stage.width as f32,
                c.program().stage.height as f32,
            ],
            dialogue_appearance: c.sample_dialogue_appearance(),
            interface_hidden: self.interface_hidden && screen == Screen::Story,
            hidden_dialogue: c.state().dialogue_hidden && c.dialogue().is_some(),
            dialogue: c
                .dialogue()
                .filter(|_| !c.state().dialogue_hidden)
                .map(|(_, d)| DialogueView {
                    full_text: d.full_text(),
                    visible_text: d.visible_text(),
                    speaker: d.speaker.clone(),
                    ready: d.awaiting_advance,
                    gate: d.at_gate,
                    locale: d.locale.clone(),
                    font_plan_digest: d.font_plan_digest.clone(),
                    font_assets: c.program().locale_config.text[&d.locale].fonts.clone(),
                    emphasis: {
                        let mut offset = 0;
                        d.spans
                            .iter()
                            .filter_map(|s| {
                                let start = offset;
                                offset += s.text.len();
                                s.emphasis.then_some((start, offset))
                            })
                            .collect()
                    },
                }),
            choices: c
                .state()
                .choice
                .as_ref()
                .map(|c| {
                    c.options
                        .iter()
                        .map(|o| ChoiceView {
                            id: o.id.clone(),
                            label: o.label.clone(),
                            enabled: o.enabled,
                            locale: c.locale.clone(),
                            font_plan_digest: c.font_plan_digest.clone(),
                            font_assets: self.core.program().locale_config.text[&c.locale]
                                .fonts
                                .clone(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            prefs: self.preferences.clone(),
            ui_locale: ui_locale.into(),
            ui_fonts: ui_plan.fonts.clone(),
            ui_font_plan_digest: ui_plan.digest.clone(),
            text_locale: self.effective_text_locale.clone(),
            available_ui_locales: ["zh-Hans", "en"]
                .into_iter()
                .filter(|locale| c.program().locale_config.ui.contains_key(*locale))
                .map(str::to_owned)
                .collect(),
            available_text_locales: ["zh-Hans", "en", "ja"]
                .into_iter()
                .filter(|locale| c.program().locale_config.text.contains_key(*locale))
                .map(str::to_owned)
                .collect(),
            text_fonts: text_plan.fonts.clone(),
            text_font_plan_digest: text_plan.digest.clone(),
            locale_pending: self.locale_pending(),
            locale_error: self.locale_error.clone(),
            preflight_texts: vec![],
            theme: (*c.program().theme).clone(),
            history: if screen == Screen::History {
                Self::history_rows(c, self.history_offset, 3)
                    .into_iter()
                    .map(|row| row.entry)
                    .collect()
            } else {
                vec![]
            },
            history_total: c.state().history.len(),
            menu_history: self.menu_history_model(c, screen),
            menu_history_flow: self.menu_history_flow_model(screen),
            menu_opacity: self
                .menu_effects
                .opacity(self.ui_clock_us.0, self.preferences.reduced_motion),
            slots: self.slots.clone(),
            save_confirmation: self.save_confirmation.as_ref().map(|c| (c.token, c.slot)),
            busy_slots: self.save_jobs.values().map(|(slot, _)| *slot).collect(),
            can_save: self.screen != Screen::Title
                && self.return_screen != Screen::Title
                && self.slot_load.is_none(),
            menu_reading_modes: self.menu_reading_modes(),
            menu_story: self.menu_story_values(),
            paused: self.paused(),
            loading: self.is_loading(),
            status: self.status.clone(),
            fault: self.error.clone(),
            fault_recovery: self
                .diagnostic
                .as_ref()
                .and_then(|d| d.details.as_ref())
                .map(|d| d.recovery.clone())
                .unwrap_or_default(),
            auto: self.auto,
            skip: self.skip || self.held_skip,
            outcome: c.state().outcome.clone(),
            history_offset: self.history_offset,
        }
    }
    pub fn preview(&self) -> UiModel {
        let mut model = self.preview_inner();
        // Prepare the visible state too: restoring a transient mask must not
        // expose text that was skipped by resource/glyph preparation.
        model.interface_hidden = false;
        model
    }
    fn preview_inner(&self) -> UiModel {
        if let Some(p) = &self.prepare {
            match p.purpose {
                Purpose::Activation => {
                    let mut c = self.core.clone();
                    c.step(
                        CoreInput::Prepared {
                            activation: p.job.activation,
                        },
                        0,
                    );
                    return self.model_for(&c, Screen::Story);
                }
                Purpose::Restore | Purpose::Rollback => {
                    if let Some(c) = &self.candidate {
                        return self.model_for(c, Screen::Story);
                    }
                }
                _ => {}
            }
        }
        self.model()
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

#[cfg(test)]
mod media_tests {
    use super::*;
    const LIMIT: u64 = 128 * 1024 * 1024;

    #[test]
    fn imported_japanese_story_settings_only_offer_configured_languages() {
        let mut program: Program =
            serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        let text = program.locales.remove("en").unwrap();
        program.locales.clear();
        program.locales.insert("ja".into(), text);
        let plan = program.locale_config.text.remove("en").unwrap();
        program.locale_config.text.clear();
        program.locale_config.text.insert("ja".into(), plan);
        program.locale_config.ui.remove("zh-Hans");
        program.locale_config.default_ui = "en".into();
        program.locale_config.default_text = "ja".into();
        program.default_locale = "ja".into();
        let player = Player::new(program, "release".into(), "Test".into()).unwrap();
        let mut model = player.model();
        model.screen = Screen::Settings;
        assert_eq!(model.available_ui_locales, ["en"]);
        assert_eq!(model.available_text_locales, ["ja"]);
        let packet =
            nir_presentation::project(&model, 1280., 720., &nir_presentation::Messages::default());
        let language_actions: Vec<_> = packet
            .semantics
            .iter()
            .filter_map(|node| match &node.action {
                UiAction::TextLocale { locale } => Some(locale.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(language_actions, ["ja"]);
    }

    #[test]
    fn image_menu_scales_hover_and_enforces_replay_unlocks() {
        let mut program: Program =
            serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        program.functions.insert("replay".into(), serde_json::from_value(serde_json::json!({"entry":"start","blocks":{"start":{"ops":[],"terminator":{"type":"call","function":"main","next":"end"}},"end":{"ops":[],"terminator":{"type":"end","outcome":"replay"}}}})).unwrap());
        let button = nir_format::ImageButton {
            id: "replay-button".into(),
            label: "Replay".into(),
            asset: "bg.station".into(),
            hover_asset: Some("bg.river".into()),
            locked_asset: None,
            rect: [100., 100., 200., 60.],
            action: nir_format::ImageMenuAction::Entry {
                function: "replay".into(),
            },
            requires: Some("seen".into()),
        };
        program.theme.image_menus.insert(
            "title".into(),
            nir_format::ImageMenu {
                builtin_navigation: true,
                story_exports: BTreeMap::new(),
                locals: BTreeMap::new(),
                elements: vec![],
                background: "bg.station".into(),
                buttons: vec![button],
                effects: None,
            },
        );
        let mut player = Player::new(program, "release".into(), "Test".into()).unwrap();
        player.cancel_preparation();
        player.pauses.remove("prepare");
        let entry = UiAction::ImageMenuEntry {
            function: "replay".into(),
        };
        player.action(entry.clone(), 0, 1, &mut 100).unwrap();
        assert_eq!(player.screen, Screen::Title);
        let packet = nir_presentation::project(
            &player.model(),
            320.,
            240.,
            &nir_presentation::Messages::default(),
        );
        assert!(packet.hit(30., 60.).is_none());
        player.profile.insert("seen".into());
        player
            .action(
                UiAction::HoverImage {
                    id: Some("replay-button".into()),
                },
                0,
                2,
                &mut 100,
            )
            .unwrap();
        let packet = nir_presentation::project(
            &player.model(),
            320.,
            240.,
            &nir_presentation::Messages::default(),
        );
        assert_eq!(packet.hit(30., 60.), Some(entry.clone()));
        assert!(packet
            .quads
            .iter()
            .any(|q| q.asset.as_deref() == Some("bg.river") && q.rect == [25., 55., 50., 15.]));
        assert!(player.title_assets().contains("bg.river"));
        player
            .action(
                UiAction::ImageMenuEntry {
                    function: "not-configured".into(),
                },
                0,
                3,
                &mut 100,
            )
            .unwrap();
        assert_eq!(player.screen, Screen::Title);
        player.action(entry, 0, 4, &mut 100).unwrap();
        assert_eq!(player.screen, Screen::Story);
        assert_eq!(player.core.state().frames[0].function, "replay");
    }

    #[test]
    fn story_releases_menu_images_but_keeps_dialogue_background() {
        let mut program: Program =
            serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        program.theme.image_menus.insert(
            "title".into(),
            nir_format::ImageMenu {
                builtin_navigation: true,
                story_exports: BTreeMap::new(),
                locals: BTreeMap::new(),
                elements: vec![],
                background: "bg.river".into(),
                buttons: vec![],
                effects: None,
            },
        );
        program.theme.dialogue.background = Some("bg.station".into());
        let player = at_intro_wait(program);
        let retained = player.retained_assets();
        assert!(retained.contains("bg.station"));
        assert!(!retained.contains("bg.river"));
        assert!(player.title_assets().contains("bg.river"));
    }

    #[test]
    fn next_activation_releases_obsolete_active_reservations() {
        let mut player = player();
        player.cancel_preparation();
        player.pauses.retain(|_| false);
        player.screen = Screen::Story;
        player.return_screen = Screen::Story;
        player.active = Some(
            player
                .ledger
                .reserve(&BTreeMap::from([(
                    "retired-audio".into(),
                    LIMIT - player.ledger.used() - 1,
                )]))
                .unwrap(),
        );
        player
            .begin_prepare(Purpose::Activation, 7, BTreeSet::from(["bg.river".into()]))
            .unwrap();
        assert!(player.prepare.is_some());
        assert!(player.memory_used() < LIMIT / 2);
    }

    #[test]
    fn admission_failure_pauses_and_retries_same_activation() {
        let mut player = player();
        player.cancel_preparation();
        player.pauses.retain(|_| false);
        player.commands.clear();
        player.screen = Screen::Story;
        player.return_screen = Screen::Story;
        let occupied = player
            .ledger
            .reserve(&BTreeMap::from([(
                "test-pressure".into(),
                LIMIT - player.ledger.used() - 1,
            )]))
            .unwrap();
        let error = player
            .action(UiAction::NewGame, 0, 0, &mut 100)
            .unwrap_err();
        let activation = player.core.state().pending.as_ref().unwrap().id;
        assert_eq!(error.code, "E_BUDGET");
        assert!(error
            .details
            .as_ref()
            .unwrap()
            .recovery
            .contains(&Recovery::Retry));
        player.report(error, true);
        assert!(player.paused());
        assert!(!player.needs_clock());
        assert!(player.failed_admission.is_some());
        assert!(player.prepare.is_none());
        drop(occupied);
        player.action(UiAction::Retry, 0, 0, &mut 100).unwrap();
        assert!(player.failed_admission.is_none());
        assert_eq!(player.prepare.as_ref().unwrap().job.activation, activation);
        assert!(player
            .commands
            .iter()
            .any(|c| matches!(c, AppCommand::GetAssets { .. })));
        player.action(UiAction::Title, 0, 0, &mut 100).unwrap();
        assert!(player.failed_admission.is_none());
    }

    #[test]
    fn image_menu_rejects_unknown_assets_and_entry_functions() {
        let mut program: Program =
            serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        program.theme.image_menus.insert(
            "title".into(),
            nir_format::ImageMenu {
                builtin_navigation: true,
                story_exports: BTreeMap::new(),
                locals: BTreeMap::new(),
                elements: vec![],
                background: "missing".into(),
                buttons: vec![],
                effects: None,
            },
        );
        assert!(Player::new(program, "release".into(), "Test".into()).is_err());
    }

    fn player() -> Player {
        let program: Program =
            serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        Player::new(program, "release".into(), "Test".into()).unwrap()
    }

    #[test]
    fn title_menus_project_only_the_retained_title_scene() {
        let mut program: Program =
            serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        program.title_scene = Some("station".into());
        let mut player = at_intro_wait(program);
        player.action(UiAction::Title, 0, 0, &mut 100).unwrap();
        for action in [
            UiAction::Menu,
            UiAction::Settings,
            UiAction::Saves,
            UiAction::History,
        ] {
            player.action(action, 0, 0, &mut 100).unwrap();
            let model = player.model();
            assert_eq!(
                serde_json::to_value(&model.nodes).unwrap(),
                serde_json::to_value(player.title_nodes()).unwrap()
            );
            assert!(model.transition.is_none());
            assert!(model
                .nodes
                .iter()
                .filter_map(|n| n.asset.as_ref())
                .all(|id| player.retained_assets().contains(id)));
        }
    }

    fn speculative(player: &mut Player, request: u32, id: &str) {
        let assets = BTreeSet::from([id.to_owned()]);
        let job = PrepareJob::new(
            0,
            player.generation,
            player.costs(&assets).unwrap(),
            &player.ledger,
        )
        .unwrap();
        player.media_lookahead = Some(MediaLookahead {
            request,
            fingerprint: "wait".into(),
            assets,
            job,
        });
        player.request = request;
    }

    fn at_intro_wait(mut program: Program) -> Player {
        program.player.prefetch_media = true;
        let mut player = Player::new(program, "release".into(), "Test".into()).unwrap();
        player.cancel_preparation();
        player.pauses.remove("prepare");
        player.commands.clear();
        for _ in 0..20 {
            let input = player
                .core
                .state()
                .pending
                .as_ref()
                .map(|pending| CoreInput::Prepared {
                    activation: pending.id,
                })
                .unwrap_or(CoreInput::None);
            player.core.step(input, 10_000);
            assert!(player.core.state().fault.is_none());
            if player.core.state().waiting.is_some() {
                break;
            }
        }
        assert_eq!(player.core.predict_next_cue().as_deref(), Some("enter"));
        player.screen = Screen::Story;
        player.return_screen = Screen::Story;
        player
    }

    #[test]
    fn cancelled_speculation_keeps_ledger_charge_until_host_settles() {
        let mut player = player();
        let before = player.memory_used();
        let assets = BTreeSet::from(["speculative-only".to_owned()]);
        let job = PrepareJob::new(
            0,
            player.generation,
            BTreeMap::from([("speculative-only".into(), 4096)]),
            &player.ledger,
        )
        .unwrap();
        player.media_lookahead = Some(MediaLookahead {
            request: 91,
            fingerprint: "wait".into(),
            assets,
            job,
        });
        assert_eq!(player.memory_used(), before + 4096);
        player.cancel_media_lookahead();
        assert_eq!(player.memory_used(), before + 4096);
        player
            .event(AppEvent::AssetsCancelled { request: 91 }, &mut 100)
            .unwrap();
        assert_eq!(player.memory_used(), before);
    }

    #[test]
    fn promoted_ready_media_is_not_requested_again() {
        let mut player = player();
        player.cancel_preparation();
        player.commands.clear();
        player.request = 41;
        let id = "bg.river".to_owned();
        let mut job = PrepareJob::new(
            0,
            player.generation,
            player.costs(&BTreeSet::from([id.clone()])).unwrap(),
            &player.ledger,
        )
        .unwrap();
        assert!(job.ready(&id, player.generation));
        player.media_lookahead = Some(MediaLookahead {
            request: 41,
            fingerprint: "wait".into(),
            assets: BTreeSet::from([id.clone()]),
            job,
        });
        player
            .begin_prepare(Purpose::Activation, 7, BTreeSet::from([id.clone()]))
            .unwrap();
        assert!(player
            .commands
            .iter()
            .any(|c| matches!(c, AppCommand::PromoteAssets { request: 41, .. })));
        assert!(player.commands.iter().all(|c| match c {
            AppCommand::GetAssets { assets, .. } => !assets.contains(&id),
            _ => true,
        }));
        assert!(!player.prepare.as_ref().unwrap().job.missing.contains(&id));
        assert!(player.retained_assets().contains(&id));
    }

    #[test]
    fn promoted_inflight_media_ready_arrives_on_original_request() {
        let mut player = player();
        player.cancel_preparation();
        player.commands.clear();
        speculative(&mut player, 41, "bg.river");
        player
            .begin_prepare(Purpose::Activation, 7, BTreeSet::from(["bg.river".into()]))
            .unwrap();
        let required_request = player.prepare.as_ref().unwrap().request;
        assert_ne!(required_request, 41);
        assert_eq!(player.prepare.as_ref().unwrap().promoted_from, Some(41));
        assert!(player.accepts_resource(41));
        player
            .event(
                AppEvent::AssetReady {
                    request: 41,
                    asset: "bg.river".into(),
                },
                &mut 100,
            )
            .unwrap();
        assert!(!player
            .prepare
            .as_ref()
            .unwrap()
            .job
            .missing
            .contains("bg.river"));
        assert!(player.retained_assets().contains("bg.river"));
    }

    #[test]
    fn required_prepare_waits_for_cancel_ack_under_budget_pressure() {
        let mut player = player();
        player.cancel_preparation();
        player.commands.clear();
        let mut required = player.title_assets();
        required.insert("bg.station".into());
        let required_cost: u64 = player.costs(&required).unwrap().values().sum();
        let fill = LIMIT - player.memory_used() - required_cost;
        let _fill = player
            .ledger
            .reserve(&BTreeMap::from([("@test-fill".into(), fill)]))
            .unwrap();
        speculative(&mut player, 41, "audio.bell");
        player
            .begin_prepare(Purpose::Activation, 7, required)
            .unwrap();
        assert!(player.prepare.is_none());
        assert!(player.deferred_prepare.is_some());
        assert!(player.media_retired.contains_key(&41));
        assert!(player
            .commands
            .iter()
            .any(|c| matches!(c, AppCommand::CancelAssets { request: 41 })));
        player.commands.clear();
        player
            .event(AppEvent::AssetsCancelled { request: 40 }, &mut 100)
            .unwrap();
        assert!(player.prepare.is_none(), "stale ack cannot release budget");
        player
            .event(AppEvent::AssetsCancelled { request: 41 }, &mut 100)
            .unwrap();
        assert!(player.prepare.is_some());
        assert!(player.deferred_prepare.is_none());
        assert!(player.commands.iter().any(|c| matches!(
            c,
            AppCommand::GetAssets {
                priority: PreparePriority::Required,
                ..
            }
        )));
    }

    #[test]
    fn title_supersedes_deferred_activation_before_ack() {
        let mut player = player();
        player.cancel_preparation();
        let mut required = player.title_assets();
        required.insert("bg.station".into());
        let cost: u64 = player.costs(&required).unwrap().values().sum();
        let _fill = player
            .ledger
            .reserve(&BTreeMap::from([(
                "@test-fill".into(),
                LIMIT - player.memory_used() - cost,
            )]))
            .unwrap();
        speculative(&mut player, 41, "audio.bell");
        player
            .begin_prepare(Purpose::Activation, 7, required)
            .unwrap();
        assert!(player.deferred_prepare.is_some());
        let old_session = player.generation.session;
        player.action(UiAction::Title, 0, 0, &mut 100).unwrap();
        assert!(player.generation.session > old_session);
        assert!(!player.deferred_prepare.as_ref().is_some_and(
            |(_, purpose, activation, _)| matches!(purpose, Purpose::Activation)
                && *activation == 7
        ));
        player
            .event(AppEvent::AssetsCancelled { request: 41 }, &mut 100)
            .unwrap();
        assert!(!player
            .prepare
            .as_ref()
            .is_some_and(|p| matches!(p.purpose, Purpose::Activation)));
    }

    #[test]
    fn locale_font_admission_retries_after_speculative_cancel() {
        let mut program: Program =
            serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        let mut font = program.assets["font.reader"].clone();
        font.object = "alt-font".into();
        program.assets.insert("font.alt".into(), font);
        let media: BTreeMap<_, _> = program
            .assets
            .iter()
            .map(|(id, asset)| (id.clone(), asset.object.clone()))
            .collect();
        for plan in [
            program.locale_config.ui.get_mut("en").unwrap(),
            program.locale_config.text.get_mut("en").unwrap(),
        ] {
            plan.fonts = vec!["font.alt".into()];
            plan.digest = LocaleFontPlan::digest_for(&plan.fonts, &media);
        }
        let mut player = Player::new(program, "release".into(), "Test".into()).unwrap();
        player.cancel_preparation();
        speculative(&mut player, 41, "bg.river");
        let font_cost = player.costs(&BTreeSet::from(["font.alt".into()])).unwrap()["font.alt"];
        let _fill = player
            .ledger
            .reserve(&BTreeMap::from([(
                "@test-fill".into(),
                LIMIT - player.memory_used() - font_cost + 1,
            )]))
            .unwrap();
        player.preferences.ui_locale = "en".into();
        player.preferences.text_locale = "en".into();
        player.start_locale_switch().unwrap();
        assert!(player.deferred_locale.is_some());
        assert!(player.media_retired.contains_key(&41));
        assert!(player.locale_error.is_none());
        player
            .event(AppEvent::AssetsCancelled { request: 41 }, &mut 100)
            .unwrap();
        assert!(player.deferred_locale.is_none());
        assert!(player.locale_job.is_some());
        assert!(player.locale_error.is_none());
    }

    #[test]
    fn oversized_next_cue_is_skipped_once_for_its_wait() {
        let mut program: Program =
            serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        let actor = program.assets.get_mut("actor.aki").unwrap();
        actor.width = 2048;
        actor.height = 2048;
        let mut player = at_intro_wait(program);
        let before = player.request;
        player.maybe_prefetch_media();
        let fingerprint = player.media_attempted.clone();
        assert!(fingerprint.is_some());
        assert!(player.media_lookahead.is_none());
        assert_eq!(player.request, before);
        player.pauses.insert("hidden".into());
        player.maybe_prefetch_media();
        player.pauses.remove("hidden");
        player.maybe_prefetch_media();
        assert_eq!(player.media_attempted, fingerprint);
        assert_eq!(player.request, before);
    }

    #[test]
    fn failed_next_cue_is_not_retried_at_the_same_wait() {
        let program: Program =
            serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        let mut player = at_intro_wait(program);
        player.maybe_prefetch_media();
        let request = player.media_lookahead.as_ref().unwrap().request;
        assert!(player.commands.iter().any(|c| matches!(c, AppCommand::GetAssets { request: r, priority: PreparePriority::Near, .. } if *r == request)));
        player
            .event(
                AppEvent::AssetFailed {
                    request,
                    message: "decode failed".into(),
                },
                &mut 100,
            )
            .unwrap();
        assert!(player.media_lookahead.is_none());
        assert!(player.media_retired.contains_key(&request));
        player
            .event(AppEvent::AssetsCancelled { request }, &mut 100)
            .unwrap();
        let before = player.request;
        player.commands.clear();
        player.maybe_prefetch_media();
        assert_eq!(player.request, before);
        assert!(!player.commands.iter().any(|c| matches!(
            c,
            AppCommand::GetAssets {
                priority: PreparePriority::Near,
                ..
            }
        )));
    }

    #[test]
    fn device_loss_retires_inflight_media_and_discards_late_ready() {
        let program: Program =
            serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        let mut player = at_intro_wait(program);
        let baseline = player.memory_used();
        player.maybe_prefetch_media();
        let request = player.media_lookahead.as_ref().unwrap().request;
        assert!(player.memory_used() > baseline);
        player.pump(vec![AppEvent::DeviceLost], 1000);
        assert!(player.media_lookahead.is_none());
        assert!(player.media_retired.contains_key(&request));
        assert!(player.memory_used() > baseline);
        player.pump(
            vec![AppEvent::AssetReady {
                request,
                asset: "actor.aki".into(),
            }],
            1000,
        );
        player.pump(
            vec![AppEvent::AssetFault {
                request,
                diagnostic: Box::new(Diagnostic::new("E_DECODE", "media", "late decode")),
            }],
            1000,
        );
        assert!(player.error.is_none());
        assert!(player.media_retired.contains_key(&request));
        player.pump(vec![AppEvent::AssetsCancelled { request }], 1000);
        assert_eq!(player.memory_used(), baseline);
    }
}
