use crate::validate::RuntimeProgramView;
use crate::ValidatedProgram;
use nir_format::*;
use rand_chacha::ChaCha8Rng;
use rand_core::{RngCore, SeedableRng};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    pub id: u32,
    pub function: String,
    pub block: String,
    pub op: usize,
    pub op_id: String,
    pub locals: BTreeMap<String, Value>,
    pub return_to: Option<String>,
    pub result: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Running,
    Finished,
    Cancelled,
    Failed,
}
/// Why a task became terminal; never replaces its Await-compatible state.
/// Snapshot v2 requires a reason for every terminal task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskEndReason {
    Completed,
    NaturalEnd,
    FinishedByControl,
    CancelledByControl,
    Replaced,
    ScopeExited,
    Failed,
}
fn unit_envelope() -> f32 {
    1.0
}
impl TaskEndReason {
    fn state(self) -> TaskState {
        match self {
            Self::Completed | Self::NaturalEnd | Self::FinishedByControl => TaskState::Finished,
            Self::CancelledByControl | Self::Replaced | Self::ScopeExited => TaskState::Cancelled,
            Self::Failed => TaskState::Failed,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenSpan {
    pub id: String,
    pub text: String,
    pub emphasis: bool,
    pub gate: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pause: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause_timeout_us: Option<Micros>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ruby: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<InlineImage>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DialogueReading {
    pub voice: Option<u32>,
    /// Live voices associated with this dialogue, independent of history's budget.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_voices: Vec<u32>,
    pub wait: VoiceWaitPolicy,
    pub revision: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dialogue {
    pub text_id: String,
    pub meaning_revision: u32,
    pub source_revision: u32,
    pub contract_digest: String,
    pub locale: String,
    pub font_plan_digest: String,
    pub speaker: String,
    /// Authored text reference, independent of the translated display name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub speaker_id: String,
    pub spans: Vec<FrozenSpan>,
    pub span: usize,
    pub cluster: usize,
    pub at_gate: bool,
    pub awaiting_advance: bool,
    pub last_reveal_us: Micros,
    pub interaction: u32,
    #[serde(default)]
    pub reading: Option<DialogueReading>,
    #[serde(default)]
    pub reveal_interval_us: Option<Micros>,
}
impl Dialogue {
    pub fn inline_images(&self) -> Vec<InlineImagePlacement> {
        let mut offset = 0;
        self.spans
            .iter()
            .filter_map(|span| {
                let at = offset;
                offset += span.text.len() as u32;
                span.image.as_ref().map(|image| InlineImagePlacement {
                    offset: at,
                    image: image.clone(),
                })
            })
            .collect()
    }
    pub fn full_text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
    pub fn visible_text(&self) -> String {
        let mut s = String::new();
        for (i, span) in self.spans.iter().enumerate() {
            if i < self.span {
                s.push_str(&span.text);
            } else if i == self.span {
                s.extend(span.text.graphemes(true).take(self.cluster));
                break;
            }
        }
        s
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modal_interaction: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shake: Option<crate::ShakeCapture>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sprite_shakes: BTreeMap<String, crate::ShakeCapture>,
    pub id: u32,
    pub name: String,
    pub frame: u32,
    pub scene_generation: u32,
    pub scope: Scope,
    pub effect: Effect,
    pub state: TaskState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_reason: Option<TaskEndReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_task: Option<u32>,
    #[serde(default = "unit_envelope")]
    pub audio_envelope: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_position_us: Option<Micros>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_device_elapsed_us: Option<Micros>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub voice_character: String,
    pub started_us: Micros,
    pub elapsed_us: Micros,
    pub milestones: BTreeSet<Milestone>,
    pub captured: f32,
    pub base: f32,
    pub dialogue: Option<Dialogue>,
    pub source: Vec<Node>,
    pub target: Vec<Node>,
    /// Spawned child task ids, in order, for a composition effect.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<u32>,
    /// How many of the composition's children have been spawned; always
    /// equals `children.len()`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cursor: u32,
}
fn is_zero(value: &u32) -> bool {
    *value == 0
}
/// The ramp duration of an envelope owner task (a timed stop or a gain
/// tween), or `None` when the task does not own an audio envelope.
fn envelope_owner_duration(task: &Task) -> Option<Micros> {
    match &task.effect {
        Effect::AudioStop { duration_us, .. } => Some(*duration_us),
        Effect::Tween {
            target: TweenTarget::AudioInstance { .. },
            duration_us,
            ..
        } => Some(*duration_us),
        _ => None,
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingActivation {
    pub id: u32,
    pub cue: String,
    pub next: String,
    pub effects: Vec<EffectDef>,
    pub dialogues: BTreeMap<String, Dialogue>,
    pub failed: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OfferedOption {
    pub id: String,
    pub label: String,
    pub enabled: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OfferedChoice {
    pub id: String,
    pub locale: String,
    pub font_plan_digest: String,
    pub interaction: u32,
    pub options: Vec<OfferedOption>,
    pub branches: BTreeMap<String, String>,
    pub deadline_us: Option<Micros>,
    pub default: Option<String>,
    /// Typed-result mode: the variable the chosen option's value writes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Explicit cancel target; absent means the interaction is modal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_cancel: Option<String>,
    /// Typed values of the offered options, by option id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<String, Value>,
    /// Semantic selection cursor for typed-result interactions; hover and
    /// keyboard focus are presentation transients and never reach this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Waiting {
    pub conditions: Vec<(u32, Milestone)>,
    pub next: String,
    pub on_cancelled: String,
    pub on_failed: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advance: Option<AdvanceContinuation>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdvanceContinuation {
    pub interaction: u32,
    pub next: String,
}
/// In-flight message-window reveal. Coverage interpolates linearly from the
/// captured `from_coverage` toward the hidden/visible endpoint so a reversing
/// op (show mid-hide) continues from the visual state it interrupted. Not a
/// task: it is committed by a `DialogueVisibility` operation, joins the
/// Story clock's deadline set, and is validated structurally on restore.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowReveal {
    pub style: StageTransition,
    pub to_visible: bool,
    pub from_coverage: f32,
    pub started_us: Micros,
    pub duration_us: Micros,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryVoice {
    /// Original instance identity is only a deduplication key, not a live
    /// task reference. Old tasks and decoded media may be reclaimed.
    pub instance: u32,
    pub asset: String,
    pub gain: f32,
}
pub const MAX_HISTORY_VOICES: usize = 64;

/// Frozen, offered rows only. Hidden options and typed values are not exposed
/// by history; revisiting it never re-evaluates predicates or commits a branch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryChoiceOption {
    pub id: String,
    pub text_id: String,
    pub label: String,
    pub enabled: bool,
    pub meaning_revision: u32,
    pub source_revision: u32,
    pub contract_digest: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum HistoryChoiceResolution {
    Selected { option: String },
    TimedOut { option: String },
    Cancelled,
}
impl HistoryChoiceResolution {
    pub fn selected(&self) -> Option<&str> {
        match self {
            Self::Selected { option } | Self::TimedOut { option } => Some(option),
            Self::Cancelled => None,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryChoice {
    pub id: String,
    pub options: Vec<HistoryChoiceOption>,
    pub resolution: HistoryChoiceResolution,
}
impl HistoryChoice {
    fn display_option(&self) -> Option<&HistoryChoiceOption> {
        match self.resolution.selected() {
            Some(selected) => self.options.iter().find(|o| o.id == selected && o.enabled),
            None => self.options.first(),
        }
    }
    fn display_text(&self) -> String {
        match self.resolution.selected() {
            Some(_) => self
                .display_option()
                .map(|o| o.label.clone())
                .unwrap_or_default(),
            None => self
                .options
                .iter()
                .map(|o| o.label.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryEntry {
    #[serde(default)]
    pub interaction: u32,
    pub text_id: String,
    pub meaning_revision: u32,
    pub source_revision: u32,
    pub contract_digest: String,
    pub locale: String,
    pub font_plan_digest: String,
    pub speaker: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub speaker_id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<InlineImagePlacement>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub voices: Vec<HistoryVoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<HistoryChoice>,
}
fn history_bytes(entry: &HistoryEntry) -> usize {
    entry.text.len()
        + entry
            .images
            .iter()
            .map(|i| i.image.asset.len() + 64)
            .sum::<usize>()
        + entry.speaker.len()
        + entry.speaker_id.len()
        + entry
            .voices
            .iter()
            .map(|voice| voice.asset.len() + 32)
            .sum::<usize>()
        + entry.choice.as_ref().map_or(0, |choice| {
            choice.id.len()
                + choice.resolution.selected().map_or(0, str::len)
                + choice
                    .options
                    .iter()
                    .map(|o| {
                        o.id.len() + o.text_id.len() + o.label.len() + o.contract_digest.len() + 32
                    })
                    .sum::<usize>()
        })
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub format: u32,
    pub game_id: String,
    pub revision: String,
    pub release: String,
    pub tick_us: Micros,
    pub variables: BTreeMap<String, Value>,
    pub frames: Vec<Frame>,
    pub scene: Vec<Node>,
    pub draft: Vec<Node>,
    pub scene_generation: u32,
    pub tasks: BTreeMap<u32, Task>,
    pub handles: BTreeMap<String, u32>,
    pub pending: Option<PendingActivation>,
    pub waiting: Option<Waiting>,
    pub choice: Option<OfferedChoice>,
    #[serde(default)]
    pub dialogue_hidden: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialogue_style: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dialogue_decorations: BTreeMap<DialogueDecorationSlot, DialogueDecoration>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub menu_disabled: bool,
    /// Deferred visibility flip: while set, `dialogue_hidden` still holds the
    /// pre-op value and the window's committed state lands at the deadline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_reveal: Option<WindowReveal>,
    #[serde(default)]
    pub dialogue_appearance: DialogueAppearance,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub audio_paused: BTreeSet<AudioBus>,
    pub history: Vec<HistoryEntry>,
    pub locale: String,
    pub next_id: u32,
    pub last_input: u32,
    pub rng: ChaCha8Rng,
    pub outcome: Option<String>,
    pub fault: Option<Diagnostic>,
    pub unsuspended_ops: u64,
}
#[derive(Debug, Clone)]
pub enum CoreInput {
    ModalClosed {
        task: u32,
        interaction: u32,
    },
    None,
    Advance {
        interaction: u32,
        sequence: u32,
    },
    /// Player reading policy; applies only when the dialogue actually completes.
    AdvanceReading {
        interaction: u32,
        sequence: u32,
        stop_voice: bool,
    },
    Choose {
        interaction: u32,
        option: String,
        sequence: u32,
    },
    /// Move the semantic selection cursor of a typed-result interaction.
    /// No story progress, no checkpoint; snapshot-relevant only.
    SelectChoice {
        interaction: u32,
        option: String,
        sequence: u32,
    },
    /// Cancel a typed-result interaction through its declared cancel target.
    CancelChoice {
        interaction: u32,
        sequence: u32,
    },
    Time {
        delta_us: u64,
    },
    Prepared {
        activation: u32,
    },
    PreparationFailed {
        activation: u32,
        message: String,
    },
    AudioEnded {
        task: u32,
    },
    TaskFailed {
        task: u32,
        message: String,
    },
}
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CoreIntent {
    PrepareContent {
        module: String,
        locale: String,
    },
    Prepare {
        activation: u32,
        cue: String,
    },
    AudioStart {
        task: u32,
        asset: String,
        bus: AudioBus,
        looped: bool,
        loop_region: Option<AudioLoopRegion>,
        position_us: Micros,
        gain: f32,
    },
    AudioCharacter {
        task: u32,
        character: String,
    },
    AudioEnvelope {
        owner: Option<u32>,
        elapsed_us: Micros,
        task: u32,
        from: f32,
        to: f32,
        duration_us: Micros,
    },
    AudioStop {
        task: u32,
    },
    ProfileMerge {
        key: String,
    },
    ProfileValueAssign {
        key: String,
        value: Value,
    },
    Checkpoint,
    Trace {
        event: String,
        at: String,
    },
}
#[derive(Debug, Clone)]
pub struct CoreStep {
    /// Instructions and clock boundaries consumed by this call.
    pub work_used: u32,
    /// Elapsed time the caller must resubmit after a budget yield.
    pub remaining_time_us: u64,
    pub intents: Vec<CoreIntent>,
    pub waiting: bool,
    pub location: String,
    pub changed: bool,
}
#[derive(Clone)]
pub struct Core {
    program: ValidatedProgram,
    state: Snapshot,
    intents: Vec<CoreIntent>,
    work_remaining: u32,
    remaining_time_us: u64,
    text_speed: f32,
    profile_facts: BTreeSet<String>,
    profile_values: BTreeMap<String, Value>,
}
impl Core {
    pub fn new(program: ValidatedProgram, release: String, locale: String) -> Result<Self> {
        let entry = program.program().entry.clone();
        Self::new_at(program, release, locale, &entry)
    }
    /// Start a fresh session at a declared, parameterless entry (e.g. a replay).
    pub fn new_at(
        program: ValidatedProgram,
        release: String,
        locale: String,
        function: &str,
    ) -> Result<Self> {
        let p = program.program();
        if !p.locales.contains_key(&locale) {
            return Err(Diagnostic::new("E_LOCALE", "new", locale));
        }
        let entry = p
            .function_signature(function)
            .ok_or_else(|| Diagnostic::new("E_FUNCTION", "entry", "missing interface"))?;
        if !entry.params.is_empty() || entry.returns.is_some() {
            return Err(Diagnostic::new(
                "E_FUNCTION",
                function,
                "entry must not require parameters or return a value",
            ));
        }
        let frame = Frame {
            id: 1,
            function: function.into(),
            block: entry.entry,
            op: 0,
            op_id: entry.entry_op,
            locals: BTreeMap::new(),
            return_to: None,
            result: None,
        };
        let state = Snapshot {
            format: SNAPSHOT_VERSION,
            game_id: p.game_id.clone(),
            revision: p.revision.clone(),
            release,
            tick_us: Micros(0),
            variables: (*p.variables).clone(),
            frames: vec![frame],
            scene: vec![],
            draft: vec![],
            scene_generation: 0,
            tasks: BTreeMap::new(),
            handles: BTreeMap::new(),
            pending: None,
            waiting: None,
            choice: None,
            dialogue_hidden: false,
            dialogue_style: None,
            dialogue_decorations: BTreeMap::new(),
            menu_disabled: false,
            window_reveal: None,
            dialogue_appearance: DialogueAppearance::default(),
            audio_paused: BTreeSet::new(),
            history: vec![],
            locale,
            next_id: 2,
            last_input: 0,
            rng: ChaCha8Rng::from_seed([42; 32]),
            outcome: None,
            fault: None,
            unsuspended_ops: 0,
        };
        Ok(Self {
            program,
            state,
            intents: vec![],
            work_remaining: 0,
            remaining_time_us: 0,
            text_speed: 1.,
            profile_facts: BTreeSet::new(),
            profile_values: BTreeMap::new(),
        })
    }
    /// Player preference captured by the next dialogue; never scales Story time.
    pub fn set_text_speed(&mut self, speed: f32) -> Result<()> {
        if !speed.is_finite() || !(0.25..=4.).contains(&speed) {
            return Err(self.error("E_PREFERENCE", "text speed outside 0.25..4"));
        }
        self.text_speed = speed;
        Ok(())
    }
    /// Facts are independent of Story snapshots and may only accumulate.
    pub fn merge_profile_facts(&mut self, facts: &BTreeSet<String>) -> Result<()> {
        if facts.iter().any(|k| k.is_empty() || k.len() > 1024) {
            return Err(self.error("E_PROFILE", "profile fact limit"));
        }
        self.profile_facts.extend(facts.iter().cloned());
        Ok(())
    }
    pub fn state(&self) -> &Snapshot {
        &self.state
    }
    /// Profile data is supplied by the owner, separately from a Story save.
    pub fn set_profile_values(&mut self, values: &BTreeMap<String, Value>) -> Result<()> {
        if !valid_profile_values(values) {
            return Err(self.error("E_PROFILE", "profile value limit"));
        }
        self.profile_values = values.clone();
        Ok(())
    }
    pub fn program(&self) -> &RuntimeProgramView {
        self.program.program()
    }
    pub fn validated_program(&self) -> &ValidatedProgram {
        &self.program
    }
    /// Installing verified immutable bodies does not replay any story operation.
    pub fn replace_program(&mut self, program: ValidatedProgram) -> Result<()> {
        self.check_program_replacement(&program)?;
        self.program = program;
        Ok(())
    }
    /// Check an admission before committing it to the shared residency ledger.
    /// The caller can then commit and rebind without cloning the snapshot.
    pub fn check_program_replacement(&self, program: &ValidatedProgram) -> Result<()> {
        let same_root = match (self.program.runtime_root(), program.runtime_root()) {
            (Some(current), Some(next)) => std::ptr::eq(current, next),
            (None, None) => true,
            _ => false,
        };
        if !same_root
            || program.program().revision != self.state.revision
            || program.program().game_id != self.state.game_id
        {
            return Err(self.error("E_MODULE", "content identity changed"));
        }
        // A runtime view may have evicted unpinned bodies. Rebinding is safe
        // only when every body the live continuation can execute or consult
        // is still present in the replacement view. Task effects and frozen
        // dialogue payloads are owned by the snapshot and intentionally do
        // not keep their original definition packages resident.
        if let Some(root) = program.runtime_root() {
            let missing = |message: &str| self.error("E_CONTENT_MISSING", message);
            for frame in &self.state.frames {
                // Core starts with a metadata-only entry frame while the host
                // is still presenting the title screen. In that state there
                // is no executable body to preserve yet. Once a frame body
                // has been resident, every replacement must retain the
                // already-present parts of its execution package. A freshly
                // created Core may have cached Code while awaiting Static.
                if self
                    .program
                    .program()
                    .functions
                    .get(&frame.function)
                    .is_none()
                {
                    continue;
                }
                let module = root
                    .function_index
                    .get(&frame.function)
                    .map(|index| index.module.as_str())
                    .ok_or_else(|| missing("active function index"))?;
                let code_key = ContentKey::Code {
                    module: module.into(),
                };
                let mut keys = root.content_prerequisites(&code_key);
                keys.insert(code_key);
                for key in keys {
                    if self.program.contains_content_body(&key) && !program.is_resident(&key) {
                        return Err(missing(&format!("active frame body missing: {key:?}")));
                    }
                }
                if program.program().functions.get(&frame.function).is_none() {
                    return Err(missing("active function body missing"));
                }
            }
            for nodes in std::iter::once(&self.state.scene)
                .chain(std::iter::once(&self.state.draft))
                .chain(
                    self.state
                        .tasks
                        .values()
                        .flat_map(|t| [&t.source, &t.target]),
                )
            {
                for binding in nodes.iter().filter_map(|n| n.timeline_binding.as_deref()) {
                    if program.program().sprite_timelines.get(binding).is_none() {
                        return Err(missing("frozen scene timeline body missing"));
                    }
                }
            }
            for task in self
                .state
                .tasks
                .values()
                .filter(|t| t.state == TaskState::Running)
            {
                if let Effect::SpriteTimeline { timeline, .. } = &task.effect {
                    if program.program().sprite_timelines.get(timeline).is_none() {
                        return Err(missing("active timeline body missing"));
                    }
                }
            }
            if let Some(pending) = &self.state.pending {
                let module = root
                    .cue_owners
                    .get(&pending.cue)
                    .ok_or_else(|| missing("active cue index"))?;
                if !program.is_resident(&ContentKey::Static {
                    module: module.clone(),
                }) || program.program().cues.get(&pending.cue).is_none()
                {
                    return Err(missing("active cue body missing"));
                }
            }
            if let Some(choice) = &self.state.choice {
                let module = root
                    .choice_owners
                    .get(&choice.id)
                    .ok_or_else(|| missing("active choice index"))?;
                if !program.is_resident(&ContentKey::Static {
                    module: module.clone(),
                }) || program.program().choices.get(&choice.id).is_none()
                {
                    return Err(missing("active choice body missing"));
                }
            }
        }
        Ok(())
    }
    pub fn snapshot(&self) -> Snapshot {
        self.state.clone()
    }
    pub fn location(&self) -> String {
        self.state
            .frames
            .last()
            .map(|f| format!("{}/{}/{}", f.function, f.block, f.op))
            .unwrap_or_else(|| "end".into())
    }
    /// Predict one content-only target along the normal success path of a
    /// suspended story. This never evaluates expressions or advances the VM.
    pub fn prefetch_module(&self) -> Option<String> {
        if self.state.waiting.is_none()
            || self.state.pending.is_some()
            || self.state.choice.is_some()
            || self.state.fault.is_some()
            || self.state.outcome.is_some()
        {
            return None;
        }
        let root = self.program.runtime_root()?;
        let frame = self.state.frames.last()?;
        let module = &root.function_index.get(&frame.function)?.module;
        let function = self.program.program().functions.get(&frame.function)?;
        let mut block = frame.block.as_str();
        let mut visited = BTreeSet::new();
        for _ in 0..64 {
            if !visited.insert(block) {
                return None;
            }
            block = match &function.blocks.get(block)?.terminator {
                Terminator::Goto { target } => target,
                Terminator::Activate { next, .. } | Terminator::Await { next, .. } => next,
                Terminator::Call { function, .. } => {
                    let target = &root.function_index.get(function)?.module;
                    return (target != module).then(|| target.clone());
                }
                _ => return None,
            };
        }
        None
    }
    /// The first cue reached through straight-line control flow after a stable
    /// wait. This only reads resident definitions and never executes an op.
    pub fn predict_next_cue(&self) -> Option<String> {
        if self.state.pending.is_some()
            || self.state.choice.is_some()
            || self.state.fault.is_some()
            || self.state.outcome.is_some()
        {
            return None;
        }
        let waiting = self.state.waiting.as_ref()?;
        let frame = self.state.frames.last()?;
        let function = self.program.program().functions.get(&frame.function)?;
        let mut block = waiting.next.as_str();
        let mut visited = BTreeSet::new();
        for _ in 0..64 {
            if !visited.insert(block) {
                return None;
            }
            let body = function.blocks.get(block)?;
            match &body.terminator {
                Terminator::Goto { target } => block = target,
                Terminator::Activate { cue, .. } => {
                    return self
                        .program
                        .program()
                        .cues
                        .contains_key(cue)
                        .then(|| cue.clone());
                }
                _ => return None,
            }
        }
        None
    }
    pub fn set_locale(&mut self, locale: &str) -> Result<()> {
        if !self.program().locales.contains_key(locale)
            || !self.program().locale_config.text.contains_key(locale)
        {
            return Err(self.error("E_LOCALE", locale));
        }
        self.state.locale = locale.into();
        Ok(())
    }
    fn error(&self, code: &str, message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(code, self.location(), message)
    }
    fn id(&mut self) -> Result<u32> {
        let id = self.state.next_id;
        self.state.next_id = id
            .checked_add(1)
            .ok_or_else(|| self.error("E_LIMIT", "instance counter overflow"))?;
        Ok(id)
    }
    fn frame(&self) -> &Frame {
        self.state.frames.last().expect("validated active frame")
    }
    fn frame_mut(&mut self) -> &mut Frame {
        self.state
            .frames
            .last_mut()
            .expect("validated active frame")
    }
    fn jump(&mut self, block: String) {
        let f = self.frame_mut();
        f.block = block;
        f.op = 0;
        self.sync_op_id();
    }
    fn sync_op_id(&mut self) {
        let f = self.frame();
        let id = self.program().functions[&f.function].blocks[&f.block]
            .ops
            .get(f.op)
            .map(|o| o.id.clone())
            .unwrap_or_else(|| "@terminator".into());
        self.frame_mut().op_id = id;
    }
    fn trace(&mut self, event: impl Into<String>) {
        self.intents.push(CoreIntent::Trace {
            event: event.into(),
            at: self.location(),
        });
    }
    fn read(&self, name: &str) -> Result<Value> {
        self.frame()
            .locals
            .get(name)
            .or_else(|| self.state.variables.get(name))
            .cloned()
            .ok_or_else(|| self.error("E_UNINITIALIZED", name))
    }
    fn write(&mut self, name: &str, v: Value) -> Result<()> {
        if self.state.variables.contains_key(name) {
            self.state.variables.insert(name.into(), v);
        } else {
            self.frame_mut().locals.insert(name.into(), v);
        }
        Ok(())
    }
    pub fn eval(&self, e: &Expr) -> Result<Value> {
        use BinaryOp::*;
        use Value::*;
        match e {
            Expr::Const { value } => Ok(value.clone()),
            Expr::Var { name } => self.read(name),
            Expr::ToF80 { value } => match self.eval(value)? {
                I32(v) => Ok(F80(Float80::from_i32(v))),
                F80(v) => Ok(F80(v)),
                _ => Err(self.error("E_TYPE", "to_f80 requires numeric input")),
            },
            Expr::ToI32 { value } => match self.eval(value)? {
                I32(v) => Ok(I32(v)),
                F80(v) => v
                    .to_i32()
                    .map(I32)
                    .ok_or_else(|| self.error("E_ARITHMETIC", "integer conversion overflow")),
                _ => Err(self.error("E_TYPE", "to_i32 requires numeric input")),
            },
            Expr::Not { value } => match self.eval(value)? {
                Bool(b) => Ok(Bool(!b)),
                _ => Err(self.error("E_TYPE", "not")),
            },
            Expr::Binary { op, left, right } => {
                let l = self.eval(left)?;
                if matches!((op, &l), (And, Bool(false))) {
                    return Ok(Bool(false));
                }
                if matches!((op, &l), (Or, Bool(true))) {
                    return Ok(Bool(true));
                }
                let r = self.eval(right)?;
                let overflow = || self.error("E_ARITHMETIC", "overflow or division by zero");
                match (op, l, r) {
                    (Add, I32(a), I32(b)) => a.checked_add(b).map(I32).ok_or_else(overflow),
                    (Sub, I32(a), I32(b)) => a.checked_sub(b).map(I32).ok_or_else(overflow),
                    (Mul, I32(a), I32(b)) => a.checked_mul(b).map(I32).ok_or_else(overflow),
                    (Div, I32(a), I32(b)) => a.checked_div(b).map(I32).ok_or_else(overflow),
                    (Rem, I32(a), I32(b)) => a.checked_rem(b).map(I32).ok_or_else(overflow),
                    (op @ (Add | Sub | Mul | Div | Rem), F80(a), F80(b)) => {
                        a.checked_binary(*op, b).map(F80).ok_or_else(overflow)
                    }
                    (Lt, F80(a), F80(b)) => Ok(Bool(a < b)),
                    (Le, F80(a), F80(b)) => Ok(Bool(a <= b)),
                    (Gt, F80(a), F80(b)) => Ok(Bool(a > b)),
                    (Ge, F80(a), F80(b)) => Ok(Bool(a >= b)),
                    (Eq, a, b) => Ok(Bool(a == b)),
                    (Ne, a, b) => Ok(Bool(a != b)),
                    (Lt, I32(a), I32(b)) => Ok(Bool(a < b)),
                    (Le, I32(a), I32(b)) => Ok(Bool(a <= b)),
                    (Gt, I32(a), I32(b)) => Ok(Bool(a > b)),
                    (Ge, I32(a), I32(b)) => Ok(Bool(a >= b)),
                    (And, Bool(a), Bool(b)) => Ok(Bool(a && b)),
                    (Or, Bool(a), Bool(b)) => Ok(Bool(a || b)),
                    (Concat, String(a), String(b)) => {
                        if a.len() + b.len() > 128 * 1024 {
                            return Err(self.error("E_LIMIT", "string length"));
                        }
                        Ok(String(a + &b))
                    }
                    _ => Err(self.error("E_TYPE", "binary operands")),
                }
            }
        }
    }
    fn predicate(&self, e: &Option<Expr>) -> Result<bool> {
        match e {
            None => Ok(true),
            Some(e) => match self.eval(e)? {
                Value::Bool(b) => Ok(b),
                _ => Err(self.error("E_TYPE", "predicate")),
            },
        }
    }
    fn freeze_text(&self, id: &str) -> Result<Vec<FrozenSpan>> {
        let doc = self.program().locales[&self.state.locale]
            .get(id)
            .ok_or_else(|| self.error("E_TEXT", id))?;
        doc.spans
            .iter()
            .map(|s| {
                Ok(match s {
                    Span::Image { id, image } => FrozenSpan {
                        id: id.clone(),
                        text: "\u{fffc}".into(),
                        emphasis: false,
                        gate: false,
                        pause: false,
                        pause_timeout_us: None,
                        ruby: None,
                        image: Some(image.clone()),
                    },
                    Span::Ruby { id, text, reading } => FrozenSpan {
                        id: id.clone(),
                        text: text.clone(),
                        emphasis: false,
                        gate: false,
                        pause: false,
                        pause_timeout_us: None,
                        ruby: Some(reading.clone()),
                        image: None,
                    },
                    Span::Text { id, text, emphasis } => FrozenSpan {
                        id: id.clone(),
                        text: text.clone(),
                        emphasis: *emphasis,
                        gate: false,
                        pause: false,
                        pause_timeout_us: None,
                        ruby: None,
                        image: None,
                    },
                    Span::Break { id } => FrozenSpan {
                        id: id.clone(),
                        text: "\n".into(),
                        emphasis: false,
                        gate: false,
                        pause: false,
                        pause_timeout_us: None,
                        ruby: None,
                        image: None,
                    },
                    Span::Gate { id } => FrozenSpan {
                        id: id.clone(),
                        text: String::new(),
                        emphasis: false,
                        gate: true,
                        pause: false,
                        pause_timeout_us: None,
                        ruby: None,
                        image: None,
                    },
                    Span::Pause { id, timeout_us } => FrozenSpan {
                        id: id.clone(),
                        text: String::new(),
                        emphasis: false,
                        gate: false,
                        pause: true,
                        pause_timeout_us: *timeout_us,
                        ruby: None,
                        image: None,
                    },
                    Span::Param { id, name } => {
                        let v = match self.read(name)? {
                            Value::String(v) => v,
                            Value::I32(v) => v.to_string(),
                            Value::Bool(v) => v.to_string(),
                            Value::F80(v) => v.to_string(),
                        };
                        FrozenSpan {
                            id: id.clone(),
                            text: format!("\u{2068}{v}\u{2069}"),
                            emphasis: false,
                            gate: false,
                            pause: false,
                            pause_timeout_us: None,
                            ruby: None,
                            image: None,
                        }
                    }
                })
            })
            .collect()
    }
    fn text(&self, id: &str) -> Result<String> {
        Ok(self
            .freeze_text(id)?
            .iter()
            .map(|s| s.text.as_str())
            .collect())
    }
    pub fn step(&mut self, input: CoreInput, semantic_budget: u32) -> CoreStep {
        self.intents.clear();
        let limit = semantic_budget.min(100_000);
        self.work_remaining = limit;
        self.remaining_time_us = 0;
        let changed = !matches!(input, CoreInput::None);
        if self.state.fault.is_none() && self.state.outcome.is_none() {
            if let Err(e) = self.process(input).and_then(|_| self.run()) {
                self.state.fault = Some(e);
            }
        }
        let content_waiting = self
            .intents
            .iter()
            .any(|i| matches!(i, CoreIntent::PrepareContent { .. }));
        CoreStep {
            work_used: limit - self.work_remaining,
            remaining_time_us: self.remaining_time_us,
            intents: std::mem::take(&mut self.intents),
            waiting: content_waiting
                || self.state.pending.is_some()
                || self.state.waiting.is_some()
                || self.state.choice.is_some()
                || self.state.outcome.is_some()
                || self.state.fault.is_some(),
            location: self.location(),
            changed,
        }
    }
    fn process(&mut self, input: CoreInput) -> Result<()> {
        match input {
            CoreInput::None => {}
            CoreInput::Prepared { activation } => {
                if self
                    .state
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.id == activation)
                {
                    self.commit()?;
                }
            }
            CoreInput::PreparationFailed {
                activation,
                message,
            } => {
                if let Some(p) = self.state.pending.as_mut().filter(|p| p.id == activation) {
                    p.failed = Some(message);
                }
            }
            CoreInput::Time { delta_us } => {
                if self.state.pending.is_none() {
                    self.advance_time(delta_us)?;
                }
            }
            CoreInput::Advance {
                interaction,
                sequence,
            } => self.advance_reading(interaction, sequence, false)?,
            CoreInput::AdvanceReading {
                interaction,
                sequence,
                stop_voice,
            } => self.advance_reading(interaction, sequence, stop_voice)?,
            CoreInput::Choose {
                interaction,
                option,
                sequence,
            } => {
                if sequence <= self.state.last_input {
                    return Ok(());
                }
                if let Some(c) = &self.state.choice {
                    if c.interaction == interaction
                        && c.options.iter().any(|o| o.id == option && o.enabled)
                    {
                        self.state.last_input = sequence;
                        // The VM owns the typed write; the host only names an
                        // offered option and never supplies the value itself.
                        let typed = c.result.as_ref().and_then(|target| {
                            Some((target.clone(), c.values.get(&option)?.clone()))
                        });
                        let dest = c.branches[&option].clone();
                        let history = self.choice_history(
                            c,
                            HistoryChoiceResolution::Selected {
                                option: option.clone(),
                            },
                        );
                        if let Some((target, value)) = typed {
                            self.write(&target, value)?;
                        }
                        if let Some(history) = history {
                            self.push_history(history);
                        }
                        self.trace(format!("choose:{option}"));
                        self.state.choice = None;
                        self.jump(dest);
                        self.state.unsuspended_ops = 0;
                        self.intents.push(CoreIntent::Checkpoint);
                    }
                }
            }
            CoreInput::SelectChoice {
                interaction,
                option,
                sequence,
            } => {
                let _ = sequence;
                if let Some(c) = &mut self.state.choice {
                    if c.interaction == interaction
                        && c.result.is_some()
                        && c.options.iter().any(|o| o.id == option && o.enabled)
                    {
                        // A cursor move is an observation on a suspended
                        // interaction: no input identity, no story progress.
                        c.selected = Some(option);
                    }
                }
            }
            CoreInput::CancelChoice {
                interaction,
                sequence,
            } => {
                if sequence <= self.state.last_input {
                    return Ok(());
                }
                let dest = self.state.choice.as_ref().and_then(|c| {
                    (c.interaction == interaction)
                        .then(|| c.on_cancel.clone())
                        .flatten()
                });
                if let Some(dest) = dest {
                    let history = self.choice_history(
                        self.state.choice.as_ref().unwrap(),
                        HistoryChoiceResolution::Cancelled,
                    );
                    self.state.last_input = sequence;
                    if let Some(history) = history {
                        self.push_history(history);
                    }
                    self.trace("input:cancel");
                    self.state.choice = None;
                    self.jump(dest);
                    self.state.unsuspended_ops = 0;
                    self.intents.push(CoreIntent::Checkpoint);
                }
            }
            CoreInput::ModalClosed { task, interaction } => {
                if self.state.tasks.get(&task).is_some_and(|t| {
                    t.state == TaskState::Running
                        && matches!(t.effect, Effect::StoryModal { .. })
                        && t.modal_interaction == Some(interaction)
                }) {
                    self.finish_task(task, TaskState::Finished)?;
                    self.state.unsuspended_ops = 0;
                    self.intents.push(CoreIntent::Checkpoint);
                }
            }
            CoreInput::AudioEnded { task } => {
                if self.state.tasks.get(&task).is_some_and(|t| {
                    matches!(t.effect, Effect::Audio { looped: false, .. })
                        && t.state == TaskState::Running
                }) {
                    self.end_task(task, TaskEndReason::NaturalEnd)?;
                }
            }
            CoreInput::TaskFailed { task, message } => {
                if self.state.tasks.contains_key(&task) {
                    self.trace(format!("task_failed:{task}:{message}"));
                    self.finish_task(task, TaskState::Failed)?;
                }
            }
        }
        Ok(())
    }
    fn push_history(&mut self, entry: HistoryEntry) {
        // This is derived reading data, not a gameplay limit. Oversized rows
        // are discarded whole without evicting an otherwise valid backlog.
        if history_bytes(&entry) > 4 * 1024 * 1024 {
            return;
        }
        self.state.history.push(entry);
        while self.state.history.len() > 1000
            || self.state.history.iter().map(history_bytes).sum::<usize>() > 4 * 1024 * 1024
        {
            self.state.history.remove(0);
        }
    }
    fn choice_history(
        &self,
        offered: &OfferedChoice,
        resolution: HistoryChoiceResolution,
    ) -> Option<HistoryEntry> {
        let p = self.program();
        let definition = p.choices.get(&offered.id)?;
        let options = offered
            .options
            .iter()
            .map(|row| {
                let option = definition.options.iter().find(|o| o.id == row.id)?;
                let identity = p.text_identity(&option.text)?;
                Some(HistoryChoiceOption {
                    id: row.id.clone(),
                    text_id: option.text.clone(),
                    label: row.label.clone(),
                    enabled: row.enabled,
                    meaning_revision: identity.meaning_revision,
                    source_revision: identity.source_revision,
                    contract_digest: identity.contract_digest.clone(),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        let choice = HistoryChoice {
            id: offered.id.clone(),
            options,
            resolution,
        };
        let option = choice.display_option()?;
        Some(HistoryEntry {
            interaction: offered.interaction,
            text_id: option.text_id.clone(),
            meaning_revision: option.meaning_revision,
            source_revision: option.source_revision,
            contract_digest: option.contract_digest.clone(),
            locale: offered.locale.clone(),
            font_plan_digest: offered.font_plan_digest.clone(),
            speaker: String::new(),
            speaker_id: String::new(),
            text: choice.display_text(),
            images: vec![],
            voices: vec![],
            choice: Some(choice),
        })
    }
    fn run(&mut self) -> Result<()> {
        // Compositions advance even while the VM itself is parked on a wait,
        // a choice or a content barrier: the chain is autonomous.
        self.advance_compositions()?;
        if self
            .intents
            .iter()
            .any(|i| matches!(i, CoreIntent::PrepareContent { .. }))
        {
            return Ok(());
        }
        while self.work_remaining > 0 {
            self.advance_compositions()?;
            if self.state.pending.is_some()
                || self.state.choice.is_some()
                || self.state.outcome.is_some()
            {
                return Ok(());
            }
            if self.state.waiting.is_some() && !self.resolve_wait()? {
                return Ok(());
            }
            // Content barriers precede semantic execution. No arguments, RNG,
            // instance IDs or story time are consumed while a body is missing.
            if let Some(module) = self.missing_content()? {
                self.intents.push(CoreIntent::PrepareContent {
                    module,
                    locale: self.state.locale.clone(),
                });
                return Ok(());
            }
            self.work_remaining -= 1;
            self.state.unsuspended_ops += 1;
            if self.state.unsuspended_ops > 1_000_000 {
                return Err(self.error("E_FUEL", "non-suspending loop"));
            }
            let frame = self.frame();
            match self
                .program
                .instruction(&frame.function, &frame.block, frame.op)
                .clone()
            {
                crate::validate::Instruction::Op(op) => {
                    self.execute_op(&op.operation)?;
                    self.trace(format!("op:{}", op.id));
                    self.frame_mut().op += 1;
                    self.sync_op_id();
                }
                crate::validate::Instruction::Term(t) => self.terminate(t)?,
            }
        }
        Ok(())
    }
    fn missing_content(&self) -> Result<Option<String>> {
        let p = self.program();
        let frame = self.frame();
        let missing = |owner: Option<&str>, id: &str| {
            owner
                .map(|module| Some(module.to_owned()))
                .ok_or_else(|| self.error("E_CONTENT_INDEX", id))
        };
        if let Some(root) = p.runtime_root() {
            let module = root
                .function_module(&frame.function)
                .ok_or_else(|| self.error("E_CONTENT_INDEX", &frame.function))?;
            for prerequisite in root.content_prerequisites(&ContentKey::Code {
                module: module.into(),
            }) {
                if !self.program.is_resident(&prerequisite) {
                    return Ok(Some(module.into()));
                }
            }
        }
        let Some(function) = p.functions.get(&frame.function) else {
            return missing(p.function_module(&frame.function), &frame.function);
        };
        let block = function
            .blocks
            .get(&frame.block)
            .ok_or_else(|| self.error("E_CONTENT_INDEX", &frame.block))?;
        if frame.op < block.ops.len() {
            return Ok(None);
        }
        let mut texts = vec![];
        match &block.terminator {
            Terminator::Call { function, .. } => {
                let owner = p.function_module(function);
                let missing_static = p.runtime_root().is_some()
                    && owner.is_some_and(|module| {
                        !self.program.is_resident(&ContentKey::Static {
                            module: module.into(),
                        })
                    });
                if p.functions.get(function).is_none() || missing_static {
                    return missing(owner, function);
                }
            }
            Terminator::Activate { cue, .. } => {
                let Some(definition) = p.cues.get(cue) else {
                    return missing(
                        p.runtime_root()
                            .and_then(|root| root.cue_owners.get(cue))
                            .map(String::as_str),
                        cue,
                    );
                };
                for effect in &definition.effects {
                    if let Effect::Dialogue { text, speaker, .. } = &effect.effect {
                        texts.push(text);
                        if !speaker.is_empty() {
                            texts.push(speaker);
                        }
                    }
                }
            }
            Terminator::Interact { choice, .. } => {
                let Some(definition) = p.choices.get(choice) else {
                    return missing(
                        p.runtime_root()
                            .and_then(|root| root.choice_owners.get(choice))
                            .map(String::as_str),
                        choice,
                    );
                };
                texts.extend(definition.options.iter().map(|o| &o.text));
            }
            _ => {}
        }
        for id in texts {
            if p.texts.get(id).is_none()
                || p.locales
                    .get(&self.state.locale)
                    .and_then(|texts| texts.get(id))
                    .is_none()
            {
                return missing(p.text_module(id), id);
            }
        }
        Ok(None)
    }
    fn execute_op(&mut self, op: &Operation) -> Result<()> {
        match op {
            Operation::MenuAccess { enabled } => self.state.menu_disabled = !enabled,
            Operation::AudioPause { bus, paused } => {
                if *paused {
                    self.state.audio_paused.insert(*bus);
                } else {
                    self.state.audio_paused.remove(bus);
                }
            }
            Operation::Assign { target, value } => {
                let v = self.eval(value)?;
                self.write(target, v)?;
            }
            Operation::Random { target, min, max } => {
                let width = (*max as i64 - *min as i64 + 1) as u64;
                let zone = (1u64 << 32) / width * width;
                let mut x = self.state.rng.next_u32() as u64;
                while x >= zone {
                    x = self.state.rng.next_u32() as u64;
                }
                self.write(
                    target,
                    Value::I32((*min as i64 + (x % width) as i64) as i32),
                )?;
            }
            Operation::DraftPatch {
                node,
                property,
                value,
            } => {
                if self.state.draft.is_empty() {
                    self.state.draft = self.sample_scene();
                }
                let at = self.location();
                let n = self
                    .state
                    .draft
                    .iter_mut()
                    .find(|n| &n.id == node)
                    .ok_or_else(|| Diagnostic::new("E_NODE", at, node))?;
                n.set(*property, *value);
            }
            Operation::DialogueVisibility {
                visible,
                transition,
                duration_us,
            } => {
                // An instant flip commits directly. A styled flip defers the
                // committed value to the reveal deadline; the window keeps its
                // old committed state while presentation interpolates coverage
                // from the captured start (so a reversing op continues from
                // the visual state it interrupted).
                let reveal = transition
                    .clone()
                    .filter(|style| duration_us.0 > 0 && style.valid())
                    .filter(|_| {
                        self.state.window_reveal.is_some() || self.state.dialogue_hidden == *visible
                    });
                match reveal {
                    Some(style) => {
                        let from_coverage = self.window_coverage();
                        self.state.window_reveal = Some(WindowReveal {
                            style,
                            to_visible: *visible,
                            from_coverage,
                            started_us: self.state.tick_us,
                            duration_us: *duration_us,
                        });
                    }
                    None => {
                        self.state.dialogue_hidden = !visible;
                        self.state.window_reveal = None;
                    }
                }
            }
            Operation::ProfileRead { target, key } => {
                self.write(
                    target,
                    Value::I32(i32::from(self.profile_facts.contains(key))),
                )?;
            }
            Operation::ProfileValueRead { target, key } => {
                let default = self
                    .program()
                    .variables
                    .get(target)
                    .ok_or_else(|| self.error("E_PROFILE", "unknown profile target"))?;
                let value = self.profile_values.get(key).unwrap_or(default).clone();
                if value.ty() != default.ty() {
                    return Err(self.error("E_PROFILE", "stored profile value has wrong type"));
                }
                self.write(target, value)?;
            }
            Operation::ProfileValueAssign { target, key, value } => {
                let value = self.eval(value)?;
                let mut values = self.profile_values.clone();
                values.insert(key.clone(), value.clone());
                if !valid_profile_values(&values) {
                    return Err(self.error("E_PROFILE", "profile value limit"));
                }
                self.write(target, value.clone())?;
                self.profile_values = values;
                self.intents.push(CoreIntent::ProfileValueAssign {
                    key: key.clone(),
                    value,
                });
            }
            Operation::ProfileMerge { key } => {
                self.merge_profile_facts(&BTreeSet::from([key.clone()]))?;
                self.intents
                    .push(CoreIntent::ProfileMerge { key: key.clone() });
            }
            Operation::TaskControl { task, action } => {
                let id = *self
                    .state
                    .handles
                    .get(task)
                    .ok_or_else(|| self.error("E_TASK", task))?;
                self.end_task(
                    id,
                    match action {
                        TaskAction::Cancel => TaskEndReason::CancelledByControl,
                        TaskAction::Finish => TaskEndReason::FinishedByControl,
                    },
                )?;
            }
            Operation::DialogueVoice { task, voice, wait } => {
                let id = *self
                    .state
                    .handles
                    .get(task)
                    .ok_or_else(|| self.error("E_TASK", task))?;
                let voice_id = voice
                    .as_ref()
                    .map(|name| {
                        let id = *self
                            .state
                            .handles
                            .get(name)
                            .ok_or_else(|| self.error("E_TASK", name))?;
                        if !matches!(
                            self.state.tasks[&id].effect,
                            Effect::Audio {
                                bus: AudioBus::Voice,
                                looped: false,
                                ..
                            }
                        ) {
                            return Err(self.error(
                                "E_TASK_TYPE",
                                "reading voice must be non-looping Voice audio",
                            ));
                        }
                        Ok(id)
                    })
                    .transpose()?;
                let recorded_voice = voice_id.map(|instance| {
                    let Effect::Audio { asset, gain, .. } = &self.state.tasks[&instance].effect
                    else {
                        unreachable!("validated reading voice");
                    };
                    HistoryVoice {
                        instance,
                        asset: asset.clone(),
                        gain: *gain,
                    }
                });
                let mut active_voices = self.state.tasks[&id]
                    .dialogue
                    .as_ref()
                    .and_then(|d| d.reading.as_ref())
                    .map(|r| {
                        r.active_voices
                            .iter()
                            .copied()
                            .chain(r.voice)
                            .collect::<BTreeSet<_>>()
                    })
                    .unwrap_or_default();
                active_voices.extend(voice_id);
                active_voices.retain(|id| self.state.tasks[id].state == TaskState::Running);
                let at = self.location();
                let t = self.state.tasks.get_mut(&id).unwrap();
                if t.state != TaskState::Running {
                    return Err(Diagnostic::new("E_TASK", at, "dialogue has ended"));
                }
                let d = t
                    .dialogue
                    .as_mut()
                    .ok_or_else(|| Diagnostic::new("E_TASK_TYPE", &at, task))?;
                let revision = d
                    .reading
                    .as_ref()
                    .map_or(0, |r| r.revision)
                    .checked_add(1)
                    .ok_or_else(|| Diagnostic::new("E_LIMIT", at, "reading binding revisions"))?;
                d.reading = Some(DialogueReading {
                    voice: voice_id,
                    active_voices: active_voices.into_iter().collect(),
                    wait: *wait,
                    revision,
                });
                let character = d.speaker_id.clone();
                if let Some(voice) = recorded_voice {
                    if let Some(entry) = self
                        .state
                        .history
                        .iter_mut()
                        .find(|h| h.interaction == d.interaction)
                    {
                        if !entry.voices.iter().any(|v| v.instance == voice.instance) {
                            entry.voices.push(voice);
                        }
                    }
                    while self.state.history.iter().map(history_bytes).sum::<usize>()
                        > 4 * 1024 * 1024
                    {
                        self.state.history.remove(0);
                    }
                    // History is a bounded derived view, never a new reason
                    // for valid authored dialogue to fault. Evict a record
                    // exceeding its budget rather than publish partial audio.
                    self.state
                        .history
                        .retain(|h| h.voices.len() <= MAX_HISTORY_VOICES);
                }
                if let Some(voice) = voice_id {
                    let task = self.state.tasks.get_mut(&voice).unwrap();
                    task.voice_character = character.clone();
                    if task.state == TaskState::Running {
                        self.intents.push(CoreIntent::AudioCharacter {
                            task: voice,
                            character,
                        });
                    }
                }
            }
            Operation::DialogueContinue { task } => {
                let id = *self
                    .state
                    .handles
                    .get(task)
                    .ok_or_else(|| self.error("E_TASK", task))?;
                let now = self.state.tick_us;
                let at = self.location();
                let t = self
                    .state
                    .tasks
                    .get_mut(&id)
                    .ok_or_else(|| Diagnostic::new("E_TASK", &at, task))?;
                let d = t
                    .dialogue
                    .as_mut()
                    .ok_or_else(|| Diagnostic::new("E_TASK_TYPE", at, task))?;
                if !d.at_gate {
                    return Err(self.error("E_GATE", "dialogue is not at gate"));
                }
                d.at_gate = false;
                d.span += 1;
                d.cluster = 0;
                d.last_reveal_us = now;
            }
        }
        Ok(())
    }
    fn terminate(&mut self, t: Terminator) -> Result<()> {
        match t {
            Terminator::Goto { target } => self.jump(target),
            Terminator::Branch { condition, yes, no } => {
                let Value::Bool(b) = self.eval(&condition)? else {
                    return Err(self.error("E_TYPE", "branch"));
                };
                self.trace(format!("branch:{b}"));
                self.jump(if b { yes } else { no });
            }
            Terminator::Switch {
                value,
                cases,
                default,
            } => {
                let k = match self.eval(&value)? {
                    Value::I32(v) => v.to_string(),
                    Value::String(s) => s,
                    _ => return Err(self.error("E_TYPE", "switch")),
                };
                self.jump(cases.get(&k).cloned().unwrap_or(default));
            }
            Terminator::Call {
                function,
                args,
                next,
                result,
            } => {
                if self.state.frames.len() >= MAX_FRAMES {
                    return Err(self.error("E_LIMIT", "call depth"));
                }
                let locals = args
                    .iter()
                    .map(|(k, e)| Ok((k.clone(), self.eval(e)?)))
                    .collect::<Result<_>>()?;
                let entry = self.program().functions[&function].entry.clone();
                let id = self.id()?;
                self.state.frames.push(Frame {
                    id,
                    function,
                    block: entry,
                    op: 0,
                    op_id: String::new(),
                    locals,
                    return_to: Some(next),
                    result,
                });
                self.sync_op_id();
            }
            Terminator::Return { value } => {
                let value = value.as_ref().map(|e| self.eval(e)).transpose()?;
                let fid = self.frame().id;
                let ids: Vec<_> = self
                    .state
                    .tasks
                    .values()
                    .filter(|t| {
                        (self.state.frames.len() == 1
                            || (t.frame == fid && t.scope == Scope::Frame))
                            && t.state == TaskState::Running
                    })
                    .map(|t| t.id)
                    .collect();
                for id in ids {
                    self.end_task(id, TaskEndReason::ScopeExited)?;
                }
                let f = self.frame().clone();
                if let Some(next) = f.return_to {
                    self.state.frames.pop();
                    if let (Some(target), Some(v)) = (f.result, value) {
                        self.write(&target, v)?;
                    }
                    self.jump(next);
                } else {
                    self.state.outcome = Some("returned".into());
                }
            }
            Terminator::Activate { cue, next } => {
                let effects = self.program().cues[&cue].effects.clone();
                let id = self.id()?;
                let mut dialogues = BTreeMap::new();
                for def in &effects {
                    if let Effect::Dialogue {
                        text,
                        speaker,
                        reveal_us,
                    } = &def.effect
                    {
                        let d = Dialogue {
                            text_id: text.clone(),
                            meaning_revision: self.program().texts[text].meaning_revision,
                            source_revision: self.program().texts[text].source_revision,
                            contract_digest: self.program().texts[text].contract_digest.clone(),
                            locale: self.state.locale.clone(),
                            font_plan_digest: self.program().locale_config.text[&self.state.locale]
                                .digest
                                .clone(),
                            speaker: if speaker.is_empty() {
                                String::new()
                            } else {
                                self.text(speaker)?
                            },
                            speaker_id: if speaker.len() <= MAX_CHARACTER_ID_BYTES {
                                speaker.clone()
                            } else {
                                String::new()
                            },
                            spans: self.freeze_text(text)?,
                            span: 0,
                            cluster: 0,
                            at_gate: false,
                            awaiting_advance: false,
                            last_reveal_us: self.state.tick_us,
                            interaction: self.id()?,
                            reading: None,
                            reveal_interval_us: Some(Micros(
                                (reveal_us.0 as f64 / self.text_speed as f64).round() as u64,
                            )),
                        };
                        dialogues.insert(def.id.clone(), d);
                    }
                }
                self.state.pending = Some(PendingActivation {
                    id,
                    cue: cue.clone(),
                    next,
                    effects,
                    dialogues,
                    failed: None,
                });
                self.state.unsuspended_ops = 0;
                self.intents.push(CoreIntent::Prepare {
                    activation: id,
                    cue,
                });
            }
            Terminator::Await {
                conditions,
                next,
                on_cancelled,
                on_failed,
                on_advance,
            } => {
                let conditions = conditions
                    .iter()
                    .map(|c| {
                        self.state
                            .handles
                            .get(&c.task)
                            .copied()
                            .map(|id| (id, c.milestone.clone()))
                            .ok_or_else(|| self.error("E_TASK", &c.task))
                    })
                    .collect::<Result<_>>()?;
                let advance = match on_advance {
                    Some(next) => Some(AdvanceContinuation {
                        interaction: self.id()?,
                        next,
                    }),
                    None => None,
                };
                self.state.waiting = Some(Waiting {
                    conditions,
                    next,
                    on_cancelled,
                    on_failed,
                    advance,
                });
                self.state.unsuspended_ops = 0;
            }
            Terminator::Interact {
                choice,
                branches,
                on_empty,
                result,
                on_cancel,
            } => {
                let c = self.program().choices[&choice].clone();
                let mut options = vec![];
                for o in &c.options {
                    if self.predicate(&o.visible)? {
                        options.push(OfferedOption {
                            id: o.id.clone(),
                            label: self.text(&o.text)?,
                            enabled: self.predicate(&o.enabled)?,
                        });
                    }
                }
                if !options.iter().any(|o| o.enabled) {
                    self.jump(on_empty);
                    return Ok(());
                }
                if let Some(default) = &c.default {
                    if !options.iter().any(|o| &o.id == default && o.enabled) {
                        return Err(self.error("E_CHOICE_DEFAULT", "default not offered/enabled"));
                    }
                }
                let interaction = self.id()?;
                let deadline_us = c
                    .timeout_us
                    .map(|v| {
                        self.state
                            .tick_us
                            .0
                            .checked_add(v.0)
                            .map(Micros)
                            .ok_or_else(|| self.error("E_TIME", "choice deadline overflow"))
                    })
                    .transpose()?;
                // Typed-result mode carries the declared values of the offered
                // options and a semantic selection cursor; the host selects
                // among ids, never values. The cursor starts at the declared
                // default when it is offered, else at the first enabled row.
                let values = result.as_ref().map(|_| {
                    options
                        .iter()
                        .filter_map(|o| {
                            c.options
                                .iter()
                                .find(|d| d.id == o.id)
                                .and_then(|d| d.value.clone())
                                .map(|value| (o.id.clone(), value))
                        })
                        .collect::<BTreeMap<_, _>>()
                });
                let selected = values.as_ref().and_then(|_| {
                    c.default
                        .as_ref()
                        .filter(|d| options.iter().any(|o| &o.id == *d && o.enabled))
                        .or_else(|| options.iter().find(|o| o.enabled).map(|o| &o.id))
                        .cloned()
                });
                self.state.choice = Some(OfferedChoice {
                    id: choice,
                    locale: self.state.locale.clone(),
                    font_plan_digest: self.program().locale_config.text[&self.state.locale]
                        .digest
                        .clone(),
                    interaction,
                    options,
                    branches,
                    deadline_us,
                    default: c.default,
                    result,
                    on_cancel,
                    values: values.unwrap_or_default(),
                    selected,
                });
                self.state.unsuspended_ops = 0;
                self.intents.push(CoreIntent::Checkpoint);
            }
            Terminator::End { outcome } => {
                let ids: Vec<_> = self.state.tasks.keys().copied().collect();
                for id in ids {
                    self.end_task(id, TaskEndReason::ScopeExited)?;
                }
                self.trace(format!("end:{outcome}"));
                self.state.outcome = Some(outcome);
            }
            Terminator::Fault { code, message } => return Err(self.error(&code, message)),
        }
        Ok(())
    }
    fn commit(&mut self) -> Result<()> {
        let old = self.state.clone();
        let n = self.intents.len();
        if let Err(e) = self.commit_inner() {
            self.state = old;
            self.intents.truncate(n);
            return Err(e);
        }
        Ok(())
    }
    fn commit_inner(&mut self) -> Result<()> {
        let p = self.state.pending.take().unwrap();
        let planned: usize = p
            .effects
            .iter()
            .map(|def| def.effect.compose_leaves())
            .sum();
        if self
            .state
            .tasks
            .values()
            .filter(|t| t.state == TaskState::Running)
            .count()
            + planned
            > MAX_TASKS
        {
            return Err(self.error("E_LIMIT", "active tasks"));
        }
        for def in &p.effects {
            self.commit_effect(def, &p.dialogues)?;
        }
        self.jump(p.next);
        self.trace(format!("activate:{}", p.cue));
        self.intents.push(CoreIntent::Checkpoint);
        let ids:Vec<_>=self.state.tasks.values().filter(|t|t.state==TaskState::Running&&(matches!(t.effect,Effect::DialogueStyle{..}|Effect::DialogueDecoration{..})||matches!(t.effect,Effect::StagePresent{duration_us,..}|Effect::Clip{duration_us,..}|Effect::SourceMotion{duration_us,..}|Effect::Tween{duration_us,..}|Effect::Delay{duration_us}|Effect::AudioStop{duration_us,..} if duration_us.0==0))).map(|t|t.id).collect();
        for id in ids {
            self.finish_task(id, TaskState::Finished)?;
        }
        let ended_stops: Vec<_> = self
            .state
            .tasks
            .values()
            .filter(|task| {
                task.state == TaskState::Running
                    && task
                        .target_task
                        .is_some_and(|id| self.state.tasks[&id].state != TaskState::Running)
            })
            .map(|task| task.id)
            .collect();
        for id in ended_stops {
            self.finish_task(id, TaskState::Finished)?;
        }
        self.advance_compositions()?;
        let keep: BTreeSet<_> = self
            .state
            .handles
            .values()
            .copied()
            .chain(
                self.state
                    .tasks
                    .values()
                    .filter_map(|task| task.target_task),
            )
            .chain(
                self.state
                    .tasks
                    .values()
                    .filter_map(|t| t.dialogue.as_ref()?.reading.as_ref()?.voice),
            )
            .chain(
                self.state
                    .waiting
                    .iter()
                    .flat_map(|w| w.conditions.iter().map(|(id, _)| *id)),
            )
            .collect();
        self.state
            .tasks
            .retain(|id, t| t.state == TaskState::Running || keep.contains(id));
        Ok(())
    }
    /// Spawn one effect definition as a task: ownership checks, property
    /// capture at start, side-effect intents, handle registration. Cue
    /// commits and composition child spawns share this path so a child gets
    /// exactly the semantics a top-level effect gets; children arrive with
    /// an empty dialogue map because composition validation forbids
    /// dialogue children.
    fn commit_effect(
        &mut self,
        def: &EffectDef,
        dialogues: &BTreeMap<String, Dialogue>,
    ) -> Result<u32> {
        let mut source = vec![];
        let mut target = vec![];
        let mut captured = 0.;
        let mut base = 0.;
        let mut target_task = None;
        let mut shake = None;
        let mut sprite_shakes = BTreeMap::new();
        match &def.effect {
            Effect::StoryModal { .. } => {
                if self.state.choice.is_some()
                    || self.state.tasks.values().any(|task| {
                        task.state == TaskState::Running
                            && (task.dialogue.is_some()
                                || matches!(task.effect, Effect::StoryModal { .. }))
                    })
                {
                    return Err(
                        self.error("E_OWNERSHIP", "story modal requires exclusive interaction")
                    );
                }
            }
            Effect::DialogueStyle { style } => {
                if !self.program().theme.dialogue_styles.contains_key(style) {
                    return Err(self.error("E_DIALOGUE_STYLE", style));
                }
                if self
                    .state
                    .tasks
                    .values()
                    .any(|task| task.state == TaskState::Running && task.dialogue.is_some())
                {
                    return Err(
                        self.error("E_OWNERSHIP", "dialogue style requires completed reading")
                    );
                }
                if self.state.window_reveal.is_some()
                    || self.state.tasks.values().any(|task| {
                        task.state == TaskState::Running
                            && task
                                .effect
                                .scalar_track(task.captured, task.base)
                                .is_some_and(|(target, _, _)| {
                                    matches!(target, TweenTarget::DialogueRoot { .. })
                                })
                    })
                {
                    return Err(self.error("E_OWNERSHIP", "dialogue animation owns the window"));
                }
                self.state.dialogue_style = Some(style.clone());
                for task in self.state.tasks.values_mut() {
                    task.dialogue = None;
                }
                self.state.dialogue_appearance = DialogueAppearance::default();
            }
            Effect::DialogueDecoration { slot, image } => {
                if let Some(image) = image {
                    if !image.valid()
                        || self.program().asset(&image.asset).is_none_or(|asset| {
                            asset.kind != AssetKind::Image
                                || [asset.width, asset.height] != image.size
                        })
                    {
                        return Err(self.error("E_DIALOGUE_DECORATION", "image metadata mismatch"));
                    }
                    let point = match image.placement {
                        DialogueDecorationPlacement::Absolute { point } => point,
                        DialogueDecorationPlacement::TextOrigin { offset } => {
                            let theme = &self.program().theme;
                            let dialogue = self
                                .state
                                .dialogue_style
                                .as_ref()
                                .and_then(|id| theme.dialogue_styles.get(id))
                                .map(|style| &style.dialogue)
                                .unwrap_or(&theme.dialogue);
                            let rect = dialogue.text_rect.ok_or_else(|| {
                                self.error(
                                    "E_DIALOGUE_DECORATION",
                                    "explicit text rectangle required",
                                )
                            })?;
                            [rect[0] + offset[0], rect[1] + offset[1]]
                        }
                    };
                    let decoration = DialogueDecoration {
                        asset: image.asset.clone(),
                        rect: [
                            point[0],
                            point[1],
                            image.size[0] as f32,
                            image.size[1] as f32,
                        ],
                    };
                    if !decoration.valid() {
                        return Err(self
                            .error("E_DIALOGUE_DECORATION", "resolved rectangle exceeds bounds"));
                    }
                    self.state.dialogue_decorations.insert(*slot, decoration);
                } else {
                    self.state.dialogue_decorations.remove(slot);
                }
            }
            Effect::SpriteTimeline {
                timeline,
                root,
                duration_us,
                ..
            } => {
                crate::validate::validate_timeline_bindings(&self.state.scene, root, |id| {
                    self.program().sprite_timelines.get(id)
                })?;
                let resource = self
                    .program()
                    .sprite_timelines
                    .get(timeline)
                    .ok_or_else(|| self.error("E_CONTENT_MISSING", timeline))?;
                if resource.duration_us != *duration_us
                    || !self.state.scene.iter().any(|n| {
                        &n.id == root && n.timeline_binding.as_deref() == Some(timeline.as_str())
                    })
                {
                    return Err(self.error("E_TIMELINE", "timeline root identity mismatch"));
                }
                let track_nodes: BTreeSet<_> =
                    resource.tracks.iter().map(|t| t.node.as_str()).collect();
                if self.state.tasks.values().any(|t| t.state == TaskState::Running
                    && self.track_is_current(t, &TweenTarget::SceneNode { node: String::new(), property: Property::X })
                    && t.effect.scalar_track(0.,0.).is_some_and(|(a,_,_)|
                        matches!(a, TweenTarget::SceneNode { node, .. } if track_nodes.contains(node.as_str())))) {
                    return Err(self.error("E_OWNERSHIP", "timeline owns descendant poses"));
                }
                let old: Vec<_> = self.state.tasks.values().filter(|t| t.state == TaskState::Running
                    && matches!(&t.effect, Effect::SpriteTimeline { root: old, .. } if old == root)).map(|t|t.id).collect();
                for id in old {
                    self.end_task(id, TaskEndReason::Replaced)?;
                }
                let resource = self.program().sprite_timelines.shared(timeline).unwrap();
                resource.apply(&mut self.state.scene, 0);
            }
            Effect::SpriteWave { nodes, spec } => {
                for node in nodes {
                    if !self.state.scene.iter().any(|n| &n.id == node) {
                        return Err(self.error("E_NODE", node));
                    }
                    if self.state.tasks.values().any(|task| {
                        task.state == TaskState::Running
                            && matches!(&task.effect, Effect::SpriteWave { nodes: old, .. } | Effect::SpriteShake { nodes: old, .. }
                                if old.contains(node))
                    }) {
                        return Err(self.error("E_OWNERSHIP", "sprite wave owns translation"));
                    }
                }
                shake = Some(crate::ShakeCapture::wave(*spec));
            }
            Effect::SpriteShake { nodes, mode, spec } => {
                for node in nodes {
                    if !self.state.scene.iter().any(|n| &n.id == node) {
                        return Err(self.error("E_NODE", node));
                    }
                    if self.state.tasks.values().any(|task| task.state == TaskState::Running
                        && matches!(&task.effect,Effect::SpriteWave { nodes: old, .. } | Effect::SpriteShake { nodes: old, .. } if old.contains(node))) {
                        return Err(self.error("E_OWNERSHIP","sprite shake owns translation"));
                    }
                }
                sprite_shakes = crate::ShakeCapture::sprite_group(*mode, *spec, nodes, |upper| {
                    let width = upper as u64;
                    let zone = (1u64 << 32) / width * width;
                    loop {
                        let value = self.state.rng.next_u32() as u64;
                        if value < zone {
                            break (value % width) as u32;
                        }
                    }
                });
            }
            Effect::DialogueShake { spec } => {
                let from = self.sample_dialogue_appearance().text_offset;
                let old: Vec<_> = self
                    .state
                    .tasks
                    .values()
                    .filter(|task| {
                        task.state == TaskState::Running
                            && matches!(task.effect, Effect::DialogueShake { .. })
                    })
                    .map(|task| task.id)
                    .collect();
                for id in old {
                    self.end_task(id, TaskEndReason::Replaced)?;
                }
                shake = Some(crate::ShakeCapture::capture(*spec, from, |upper| {
                    let width = upper as u64;
                    let zone = (1u64 << 32) / width * width;
                    loop {
                        let value = self.state.rng.next_u32() as u64;
                        if value < zone {
                            break (value % width) as u32;
                        }
                    }
                }));
            }
            Effect::AudioStop { target, .. } => {
                let target_id = *self
                    .state
                    .handles
                    .get(target)
                    .ok_or_else(|| self.error("E_TASK", target))?;
                let audio = &self.state.tasks[&target_id];
                if !matches!(audio.effect, Effect::Audio { .. }) {
                    return Err(self.error("E_TASK_TYPE", "audio stop requires an audio instance"));
                }
                if self.state.tasks.values().any(|task| {
                    task.state == TaskState::Running && task.target_task == Some(target_id)
                }) {
                    return Err(self.error("E_OWNERSHIP", "audio envelope already owned"));
                }
                target_task = Some(target_id);
                captured = audio.audio_envelope;
            }
            Effect::StagePresent {
                scene,
                inherit_images,
                inherit_image_geometry,
                dialogue_visible,
                transition,
                duration_us,
            } => {
                if self.state.tasks.values().any(|t| {
                    t.state == TaskState::Running && matches!(t.effect, Effect::StagePresent { .. })
                }) {
                    return Err(self.error("E_OWNERSHIP", "stage transition owns root"));
                }
                source = self.sample_scene();
                let mut recipe = if !self.state.draft.is_empty() {
                    self.state.draft.clone()
                } else {
                    self.program().scenes[scene].clone()
                };
                let absent: Vec<_> = recipe
                    .iter()
                    .filter(|node| {
                        node.inherit_existence && !source.iter().any(|old| old.id == node.id)
                    })
                    .map(|node| node.id.clone())
                    .collect();
                for root in absent {
                    crate::bitmap::remove_tree(&mut recipe, &root);
                }
                for id in inherit_images {
                    let image = source
                        .iter()
                        .find(|node| &node.id == id)
                        .and_then(|node| node.asset.as_ref())
                        .ok_or_else(|| {
                            self.error("E_STAGE_INHERIT", "committed image is missing")
                        })?;
                    let node = recipe
                        .iter_mut()
                        .find(|node| &node.id == id)
                        .ok_or_else(|| self.error("E_STAGE_INHERIT", "recipe node is missing"))?;
                    node.asset = Some(image.clone());
                }
                for id in inherit_image_geometry {
                    let old = source.iter().find(|node| &node.id == id).ok_or_else(|| {
                        self.error("E_STAGE_INHERIT", "committed image rectangle is missing")
                    })?;
                    let node = recipe
                        .iter_mut()
                        .find(|node| &node.id == id)
                        .ok_or_else(|| {
                            self.error("E_STAGE_INHERIT", "recipe rectangle is missing")
                        })?;
                    [node.x, node.y, node.width, node.height] =
                        [old.x, old.y, old.width, old.height];
                }
                target = crate::bitmap::materialize(&recipe, &self.state.variables)
                    .map_err(|message| self.error("E_BITMAP_TEXT", message))?;
                let next_generation = self
                    .state
                    .scene_generation
                    .checked_add(1)
                    .ok_or_else(|| self.error("E_LIMIT", "scene generations"))?;
                let mut preserved = BTreeMap::new();
                for node in target
                    .iter_mut()
                    .filter(|node| !node.preserve_pose.is_empty())
                {
                    if let Some(old) = source.iter().find(|old| old.id == node.id) {
                        for property in &node.preserve_pose.clone() {
                            node.set(*property, old.get(*property));
                            preserved
                                .entry(node.id.clone())
                                .or_insert_with(BTreeSet::new)
                                .insert(*property);
                        }
                    }
                }
                crate::validate::validate_timeline_bindings(&target, scene, |id| {
                    self.program().sprite_timelines.get(id)
                })?;
                self.state.draft.clear();
                let removed: Vec<_> = self.state.tasks.values().filter(|t|t.state == TaskState::Running
                    && matches!(&t.effect, Effect::SpriteTimeline { timeline, root, .. }
                        if !target.iter().any(|n| &n.id == root && n.timeline_binding.as_deref() == Some(timeline.as_str()))))
                    .map(|t|t.id).collect();
                for id in removed {
                    self.end_task(id, TaskEndReason::ScopeExited)?;
                }
                let ids: Vec<_> = self
                    .state
                    .tasks
                    .values()
                    .filter(|t| t.scope == Scope::Scene && t.state == TaskState::Running)
                    .map(|t| t.id)
                    .collect();
                for id in ids {
                    self.end_task(id, TaskEndReason::ScopeExited)?;
                }
                for task in self.state.tasks.values_mut() {
                    if task.state == TaskState::Running
                        && task.scene_generation == self.state.scene_generation
                    {
                        if let Effect::SpriteTimeline { timeline, root, .. } = &task.effect {
                            if target.iter().any(|n| {
                                &n.id == root
                                    && n.timeline_binding.as_deref() == Some(timeline.as_str())
                            }) {
                                task.scene_generation = next_generation;
                            }
                        }
                        if let Some((TweenTarget::SceneNode { node, property }, _, _)) =
                            task.effect.scalar_track(task.captured, task.base)
                        {
                            if preserved
                                .get(&node)
                                .is_some_and(|props| props.contains(&property))
                            {
                                task.scene_generation = next_generation;
                            }
                        }
                    }
                }
                self.state.scene = target.clone();
                self.state.scene_generation = next_generation;
                if let Some(visible) = dialogue_visible {
                    self.execute_op(&Operation::DialogueVisibility {
                        visible: *visible,
                        transition: Some(transition.clone()),
                        duration_us: *duration_us,
                    })?;
                }
            }
            Effect::Dialogue { .. } => {
                let old: Vec<_> = self
                    .state
                    .tasks
                    .values()
                    .filter(|t| {
                        t.state == TaskState::Running
                            && (t.dialogue.is_some() || t.scope == Scope::Interaction)
                    })
                    .map(|t| t.id)
                    .collect();
                for id in old {
                    self.end_task(id, TaskEndReason::Replaced)?;
                }
            }
            // Compositions spawn their children through advance_compositions
            // so each child captures the properties current at its own start.
            Effect::Sequence { .. } | Effect::ParallelAll { .. } => {}
            _ => {}
        }
        if let Some((address, _, replace)) = def.effect.scalar_track(0., 0.) {
            match &address {
                TweenTarget::SceneNode { node, property } => {
                    if self.state.tasks.values().any(|t| t.state == TaskState::Running
                        && t.scene_generation == self.state.scene_generation
                        && matches!(&t.effect, Effect::SpriteTimeline { timeline, .. }
                            if self.program().sprite_timelines.get(timeline).is_some_and(|r| r.tracks.iter().any(|track| &track.node == node)))) {
                        return Err(self.error("E_OWNERSHIP", "timeline owns descendant poses"));
                    }
                    if self.state.tasks.values().any(|t| t.state == TaskState::Running && matches!(t.effect, Effect::StagePresent { duration_us, .. } if duration_us.0 > 0)) {
                        return Err(self.error("E_OWNERSHIP", "transition owns root"));
                    }
                    captured = self
                        .sample_scene()
                        .iter()
                        .find(|n| &n.id == node)
                        .ok_or_else(|| self.error("E_NODE", node))?
                        .get(*property);
                    base = self
                        .state
                        .scene
                        .iter()
                        .find(|n| &n.id == node)
                        .ok_or_else(|| self.error("E_NODE", node))?
                        .get(*property);
                }
                TweenTarget::DialogueRoot { property } => {
                    captured = self.sample_dialogue_appearance().get(*property);
                    base = self.state.dialogue_appearance.get(*property);
                }
                TweenTarget::AudioInstance { task, .. } => {
                    let target_id = *self
                        .state
                        .handles
                        .get(task)
                        .ok_or_else(|| self.error("E_TASK", task))?;
                    let audio = &self.state.tasks[&target_id];
                    if !matches!(audio.effect, Effect::Audio { .. }) {
                        return Err(
                            self.error("E_TASK_TYPE", "gain tween requires an audio instance")
                        );
                    }
                    if self
                        .state
                        .tasks
                        .values()
                        .any(|t| t.state == TaskState::Running && t.target_task == Some(target_id))
                    {
                        return Err(self.error("E_OWNERSHIP", "audio envelope already owned"));
                    }
                    target_task = Some(target_id);
                    // The envelope is a 0..1 multiplier with unit base; the
                    // captured value carries whatever a previous owner left.
                    captured = audio.audio_envelope;
                    base = 1.;
                }
            }
            let old: Vec<_> = self
                .state
                .tasks
                .values()
                .filter(|t| {
                    t.state == TaskState::Running
                        && self.track_is_current(t, &address)
                        && t.effect
                            .scalar_track(0., 0.)
                            .is_some_and(|(a, _, _)| a == address)
                })
                .map(|t| t.id)
                .collect();
            if !old.is_empty() && !replace {
                return Err(self.error("E_OWNERSHIP", format!("{address:?}")));
            }
            for id in old {
                self.end_task(id, TaskEndReason::Replaced)?;
            }
        }
        let id = self.id()?;
        let envelope_segment = match &def.effect {
            Effect::AudioStop { duration_us, .. } => Some((captured, 0., *duration_us)),
            Effect::Tween {
                target: TweenTarget::AudioInstance { .. },
                to,
                duration_us,
                ..
            } => Some((captured, *to, *duration_us)),
            _ => None,
        };
        if let Some((from, to, duration_us)) = envelope_segment {
            let target = target_task.expect("resolved audio envelope target");
            if self.state.tasks[&target].state == TaskState::Running {
                self.intents.push(CoreIntent::AudioEnvelope {
                    owner: Some(id),
                    elapsed_us: Micros(0),
                    task: target,
                    from,
                    to,
                    duration_us,
                });
            }
        }
        let dialogue = dialogues.get(&def.id).cloned();
        if let Some(d) = &dialogue {
            self.push_history(HistoryEntry {
                interaction: d.interaction,
                text_id: d.text_id.clone(),
                meaning_revision: d.meaning_revision,
                source_revision: d.source_revision,
                contract_digest: d.contract_digest.clone(),
                locale: d.locale.clone(),
                font_plan_digest: d.font_plan_digest.clone(),
                speaker: d.speaker.clone(),
                speaker_id: d.speaker_id.clone(),
                text: d.full_text(),
                images: d.inline_images(),
                voices: vec![],
                choice: None,
            });
        }
        if let Effect::Audio {
            asset,
            bus,
            looped,
            gain,
            loop_region,
        } = &def.effect
        {
            self.intents.push(CoreIntent::AudioStart {
                task: id,
                asset: asset.clone(),
                bus: *bus,
                looped: *looped,
                loop_region: *loop_region,
                gain: *gain,
                position_us: Micros(0),
            });
        }
        let task = Task {
            modal_interaction: if matches!(def.effect, Effect::StoryModal { .. }) {
                Some(self.id()?)
            } else {
                None
            },
            shake,
            sprite_shakes,
            id,
            name: def.id.clone(),
            frame: self.frame().id,
            scene_generation: self.state.scene_generation,
            scope: def.scope,
            effect: def.effect.clone(),
            state: TaskState::Running,
            end_reason: None,
            target_task,
            audio_envelope: 1.,
            audio_position_us: None,
            audio_device_elapsed_us: None,
            voice_character: String::new(),
            started_us: self.state.tick_us,
            elapsed_us: Micros(0),
            milestones: BTreeSet::from([Milestone::Started]),
            captured,
            base,
            dialogue,
            source,
            target,
            children: vec![],
            cursor: 0,
        };
        self.state.handles.insert(def.id.clone(), id);
        self.state.tasks.insert(id, task);
        Ok(id)
    }
    /// Advance every composition: spawn due children, merge child results,
    /// finish completed chains. Each spawn costs one unit of step budget, so
    /// a finite zero-duration chain still pays for every spawn and spreads
    /// across steps when the budget runs out; the guard turns a chase that
    /// refuses to converge into an explicit fault instead of a hang.
    fn advance_compositions(&mut self) -> Result<()> {
        let mut guard = 0usize;
        loop {
            let composites: Vec<u32> = self
                .state
                .tasks
                .values()
                .filter(|t| t.state == TaskState::Running && t.effect.compose_children().is_some())
                .map(|t| t.id)
                .collect();
            if composites.is_empty() {
                return Ok(());
            }
            guard += 1;
            if guard > MAX_TASKS * 2 {
                return Err(self.error("E_LIMIT", "composition did not converge"));
            }
            let mut acted = false;
            for id in composites {
                acted |= self.advance_composition(id)?;
            }
            if !acted {
                return Ok(());
            }
        }
    }
    /// One chase round for one composition; true when its state changed.
    fn advance_composition(&mut self, id: u32) -> Result<bool> {
        let defs = match self.state.tasks.get(&id) {
            Some(t) if t.state == TaskState::Running => match &t.effect {
                Effect::Sequence { children } | Effect::ParallelAll { children } => {
                    children.clone()
                }
                _ => return Ok(false),
            },
            _ => return Ok(false),
        };
        let spawned: Vec<u32> = self.state.tasks[&id].children.clone();
        let running_child = spawned.iter().any(|c| {
            self.state
                .tasks
                .get(c)
                .is_some_and(|t| t.state == TaskState::Running)
        });
        if matches!(self.state.tasks[&id].effect, Effect::Sequence { .. }) {
            if running_child {
                return Ok(false);
            }
            if let Some(last) = spawned.last() {
                let (state, reason) = {
                    let child = &self.state.tasks[last];
                    (child.state, child.end_reason)
                };
                if state != TaskState::Finished {
                    // Failure and cancellation propagate to the chain;
                    // completed children stay settled because their side
                    // effects are not rolled back.
                    self.end_task(id, reason.unwrap_or(TaskEndReason::Failed))?;
                    return Ok(true);
                }
            }
            if spawned.len() == defs.len() {
                self.finish_task(id, TaskState::Finished)?;
                return Ok(true);
            }
            self.spawn_child(id, &defs[spawned.len()])
        } else {
            while self.state.tasks[&id].children.len() < defs.len() {
                let next = self.state.tasks[&id].children.len();
                if !self.spawn_child(id, &defs[next])? {
                    return Ok(false);
                }
            }
            let kids: Vec<TaskState> = self.state.tasks[&id]
                .children
                .iter()
                .map(|c| self.state.tasks[c].state)
                .collect();
            // Failure beats cancellation beats completion, like every All.
            if let Some(index) = kids.iter().position(|s| *s == TaskState::Failed) {
                let reason = self.state.tasks[&self.state.tasks[&id].children[index]].end_reason;
                self.end_task(id, reason.unwrap_or(TaskEndReason::Failed))?;
                return Ok(true);
            }
            if let Some(index) = kids.iter().position(|s| *s == TaskState::Cancelled) {
                let reason = self.state.tasks[&self.state.tasks[&id].children[index]].end_reason;
                self.end_task(id, reason.unwrap_or(TaskEndReason::CancelledByControl))?;
                return Ok(true);
            }
            if kids.iter().all(|s| *s == TaskState::Finished) {
                self.finish_task(id, TaskState::Finished)?;
                return Ok(true);
            }
            Ok(false)
        }
    }
    /// Spawn the next child of a composition; false when the step budget is
    /// spent and the spawn defers to the next step.
    fn spawn_child(&mut self, parent: u32, def: &EffectDef) -> Result<bool> {
        if self
            .state
            .tasks
            .values()
            .filter(|t| t.state == TaskState::Running)
            .count()
            + 1
            > MAX_TASKS
        {
            return Err(self.error("E_LIMIT", "active tasks"));
        }
        if self.work_remaining == 0 {
            return Ok(false);
        }
        self.work_remaining -= 1;
        let child = self.commit_effect(def, &BTreeMap::new())?;
        let task = self.state.tasks.get_mut(&parent).unwrap();
        task.children.push(child);
        task.cursor += 1;
        // Zero-duration children complete inside the same chase round, and a
        // stop whose target already ended completes immediately.
        let finish_now = match &self.state.tasks[&child].effect {
            Effect::Clip { duration_us, .. }
            | Effect::SourceMotion { duration_us, .. }
            | Effect::Tween { duration_us, .. }
            | Effect::Delay { duration_us } => duration_us.0 == 0,
            Effect::AudioStop {
                duration_us,
                target,
            } => {
                duration_us.0 == 0
                    || self
                        .state
                        .handles
                        .get(target)
                        .is_some_and(|id| self.state.tasks[id].state != TaskState::Running)
            }
            _ => false,
        };
        if finish_now {
            self.finish_task(child, TaskState::Finished)?;
        }
        Ok(true)
    }
    fn resolve_wait(&mut self) -> Result<bool> {
        let w = self.state.waiting.as_ref().unwrap();
        let mut failed = false;
        let mut cancelled = false;
        let mut all = true;
        for (id, m) in &w.conditions {
            let t = self
                .state
                .tasks
                .get(id)
                .ok_or_else(|| self.error("E_TASK", "wait references missing task"))?;
            failed |= t.state == TaskState::Failed;
            cancelled |= t.state == TaskState::Cancelled;
            all &= t.milestones.contains(m);
        }
        let dest = if failed {
            Some(w.on_failed.clone())
        } else if cancelled {
            Some(w.on_cancelled.clone())
        } else if all {
            Some(w.next.clone())
        } else {
            None
        };
        if let Some(dest) = dest {
            self.state.waiting = None;
            self.jump(dest);
            Ok(true)
        } else {
            Ok(false)
        }
    }
    fn advance_reading(&mut self, interaction: u32, sequence: u32, stop_voice: bool) -> Result<()> {
        if sequence <= self.state.last_input {
            return Ok(());
        }
        if let Some(advance) = self.state.waiting.as_ref().and_then(|w| w.advance.as_ref()) {
            if advance.interaction != interaction {
                return Ok(());
            }
            let next = advance.next.clone();
            self.state.last_input = sequence;
            self.state.waiting = None;
            self.jump(next);
            return Ok(());
        }
        let Some(id) = self
            .state
            .tasks
            .values()
            .find(|t| {
                t.state == TaskState::Running
                    && t.dialogue
                        .as_ref()
                        .is_some_and(|d| d.interaction == interaction)
            })
            .map(|t| t.id)
        else {
            return Ok(());
        };
        self.state.last_input = sequence;
        let d = self.state.tasks[&id].dialogue.as_ref().unwrap();
        if d.awaiting_advance && d.spans.get(d.span).is_some_and(|s| s.pause) {
            self.resume_text_pause(id)?;
        } else if d.awaiting_advance {
            let voices: BTreeSet<_> = if !stop_voice {
                BTreeSet::new()
            } else if let Some(reading) = &d.reading {
                // Older snapshots have only the latest binding. Never infer
                // ownership from history, which may have been evicted.
                reading
                    .active_voices
                    .iter()
                    .copied()
                    .chain(reading.voice)
                    .collect()
            } else {
                // Legacy reading has no explicit association, as with its
                // voice wait policy. Looped ambient Voice is not an utterance.
                self.state
                    .tasks
                    .values()
                    .filter(|t| {
                        t.state == TaskState::Running
                            && matches!(
                                t.effect,
                                Effect::Audio {
                                    bus: AudioBus::Voice,
                                    looped: false,
                                    ..
                                }
                            )
                    })
                    .map(|t| t.id)
                    .collect()
            };
            self.finish_task(id, TaskState::Finished)?;
            for voice in voices {
                // Same terminal semantics as an authored AudioStop; this is
                // not a natural completion and must not forge that milestone.
                self.end_task(voice, TaskEndReason::CancelledByControl)?;
            }
        } else if !d.at_gate {
            self.reveal(id, true)?;
        }
        Ok(())
    }
    fn finish_task(&mut self, id: u32, status: TaskState) -> Result<()> {
        let reason = match status {
            TaskState::Finished => TaskEndReason::Completed,
            TaskState::Cancelled => TaskEndReason::CancelledByControl,
            TaskState::Failed => TaskEndReason::Failed,
            TaskState::Running => return Err(self.error("E_TASK", "invalid terminal state")),
        };
        self.end_task(id, reason)
    }
    fn end_task(&mut self, id: u32, reason: TaskEndReason) -> Result<()> {
        let status = reason.state();
        let Some(t) = self.state.tasks.get(&id).cloned() else {
            return Err(self.error("E_TASK", id.to_string()));
        };
        if t.state != TaskState::Running {
            return Ok(());
        }
        // A composition ending takes its children with it: finished children
        // stay settled (their side effects are not rolled back), running
        // children end so their own finish/cancel policies apply, and
        // children never spawned never run. A failing or cancelled chain
        // stops its running children as cancellations; a finished chain
        // settles them at their end values.
        if t.effect.compose_children().is_some() {
            let reason = if reason == TaskEndReason::Failed {
                TaskEndReason::CancelledByControl
            } else {
                reason
            };
            for child in t.children.clone() {
                if self
                    .state
                    .tasks
                    .get(&child)
                    .is_some_and(|c| c.state == TaskState::Running)
                {
                    self.end_task(child, reason)?;
                }
            }
        }
        if let Effect::SpriteTimeline {
            timeline,
            duration_us,
            root,
            delete_on_finish,
        } = &t.effect
        {
            if t.scene_generation == self.state.scene_generation {
                let resource = self
                    .program()
                    .sprite_timelines
                    .shared(timeline)
                    .ok_or_else(|| self.error("E_CONTENT_MISSING", timeline))?;
                resource.apply(
                    &mut self.state.scene,
                    if status == TaskState::Finished {
                        duration_us.0
                    } else {
                        t.elapsed_us.0
                    },
                );
                if *delete_on_finish
                    && reason == TaskEndReason::Completed
                    && self.state.scene.iter().any(|n| {
                        &n.id == root && n.timeline_binding.as_deref() == Some(timeline.as_str())
                    })
                {
                    let removed = crate::bitmap::tree_ids(&self.state.scene, root);
                    let owners: Vec<_> = self.state.tasks.values().filter(|owner|
                        owner.id != id && owner.state == TaskState::Running
                        && owner.scene_generation == self.state.scene_generation
                        && owner.effect.scalar_track(owner.captured, owner.base).is_some_and(|(address, _, _)|
                            matches!(address, TweenTarget::SceneNode { node, .. } if removed.contains(&node))))
                        .map(|owner| owner.id).collect();
                    for owner in owners {
                        self.end_task(owner, TaskEndReason::ScopeExited)?;
                    }
                    crate::bitmap::remove_tree(&mut self.state.scene, root);
                    for owner in self.state.tasks.values_mut().filter(|owner| {
                        owner.state == TaskState::Running
                            && matches!(owner.effect, Effect::StagePresent { .. })
                    }) {
                        if owner.target.iter().any(|n| {
                            &n.id == root
                                && n.timeline_binding.as_deref() == Some(timeline.as_str())
                        }) {
                            crate::bitmap::remove_tree(&mut owner.target, root);
                        }
                    }
                }
            }
        }
        if matches!(t.effect, Effect::DialogueShake { .. }) {
            self.state.dialogue_appearance.text_offset = [0.; 2];
        }
        if let Some((address, track, _)) = t.effect.scalar_track(t.captured, t.base) {
            // Envelope commits follow the device clock when one was observed,
            // like timed stops; story-owned targets use the story clock.
            let elapsed = match address {
                TweenTarget::AudioInstance { .. } => {
                    t.audio_device_elapsed_us.unwrap_or(t.elapsed_us)
                }
                _ => t.elapsed_us,
            };
            let value = track.settle(elapsed, status == TaskState::Finished);
            if self.track_is_current(&t, &address) {
                match address {
                    TweenTarget::SceneNode { node, property } => {
                        if let Some(n) = self.state.scene.iter_mut().find(|n| n.id == node) {
                            n.set(property, value);
                        }
                    }
                    TweenTarget::DialogueRoot { property } => {
                        self.state.dialogue_appearance.set(property, value)
                    }
                    TweenTarget::AudioInstance { .. } => {
                        // Commit the settled envelope only onto a live
                        // instance; the audio may have ended first.
                        if let Some(target) = t
                            .target_task
                            .filter(|id| self.state.tasks[id].state == TaskState::Running)
                        {
                            self.state.tasks.get_mut(&target).unwrap().audio_envelope = value;
                        }
                    }
                }
            }
        }
        if matches!(t.effect, Effect::Audio { .. }) {
            self.intents.push(CoreIntent::AudioStop { task: id });
            for task in self.state.tasks.values_mut() {
                if let Some(reading) = task.dialogue.as_mut().and_then(|d| d.reading.as_mut()) {
                    reading.active_voices.retain(|voice| *voice != id);
                }
            }
        }
        if status == TaskState::Finished {
            if let Some(d) = &t.dialogue {
                let key = format!("read:{}:{}", d.text_id, d.meaning_revision);
                self.profile_facts.insert(key.clone());
                self.intents.push(CoreIntent::ProfileMerge { key });
            }
        }
        let task = self.state.tasks.get_mut(&id).unwrap();
        task.state = status;
        task.end_reason = Some(reason);
        if status == TaskState::Finished {
            task.milestones.insert(Milestone::Finished);
        }
        if let Effect::AudioStop { duration_us, .. } = t.effect {
            let target = t
                .target_task
                .ok_or_else(|| self.error("E_TASK", "missing audio target"))?;
            if self.state.tasks[&target].state == TaskState::Running {
                if status == TaskState::Finished {
                    self.end_task(target, TaskEndReason::CancelledByControl)?;
                } else {
                    let progress = if duration_us.0 == 0 {
                        1.
                    } else {
                        (t.audio_device_elapsed_us.unwrap_or(t.elapsed_us).0 as f64
                            / duration_us.0 as f64)
                            .min(1.) as f32
                    };
                    let value = interpolate(t.captured, 0., progress, Easing::Linear);
                    self.state.tasks.get_mut(&target).unwrap().audio_envelope = value;
                    self.intents.push(CoreIntent::AudioEnvelope {
                        owner: None,
                        elapsed_us: Micros(0),
                        task: target,
                        from: value,
                        to: value,
                        duration_us: Micros(0),
                    });
                }
            }
        }
        if let Effect::Tween {
            target: TweenTarget::AudioInstance { .. },
            ..
        } = t.effect
        {
            // The device ramp must stop following the ended owner: pin the
            // committed envelope value so the plan's owner is released.
            let target = t
                .target_task
                .ok_or_else(|| self.error("E_TASK", "missing audio target"))?;
            if self.state.tasks[&target].state == TaskState::Running {
                let value = self.state.tasks[&target].audio_envelope;
                self.intents.push(CoreIntent::AudioEnvelope {
                    owner: None,
                    elapsed_us: Micros(0),
                    task: target,
                    from: value,
                    to: value,
                    duration_us: Micros(0),
                });
            }
        }
        if matches!(t.effect, Effect::Audio { .. }) {
            let dependents: Vec<_> = self
                .state
                .tasks
                .values()
                .filter(|task| task.state == TaskState::Running && task.target_task == Some(id))
                .map(|task| task.id)
                .collect();
            for dependent in dependents {
                self.end_task(
                    dependent,
                    if reason == TaskEndReason::NaturalEnd {
                        TaskEndReason::Completed
                    } else {
                        reason
                    },
                )?;
            }
        }
        self.trace(format!("task:{id}:{status:?}"));
        Ok(())
    }
    /// Current envelope and remaining linear segment, independent of event gain.
    pub fn audio_envelope(&self, id: u32) -> (f32, f32, Micros) {
        for task in self.state.tasks.values() {
            if task.state == TaskState::Running && task.target_task == Some(id) {
                match task.effect {
                    Effect::AudioStop { duration_us, .. } => {
                        let elapsed = task.audio_device_elapsed_us.unwrap_or(task.elapsed_us);
                        let remaining = duration_us.0.saturating_sub(elapsed.0);
                        let value = ScalarTween {
                            from: task.captured,
                            base: task.captured,
                            to: 0.,
                            duration_us,
                            easing: Easing::Linear,
                            finish: FinishPolicy::CommitEnd,
                            cancel: CancelPolicy::CommitCurrent,
                            source_curve: None,
                        }
                        .sample(elapsed);
                        return (value, 0., Micros(remaining));
                    }
                    Effect::Tween {
                        target: TweenTarget::AudioInstance { .. },
                        to,
                        duration_us,
                        easing,
                        ..
                    } => {
                        let elapsed = task.audio_device_elapsed_us.unwrap_or(task.elapsed_us);
                        let remaining = duration_us.0.saturating_sub(elapsed.0);
                        let value = ScalarTween {
                            from: task.captured,
                            base: 1.,
                            to,
                            duration_us,
                            easing,
                            finish: FinishPolicy::CommitEnd,
                            cancel: CancelPolicy::CommitCurrent,
                            source_curve: None,
                        }
                        .sample(elapsed);
                        return (value, to, Micros(remaining));
                    }
                    _ => {}
                }
            }
        }
        let value = self
            .state
            .tasks
            .get(&id)
            .map_or(1., |task| task.audio_envelope);
        (value, value, Micros(0))
    }
    pub fn audio_envelope_checkpoint(&self, id: u32) -> (Option<u32>, Micros) {
        self.state
            .tasks
            .values()
            .find(|task| {
                task.state == TaskState::Running
                    && task.target_task == Some(id)
                    && matches!(
                        task.effect,
                        Effect::AudioStop { .. }
                            | Effect::Tween {
                                target: TweenTarget::AudioInstance { .. },
                                ..
                            }
                    )
            })
            .map_or((None, Micros(0)), |task| {
                (
                    Some(task.id),
                    task.audio_device_elapsed_us.unwrap_or(task.elapsed_us),
                )
            })
    }
    fn resume_text_pause(&mut self, id: u32) -> Result<()> {
        let now = self.state.tick_us;
        let d = self
            .state
            .tasks
            .get_mut(&id)
            .unwrap()
            .dialogue
            .as_mut()
            .unwrap();
        d.awaiting_advance = false;
        d.span += 1;
        d.cluster = 0;
        d.last_reveal_us = now;
        if let Some(reading) = &mut d.reading {
            reading.revision = reading.revision.checked_add(1).ok_or_else(|| {
                Diagnostic::new("E_LIMIT", "dialogue", "reading revision overflow")
            })?;
        }
        Ok(())
    }
    fn reveal(&mut self, id: u32, until_gate: bool) -> Result<()> {
        let now = self.state.tick_us;
        let task = self.state.tasks.get_mut(&id).unwrap();
        let d = task.dialogue.as_mut().unwrap();
        if d.at_gate || d.awaiting_advance {
            return Ok(());
        }
        loop {
            if d.span >= d.spans.len() {
                d.awaiting_advance = true;
                break;
            }
            let s = &d.spans[d.span];
            if s.pause {
                d.awaiting_advance = true;
                break;
            }
            if s.gate {
                d.at_gate = true;
                task.milestones.insert(Milestone::Marker(s.id.clone()));
                break;
            }
            let count = s.text.graphemes(true).count();
            if d.cluster < count {
                d.cluster += 1;
                if !until_gate {
                    break;
                }
            } else {
                d.span += 1;
                d.cluster = 0;
            }
        }
        d.last_reveal_us = now;
        Ok(())
    }
    fn advance_time(&mut self, delta: u64) -> Result<()> {
        let end = self
            .state
            .tick_us
            .0
            .checked_add(delta)
            .ok_or_else(|| self.error("E_TIME", "clock overflow"))?;
        // Finish runnable logic at this instant before moving to the next deadline.
        self.run()?;
        while self.state.tick_us.0 < end
            && self.state.pending.is_none()
            && self.state.outcome.is_none()
            && !self
                .intents
                .iter()
                .any(|i| matches!(i, CoreIntent::PrepareContent { .. }))
        {
            if self.work_remaining == 0 {
                self.remaining_time_us = end - self.state.tick_us.0;
                break;
            }
            self.work_remaining -= 1;
            let now = self.state.tick_us.0;
            let mut next = end;
            for t in self
                .state
                .tasks
                .values()
                .filter(|t| t.state == TaskState::Running)
            {
                if self.task_audio_paused(t) {
                    continue;
                }
                let due = match &t.effect {
                    Effect::DialogueShake { spec }
                    | Effect::SpriteWave { spec, .. }
                    | Effect::SpriteShake { spec, .. } => {
                        Some(t.started_us.0.saturating_add(spec.duration_us.0))
                    }
                    Effect::SpriteTimeline { duration_us, .. }
                    | Effect::Clip { duration_us, .. }
                    | Effect::SourceMotion { duration_us, .. }
                    | Effect::Tween { duration_us, .. }
                    | Effect::Delay { duration_us }
                    | Effect::AudioStop { duration_us, .. }
                    | Effect::StagePresent { duration_us, .. } => {
                        Some(t.started_us.0.saturating_add(duration_us.0))
                    }
                    Effect::Dialogue { .. }
                        if t.dialogue.as_ref().is_some_and(|d| {
                            d.awaiting_advance && d.spans.get(d.span).is_some_and(|s| s.pause)
                        }) =>
                    {
                        t.dialogue.as_ref().and_then(|d| {
                            d.spans[d.span]
                                .pause_timeout_us
                                .map(|v| d.last_reveal_us.0.saturating_add(v.0))
                        })
                    }
                    Effect::Dialogue { reveal_us, .. } => t
                        .dialogue
                        .as_ref()
                        .filter(|d| !d.at_gate && !d.awaiting_advance)
                        .map(|d| {
                            d.last_reveal_us
                                .0
                                .saturating_add(d.reveal_interval_us.unwrap_or(*reveal_us).0.max(1))
                        }),
                    _ => None,
                };
                if let Some(due) = due {
                    next = next.min(due.max(now.saturating_add(1)));
                }
            }
            if let Some(deadline) = self.state.choice.as_ref().and_then(|c| c.deadline_us) {
                next = next.min(deadline.0.max(now.saturating_add(1)));
            }
            if let Some(reveal) = &self.state.window_reveal {
                let due = reveal
                    .started_us
                    .0
                    .saturating_add(reveal.duration_us.0)
                    .max(now.saturating_add(1));
                next = next.min(due);
            }
            self.state.tick_us = Micros(next);
            let frozen: BTreeSet<_> = self
                .state
                .tasks
                .values()
                .filter(|t| self.task_audio_paused(t))
                .map(|t| t.id)
                .collect();
            for t in self
                .state
                .tasks
                .values_mut()
                .filter(|t| t.state == TaskState::Running)
            {
                if frozen.contains(&t.id) {
                    t.started_us.0 += next - now;
                }
                t.elapsed_us = Micros(next - t.started_us.0);
            }
            let ids: Vec<_> = self
                .state
                .tasks
                .values()
                .filter(|t| t.state == TaskState::Running)
                .map(|t| t.id)
                .collect();
            for id in ids {
                let t = &self.state.tasks[&id];
                if self.task_audio_paused(t) {
                    continue;
                }
                match t.effect {
                    Effect::DialogueShake { spec }
                    | Effect::SpriteWave { spec, .. }
                    | Effect::SpriteShake { spec, .. }
                        if t.elapsed_us.0 >= spec.duration_us.0 =>
                    {
                        self.finish_task(id, TaskState::Finished)?
                    }
                    Effect::SpriteTimeline { duration_us, .. }
                    | Effect::Clip { duration_us, .. }
                    | Effect::SourceMotion { duration_us, .. }
                    | Effect::Tween { duration_us, .. }
                    | Effect::Delay { duration_us }
                    | Effect::AudioStop { duration_us, .. }
                    | Effect::StagePresent { duration_us, .. }
                        if t.elapsed_us.0 >= duration_us.0 =>
                    {
                        self.finish_task(id, TaskState::Finished)?
                    }
                    Effect::Dialogue { .. }
                        if t.dialogue.as_ref().is_some_and(|d| {
                            d.awaiting_advance
                                && d.spans.get(d.span).is_some_and(|s| {
                                    s.pause
                                        && s.pause_timeout_us.is_some_and(|v| {
                                            next >= d.last_reveal_us.0.saturating_add(v.0)
                                        })
                                })
                        }) =>
                    {
                        self.resume_text_pause(id)?;
                    }
                    Effect::Dialogue { reveal_us, .. }
                        if t.dialogue.as_ref().is_some_and(|d| {
                            !d.at_gate
                                && !d.awaiting_advance
                                && next - d.last_reveal_us.0
                                    >= d.reveal_interval_us.unwrap_or(reveal_us).0.max(1)
                        }) =>
                    {
                        self.reveal(id, false)?
                    }
                    _ => {}
                }
            }
            if let Some(reveal) = self.state.window_reveal.clone() {
                if next >= reveal.started_us.0.saturating_add(reveal.duration_us.0) {
                    self.state.window_reveal = None;
                    self.state.dialogue_hidden = !reveal.to_visible;
                }
            }
            if let Some(c) = self.state.choice.clone() {
                if c.deadline_us.is_some_and(|v| v.0 <= next) {
                    if let Some(option) = &c.default {
                        // A timeout commits the default option, typed value
                        // included, exactly like an explicit choice.
                        if let (Some(target), Some(value)) = (&c.result, c.values.get(option)) {
                            let target = target.clone();
                            let value = value.clone();
                            self.write(&target, value)?;
                        }
                        if let Some(history) = self.choice_history(
                            &c,
                            HistoryChoiceResolution::TimedOut {
                                option: option.clone(),
                            },
                        ) {
                            self.push_history(history);
                        }
                        self.state.choice = None;
                        self.jump(c.branches[option].clone());
                        self.trace(format!("timeout:{option}"));
                    }
                }
            }
            self.run()?;
        }
        Ok(())
    }
    fn track_is_current(&self, task: &Task, target: &TweenTarget) -> bool {
        !matches!(target, TweenTarget::SceneNode { .. })
            || task.scene_generation == self.state.scene_generation
    }
    fn sample_timelines(&self, nodes: &mut [Node]) {
        for task in self.state.tasks.values().filter(|t| {
            t.state == TaskState::Running && t.scene_generation == self.state.scene_generation
        }) {
            if let Effect::SpriteTimeline { timeline, root, .. } = &task.effect {
                if nodes.iter().any(|n| {
                    &n.id == root && n.timeline_binding.as_deref() == Some(timeline.as_str())
                }) {
                    if let Some(resource) = self.program().sprite_timelines.get(timeline) {
                        resource.apply(nodes, task.elapsed_us.0);
                    }
                }
            }
        }
    }
    pub fn sample_scene(&self) -> Vec<Node> {
        let mut nodes = self.state.scene.clone();
        self.sample_timelines(&mut nodes);
        for task in self
            .state
            .tasks
            .values()
            .filter(|t| t.state == TaskState::Running)
        {
            if let (
                Effect::SpriteWave {
                    nodes: targets,
                    spec,
                },
                Some(shake),
            ) = (&task.effect, &task.shake)
            {
                if task.scene_generation == self.state.scene_generation {
                    let offset = shake.sample(*spec, task.elapsed_us);
                    for node in &mut nodes {
                        if targets.contains(&node.id) {
                            node.offset[0] += offset[0];
                            node.offset[1] += offset[1];
                        }
                    }
                }
            }
            if let Effect::SpriteShake { spec, .. } = &task.effect {
                if task.scene_generation == self.state.scene_generation {
                    for node in &mut nodes {
                        if let Some(capture) = task.sprite_shakes.get(&node.id) {
                            let offset = capture.sample(*spec, task.elapsed_us);
                            node.offset[0] += offset[0];
                            node.offset[1] += offset[1];
                        }
                    }
                }
            }
            if let Some((address, track, _)) = task.effect.scalar_track(task.captured, task.base) {
                if self.track_is_current(task, &address) {
                    if let TweenTarget::SceneNode { node, property } = address {
                        if let Some(n) = nodes.iter_mut().find(|n| n.id == node) {
                            n.set(property, track.sample(task.elapsed_us));
                        }
                    }
                }
            }
        }
        nodes
    }
    pub fn sample_dialogue_appearance(&self) -> DialogueAppearance {
        let mut appearance = self.state.dialogue_appearance;
        for task in self
            .state
            .tasks
            .values()
            .filter(|t| t.state == TaskState::Running)
        {
            if let (Effect::DialogueShake { spec }, Some(shake)) = (&task.effect, &task.shake) {
                appearance.text_offset = shake.sample(*spec, task.elapsed_us);
            }
            if let Some((TweenTarget::DialogueRoot { property }, track, _)) =
                task.effect.scalar_track(task.captured, task.base)
            {
                appearance.set(property, track.sample(task.elapsed_us));
            }
        }
        appearance
    }
    pub fn advance_wait(&self) -> Option<u32> {
        self.state
            .waiting
            .as_ref()
            .and_then(|w| w.advance.as_ref())
            .map(|a| a.interaction)
    }
    pub fn dialogue(&self) -> Option<(u32, &Dialogue)> {
        self.state.tasks.values().rev().find_map(|t| {
            if t.state == TaskState::Running {
                t.dialogue.as_ref().map(|d| (t.id, d))
            } else {
                None
            }
        })
    }
    pub fn transition_style(&self) -> StageTransition {
        self.state
            .tasks
            .values()
            .find_map(|t| {
                if t.state == TaskState::Running {
                    if let Effect::StagePresent { transition, .. } = &t.effect {
                        return Some(transition.clone());
                    }
                }
                None
            })
            .unwrap_or_default()
    }
    pub fn transition(&self) -> Option<(Vec<Node>, f32)> {
        self.state.tasks.values().find_map(|t| {
            if t.state == TaskState::Running {
                if let Effect::StagePresent { duration_us, .. } = t.effect {
                    if duration_us.0 > 0 {
                        let mut source = t.source.clone();
                        self.sample_timelines(&mut source);
                        let current = if t.target.iter().any(|node| !node.preserve_pose.is_empty())
                        {
                            self.sample_scene()
                        } else {
                            vec![]
                        };
                        for node in t
                            .target
                            .iter()
                            .filter(|node| !node.preserve_pose.is_empty())
                        {
                            if let (Some(old), Some(now)) = (
                                source.iter_mut().find(|n| n.id == node.id),
                                current.iter().find(|n| n.id == node.id),
                            ) {
                                for property in &node.preserve_pose {
                                    old.set(*property, now.get(*property));
                                }
                            }
                        }
                        return Some((
                            source,
                            (t.elapsed_us.0 as f64 / duration_us.0 as f64).min(1.) as f32,
                        ));
                    }
                }
                None
            } else {
                None
            }
        })
    }
    pub fn transition_progress(&self) -> Option<f32> {
        self.state.tasks.values().find_map(|task| {
            if let Effect::StagePresent { duration_us, .. } = task.effect {
                if task.state == TaskState::Running && duration_us.0 > 0 {
                    return Some((task.elapsed_us.0 as f64 / duration_us.0 as f64).min(1.) as f32);
                }
            }
            None
        })
    }
    /// In-flight message-window reveal: (style, direction, linear progress).
    /// The committed `dialogue_hidden` still holds the pre-op value; coverage
    /// blends from `from_coverage` so reversals stay continuous.
    pub fn window_reveal(&self) -> Option<(&StageTransition, bool, f32)> {
        let reveal = self.state.window_reveal.as_ref()?;
        let progress = (self.state.tick_us.0.saturating_sub(reveal.started_us.0) as f64
            / reveal.duration_us.0 as f64)
            .min(1.) as f32;
        Some((&reveal.style, reveal.to_visible, progress))
    }
    fn window_coverage(&self) -> f32 {
        match &self.state.window_reveal {
            Some(reveal) => {
                let progress = (self.state.tick_us.0.saturating_sub(reveal.started_us.0) as f64
                    / reveal.duration_us.0 as f64)
                    .clamp(0., 1.) as f32;
                let target = if reveal.to_visible { 1. } else { 0. };
                reveal.from_coverage + (target - reveal.from_coverage) * progress
            }
            None => {
                if self.state.dialogue_hidden {
                    0.
                } else {
                    1.
                }
            }
        }
    }
    /// Observations never execute story code or change task time/milestones.
    pub fn observe_audio_positions(&mut self, positions: &[AudioPosition]) -> Result<()> {
        if positions.len() > MAX_TASKS
            || positions
                .iter()
                .map(|p| p.task)
                .collect::<BTreeSet<_>>()
                .len()
                != positions.len()
        {
            return Err(self.error(
                "E_AUDIO_POSITION",
                "duplicate or excessive device positions",
            ));
        }
        // Validate the complete batch before mutating any checkpoint.
        for position in positions {
            if let Some(envelope) = &position.envelope {
                if let Some(task) = self.state.tasks.get(&envelope.owner).filter(|t| {
                    t.state == TaskState::Running && t.target_task == Some(position.task)
                }) {
                    if !envelope_owner_duration(task)
                        .is_some_and(|duration| envelope.elapsed_us.0 <= duration.0)
                    {
                        return Err(
                            self.error("E_AUDIO_POSITION", "invalid device envelope progress")
                        );
                    }
                }
            }
        }
        for position in positions {
            let active = self.state.tasks.get(&position.task).is_some_and(|t| {
                t.state == TaskState::Running && matches!(t.effect, Effect::Audio { .. })
            });
            if active {
                if let Some(envelope) = &position.envelope {
                    if let Some(task) = self.state.tasks.get_mut(&envelope.owner).filter(|t| {
                        t.state == TaskState::Running
                            && t.target_task == Some(position.task)
                            && envelope_owner_duration(t).is_some()
                    }) {
                        // Within one device incarnation progress is monotonic.
                        task.audio_device_elapsed_us = Some(Micros(
                            task.audio_device_elapsed_us
                                .map_or(envelope.elapsed_us.0, |old| {
                                    old.0.max(envelope.elapsed_us.0)
                                }),
                        ));
                    }
                }
            }
            if let Some(task) = self.state.tasks.get_mut(&position.task).filter(|t| {
                t.state == TaskState::Running && matches!(t.effect, Effect::Audio { .. })
            }) {
                task.audio_position_us = Some(position.position_us);
            }
        }
        Ok(())
    }
    pub fn needs_clock(&self) -> bool {
        if self.state.fault.is_some() || self.state.outcome.is_some() {
            return false;
        }
        // A budget yield is runnable work, not a suspension. Keep pumping until
        // a real wait, end or the non-suspending-loop guard is reached.
        if self.state.pending.is_none()
            && self.state.waiting.is_none()
            && self.state.choice.is_none()
        {
            return true;
        }
        self.state
            .choice
            .as_ref()
            .is_some_and(|c| c.deadline_us.is_some())
            // A reveal in flight must reach its deadline even when the VM is
            // otherwise parked at an input barrier.
            || self.state.window_reveal.is_some()
            || self.state.tasks.values().any(|task| self.task_needs_clock(task))
    }
    /// Audio keeps the Story clock alive for save offsets without changing
    /// the projected picture. All other runnable effects remain conservative.
    pub fn needs_visual_clock(&self) -> bool {
        if self.state.fault.is_some() || self.state.outcome.is_some() {
            return false;
        }
        // A budget yield can retain logic before the next time step. Do not
        // mistake its semantic work for the audio time bookkeeping allowance.
        (self.state.pending.is_none()
            && self.state.waiting.is_none()
            && self.state.choice.is_none())
            || self
                .state
                .choice
                .as_ref()
                .is_some_and(|c| c.deadline_us.is_some())
            || self.state.window_reveal.is_some()
            || self.state.tasks.values().any(|task| {
                !matches!(task.effect, Effect::Audio { .. }) && self.task_needs_clock(task)
            })
    }
    fn task_needs_clock(&self, task: &Task) -> bool {
        task.state == TaskState::Running
            && !self.task_audio_paused(task)
            && match task.effect {
                Effect::Dialogue { .. } => task.dialogue.as_ref().is_some_and(|dialogue| {
                    !dialogue.at_gate
                        && (!dialogue.awaiting_advance
                            || dialogue
                                .spans
                                .get(dialogue.span)
                                .is_some_and(|span| span.pause && span.pause_timeout_us.is_some()))
                }),
                _ => true,
            }
    }
    pub fn restore(program: ValidatedProgram, s: Snapshot, release: &str) -> Result<Self> {
        Self::restore_inner(program, s, release, false)
    }
    fn task_audio_paused(&self, task: &Task) -> bool {
        let target = task
            .target_task
            .and_then(|id| self.state.tasks.get(&id))
            .unwrap_or(task);
        matches!(target.effect, Effect::Audio { bus, .. } if self.state.audio_paused.contains(&bus))
    }
    pub fn restore_verified(
        program: ValidatedProgram,
        proof: crate::VerifiedSnapshot,
        release: &str,
    ) -> Result<Self> {
        let snapshot = proof.consume(&program, release)?;
        Self::restore_inner(program, snapshot, release, true)
    }
    fn restore_inner(
        program: ValidatedProgram,
        s: Snapshot,
        release: &str,
        canonical_verified: bool,
    ) -> Result<Self> {
        let fail = |msg: &str| Diagnostic::new("E_SNAPSHOT", "restore", msg);
        let p = program.program();
        if s.menu_disabled && !p.requires.iter().any(|c| c == "player.menu-access.v1") {
            return Err(fail("menu access capability missing"));
        }
        if !s.audio_paused.is_empty() && !p.requires.iter().any(|c| c == "audio.pause.v1") {
            return Err(fail("audio pause capability missing"));
        }
        if s.format != SNAPSHOT_VERSION
            || s.game_id != p.game_id
            || s.revision != p.revision
            || s.release != release
        {
            return Err(fail("incompatible content/release"));
        }
        if !p.locales.contains_key(&s.locale)
            || s.frames.is_empty()
            || s.frames.len() > MAX_FRAMES
            || s.tasks.len() > MAX_TASKS * 2
            || s.scene.len() > MAX_NODES
            || s.draft.len() > MAX_NODES
            || s.history.len() > 1000
            || s.history.iter().map(history_bytes).sum::<usize>() > 4 * 1024 * 1024
            || s.next_id == 0
        {
            return Err(fail("state limits/locale"));
        }
        if s.dialogue_style
            .as_ref()
            .is_some_and(|style| !p.theme.dialogue_styles.contains_key(style))
        {
            return Err(fail("unknown dialogue style"));
        }
        if !s.dialogue_decorations.is_empty()
            && (!p.requires.iter().any(|c| c == "dialogue.decoration.v1")
                || s.dialogue_decorations.values().any(|d| {
                    !d.valid()
                        || p.asset(&d.asset).is_none_or(|asset| {
                            asset.kind != AssetKind::Image
                                || d.rect[2] != asset.width as f32
                                || d.rect[3] != asset.height as f32
                        })
                }))
        {
            return Err(fail("invalid dialogue decoration"));
        }
        if s.variables.len() != p.variables.len()
            || p.variables
                .iter()
                .any(|(k, v)| s.variables.get(k).map(Value::ty) != Some(v.ty()))
        {
            return Err(fail("variable layout"));
        }
        for h in &s.history {
            let identity = p
                .text_identity(&h.text_id)
                .ok_or_else(|| fail("unknown history text"))?;
            if h.images.len() != identity.images.len()
                || h.images.len() > 256
                || h.images
                    .iter()
                    .zip(&identity.images)
                    .any(|(placement, binding)| {
                        let at = placement.offset as usize;
                        !placement.image.valid()
                            || placement.image.asset != binding.asset
                            || h.text.get(at..at.saturating_add(3)) != Some("\u{fffc}")
                    })
                || h.images
                    .windows(2)
                    .any(|pair| pair[0].offset >= pair[1].offset)
                || (h.choice.is_some() && !h.images.is_empty())
            {
                return Err(fail("invalid history inline images"));
            }
            if let Some(choice) = &h.choice {
                // Completed history is passive metadata. The root declares
                // choice and text identities even after the old module's
                // bodies are evicted; do not fetch/re-run that old chapter.
                if h.interaction == 0
                    || !h.voices.is_empty()
                    || !h.speaker.is_empty()
                    || !p.choices.contains_key(&choice.id)
                    || choice.options.is_empty()
                    || (choice.resolution == HistoryChoiceResolution::Cancelled
                        && !p.requires.iter().any(|c| c == "story.typed-result.v1"))
                {
                    return Err(fail("invalid history choice"));
                }
                let mut options = BTreeSet::new();
                for option in &choice.options {
                    let identity = p
                        .text_identity(&option.text_id)
                        .ok_or_else(|| fail("unknown history option text"))?;
                    if option.id.is_empty()
                        || !options.insert(&option.id)
                        || option.meaning_revision != identity.meaning_revision
                        || option.source_revision != identity.source_revision
                        || option.contract_digest != identity.contract_digest
                    {
                        return Err(fail("history option identity"));
                    }
                }
                let option = choice
                    .display_option()
                    .ok_or_else(|| fail("invalid history resolution"))?;
                if h.text_id != option.text_id
                    || h.text != choice.display_text()
                    || h.source_revision != option.source_revision
                    || h.meaning_revision != option.meaning_revision
                    || h.contract_digest != option.contract_digest
                {
                    return Err(fail("history choice display"));
                }
            }
            if h.interaction >= s.next_id
                || (!h.speaker_id.is_empty()
                    && (h.choice.is_some()
                        || h.speaker_id.len() > MAX_CHARACTER_ID_BYTES
                        || p.text_identity(&h.speaker_id).is_none()))
                || h.voices.len() > MAX_HISTORY_VOICES
                || (!h.voices.is_empty()
                    && !p.requires.iter().any(|c| c == "text.voice-binding.v1"))
            {
                return Err(fail("history voice limits/identity"));
            }
            let mut voices = BTreeSet::new();
            for voice in &h.voices {
                if h.interaction == 0
                    || voice.instance == 0
                    || voice.instance >= s.next_id
                    || !voices.insert(voice.instance)
                    || !voice.gain.is_finite()
                    || !(0. ..=4.).contains(&voice.gain)
                    || !p
                        .asset_kind(&voice.asset)
                        .is_some_and(|kind| kind == AssetKind::Audio)
                {
                    return Err(fail("invalid history voice"));
                }
            }
            let c = p
                .text_identity(&h.text_id)
                .ok_or_else(|| fail("unknown history text"))?;
            if h.meaning_revision != c.meaning_revision
                || h.source_revision != c.source_revision
                || h.contract_digest != c.contract_digest
                || !p.locales.contains_key(&h.locale)
                || p.locale_config
                    .text
                    .get(&h.locale)
                    .map(|plan| plan.digest.as_str())
                    != Some(h.font_plan_digest.as_str())
            {
                return Err(fail("history text identity"));
            }
        }
        let mut instances = BTreeSet::new();
        for f in &s.frames {
            let def = p
                .functions
                .get(&f.function)
                .ok_or_else(|| fail("missing function"))?;
            let b = def
                .blocks
                .get(&f.block)
                .ok_or_else(|| fail("missing block"))?;
            if f.op > b.ops.len()
                || f.op_id
                    != b.ops
                        .get(f.op)
                        .map(|o| o.id.as_str())
                        .unwrap_or("@terminator")
                || f.id >= s.next_id
                || !instances.insert(f.id)
            {
                return Err(fail("invalid frame address/identity"));
            }
            for (k, v) in &f.locals {
                if def.locals.get(k).or_else(|| def.params.get(k)) != Some(&v.ty()) {
                    return Err(fail("local layout"));
                }
            }
        }
        for (id, t) in &s.tasks {
            if matches!(t.effect, Effect::StoryModal { .. }) {
                if t.modal_interaction.is_none_or(|token| {
                    token == 0 || token >= s.next_id || !instances.insert(token)
                }) {
                    return Err(fail("invalid story modal interaction"));
                }
            } else if t.modal_interaction.is_some() {
                return Err(fail("unexpected story modal interaction"));
            }
            match &t.effect {
                Effect::SpriteShake { nodes, mode, spec }
                    if t.sprite_shakes.len() == nodes.len()
                        && nodes.iter().all(|node| {
                            t.sprite_shakes
                                .get(node)
                                .is_some_and(|capture| capture.valid_sprite(*mode, *spec))
                        })
                        && (*mode != nir_format::SpriteShakeMode::Quake
                            || crate::ShakeCapture::shared_quake_phases(&t.sprite_shakes)) => {}
                Effect::SpriteShake { .. } => {
                    return Err(fail("invalid independent sprite trajectories"))
                }
                _ if !t.sprite_shakes.is_empty() => {
                    return Err(fail("unexpected independent sprite trajectories"))
                }
                _ => {}
            }
            match (&t.effect, &t.shake) {
                (Effect::DialogueShake { spec }, Some(shake)) if shake.valid(*spec) => {}
                (Effect::SpriteWave { spec, .. }, Some(shake)) if shake.valid_wave(*spec) => {}
                (Effect::DialogueShake { .. } | Effect::SpriteWave { .. }, _) | (_, Some(_)) => {
                    return Err(fail("invalid frozen shake"))
                }
                _ => {}
            }
            if *id != t.id
                || t.id >= s.next_id
                || !instances.insert(t.id)
                || t.started_us.0 > s.tick_us.0
                || t.scene_generation > s.scene_generation
                || !t.captured.is_finite()
                || !t.base.is_finite()
            {
                return Err(fail("task state"));
            }
            if let (Effect::Dialogue { reveal_us, .. }, Some(d)) = (&t.effect, &t.dialogue) {
                if d.reveal_interval_us
                    .is_some_and(|v| !valid_reveal_interval(*reveal_us, v))
                {
                    return Err(fail("invalid frozen reveal interval"));
                }
            }
            if !t.voice_character.is_empty()
                && (t.voice_character.len() > MAX_CHARACTER_ID_BYTES
                    || p.text_identity(&t.voice_character).is_none()
                    || !matches!(
                        t.effect,
                        Effect::Audio {
                            bus: AudioBus::Voice,
                            looped: false,
                            ..
                        }
                    ))
            {
                return Err(fail("invalid voice character"));
            }
            if let (Effect::Dialogue { speaker, .. }, Some(d)) = (&t.effect, &t.dialogue) {
                if !d.speaker_id.is_empty() && &d.speaker_id != speaker {
                    return Err(fail("invalid speaker identity"));
                }
            }
            // Composition snapshots: the cursor counts spawned children, each
            // spawned child matches its declared definition and scope, a
            // sequence runs at most its last spawned child, and a finished
            // composition has no pending work left.
            if let Some(defs) = t.effect.compose_children() {
                if t.cursor as usize != t.children.len()
                    || t.children.len() > defs.len()
                    || t.children.iter().any(|c| !s.tasks.contains_key(c))
                {
                    return Err(fail("composition cursor/children"));
                }
                for (index, child) in t.children.iter().enumerate() {
                    let c = &s.tasks[child];
                    if c.name != defs[index].id
                        || c.scope != t.scope
                        || serde_json::to_value(&c.effect).unwrap()
                            != serde_json::to_value(&defs[index].effect).unwrap()
                    {
                        return Err(fail("composition child mismatch"));
                    }
                }
                if matches!(t.effect, Effect::Sequence { .. })
                    && t.children.len() > 1
                    && t.children[..t.children.len() - 1]
                        .iter()
                        .any(|c| s.tasks[c].state == TaskState::Running)
                {
                    return Err(fail("sequence runs more than one child"));
                }
                if t.milestones.contains(&Milestone::Finished)
                    && (t.children.len() != defs.len()
                        || t.children
                            .iter()
                            .any(|c| s.tasks[c].state == TaskState::Running))
                {
                    return Err(fail("finished composition has pending children"));
                }
            }
            if let Some(d) = &t.dialogue {
                if !p.locales.contains_key(&d.locale)
                    || p.locale_config
                        .text
                        .get(&d.locale)
                        .map(|plan| plan.digest.as_str())
                        != Some(d.font_plan_digest.as_str())
                    || !p.texts.contains_key(&d.text_id)
                    || d.span > d.spans.len()
                    || d.spans.len() > 8192
                    || d.full_text().len() > 128 * 1024
                    || d.span < d.spans.len()
                        && d.cluster > d.spans[d.span].text.graphemes(true).count()
                {
                    return Err(fail("dialogue state"));
                }
            }
        }
        if s.tasks
            .values()
            .filter(|t| {
                t.state == TaskState::Running && matches!(t.effect, Effect::StoryModal { .. })
            })
            .count()
            > 1
            || (s.choice.is_some()
                && s.tasks.values().any(|t| {
                    t.state == TaskState::Running && matches!(t.effect, Effect::StoryModal { .. })
                }))
        {
            return Err(fail("multiple story modal interactions"));
        }
        if s.handles.values().any(|id| !s.tasks.contains_key(id))
            || s.waiting
                .as_ref()
                .is_some_and(|w| w.conditions.iter().any(|(id, _)| !s.tasks.contains_key(id)))
        {
            return Err(fail("dangling task handle"));
        }
        if let Some(pending) = &s.pending {
            let c = p
                .cues
                .get(&pending.cue)
                .ok_or_else(|| fail("missing pending cue"))?;
            if serde_json::to_value(&c.effects).unwrap()
                != serde_json::to_value(&pending.effects).unwrap()
            {
                return Err(fail("pending cue mismatch"));
            }
        }
        if let Some(c) = &s.choice {
            let d = p.choices.get(&c.id).ok_or_else(|| fail("missing choice"))?;
            if c.options
                .iter()
                .any(|o| !d.options.iter().any(|x| x.id == o.id))
                || c.options.is_empty()
            {
                return Err(fail("invalid offered choice"));
            }
        }
        // A save is untrusted input too: every continuation is checked against the
        // canonical terminator before a trusted Session can be constructed.
        let top = s.frames.last().unwrap();
        let block = &p.functions[&top.function].blocks[&top.block];
        if s.pending.is_some() as u8 + s.waiting.is_some() as u8 + s.choice.is_some() as u8 > 1 {
            return Err(fail("multiple active terminators"));
        }
        if let Some(pending) = &s.pending {
            if !matches!(&block.terminator,Terminator::Activate{cue,next} if cue==&pending.cue&&next==&pending.next)
                || top.op != block.ops.len()
                || pending.id >= s.next_id
            {
                return Err(fail("pending continuation mismatch"));
            }
            for (name, d) in &pending.dialogues {
                if d.reading.is_some() {
                    return Err(fail("pending dialogue cannot have a voice binding"));
                }
                if d.reveal_interval_us.is_some_and(|v| !pending.effects.iter().any(|e| {
                    &e.id == name && matches!(e.effect, Effect::Dialogue { reveal_us, .. } if valid_reveal_interval(reveal_us, v))
                })) { return Err(fail("invalid pending reveal interval")); }
                if !pending.effects.iter().any(|e| {
                    &e.id == name
                        && matches!(&e.effect,Effect::Dialogue{text,..} if text==&d.text_id)
                }) {
                    return Err(fail("pending dialogue mismatch"));
                }
                if !canonical_verified {
                    validate_dialogue(d, p, s.tick_us, s.next_id)?;
                }
            }
        }
        if let Some(w) = &s.waiting {
            let Terminator::Await {
                conditions,
                next,
                on_cancelled,
                on_failed,
                on_advance,
            } = &block.terminator
            else {
                return Err(fail("unexpected wait"));
            };
            let expected: Option<Vec<_>> = conditions
                .iter()
                .map(|c| Some((*s.handles.get(&c.task)?, c.milestone.clone())))
                .collect();
            if expected.as_ref() != Some(&w.conditions)
                || next != &w.next
                || on_cancelled != &w.on_cancelled
                || on_failed != &w.on_failed
                || on_advance.as_deref() != w.advance.as_ref().map(|a| a.next.as_str())
                || top.op != block.ops.len()
            {
                return Err(fail("wait continuation mismatch"));
            }
            if w.advance.as_ref().is_some_and(|a| {
                a.interaction == 0
                    || a.interaction >= s.next_id
                    || s.tasks.contains_key(&a.interaction)
                    || s.tasks.values().any(|t| {
                        t.dialogue
                            .as_ref()
                            .is_some_and(|d| d.interaction == a.interaction)
                    })
            }) {
                return Err(fail("invalid advance-wait interaction"));
            }
        }
        if let Some(c) = &s.choice {
            let Terminator::Interact {
                choice,
                branches,
                on_empty: _,
                result,
                on_cancel,
            } = &block.terminator
            else {
                return Err(fail("choice continuation mismatch"));
            };
            if choice != &c.id
                || branches != &c.branches
                || result != &c.result
                || on_cancel != &c.on_cancel
                || top.op != block.ops.len()
                || c.interaction >= s.next_id
                || !p.locales.contains_key(&c.locale)
                || p.locale_config
                    .text
                    .get(&c.locale)
                    .map(|plan| plan.digest.as_str())
                    != Some(c.font_plan_digest.as_str())
            {
                return Err(fail("choice continuation mismatch"));
            }
            let ids: BTreeSet<_> = c.options.iter().map(|o| &o.id).collect();
            if ids.len() != c.options.len() || !c.options.iter().any(|o| o.enabled) {
                return Err(fail("invalid offered options"));
            }
            if c.default
                .as_ref()
                .is_some_and(|d| !c.options.iter().any(|o| &o.id == d && o.enabled))
            {
                return Err(fail("unavailable timeout default"));
            }
            // Typed-result snapshots: the values are exactly the declared
            // values of the offered options, the cursor names a live enabled
            // row, and non-result interactions carry neither.
            let definition = p.choices.get(&c.id).ok_or_else(|| fail("missing choice"))?;
            let expected: BTreeMap<_, _> = c
                .options
                .iter()
                .filter_map(|o| {
                    definition
                        .options
                        .iter()
                        .find(|d| d.id == o.id)
                        .and_then(|d| d.value.clone())
                        .map(|value| (o.id.clone(), value))
                })
                .collect();
            if c.result.is_some() {
                if expected != c.values
                    || c.selected
                        .as_ref()
                        .is_none_or(|id| !c.options.iter().any(|o| &o.id == id && o.enabled))
                {
                    return Err(fail("invalid typed-result interaction"));
                }
            } else if !c.values.is_empty() || c.selected.is_some() {
                return Err(fail("plain interaction carries no result state"));
            }
        }
        for (index, f) in s.frames.iter().enumerate() {
            if index == 0 {
                if f.return_to.is_some() {
                    return Err(fail("root return address"));
                }
            } else {
                let caller = &p.functions[&s.frames[index - 1].function];
                if !f
                    .return_to
                    .as_ref()
                    .is_some_and(|b| caller.blocks.contains_key(b))
                {
                    return Err(fail("invalid return address"));
                }
            }
        }
        if !s.dialogue_appearance.valid() {
            return Err(fail("invalid dialogue appearance"));
        }
        if s.dialogue_appearance.text_offset != [0.; 2]
            && !p
                .requires
                .iter()
                .any(|cap| cap == "stage.dialogue-shake.v1")
        {
            return Err(fail("dialogue offset capability missing"));
        }
        if let Some(reveal) = &s.window_reveal {
            // Structural only: the reveal is operation-committed, so unlike
            // tasks it has no cue declaration to match against.
            if !p.requires.iter().any(|c| c == "text.window-transition.v1")
                || !reveal.style.valid()
                || reveal.duration_us.0 == 0
                || !reveal.from_coverage.is_finite()
                || !(0. ..=1.).contains(&reveal.from_coverage)
                || reveal.started_us.0.saturating_add(reveal.duration_us.0) <= s.tick_us.0
            {
                return Err(fail("invalid window reveal"));
            }
            if reveal
                .style
                .asset()
                .is_some_and(|asset| p.asset(asset).is_none_or(|a| a.kind != AssetKind::Image))
            {
                return Err(fail("invalid window reveal mask"));
            }
        }
        let mut property_owners = BTreeSet::new();
        let mut envelope_owners = BTreeSet::new();
        let mut wave_owners = BTreeSet::new();
        let mut timeline_owners = BTreeSet::new();
        for t in s.tasks.values() {
            if t.state == TaskState::Running {
                match &t.effect {
                    Effect::SpriteTimeline {
                        timeline,
                        root,
                        duration_us,
                        ..
                    } => {
                        let resource = p
                            .sprite_timelines
                            .get(timeline)
                            .ok_or_else(|| fail("timeline not resident"))?;
                        if resource.duration_us != *duration_us
                            || t.elapsed_us.0 >= duration_us.0
                            || t.scene_generation != s.scene_generation
                            || !s.scene.iter().any(|n| {
                                &n.id == root
                                    && n.timeline_binding.as_deref() == Some(timeline.as_str())
                            })
                            || resource
                                .tracks
                                .iter()
                                .any(|track| !timeline_owners.insert(track.node.as_str()))
                        {
                            return Err(fail("invalid live timeline identity or ownership"));
                        }
                    }
                    Effect::DialogueShake { spec }
                    | Effect::SpriteWave { spec, .. }
                    | Effect::SpriteShake { spec, .. }
                        if t.elapsed_us.0 >= spec.duration_us.0 =>
                    {
                        return Err(fail("expired shake trajectory"));
                    }
                    Effect::SpriteWave { nodes, .. } | Effect::SpriteShake { nodes, .. }
                        if t.scene_generation == s.scene_generation =>
                    {
                        for node in nodes {
                            if !s.scene.iter().any(|n| &n.id == node) || !wave_owners.insert(node) {
                                return Err(fail("invalid sprite wave target ownership"));
                            }
                        }
                    }
                    _ => {}
                }
            }
            if let Some(reading) = t.dialogue.as_ref().and_then(|d| d.reading.as_ref()) {
                if !p.requires.iter().any(|c| c == "text.voice-binding.v1")
                    || (reading.wait == VoiceWaitPolicy::SampledRemaining
                        && (!p.requires.iter().any(|c|c=="text.voice-timer.v1")
                            || reading.voice.is_some_and(|id|s.tasks.get(&id).is_some_and(|t|
                                matches!(&t.effect,Effect::Audio{asset,..} if p.asset(asset).is_none_or(|a|a.duration_us.0==0))
                            ))))
                    || reading.revision == 0
                    || reading.active_voices.len() > MAX_TASKS
                    || reading.active_voices.iter().copied().collect::<BTreeSet<_>>().len()
                        != reading.active_voices.len()
                    || reading.active_voices.iter().any(|id| {
                        !s.tasks.get(id).is_some_and(|v| {
                            v.state == TaskState::Running && matches!(v.effect,
                                Effect::Audio { bus: AudioBus::Voice, looped: false, .. })
                        })
                    })
                    || reading.voice.is_some_and(|id| {
                        !s.tasks.get(&id).is_some_and(|v| {
                            matches!(
                                v.effect,
                                Effect::Audio {
                                    bus: AudioBus::Voice,
                                    looped: false,
                                    ..
                                }
                            )
                        })
                    })
                {
                    return Err(fail("invalid dialogue voice binding"));
                }
            }

            if let Some((address, track, _)) = t.effect.scalar_track(t.captured, t.base) {
                if !address.accepts(t.captured) || !address.accepts(t.base) {
                    return Err(fail("invalid captured property"));
                }
                if t.state == TaskState::Running {
                    if t.elapsed_us.0 >= track.duration_us.0 {
                        return Err(fail("expired property track"));
                    }
                    let current = !matches!(address, TweenTarget::SceneNode { .. })
                        || t.scene_generation == s.scene_generation;
                    if current {
                        if let TweenTarget::SceneNode { node, .. } = &address {
                            if !s.scene.iter().any(|n| &n.id == node) {
                                return Err(fail("missing property target"));
                            }
                        }
                        if !property_owners.insert(address) {
                            return Err(fail("multiple property writers"));
                        }
                    }
                }
            }
            if !t.audio_envelope.is_finite() || !(0.0..=1.0).contains(&t.audio_envelope) {
                return Err(fail("invalid audio envelope"));
            }
            match &t.effect {
                Effect::AudioStop {
                    target,
                    duration_us,
                } => {
                    let target_id = t.target_task.ok_or_else(|| fail("missing stop target"))?;
                    let audio = s
                        .tasks
                        .get(&target_id)
                        .ok_or_else(|| fail("missing audio instance"))?;
                    if target_id >= t.id
                        || audio.name != *target
                        || !matches!(audio.effect, Effect::Audio { .. })
                        || !t.captured.is_finite()
                        || !(0.0..=1.0).contains(&t.captured)
                        || (t.state == TaskState::Running
                            && (audio.state != TaskState::Running
                                || t.elapsed_us.0 >= duration_us.0
                                || !envelope_owners.insert(target_id)))
                    {
                        return Err(fail("invalid stop ownership or progress"));
                    }
                }
                Effect::Tween {
                    target: TweenTarget::AudioInstance { task, .. },
                    to,
                    duration_us,
                    easing,
                    ..
                } => {
                    let target_id = t.target_task.ok_or_else(|| fail("missing stop target"))?;
                    let audio = s
                        .tasks
                        .get(&target_id)
                        .ok_or_else(|| fail("missing audio instance"))?;
                    if target_id >= t.id
                        || audio.name != *task
                        || !matches!(audio.effect, Effect::Audio { .. })
                        || !matches!(easing, Easing::Linear)
                        || !to.is_finite()
                        || !(0.0..=1.0).contains(to)
                        || !t.captured.is_finite()
                        || !(0.0..=1.0).contains(&t.captured)
                        || !t.base.is_finite()
                        || !(0.0..=1.0).contains(&t.base)
                        || (t.state == TaskState::Running
                            && (audio.state != TaskState::Running
                                || t.elapsed_us.0 >= duration_us.0
                                || !envelope_owners.insert(target_id)))
                    {
                        return Err(fail("invalid tween envelope ownership or progress"));
                    }
                }
                _ if t.target_task.is_some() => return Err(fail("unexpected target task")),
                _ => {}
            }
            if (t.state == TaskState::Running) != t.end_reason.is_none()
                || t.end_reason.is_some_and(|reason| {
                    reason.state() != t.state
                        || (reason == TaskEndReason::NaturalEnd
                            && !matches!(t.effect, Effect::Audio { looped: false, .. }))
                })
            {
                return Err(fail("task terminal reason mismatch"));
            }
            if !canonical_verified {
                validate_task_definition(t, p)?;
            }
            if t.state == TaskState::Running
                && t.scope == Scope::Frame
                && !s.frames.iter().any(|f| f.id == t.frame)
            {
                return Err(fail("orphan frame task"));
            }
            if let Some(elapsed) = t.audio_device_elapsed_us {
                if !envelope_owner_duration(t).is_some_and(|duration| elapsed.0 <= duration.0) {
                    return Err(fail("invalid device envelope checkpoint"));
                }
            }
            if t.audio_position_us.is_some() && !matches!(t.effect, Effect::Audio { .. }) {
                return Err(fail("device position on non-audio task"));
            }
            if t.state == TaskState::Running && t.elapsed_us.0 != s.tick_us.0 - t.started_us.0 {
                return Err(fail("task progress mismatch"));
            }
            if (t.state == TaskState::Finished) != t.milestones.contains(&Milestone::Finished) {
                return Err(fail("task terminal milestone mismatch"));
            }
            if !canonical_verified {
                if let Some(d) = &t.dialogue {
                    validate_dialogue(d, p, s.tick_us, s.next_id)?;
                }
            }
        }
        if s.tasks.values().any(|t| t.state == TaskState::Running && t.scene_generation == s.scene_generation
            && t.effect.scalar_track(0.,0.).is_some_and(|(a,_,_)|
                matches!(a, TweenTarget::SceneNode { node, .. } if timeline_owners.contains(node.as_str())))) {
            return Err(fail("timeline conflicts with scalar pose writer"));
        }
        for nodes in std::iter::once(&s.scene)
            .chain(std::iter::once(&s.draft))
            .chain(s.tasks.values().flat_map(|t| [&t.source, &t.target]))
        {
            crate::validate::validate_timeline_bindings(nodes, "snapshot", |id| {
                p.sprite_timelines.get(id)
            })?;
            if nodes.iter().any(|n| n.timeline_binding.is_some())
                && !p.requires.iter().any(|c| c == "stage.sprite-timeline.v1")
            {
                return Err(fail("missing timeline capability"));
            }
            if nodes.len() > MAX_NODES {
                return Err(fail("scene size"));
            }
            let ids: BTreeSet<_> = nodes.iter().map(|n| &n.id).collect();
            if ids.len() != nodes.len() {
                return Err(fail("duplicate scene node"));
            }
            for n in nodes {
                if n.inherit_existence
                    && (!p
                        .requires
                        .iter()
                        .any(|cap| cap == "stage.sprite-lifecycle.v1")
                        || n.parent.is_some()
                        || n.timeline_binding.is_none())
                {
                    return Err(fail("invalid inherited animated existence in snapshot"));
                }
                if let Some(transform) = n.sprite_transform {
                    if !p
                        .requires
                        .iter()
                        .any(|cap| cap == "stage.sprite-transform.v1")
                        || !transform.valid(n.width, n.height)
                        || n.clip.is_some()
                        || n.bitmap_text.is_some()
                        || nodes
                            .iter()
                            .any(|child| child.parent.as_deref() == Some(n.id.as_str()))
                    {
                        return Err(fail("invalid leaf sprite transform in snapshot"));
                    }
                }
                if n.offset != [0.; 2]
                    && (!p.requires.iter().any(|cap| cap == "stage.sprite-wave.v1")
                        || n.offset.iter().any(|v| !v.is_finite() || v.abs() > 8192.))
                {
                    return Err(fail("invalid sprite offset in snapshot"));
                }
                if !n.preserve_pose.is_empty()
                    && (!p
                        .requires
                        .iter()
                        .any(|cap| cap == "stage.sprite-continuity.v1")
                        || n.preserve_pose.iter().collect::<BTreeSet<_>>().len()
                            != n.preserve_pose.len())
                {
                    return Err(fail("invalid preserved pose properties in snapshot"));
                }
                if n.bitmap_text.is_some() {
                    return Err(fail("unmaterialized bitmap recipe in snapshot"));
                }
                if ![n.x, n.y, n.width, n.height, n.scale, n.opacity]
                    .iter()
                    .chain(n.color.iter())
                    .all(|v| v.is_finite())
                    || n.width < 0.
                    || n.height < 0.
                    || n.scale < 0.
                    || !(0.0..=1.0).contains(&n.opacity)
                    || n.clip
                        .is_some_and(|r| r.iter().any(|v| !v.is_finite()) || r[2] < 0. || r[3] < 0.)
                {
                    return Err(fail("invalid scene values"));
                }
                if n.asset
                    .as_ref()
                    .is_some_and(|a| p.asset_kind(a) != Some(AssetKind::Image))
                {
                    return Err(fail("scene asset missing"));
                }
                let mut seen = BTreeSet::from([&n.id]);
                let mut parent = n.parent.as_ref();
                while let Some(id) = parent {
                    if !seen.insert(id) {
                        return Err(fail("scene cycle"));
                    }
                    parent = nodes
                        .iter()
                        .find(|n| &n.id == id)
                        .ok_or_else(|| fail("scene parent missing"))?
                        .parent
                        .as_ref();
                }
            }
        }
        // Older snapshots froze display names only. Recover identity solely
        // from validated authored references and unambiguous live bindings;
        // never match a translated name or load past chapter bodies.
        let mut s = s;
        let mut voices: BTreeMap<u32, BTreeSet<String>> = BTreeMap::new();
        let mut speakers = BTreeMap::new();
        for task in s.tasks.values_mut() {
            if let (Effect::Dialogue { speaker, .. }, Some(d)) = (&task.effect, &mut task.dialogue)
            {
                if d.speaker_id.is_empty() && speaker.len() <= MAX_CHARACTER_ID_BYTES {
                    d.speaker_id = speaker.clone();
                }
                speakers.insert(d.interaction, d.speaker_id.clone());
                if let Some(reading) = &d.reading {
                    for voice in reading.active_voices.iter().copied().chain(reading.voice) {
                        voices
                            .entry(voice)
                            .or_default()
                            .insert(d.speaker_id.clone());
                    }
                }
            }
        }
        for (voice, roles) in voices {
            if roles.len() == 1 {
                if let Some(task) = s.tasks.get_mut(&voice) {
                    if task.voice_character.is_empty() {
                        task.voice_character = roles.into_iter().next().unwrap();
                    }
                }
            }
        }
        for entry in &mut s.history {
            if entry.speaker_id.is_empty() && entry.choice.is_none() {
                if let Some(id) = speakers.get(&entry.interaction) {
                    entry.speaker_id.clone_from(id);
                }
            }
        }
        while s.history.iter().map(history_bytes).sum::<usize>() > 4 * 1024 * 1024 {
            s.history.remove(0);
        }
        let mut core = Self {
            program,
            state: s,
            intents: vec![],
            work_remaining: 0,
            remaining_time_us: 0,
            text_speed: 1.,
            profile_facts: BTreeSet::new(),
            profile_values: BTreeMap::new(),
        };
        core.state.last_input = 0;
        let modals: Vec<_> = core
            .state
            .tasks
            .values()
            .filter(|t| {
                t.state == TaskState::Running && matches!(t.effect, Effect::StoryModal { .. })
            })
            .map(|t| t.id)
            .collect();
        for id in modals {
            let token = core.id()?;
            core.state.tasks.get_mut(&id).unwrap().modal_interaction = Some(token);
        }
        // Restored interactions receive fresh identities, in addition to the host epoch change.
        let ids: Vec<_> = core
            .state
            .tasks
            .values()
            .filter(|t| t.state == TaskState::Running && t.dialogue.is_some())
            .map(|t| t.id)
            .collect();
        for id in ids {
            let token = core.id()?;
            let dialogue = core
                .state
                .tasks
                .get_mut(&id)
                .unwrap()
                .dialogue
                .as_mut()
                .unwrap();
            let old_interaction = dialogue.interaction;
            dialogue.interaction = token;
            for entry in &mut core.state.history {
                if entry.interaction == old_interaction {
                    entry.interaction = token;
                }
            }
        }
        if core.state.choice.is_some() {
            let token = core.id()?;
            core.state.choice.as_mut().unwrap().interaction = token;
        }
        if core
            .state
            .waiting
            .as_ref()
            .is_some_and(|w| w.advance.is_some())
        {
            let token = core.id()?;
            core.state
                .waiting
                .as_mut()
                .unwrap()
                .advance
                .as_mut()
                .unwrap()
                .interaction = token;
        }
        Ok(core)
    }
}

fn valid_reveal_interval(authored: Micros, frozen: Micros) -> bool {
    let min = (authored.0 as f64 / 4.).round() as u64;
    let max = (authored.0 as f64 / 0.25).round() as u64;
    (min..=max).contains(&frozen.0)
}

pub(crate) fn validate_task_definition(task: &Task, program: &RuntimeProgramView) -> Result<()> {
    if !program
        .cues
        .values()
        .flat_map(|cue| &cue.effects)
        .any(|definition| def_matches_task(definition, task))
    {
        return Err(Diagnostic::new(
            "E_SNAPSHOT",
            "restore",
            "task does not match a declared effect",
        ));
    }
    Ok(())
}

/// A task matches its declaration anywhere in the cue's effect tree,
/// composition children included.
fn def_matches_task(def: &EffectDef, task: &Task) -> bool {
    (def.id == task.name
        && def.scope == task.scope
        && serde_json::to_value(&def.effect).unwrap()
            == serde_json::to_value(&task.effect).unwrap())
        || def
            .effect
            .compose_children()
            .is_some_and(|children| children.iter().any(|child| def_matches_task(child, task)))
}

pub(crate) fn validate_dialogue(
    d: &Dialogue,
    p: &RuntimeProgramView,
    tick: Micros,
    next_id: u32,
) -> Result<()> {
    let fail = |msg: &str| Diagnostic::new("E_SNAPSHOT", "dialogue", msg);
    let contract = p
        .texts
        .get(&d.text_id)
        .ok_or_else(|| fail("missing text"))?;
    let doc = p
        .locales
        .get(&d.locale)
        .and_then(|l| l.get(&d.text_id))
        .ok_or_else(|| fail("missing locale"))?;
    if d.meaning_revision != contract.meaning_revision
        || d.source_revision != contract.source_revision
        || d.contract_digest != contract.contract_digest
        || d.interaction >= next_id
        || d.spans.len() != doc.spans.len()
        || d.span > d.spans.len()
        || d.last_reveal_us.0 > tick.0
    {
        return Err(fail("dialogue identity/progress"));
    }
    for (span, source) in d.spans.iter().zip(&doc.spans) {
        let valid = match source {
            Span::Image { id, image } => {
                &span.id == id
                    && span.text == "\u{fffc}"
                    && span.image.as_ref() == Some(image)
                    && !span.gate
                    && !span.emphasis
            }
            Span::Text { id, text, emphasis } => {
                &span.id == id && &span.text == text && span.emphasis == *emphasis && !span.gate
            }
            Span::Ruby { id, text, reading } => {
                &span.id == id
                    && &span.text == text
                    && span.ruby.as_ref() == Some(reading)
                    && !span.gate
                    && !span.emphasis
            }
            Span::Break { id } => &span.id == id && span.text == "\n" && !span.gate,
            Span::Gate { id } => &span.id == id && span.text.is_empty() && span.gate,
            Span::Pause { id, timeout_us } => {
                &span.id == id
                    && span.text.is_empty()
                    && span.pause
                    && !span.gate
                    && span.pause_timeout_us == *timeout_us
            }
            Span::Param { id, .. } => &span.id == id && !span.gate && span.text.len() <= 128 * 1024,
        };
        if !valid
            || (!matches!(source, Span::Pause { .. })
                && (span.pause || span.pause_timeout_us.is_some()))
            || (!matches!(source, Span::Ruby { .. }) && span.ruby.is_some())
            || (!matches!(source, Span::Image { .. }) && span.image.is_some())
        {
            return Err(fail("frozen text contract mismatch"));
        }
    }
    if d.span < d.spans.len() && d.cluster > d.spans[d.span].text.graphemes(true).count()
        || d.at_gate && !d.spans.get(d.span).is_some_and(|s| s.gate)
        || d.awaiting_advance
            && d.span != d.spans.len()
            && !d.spans.get(d.span).is_some_and(|s| s.pause)
    {
        return Err(fail("invalid reveal cursor"));
    }
    Ok(())
}
