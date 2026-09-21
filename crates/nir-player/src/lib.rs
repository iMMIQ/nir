//! Single-owner application coordination. Host completions enter through pump.
#![forbid(unsafe_code)]
use nir_assets::{BudgetLedger, Generation, PrepareJob, Reservation};
use nir_core::*;
use nir_format::*;
use nir_presentation::{ChoiceView, DialogueView, Screen, SlotView, UiModel};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
mod pause;
pub use pause::PauseToken;
use pause::Pauses;

pub const EVENT_CAPACITY: usize = 256;
const INPUT_CAPACITY: usize = 128;

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
    GetAssets {
        request: u32,
        session: u32,
        device: u32,
        assets: Vec<String>,
    },
    CancelAssets {
        request: u32,
    },
    PreparePresentation {
        request: u32,
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
    PresentationReady {
        request: u32,
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
    Saved {
        job: u32,
        slot: u32,
        revision: u32,
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
    purpose: Purpose,
    job: PrepareJob,
    preflight: bool,
    failed: bool,
}
pub struct Player {
    core: Core,
    validated: ValidatedProgram,
    pub title: String,
    pub release: String,
    pub screen: Screen,
    return_screen: Screen,
    pub preferences: Preferences,
    pub generation: Generation,
    pub status: String,
    pub error: Option<String>,
    pub auto: bool,
    pub skip: bool,
    pub profile: BTreeSet<String>,
    pub slots: Vec<SlotView>,
    pauses: Pauses,
    audio_paused: bool,
    inbox: VecDeque<(u32, AppEvent)>,
    work_used: u32,
    prepare: Option<Preparation>,
    candidate: Option<Core>,
    device_resume: Option<Purpose>,
    ledger: BudgetLedger,
    _surface_budget: Reservation,
    active: Option<Reservation>,
    request: u32,
    commands: Vec<AppCommand>,
    checkpoints: Vec<Snapshot>,
    slot_revisions: BTreeMap<u32, u32>,
    save_jobs: BTreeMap<u32, u32>,
    auto_elapsed: u64,
    history_offset: usize,
}
impl Player {
    pub fn new(program: Program, release: String, title: String) -> Result<Self> {
        let validated = ValidatedProgram::new(program)?;
        let locale = validated.program().default_locale.clone();
        let core = Core::new(validated.clone(), release.clone(), locale.clone())?;
        let ledger = BudgetLedger::new(128 * 1024 * 1024);
        let _surface_budget = ledger.reserve(&BTreeMap::from([(
            "@render-surfaces".into(),
            validated.program().stage.width as u64 * validated.program().stage.height as u64 * 8
                + 8 * 1024 * 1024
                + 32 * 1024 * 1024,
        )]))?;
        let mut p = Self {
            core,
            validated,
            title,
            release,
            screen: Screen::Title,
            return_screen: Screen::Title,
            preferences: Preferences {
                locale,
                ..Default::default()
            },
            generation: Generation {
                session: 1,
                device: 1,
                surface: 1,
                typography: 1,
                language: 1,
            },
            status: String::new(),
            error: None,
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
    pub fn paused(&self) -> bool {
        !self.pauses.is_empty()
    }
    pub fn is_loading(&self) -> bool {
        self.prepare.is_some()
    }
    fn title_nodes(&self) -> Vec<Node> {
        let p = self.validated.program();
        p.title_scene
            .as_ref()
            .and_then(|s| p.scenes.get(s))
            .or_else(|| p.scenes.values().next())
            .cloned()
            .unwrap_or_default()
    }
    fn title_assets(&self) -> BTreeSet<String> {
        let mut a: BTreeSet<_> = self
            .title_nodes()
            .iter()
            .filter_map(|n| n.asset.clone())
            .collect();
        a.extend(
            self.validated
                .program()
                .assets
                .iter()
                .filter(|(_, a)| a.kind == AssetKind::Font)
                .map(|(id, _)| id.clone()),
        );
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
            a.extend(nir_content::cue_assets(core.program(), &pending.cue));
        }
        a.extend(
            core.program()
                .assets
                .iter()
                .filter(|(_, a)| a.kind == AssetKind::Font)
                .map(|(id, _)| id.clone()),
        );
        a
    }
    pub fn retained_assets(&self) -> BTreeSet<String> {
        let mut a = if self.screen == Screen::Title {
            self.title_assets()
        } else {
            self.state_assets(&self.core)
        };
        if let Some(c) = &self.candidate {
            a.extend(self.state_assets(c));
        }
        if let Some(prep) = &self.prepare {
            a.extend(prep.job.missing.clone());
        }
        a
    }
    fn costs(&self, ids: &BTreeSet<String>) -> Result<BTreeMap<String, u64>> {
        ids.iter()
            .map(|id| {
                let a = self
                    .validated
                    .program()
                    .assets
                    .get(id)
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
    fn begin_prepare(
        &mut self,
        purpose: Purpose,
        activation: u32,
        mut assets: BTreeSet<String>,
    ) -> Result<()> {
        // Old and candidate resources are admitted together; never pin half a cue.
        assets.extend(if self.screen == Screen::Title {
            self.title_assets()
        } else {
            self.state_assets(&self.core)
        });
        self.request = self
            .request
            .checked_add(1)
            .ok_or_else(|| Diagnostic::new("E_LIMIT", "request", "counter"))?;
        let job = PrepareJob::new(
            activation,
            self.generation,
            self.costs(&assets)?,
            &self.ledger,
        )?;
        let request = self.request;
        self.cancel_preparation();
        self.prepare = Some(Preparation {
            request,
            purpose,
            job,
            preflight: false,
            failed: false,
        });
        self.pauses.insert("prepare".into());
        self.commands.push(AppCommand::GetAssets {
            request,
            session: self.generation.session,
            device: self.generation.device,
            assets: assets.into_iter().collect(),
        });
        Ok(())
    }
    fn cancel_preparation(&mut self) {
        if let Some(p) = self.prepare.take() {
            if !p.failed {
                self.commands
                    .push(AppCommand::CancelAssets { request: p.request });
            }
        }
    }
    fn step(&mut self, input: CoreInput, budget: &mut u32) -> Result<()> {
        let output = self.core.step(input, *budget);
        *budget -= output.work_used;
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
                CoreIntent::Prepare { activation, cue } => self.begin_prepare(
                    Purpose::Activation,
                    activation,
                    nir_content::cue_assets(self.core.program(), &cue),
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
            self.error = Some(e.to_string());
            self.pauses.insert("fault".into());
        }
        if self.core.state().outcome.is_some() {
            self.screen = Screen::Ended;
            self.auto = false;
            self.skip = false;
        }
        Ok(())
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
                self.error = Some("E_EVENT_QUEUE: event admission limit".into());
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
                continue;
            }
            if let Err(e) = self.event(event, &mut remaining) {
                self.status = e.to_string();
                self.error = Some(e.to_string());
            }
        }
        if self.prepare.is_none() && self.screen == Screen::Story && self.pauses.is_empty() {
            if let Err(e) = self.step(CoreInput::None, &mut remaining) {
                self.error = Some(e.to_string());
            }
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
        match e {
            AppEvent::Action {
                action,
                interaction,
                sequence,
                session,
            } => {
                if session == self.generation.session {
                    self.action(action, interaction, sequence, budget)?;
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
                if let Some(p) = self
                    .prepare
                    .as_mut()
                    .filter(|p| p.request == request && !p.failed)
                {
                    p.job.ready(&asset, self.generation);
                    if p.job.missing.is_empty() && !p.preflight {
                        p.preflight = true;
                        self.commands
                            .push(AppCommand::PreparePresentation { request });
                    }
                }
            }
            AppEvent::AssetFailed { request, message } => {
                if self.accepts(request) {
                    self.prepare.as_mut().unwrap().failed = true;
                    self.commands.push(AppCommand::CancelAssets { request });
                    self.error = Some(message.clone());
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
            }
            AppEvent::PresentationReady { request } => self.complete(request, budget)?,
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
                    self.step(CoreInput::TaskFailed { task, message }, budget)?;
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
                self.restore(envelope.snapshot)?;
            }
            AppEvent::LoadFailed(m) => self.status = m,
            AppEvent::Saved {
                job,
                slot,
                revision,
            } => {
                if self.save_jobs.remove(&job) == Some(slot) {
                    self.slot_revisions.insert(slot, revision);
                    self.status = if self.preferences.locale == "en" {
                        "Saved in this browser"
                    } else {
                        "浏览器已保存"
                    }
                    .into();
                    self.commands.push(AppCommand::ListSaves);
                }
            }
            AppEvent::SaveFailed { job, message } => {
                if self.save_jobs.remove(&job).is_some() {
                    self.status = message;
                }
            }
            AppEvent::Slots(slots, revisions) => {
                self.slots = slots;
                self.slot_revisions = revisions;
            }
            AppEvent::Preferences(mut p) => {
                if !self.core.program().locales.contains_key(&p.locale) {
                    p.locale = self.core.program().default_locale.clone();
                }
                p.font_scale = finite_clamp(p.font_scale, 0.8, 1.5, 1.);
                p.bgm_volume = finite_clamp(p.bgm_volume, 0., 1., 0.3);
                p.voice_volume = finite_clamp(p.voice_volume, 0., 1., 0.8);
                p.sfx_volume = finite_clamp(p.sfx_volume, 0., 1., 0.5);
                self.preferences = p;
                self.core.set_locale(&self.preferences.locale)?;
            }
            AppEvent::Profile(keys) => self.profile.extend(keys),
            AppEvent::DeviceLost => {
                self.generation.device += 1;
                self.device_resume = self.prepare.as_ref().map(|p| p.purpose);
                self.cancel_preparation();
                self.pauses.insert("device".into());
                self.commands.push(AppCommand::AudioPause { paused: true });
            }
            AppEvent::DeviceReady => {
                let purpose = match self.device_resume.take() {
                    Some(Purpose::Restore) => Purpose::Restore,
                    Some(Purpose::Rollback) => Purpose::Rollback,
                    _ => Purpose::Device,
                };
                self.begin_prepare(purpose, 0, self.retained_assets())?;
            }
        }
        Ok(())
    }
    fn complete(&mut self, request: u32, budget: &mut u32) -> Result<()> {
        if !self.accepts(request) {
            return Ok(());
        }
        let prep = self.prepare.take().unwrap();
        let purpose = prep.purpose;
        let lease = prep.job.finish()?;
        if !lease.valid(lease.activation, self.generation) {
            return Err(Diagnostic::new(
                "E_STALE_LEASE",
                "commit",
                "generation changed",
            ));
        }
        self.pauses.remove("prepare");
        self.pauses.remove("device");
        self.error = None;
        match purpose {
            Purpose::Boot => {}
            Purpose::Activation => self.step(
                CoreInput::Prepared {
                    activation: lease.activation,
                },
                budget,
            )?,
            Purpose::Restore | Purpose::Rollback => {
                let candidate = self
                    .candidate
                    .take()
                    .ok_or_else(|| Diagnostic::new("E_RESTORE", "commit", "no candidate"))?;
                self.generation.session += 1;
                self.commands.push(AppCommand::AudioReset);
                self.core = candidate;
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
        let mut candidate = Core::restore(self.validated.clone(), s, &self.release)?;
        // Preferences are independent of saves. Frozen current instances retain
        // their saved language; future instances use the current preference.
        candidate.set_locale(&self.preferences.locale)?;
        let assets = self.state_assets(&candidate);
        self.begin_prepare(Purpose::Restore, 0, assets)?;
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
                self.generation.session += 1;
                self.commands.push(AppCommand::AudioReset);
                self.core = Core::new(
                    self.validated.clone(),
                    self.release.clone(),
                    self.preferences.locale.clone(),
                )?;
                self.screen = Screen::Story;
                self.return_screen = Screen::Story;
                self.pauses.retain(|r| r == "hidden");
                self.checkpoints.clear();
                self.error = None;
                self.status.clear();
                self.step(CoreInput::None, budget)?;
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
                            nir_content::cue_assets(self.core.program(), &pending.cue),
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
                self.commands.push(AppCommand::AudioReset);
                self.cancel_preparation();
                self.candidate = None;
                self.pauses.retain(|r| r == "hidden");
                self.screen = Screen::Title;
                self.return_screen = Screen::Title;
                self.generation.session += 1;
                self.error = None;
                self.status.clear();
                self.begin_prepare(Purpose::Boot, 0, self.title_assets())?;
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
            UiAction::Locale { locale } => {
                self.core.set_locale(&locale)?;
                self.preferences.locale = locale;
                self.generation.language += 1;
                self.status = if self.preferences.locale == "en" {
                    "Applies to the next line"
                } else {
                    "下一段生效"
                }
                .into();
                self.persist_preferences();
                self.restart_preparation()?;
            }
            UiAction::FontSize { delta } => {
                self.preferences.font_scale = (self.preferences.font_scale + delta).clamp(0.8, 1.5);
                self.generation.typography += 1;
                self.persist_preferences();
                self.restart_preparation()?;
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
                    || self.save_jobs.values().any(|v| *v == slot)
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
                self.save_jobs.insert(job, slot);
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
                self.status = if self.preferences.locale == "en" {
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
                    self.restore(s)?;
                    self.prepare.as_mut().unwrap().purpose = Purpose::Rollback;
                }
            }
            UiAction::HistoryPage { delta } => {
                self.history_offset = (self.history_offset as i64 + delta as i64)
                    .clamp(0, self.core.state().history.len().saturating_sub(1) as i64)
                    as usize;
            }
            UiAction::Retry => self.restart_preparation()?,
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
            .contains(&format!("read:{}:{}", d.text_id, d.revision));
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
            let delay = 1_200_000 + (d.full_text().chars().count() as u64 * 20_000);
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
                        })
                        .collect()
                })
                .unwrap_or_default(),
            prefs: self.preferences.clone(),
            theme: c.program().theme.clone(),
            history: c
                .state()
                .history
                .iter()
                .map(|h| (h.speaker.clone(), h.text.clone()))
                .collect(),
            slots: self.slots.clone(),
            paused: self.paused(),
            loading: self.is_loading(),
            status: self.status.clone(),
            fault: self.error.clone(),
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
        self.restart_preparation()
    }
}
