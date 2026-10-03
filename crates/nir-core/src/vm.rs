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
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DialogueReading {
    pub voice: Option<u32>,
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
pub struct HistoryEntry {
    pub text_id: String,
    pub meaning_revision: u32,
    pub source_revision: u32,
    pub contract_digest: String,
    pub locale: String,
    pub font_plan_digest: String,
    pub speaker: String,
    pub text: String,
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
    /// Deferred visibility flip: while set, `dialogue_hidden` still holds the
    /// pre-op value and the window's committed state lands at the deadline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_reveal: Option<WindowReveal>,
    #[serde(default)]
    pub dialogue_appearance: DialogueAppearance,
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
    None,
    Advance {
        interaction: u32,
        sequence: u32,
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
        position_us: Micros,
        gain: f32,
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
            window_reveal: None,
            dialogue_appearance: DialogueAppearance::default(),
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
    pub fn state(&self) -> &Snapshot {
        &self.state
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
                for key in [
                    ContentKey::Static {
                        module: module.to_owned(),
                    },
                    ContentKey::Code {
                        module: module.to_owned(),
                    },
                ] {
                    if self.program.contains_content_body(&key) && !program.is_resident(&key) {
                        return Err(missing(&format!("active frame body missing: {key:?}")));
                    }
                }
                if program.program().functions.get(&frame.function).is_none() {
                    return Err(missing("active function body missing"));
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
                    Span::Text { id, text, emphasis } => FrozenSpan {
                        id: id.clone(),
                        text: text.clone(),
                        emphasis: *emphasis,
                        gate: false,
                    },
                    Span::Break { id } => FrozenSpan {
                        id: id.clone(),
                        text: "\n".into(),
                        emphasis: false,
                        gate: false,
                    },
                    Span::Gate { id } => FrozenSpan {
                        id: id.clone(),
                        text: String::new(),
                        emphasis: false,
                        gate: true,
                    },
                    Span::Param { id, name } => {
                        let v = match self.read(name)? {
                            Value::String(v) => v,
                            Value::I32(v) => v.to_string(),
                            Value::Bool(v) => v.to_string(),
                        };
                        FrozenSpan {
                            id: id.clone(),
                            text: format!("\u{2068}{v}\u{2069}"),
                            emphasis: false,
                            gate: false,
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
            } => {
                if sequence <= self.state.last_input {
                    return Ok(());
                }
                let task = self
                    .state
                    .tasks
                    .values()
                    .find(|t| {
                        t.state == TaskState::Running
                            && t.dialogue
                                .as_ref()
                                .is_some_and(|d| d.interaction == interaction)
                    })
                    .map(|t| t.id);
                if let Some(id) = task {
                    self.state.last_input = sequence;
                    let d = self.state.tasks[&id].dialogue.as_ref().unwrap();
                    if d.awaiting_advance {
                        self.finish_task(id, TaskState::Finished)?;
                    } else if !d.at_gate {
                        self.reveal(id, true)?;
                    }
                }
            }
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
                        if let Some((target, value)) = typed {
                            self.write(&target, value)?;
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
                    self.state.last_input = sequence;
                    self.trace("input:cancel");
                    self.state.choice = None;
                    self.jump(dest);
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
            if !self.program.is_resident(&ContentKey::Static {
                module: module.into(),
            }) {
                return Ok(Some(module.into()));
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
            Operation::ProfileMerge { key } => self
                .intents
                .push(CoreIntent::ProfileMerge { key: key.clone() }),
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
                    wait: *wait,
                    revision,
                });
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
                self.state.waiting = Some(Waiting {
                    conditions,
                    next,
                    on_cancelled,
                    on_failed,
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
        let ids:Vec<_>=self.state.tasks.values().filter(|t|t.state==TaskState::Running&&matches!(t.effect,Effect::StagePresent{duration_us,..}|Effect::Clip{duration_us,..}|Effect::Tween{duration_us,..}|Effect::Delay{duration_us}|Effect::AudioStop{duration_us,..} if duration_us.0==0)).map(|t|t.id).collect();
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
        match &def.effect {
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
            Effect::StagePresent { scene, .. } => {
                if self.state.tasks.values().any(|t| {
                    t.state == TaskState::Running && matches!(t.effect, Effect::StagePresent { .. })
                }) {
                    return Err(self.error("E_OWNERSHIP", "stage transition owns root"));
                }
                source = self.sample_scene();
                target = if !self.state.draft.is_empty() {
                    std::mem::take(&mut self.state.draft)
                } else {
                    self.program().scenes[scene].clone()
                };
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
                self.state.scene = target.clone();
                self.state.scene_generation = self
                    .state
                    .scene_generation
                    .checked_add(1)
                    .ok_or_else(|| self.error("E_LIMIT", "scene generations"))?;
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
            self.state.history.push(HistoryEntry {
                text_id: d.text_id.clone(),
                meaning_revision: d.meaning_revision,
                source_revision: d.source_revision,
                contract_digest: d.contract_digest.clone(),
                locale: d.locale.clone(),
                font_plan_digest: d.font_plan_digest.clone(),
                speaker: d.speaker.clone(),
                text: d.full_text(),
            });
            while self.state.history.len() > 1000
                || self
                    .state
                    .history
                    .iter()
                    .map(|h| h.text.len() + h.speaker.len())
                    .sum::<usize>()
                    > 4 * 1024 * 1024
            {
                self.state.history.remove(0);
            }
        }
        if let Effect::Audio {
            asset,
            bus,
            looped,
            gain,
        } = &def.effect
        {
            self.intents.push(CoreIntent::AudioStart {
                task: id,
                asset: asset.clone(),
                bus: *bus,
                looped: *looped,
                gain: *gain,
                position_us: Micros(0),
            });
        }
        let task = Task {
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
        }
        if status == TaskState::Finished {
            if let Some(d) = &t.dialogue {
                self.intents.push(CoreIntent::ProfileMerge {
                    key: format!("read:{}:{}", d.text_id, d.meaning_revision),
                });
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
                let due = match &t.effect {
                    Effect::Clip { duration_us, .. }
                    | Effect::Tween { duration_us, .. }
                    | Effect::Delay { duration_us }
                    | Effect::AudioStop { duration_us, .. }
                    | Effect::StagePresent { duration_us, .. } => {
                        Some(t.started_us.0.saturating_add(duration_us.0))
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
            for t in self
                .state
                .tasks
                .values_mut()
                .filter(|t| t.state == TaskState::Running)
            {
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
                match t.effect {
                    Effect::Clip { duration_us, .. }
                    | Effect::Tween { duration_us, .. }
                    | Effect::Delay { duration_us }
                    | Effect::AudioStop { duration_us, .. }
                    | Effect::StagePresent { duration_us, .. }
                        if t.elapsed_us.0 >= duration_us.0 =>
                    {
                        self.finish_task(id, TaskState::Finished)?
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
                    if let Some(option) = c.default {
                        // A timeout commits the default option, typed value
                        // included, exactly like an explicit choice.
                        if let (Some(target), Some(value)) = (&c.result, c.values.get(&option)) {
                            let target = target.clone();
                            let value = value.clone();
                            self.write(&target, value)?;
                        }
                        self.state.choice = None;
                        self.jump(c.branches[&option].clone());
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
    pub fn sample_scene(&self) -> Vec<Node> {
        let mut nodes = self.state.scene.clone();
        for task in self
            .state
            .tasks
            .values()
            .filter(|t| t.state == TaskState::Running)
        {
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
            if let Some((TweenTarget::DialogueRoot { property }, track, _)) =
                task.effect.scalar_track(task.captured, task.base)
            {
                appearance.set(property, track.sample(task.elapsed_us));
            }
        }
        appearance
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
    pub fn transition(&self) -> Option<(&[Node], f32)> {
        self.state.tasks.values().find_map(|t| {
            if t.state == TaskState::Running {
                if let Effect::StagePresent { duration_us, .. } = t.effect {
                    if duration_us.0 > 0 {
                        return Some((
                            t.source.as_slice(),
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
            || self.state.tasks.values().any(|t| {
                t.state == TaskState::Running
                    && match t.effect {
                        Effect::Dialogue { .. } => t
                            .dialogue
                            .as_ref()
                            .is_some_and(|d| !d.at_gate && !d.awaiting_advance),
                        // Audio advances on the device even when text is fully
                        // revealed. Keep the Story clock alive for save offsets.
                        Effect::Audio { .. } => true,
                        _ => true,
                    }
            })
    }
    pub fn restore(program: ValidatedProgram, s: Snapshot, release: &str) -> Result<Self> {
        Self::restore_inner(program, s, release, false)
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
            || s.next_id == 0
        {
            return Err(fail("state limits/locale"));
        }
        if s.variables.len() != p.variables.len()
            || p.variables
                .iter()
                .any(|(k, v)| s.variables.get(k).map(Value::ty) != Some(v.ty()))
        {
            return Err(fail("variable layout"));
        }
        for h in &s.history {
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
                || top.op != block.ops.len()
            {
                return Err(fail("wait continuation mismatch"));
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
        for t in s.tasks.values() {
            if let Some(reading) = t.dialogue.as_ref().and_then(|d| d.reading.as_ref()) {
                if !p.requires.iter().any(|c| c == "text.voice-binding.v1")
                    || (reading.wait == VoiceWaitPolicy::SampledRemaining
                        && (!p.requires.iter().any(|c|c=="text.voice-timer.v1")
                            || reading.voice.is_some_and(|id|s.tasks.get(&id).is_some_and(|t|
                                matches!(&t.effect,Effect::Audio{asset,..} if p.asset(asset).is_none_or(|a|a.duration_us.0==0))
                            ))))
                    || reading.revision == 0
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
        for nodes in std::iter::once(&s.scene)
            .chain(std::iter::once(&s.draft))
            .chain(s.tasks.values().flat_map(|t| [&t.source, &t.target]))
        {
            if nodes.len() > MAX_NODES {
                return Err(fail("scene size"));
            }
            let ids: BTreeSet<_> = nodes.iter().map(|n| &n.id).collect();
            if ids.len() != nodes.len() {
                return Err(fail("duplicate scene node"));
            }
            for n in nodes {
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
        let mut core = Self {
            program,
            state: s,
            intents: vec![],
            work_remaining: 0,
            remaining_time_us: 0,
            text_speed: 1.,
        };
        core.state.last_input = 0;
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
            core.state
                .tasks
                .get_mut(&id)
                .unwrap()
                .dialogue
                .as_mut()
                .unwrap()
                .interaction = token;
        }
        if core.state.choice.is_some() {
            let token = core.id()?;
            core.state.choice.as_mut().unwrap().interaction = token;
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
            Span::Text { id, text, emphasis } => {
                &span.id == id && &span.text == text && span.emphasis == *emphasis && !span.gate
            }
            Span::Break { id } => &span.id == id && span.text == "\n" && !span.gate,
            Span::Gate { id } => &span.id == id && span.text.is_empty() && span.gate,
            Span::Param { id, .. } => &span.id == id && !span.gate && span.text.len() <= 128 * 1024,
        };
        if !valid {
            return Err(fail("frozen text contract mismatch"));
        }
    }
    if d.span < d.spans.len() && d.cluster > d.spans[d.span].text.graphemes(true).count()
        || d.at_gate && !d.spans.get(d.span).is_some_and(|s| s.gate)
        || d.awaiting_advance && d.span != d.spans.len()
    {
        return Err(fail("invalid reveal cursor"));
    }
    Ok(())
}
