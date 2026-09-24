//! Single-owner application coordination. Host completions enter through pump.
#![forbid(unsafe_code)]
use nir_assets::{BudgetLedger, Generation, PrepareJob, Reservation};
use nir_core::*;
use nir_format::*;
use nir_presentation::{ChoiceView, DialogueView, Screen, SlotView, UiModel};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
mod content;
mod pause;
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
        task: u32,
        asset: String,
        bus: AudioBus,
        looped: bool,
        position_us: Micros,
        session: u32,
    },
    AudioStop {
        task: u32,
    },
    AudioPause {
        paused: bool,
    },
    AudioReset,
    Save {
        slot: u32,
        expected_revision: u32,
        job: u32,
        envelope: Box<SaveEnvelope>,
    },
    Load {
        slot: u32,
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
        task: u32,
        session: u32,
    },
    AudioFailed {
        task: u32,
        session: u32,
        message: String,
    },
    Hidden(bool),
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
    pub profile: BTreeSet<String>,
    pub slots: Vec<SlotView>,
    pauses: Pauses,
    audio_paused: bool,
    inbox: VecDeque<(u32, AppEvent)>,
    work_used: u32,
    prepare: Option<Preparation>,
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
    save_jobs: BTreeMap<u32, (u32, u32)>,
    auto_elapsed: u64,
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
            profile: BTreeSet::new(),
            slots: (0..3)
                .map(|slot| SlotView {
                    slot,
                    ..Default::default()
                })
                .collect(),
            pauses: Pauses::default(),
            audio_paused: true,
            inbox: VecDeque::new(),
            work_used: 0,
            prepare: None,
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
            save_jobs: BTreeMap::new(),
            auto_elapsed: 0,
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
                color: [0., 0., 0., 0.],
                emphasis: vec![],
                scroll: 0.,
                clip: None,
                region: None,
                locale: locale.into(),
                font_assets: fonts.to_vec(),
                font_plan_digest: digest.into(),
                preflight_only: true,
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
    pub fn needs_clock(&self) -> bool {
        self.screen == Screen::Story
            && self.pauses.is_empty()
            && (self.core.needs_clock() || self.auto || self.skip)
    }
    fn story_context_active(&self) -> bool {
        self.screen != Screen::Title && self.return_screen != Screen::Title
    }
    pub fn paused(&self) -> bool {
        !self.pauses.is_empty()
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
            if let Effect::Audio { asset, .. } = &t.effect {
                a.insert(asset.clone());
            }
        }
        if let Some(pending) = &s.pending {
            a.extend(self.validated.cue_assets(&pending.cue));
        }
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
        let before_location = self.core.location();
        let output = self.core.step(input, *budget);
        *budget -= output.work_used;
        if before_location != output.location {
            self.touch_snapshot_content(self.core.state())?;
        }
        if output.remaining_time_us > 0 {
            self.inbox.push_front((
                self.generation.session,
                AppEvent::Tick {
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
                } => self.commands.push(AppCommand::AudioStart {
                    task,
                    asset,
                    bus,
                    looped,
                    position_us,
                    session: self.generation.session,
                }),
                CoreIntent::AudioStop { task } => {
                    self.commands.push(AppCommand::AudioStop { task })
                }
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
        if self.core.state().outcome.is_some() {
            self.screen = Screen::Ended;
            self.auto = false;
            self.skip = false;
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
            let input = matches!(event, AppEvent::Action { .. } | AppEvent::Tick { .. });
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
                continue;
            }
            self.inbox.push_back((self.generation.session, event));
        }
        // Stable partition across retained work, so a choice beats same-turn time.
        self.inbox
            .make_contiguous()
            .sort_by_key(|(_, e)| matches!(e, AppEvent::Tick { .. }));
        let mut remaining = budget.min(100_000);
        let limit = remaining;
        while remaining > 0 {
            let Some((session, event)) = self.inbox.pop_front() else {
                break;
            };
            remaining -= 1;
            if matches!(event, AppEvent::Tick { .. }) && session != self.generation.session {
                self.observe("stale_tick_discarded", None);
                continue;
            }
            if let Err(e) = self.event(event, &mut remaining) {
                self.report(e, true);
            }
        }
        if self.prepare.is_none() && self.screen == Screen::Story && self.pauses.is_empty() {
            if let Err(e) = self.step(CoreInput::None, &mut remaining) {
                self.report(e, true);
            }
        }
        self.maybe_prefetch_content();
        self.maybe_prefetch_media();
        if let Err(error) = self.refresh_content_lease() {
            self.report(error, true);
        }
        self.work_used = limit - remaining;
        let after = self.paused();
        if self.audio_paused != after {
            self.audio_paused = after;
            self.commands.push(AppCommand::AudioPause { paused: after });
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
                if self.needs_clock() {
                    let before = self.core.state().tick_us.0;
                    self.step(
                        CoreInput::Time {
                            delta_us: delta_us.min(250_000),
                        },
                        budget,
                    )?;
                    self.read_policy(self.core.state().tick_us.0 - before, budget)?;
                }
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
            AppEvent::AudioEnded { task, session } => {
                if session == self.generation.session {
                    self.step(CoreInput::AudioEnded { task }, budget)?;
                }
            }
            AppEvent::AudioFailed {
                task,
                session,
                message,
            } => {
                if session == self.generation.session {
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
            AppEvent::Hidden(hidden) => {
                if hidden {
                    self.pauses.insert("hidden".into());
                } else {
                    self.pauses.remove("hidden");
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
                self.slots = slots;
                self.slot_revisions = revisions;
            }
            AppEvent::Preferences(mut p) => {
                let locale_config = &self.core.program().locale_config;
                if !locale_config.ui.contains_key(&p.ui_locale) {
                    p.ui_locale = locale_config.default_ui.clone();
                }
                if !locale_config.text.contains_key(&p.text_locale) {
                    p.text_locale = locale_config.default_text.clone();
                }
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
                self.commands.push(AppCommand::AudioPause { paused: true });
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
        if let Some(p) = &self.core.state().pending {
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
        self.observe("lease_ready", Some(request));
        self.pauses.remove("prepare");
        self.pauses.remove("device");
        self.error = None;
        self.diagnostic = None;
        let commit_location = self.core.location();
        let commit_cue = self.core.state().pending.as_ref().map(|p| p.cue.clone());
        let commit_generation = self.generation;
        self.observe("commit_started", Some(request));
        match purpose {
            Purpose::Boot => {}
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
                self.commands.push(AppCommand::AudioReset);
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
                self.restart_audio();
            }
            Purpose::Device => {
                self.pauses.remove("device");
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
            if let Effect::Audio { asset, bus, looped } = &t.effect {
                self.commands.push(AppCommand::AudioStart {
                    task: t.id,
                    asset: asset.clone(),
                    bus: *bus,
                    looped: *looped,
                    position_us: t.elapsed_us,
                    session: self.generation.session,
                });
            }
        }
        self.commands.push(AppCommand::AudioPause { paused: true });
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
        match a {
            UiAction::NewGame => {
                if self.prepare.is_some() {
                    return Ok(());
                }
                let restart_locale = self.locale_pending();
                self.cancel_content(false);
                self.restore_work = None;
                self.candidate = None;
                self.prefetch_attempted = None;
                self.generation.session += 1;
                self.commands.push(AppCommand::AudioReset);
                self.core = Core::new(
                    self.validated.clone(),
                    self.release.clone(),
                    self.effective_text_locale.clone(),
                )?;
                self.screen = Screen::Story;
                self.return_screen = Screen::Story;
                self.pauses.retain(|r| r == "hidden");
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
            UiAction::Scroll { .. } => {
                if interaction != self.current_interaction() {
                    return Ok(());
                }
                // Browsing revealed text takes control back from automatic reading.
                self.auto = false;
                self.skip = false;
                self.auto_elapsed = 0;
            }
            UiAction::Advance => {
                if self.screen == Screen::Story && !self.paused() {
                    self.auto_elapsed = 0;
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
                if matches!(self.screen, Screen::Title | Screen::Story | Screen::Ended) {
                    self.return_screen = self.screen;
                }
                self.screen = match a {
                    UiAction::Settings => Screen::Settings,
                    UiAction::History => Screen::History,
                    UiAction::Saves => Screen::Saves,
                    _ => Screen::Menu,
                };
                self.pauses.insert("menu".into());
                if self.screen == Screen::Saves {
                    self.commands.push(AppCommand::ListSaves);
                }
            }
            UiAction::Close => {
                self.screen = self.return_screen;
                self.pauses.remove("menu");
                self.status.clear();
            }
            UiAction::Title => {
                let restart_locale = self.locale_pending();
                self.cancel_content(false);
                self.commands.push(AppCommand::AudioReset);
                self.cancel_preparation();
                self.candidate = None;
                self.restore_work = None;
                self.prefetch_attempted = None;
                self.device_resume = None;
                self.pauses.retain(|r| r == "hidden");
                self.screen = Screen::Title;
                self.return_screen = Screen::Title;
                self.generation.session += 1;
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
                self.auto_elapsed = 0;
            }
            UiAction::ToggleSkip => {
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
                self.request += 1;
                let job = self.request;
                self.save_jobs.insert(job, (slot, self.generation.session));
                let envelope = SaveEnvelope {
                    format: 1,
                    slot,
                    revision: revision + 1,
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
            UiAction::Load { slot } => self.commands.push(AppCommand::Load { slot }),
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
        Ok(())
    }
    fn persist_preferences(&mut self) {
        self.commands.push(AppCommand::PersistPreferences {
            preferences: self.preferences.clone(),
        });
    }
    fn restart_preparation(&mut self) -> Result<()> {
        if let Some(prep) = self.prepare.as_ref() {
            let (purpose, activation) = (prep.purpose, prep.job.activation);
            let assets = self.retained_assets();
            self.begin_prepare(purpose, activation, assets)?;
        }
        Ok(())
    }
    fn read_policy(&mut self, delta: u64, budget: &mut u32) -> Result<()> {
        if self.paused() || self.core.state().choice.is_some() {
            self.skip = false;
            return Ok(());
        }
        let Some((_, d)) = self.core.dialogue() else {
            return Ok(());
        };
        let read = self
            .profile
            .contains(&format!("read:{}:{}", d.text_id, d.meaning_revision));
        if self.skip && !read {
            self.skip = false;
        }
        if self.skip && read && !d.at_gate {
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
            self.auto_elapsed = self.auto_elapsed.saturating_add(delta);
            let voice = self.core.state().tasks.values().any(|t| {
                t.state == TaskState::Running
                    && matches!(
                        t.effect,
                        Effect::Audio {
                            bus: AudioBus::Voice,
                            ..
                        }
                    )
            });
            let delay = self.core.program().player.auto_delay_us.0
                + (d.full_text().chars().count() as u64 * 20_000);
            if !voice && self.auto_elapsed >= delay {
                let token = d.interaction;
                self.auto_elapsed = 0;
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
        }
        Ok(())
    }
    pub fn model(&self) -> UiModel {
        self.model_for(&self.core, self.screen)
    }
    fn model_for(&self, c: &Core, screen: Screen) -> UiModel {
        self.model_for_locale(c, screen, &self.effective_ui_locale)
    }
    fn model_for_locale(&self, c: &Core, screen: Screen, ui_locale: &str) -> UiModel {
        let ui_plan = &c.program().locale_config.ui[ui_locale];
        let text_plan = &c.program().locale_config.text[&self.effective_text_locale];
        UiModel {
            title: self.title.clone(),
            screen,
            nodes: if screen == Screen::Title {
                self.title_nodes()
            } else {
                c.sample_scene()
            },
            transition: if self.preferences.reduced_motion {
                None
            } else {
                c.transition().map(|(n, p)| (n.to_vec(), p))
            },
            stage: [
                c.program().stage.width as f32,
                c.program().stage.height as f32,
            ],
            dialogue: c.dialogue().map(|(_, d)| DialogueView {
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
            text_fonts: text_plan.fonts.clone(),
            text_font_plan_digest: text_plan.digest.clone(),
            locale_pending: self.locale_pending(),
            locale_error: self.locale_error.clone(),
            preflight_texts: vec![],
            theme: (*c.program().theme).clone(),
            history: c
                .state()
                .history
                .iter()
                .map(|h| nir_presentation::HistoryView {
                    speaker: h.speaker.clone(),
                    text: h.text.clone(),
                    locale: h.locale.clone(),
                    font_plan_digest: h.font_plan_digest.clone(),
                    font_assets: c.program().locale_config.text[&h.locale].fonts.clone(),
                })
                .collect(),
            slots: self.slots.clone(),
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
            skip: self.skip,
            outcome: c.state().outcome.clone(),
            history_offset: self.history_offset,
        }
    }
    pub fn preview(&self) -> UiModel {
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

    fn player() -> Player {
        let program: Program =
            serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        Player::new(program, "release".into(), "Test".into()).unwrap()
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
