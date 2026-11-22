use super::*;

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
pub(super) fn unit_envelope() -> f32 {
    1.0
}
impl TaskEndReason {
    pub(super) fn state(self) -> TaskState {
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
pub(super) fn is_zero(value: &u32) -> bool {
    *value == 0
}
/// The ramp duration of an envelope owner task (a timed stop or a gain
/// tween), or `None` when the task does not own an audio envelope.
pub(super) fn envelope_owner_duration(task: &Task) -> Option<Micros> {
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
    pub(super) fn display_option(&self) -> Option<&HistoryChoiceOption> {
        match self.resolution.selected() {
            Some(selected) => self.options.iter().find(|o| o.id == selected && o.enabled),
            None => self.options.first(),
        }
    }
    pub(super) fn display_text(&self) -> String {
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
pub(super) fn history_bytes(entry: &HistoryEntry) -> usize {
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
