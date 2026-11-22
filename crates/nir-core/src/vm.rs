use crate::validate::RuntimeProgramView;
use crate::ValidatedProgram;
use nir_format::*;
use rand_chacha::ChaCha8Rng;
use rand_core::{RngCore, SeedableRng};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use unicode_segmentation::UnicodeSegmentation;

mod eval;
mod exec;
mod restore;
mod sample;
mod state;
mod tasks;

pub(crate) use restore::{validate_dialogue, validate_task_definition};
pub use state::*;
use state::{envelope_owner_duration, history_bytes};

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
    tracing: bool,
    history_bytes: usize,
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
            tracing: true,
            history_bytes: 0,
        })
    }
    /// Control emission of per-op `CoreIntent::Trace` records. Enabled by
    /// default for direct `Core` callers and tests; production players turn it
    /// off unless test/diagnostic tracing is requested.
    pub fn set_tracing(&mut self, enabled: bool) {
        self.tracing = enabled;
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
        if self.profile_facts.len() == facts.len() && self.profile_facts == *facts {
            return Ok(());
        }
        for k in facts {
            if k.is_empty() || k.len() > 1024 {
                return Err(self.error("E_PROFILE", "profile fact limit"));
            }
        }
        for k in facts {
            if !self.profile_facts.contains(k) {
                self.profile_facts.insert(k.clone());
            }
        }
        Ok(())
    }
    pub fn state(&self) -> &Snapshot {
        &self.state
    }
    /// Profile data is supplied by the owner, separately from a Story save.
    pub fn set_profile_values(&mut self, values: &BTreeMap<String, Value>) -> Result<()> {
        if self.profile_values == *values {
            return Ok(());
        }
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
        if !self.tracing {
            return;
        }
        self.intents.push(CoreIntent::Trace {
            event: event.into(),
            at: self.location(),
        });
    }
}
