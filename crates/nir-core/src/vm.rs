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
pub struct Dialogue {
    pub text_id: String,
    pub revision: u32,
    pub locale: String,
    pub speaker: String,
    pub spans: Vec<FrozenSpan>,
    pub span: usize,
    pub cluster: usize,
    pub at_gate: bool,
    pub awaiting_advance: bool,
    pub last_reveal_us: Micros,
    pub interaction: u32,
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
    pub started_us: Micros,
    pub elapsed_us: Micros,
    pub milestones: BTreeSet<Milestone>,
    pub captured: f32,
    pub base: f32,
    pub dialogue: Option<Dialogue>,
    pub source: Vec<Node>,
    pub target: Vec<Node>,
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
    pub interaction: u32,
    pub options: Vec<OfferedOption>,
    pub branches: BTreeMap<String, String>,
    pub deadline_us: Option<Micros>,
    pub default: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Waiting {
    pub conditions: Vec<(u32, Milestone)>,
    pub next: String,
    pub on_cancelled: String,
    pub on_failed: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryEntry {
    pub text_id: String,
    pub revision: u32,
    pub locale: String,
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
}
impl Core {
    pub fn new(program: ValidatedProgram, release: String, locale: String) -> Result<Self> {
        let p = program.program();
        if !p.locales.contains_key(&locale) {
            return Err(Diagnostic::new("E_LOCALE", "new", locale));
        }
        let frame = Frame {
            id: 1,
            function: p.entry.clone(),
            block: p.functions[&p.entry].entry.clone(),
            op: 0,
            op_id: p.functions[&p.entry].blocks[&p.functions[&p.entry].entry]
                .ops
                .first()
                .map(|o| o.id.clone())
                .unwrap_or_else(|| "@terminator".into()),
            locals: BTreeMap::new(),
            return_to: None,
            result: None,
        };
        let state = Snapshot {
            format: 1,
            game_id: p.game_id.clone(),
            revision: p.revision.clone(),
            release,
            tick_us: Micros(0),
            variables: p.variables.clone(),
            frames: vec![frame],
            scene: vec![],
            draft: vec![],
            scene_generation: 0,
            tasks: BTreeMap::new(),
            handles: BTreeMap::new(),
            pending: None,
            waiting: None,
            choice: None,
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
        })
    }
    pub fn state(&self) -> &Snapshot {
        &self.state
    }
    pub fn program(&self) -> &Program {
        self.program.program()
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
    pub fn set_locale(&mut self, locale: &str) -> Result<()> {
        if !self.program().locales.contains_key(locale) {
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
        let changed = !matches!(input, CoreInput::None);
        if self.state.fault.is_none() && self.state.outcome.is_none() {
            if let Err(e) = self.process(input).and_then(|_| self.run(semantic_budget)) {
                self.state.fault = Some(e);
            }
        }
        CoreStep {
            intents: std::mem::take(&mut self.intents),
            waiting: self.state.pending.is_some()
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
                        let dest = c.branches[&option].clone();
                        self.trace(format!("choose:{option}"));
                        self.state.choice = None;
                        self.jump(dest);
                        self.state.unsuspended_ops = 0;
                        self.intents.push(CoreIntent::Checkpoint);
                    }
                }
            }
            CoreInput::AudioEnded { task } => {
                if self.state.tasks.get(&task).is_some_and(|t| {
                    matches!(t.effect, Effect::Audio { looped: false, .. })
                        && t.state == TaskState::Running
                }) {
                    self.finish_task(task, TaskState::Finished)?;
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
    fn run(&mut self, budget: u32) -> Result<()> {
        for _ in 0..budget.min(100_000) {
            if self.state.pending.is_some()
                || self.state.choice.is_some()
                || self.state.outcome.is_some()
            {
                return Ok(());
            }
            if self.state.waiting.is_some() && !self.resolve_wait()? {
                return Ok(());
            }
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
            Operation::ProfileMerge { key } => self
                .intents
                .push(CoreIntent::ProfileMerge { key: key.clone() }),
            Operation::TaskControl { task, action } => {
                let id = *self
                    .state
                    .handles
                    .get(task)
                    .ok_or_else(|| self.error("E_TASK", task))?;
                self.finish_task(
                    id,
                    match action {
                        TaskAction::Cancel => TaskState::Cancelled,
                        TaskAction::Finish => TaskState::Finished,
                    },
                )?;
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
                    self.finish_task(id, TaskState::Cancelled)?;
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
                    if let Effect::Dialogue { text, speaker, .. } = &def.effect {
                        let d = Dialogue {
                            text_id: text.clone(),
                            revision: self.program().texts[text].revision,
                            locale: self.state.locale.clone(),
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
                self.state.choice = Some(OfferedChoice {
                    id: choice,
                    locale: self.state.locale.clone(),
                    interaction,
                    options,
                    branches,
                    deadline_us,
                    default: c.default,
                });
                self.state.unsuspended_ops = 0;
                self.intents.push(CoreIntent::Checkpoint);
            }
            Terminator::End { outcome } => {
                let ids: Vec<_> = self.state.tasks.keys().copied().collect();
                for id in ids {
                    self.finish_task(id, TaskState::Cancelled)?;
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
        if self
            .state
            .tasks
            .values()
            .filter(|t| t.state == TaskState::Running)
            .count()
            + p.effects.len()
            > MAX_TASKS
        {
            return Err(self.error("E_LIMIT", "active tasks"));
        }
        for def in p.effects {
            let mut source = vec![];
            let mut target = vec![];
            let mut captured = 0.;
            let mut base = 0.;
            match &def.effect {
                Effect::StagePresent { scene, .. } => {
                    if self.state.tasks.values().any(|t| {
                        t.state == TaskState::Running
                            && matches!(t.effect, Effect::StagePresent { .. })
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
                        self.finish_task(id, TaskState::Cancelled)?;
                    }
                    self.state.scene = target.clone();
                    self.state.scene_generation = self
                        .state
                        .scene_generation
                        .checked_add(1)
                        .ok_or_else(|| self.error("E_LIMIT", "scene generations"))?;
                }
                Effect::Clip {
                    node,
                    property,
                    replace,
                    ..
                } => {
                    if self.state.tasks.values().any(|t|t.state==TaskState::Running&&matches!(t.effect,Effect::StagePresent{duration_us,..} if duration_us.0>0)){return Err(self.error("E_OWNERSHIP","transition owns root"));}
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
                    let old:Vec<_>=self.state.tasks.values().filter(|t|t.state==TaskState::Running&&t.scene_generation==self.state.scene_generation&&matches!(&t.effect,Effect::Clip{node:n,property:p,..} if n==node&&p==property)).map(|t|t.id).collect();
                    if !old.is_empty() && !*replace {
                        return Err(self.error("E_OWNERSHIP", node));
                    }
                    for id in old {
                        self.finish_task(id, TaskState::Cancelled)?;
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
                        self.finish_task(id, TaskState::Cancelled)?;
                    }
                }
                _ => {}
            }
            let id = self.id()?;
            let dialogue = p.dialogues.get(&def.id).cloned();
            if let Some(d) = &dialogue {
                self.state.history.push(HistoryEntry {
                    text_id: d.text_id.clone(),
                    revision: d.revision,
                    locale: d.locale.clone(),
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
            if let Effect::Audio { asset, bus, looped } = &def.effect {
                self.intents.push(CoreIntent::AudioStart {
                    task: id,
                    asset: asset.clone(),
                    bus: *bus,
                    looped: *looped,
                    position_us: Micros(0),
                });
            }
            let task = Task {
                id,
                name: def.id.clone(),
                frame: self.frame().id,
                scene_generation: self.state.scene_generation,
                scope: def.scope,
                effect: def.effect,
                state: TaskState::Running,
                started_us: self.state.tick_us,
                elapsed_us: Micros(0),
                milestones: BTreeSet::from([Milestone::Started]),
                captured,
                base,
                dialogue,
                source,
                target,
            };
            self.state.handles.insert(def.id, id);
            self.state.tasks.insert(id, task);
        }
        self.jump(p.next);
        self.trace(format!("activate:{}", p.cue));
        self.intents.push(CoreIntent::Checkpoint);
        let ids:Vec<_>=self.state.tasks.values().filter(|t|t.state==TaskState::Running&&matches!(t.effect,Effect::StagePresent{duration_us,..}|Effect::Clip{duration_us,..}|Effect::Delay{duration_us} if duration_us.0==0)).map(|t|t.id).collect();
        for id in ids {
            self.finish_task(id, TaskState::Finished)?;
        }
        let keep: BTreeSet<_> = self
            .state
            .handles
            .values()
            .copied()
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
        let Some(t) = self.state.tasks.get(&id).cloned() else {
            return Err(self.error("E_TASK", id.to_string()));
        };
        if t.state != TaskState::Running {
            return Ok(());
        }
        if let Effect::Clip {
            node,
            property,
            to,
            duration_us,
            easing,
            finish,
            cancel,
            ..
        } = &t.effect
        {
            let progress = if duration_us.0 == 0 {
                1.
            } else {
                (t.elapsed_us.0 as f64 / duration_us.0 as f64).min(1.) as f32
            };
            let progress = ease(progress, *easing);
            let v = match status {
                TaskState::Finished => match finish {
                    FinishPolicy::CommitEnd => *to,
                    FinishPolicy::RemoveEffect => t.base,
                },
                _ => match cancel {
                    CancelPolicy::CommitCurrent => t.captured + (*to - t.captured) * progress,
                    CancelPolicy::SettleEnd => *to,
                    CancelPolicy::RestoreBase => t.base,
                },
            };
            if t.scene_generation == self.state.scene_generation {
                if let Some(n) = self.state.scene.iter_mut().find(|n| &n.id == node) {
                    n.set(*property, v);
                }
            }
        }
        if matches!(t.effect, Effect::Audio { .. }) {
            self.intents.push(CoreIntent::AudioStop { task: id });
        }
        if status == TaskState::Finished {
            if let Some(d) = &t.dialogue {
                self.intents.push(CoreIntent::ProfileMerge {
                    key: format!("read:{}:{}", d.text_id, d.revision),
                });
            }
        }
        let task = self.state.tasks.get_mut(&id).unwrap();
        task.state = status;
        if status == TaskState::Finished {
            task.milestones.insert(Milestone::Finished);
        }
        self.trace(format!("task:{id}:{status:?}"));
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
        let mut turns = 0;
        while self.state.tick_us.0 < end
            && self.state.pending.is_none()
            && self.state.outcome.is_none()
        {
            turns += 1;
            if turns > 100_000 {
                return Err(self.error("E_LIMIT", "time event budget"));
            }
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
                    | Effect::Delay { duration_us }
                    | Effect::StagePresent { duration_us, .. } => {
                        Some(t.started_us.0.saturating_add(duration_us.0))
                    }
                    Effect::Dialogue { reveal_us, .. } => t
                        .dialogue
                        .as_ref()
                        .filter(|d| !d.at_gate && !d.awaiting_advance)
                        .map(|d| d.last_reveal_us.0.saturating_add(reveal_us.0.max(1))),
                    _ => None,
                };
                if let Some(due) = due {
                    next = next.min(due.max(now.saturating_add(1)));
                }
            }
            if let Some(deadline) = self.state.choice.as_ref().and_then(|c| c.deadline_us) {
                next = next.min(deadline.0.max(now.saturating_add(1)));
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
                    | Effect::Delay { duration_us }
                    | Effect::StagePresent { duration_us, .. }
                        if t.elapsed_us.0 >= duration_us.0 =>
                    {
                        self.finish_task(id, TaskState::Finished)?
                    }
                    Effect::Dialogue { reveal_us, .. }
                        if t.dialogue.as_ref().is_some_and(|d| {
                            !d.at_gate
                                && !d.awaiting_advance
                                && next - d.last_reveal_us.0 >= reveal_us.0.max(1)
                        }) =>
                    {
                        self.reveal(id, false)?
                    }
                    _ => {}
                }
            }
            if let Some(c) = self.state.choice.clone() {
                if c.deadline_us.is_some_and(|v| v.0 <= next) {
                    if let Some(option) = c.default {
                        self.state.choice = None;
                        self.jump(c.branches[&option].clone());
                        self.trace(format!("timeout:{option}"));
                    }
                }
            }
            self.run(10_000)?;
        }
        Ok(())
    }
    pub fn sample_scene(&self) -> Vec<Node> {
        let mut nodes = self.state.scene.clone();
        for t in self.state.tasks.values().filter(|t| {
            t.state == TaskState::Running && t.scene_generation == self.state.scene_generation
        }) {
            if let Effect::Clip {
                node,
                property,
                to,
                duration_us,
                easing,
                ..
            } = &t.effect
            {
                let p = if duration_us.0 == 0 {
                    1.
                } else {
                    (t.elapsed_us.0 as f64 / duration_us.0 as f64).min(1.) as f32
                };
                if let Some(n) = nodes.iter_mut().find(|n| &n.id == node) {
                    n.set(
                        *property,
                        t.captured + (*to - t.captured) * ease(p, *easing),
                    );
                }
            }
        }
        nodes
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
            || self.state.tasks.values().any(|t| {
                t.state == TaskState::Running
                    && match t.effect {
                        Effect::Dialogue { .. } => t
                            .dialogue
                            .as_ref()
                            .is_some_and(|d| !d.at_gate && !d.awaiting_advance),
                        Effect::Audio { .. } => false,
                        _ => true,
                    }
            })
    }
    pub fn restore(program: ValidatedProgram, s: Snapshot, release: &str) -> Result<Self> {
        let fail = |msg: &str| Diagnostic::new("E_SNAPSHOT", "restore", msg);
        let p = program.program();
        if s.format != 1
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
                || !t.captured.is_finite()
                || !t.base.is_finite()
            {
                return Err(fail("task state"));
            }
            if let Some(d) = &t.dialogue {
                if !p.locales.contains_key(&d.locale)
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
                if !pending.effects.iter().any(|e| {
                    &e.id == name
                        && matches!(&e.effect,Effect::Dialogue{text,..} if text==&d.text_id)
                }) {
                    return Err(fail("pending dialogue mismatch"));
                }
                validate_dialogue(d, p, s.tick_us, s.next_id)?;
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
            if !matches!(&block.terminator,Terminator::Interact{choice,branches,..} if choice==&c.id&&branches==&c.branches)
                || top.op != block.ops.len()
                || c.interaction >= s.next_id
                || !p.locales.contains_key(&c.locale)
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
        for t in s.tasks.values() {
            let definition = p.cues.values().flat_map(|c| &c.effects).any(|d| {
                d.id == t.name
                    && d.scope == t.scope
                    && serde_json::to_value(&d.effect).unwrap()
                        == serde_json::to_value(&t.effect).unwrap()
            });
            if !definition {
                return Err(fail("task does not match a declared effect"));
            }
            if t.state == TaskState::Running
                && t.scope == Scope::Frame
                && !s.frames.iter().any(|f| f.id == t.frame)
            {
                return Err(fail("orphan frame task"));
            }
            if t.state == TaskState::Running && t.elapsed_us.0 != s.tick_us.0 - t.started_us.0 {
                return Err(fail("task progress mismatch"));
            }
            if (t.state == TaskState::Finished) != t.milestones.contains(&Milestone::Finished) {
                return Err(fail("task terminal milestone mismatch"));
            }
            if let Some(d) = &t.dialogue {
                validate_dialogue(d, p, s.tick_us, s.next_id)?;
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
                    .is_some_and(|a| p.assets.get(a).map(|a| a.kind) != Some(AssetKind::Image))
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
fn ease(p: f32, e: Easing) -> f32 {
    match e {
        Easing::Linear => p,
        Easing::Smooth => p * p * (3. - 2. * p),
    }
}

fn validate_dialogue(d: &Dialogue, p: &Program, tick: Micros, next_id: u32) -> Result<()> {
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
    if d.revision != contract.revision
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
