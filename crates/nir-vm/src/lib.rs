//! High-performance NIR v2 flat-bytecode VM with Persistent Scene Graph,
//! unified Tween/Shake/Transition evaluation, hot-reload, and lookahead prefetching.
#![forbid(unsafe_code)]

pub mod state;
pub use state::*;

use nir_ir::*;
use rand_chacha::ChaCha8Rng;
use rand_core::{RngCore, SeedableRng};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::Arc;
use unicode_segmentation::UnicodeSegmentation;

const MAX_HISTORY: usize = 500;
const HISTORY_ROLLBACK_SLACK: usize = 32;
const MAX_CALL_DEPTH: usize = 64;
const MAX_FUEL_PER_STEP: u32 = 200_000;
const RNG_SEED: [u8; 32] = [
    0x4E, 0x49, 0x52, 0x32, 42, 7, 13, 99, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
    17, 18, 19, 20, 21, 22, 23, 24,
];

fn build_label_index(program: &NirProgram) -> Arc<HashMap<u32, StrId>> {
    let mut map = HashMap::with_capacity(program.labels.len());
    for l in &program.labels {
        map.entry(l.pc).or_insert(l.name);
    }
    Arc::new(map)
}

#[derive(Clone)]
pub struct Vm {
    program: Arc<NirProgram>,
    label_by_pc: Arc<HashMap<u32, StrId>>,
    pub pc: u32,
    last_label_id: Option<StrId>,
    last_label_fallback: Option<String>,
    stack: Vec<Value>,
    pub vars: Vec<Value>,
    frames: Vec<CallFrame>,
    /// Committed persistent scene graph currently rendered.
    pub layers: Vec<LayerState>,
    /// Staged scene modifications waiting for `StageCommit` (or auto-committed at `Say`/`Choice`).
    pending_layers: Vec<LayerState>,
    pending_dirty: bool,
    pub tweens: Vec<ActiveTween>,
    pub shakes: Vec<ActiveShake>,
    pub timelines: Vec<ActiveTimeline>,
    pub transition: Option<StageTransitionState>,
    pub window_reveal: Option<WindowRevealState>,
    pub dialogue_visible: bool,
    pub dialogue_opacity: f32,
    pub dialogue_shake: [f32; 2],
    pub dialogue_style: u16,
    pub dialogue_portrait: Option<AssetId>,
    pub dialogue: Option<DialogueState>,
    pub choice: Option<ChoiceState>,
    pub wait: WaitReason,
    pub audio: Vec<AudioChannelState>,
    pub audio_paused: [bool; 3],
    pub history: VecDeque<Arc<HistoryEntry>>,
    history_seq: u64,
    pub locale: String,
    pub profile: ProfileData,
    rng: ChaCha8Rng,
    pub outcome: Option<String>,
    intents: Vec<VmIntent>,
}

impl Vm {
    pub fn new(program: Arc<NirProgram>, locale: Option<String>) -> Result<Self> {
        let entry = program.meta.entry_func;
        Self::new_at(program, entry, locale)
    }

    pub fn new_at(program: Arc<NirProgram>, func: FuncId, locale: Option<String>) -> Result<Self> {
        let fmeta = program
            .func(func)
            .ok_or_else(|| NirError::new("E_VM", "entry", "missing entry function"))?;
        let locale = locale
            .filter(|l| program.texts.contains_key(l) || program.meta.locales.contains(l))
            .unwrap_or_else(|| program.meta.default_locale.clone());
        let vars = program.vars.iter().map(|v| v.default.clone()).collect();
        let entry_pc = fmeta.entry_pc;
        let locals = vec![Value::Null; fmeta.local_count as usize];
        let label_by_pc = build_label_index(&program);
        Ok(Self {
            program,
            label_by_pc,
            pc: entry_pc,
            last_label_id: None,
            last_label_fallback: None,
            stack: Vec::with_capacity(32),
            vars,
            frames: vec![CallFrame {
                func,
                return_pc: u32::MAX,
                locals,
            }],
            layers: Vec::new(),
            pending_layers: Vec::new(),
            pending_dirty: false,
            tweens: Vec::new(),
            shakes: Vec::new(),
            timelines: Vec::new(),
            transition: None,
            window_reveal: None,
            dialogue_visible: true,
            dialogue_opacity: 1.0,
            dialogue_shake: [0.0, 0.0],
            dialogue_style: 0,
            dialogue_portrait: None,
            dialogue: None,
            choice: None,
            wait: WaitReason::None,
            audio: Vec::new(),
            audio_paused: [false; 3],
            history: VecDeque::new(),
            history_seq: 0,
            locale,
            profile: ProfileData::default(),
            rng: ChaCha8Rng::from_seed(RNG_SEED),
            outcome: None,
            intents: Vec::new(),
        })
    }

    fn current_last_label(&self) -> Option<String> {
        self.last_label_id
            .map(|id| self.program.str(id).to_owned())
            .or_else(|| self.last_label_fallback.clone())
    }

    pub fn program(&self) -> &Arc<NirProgram> {
        &self.program
    }

    pub fn set_profile(&mut self, profile: ProfileData) {
        self.profile = profile;
    }

    pub fn set_locale(&mut self, locale: impl Into<String>) {
        self.locale = locale.into();
    }

    pub fn needs_clock(&self) -> bool {
        if self.outcome.is_some() {
            return false;
        }
        let has_running_timeline = self.timelines.iter().any(|tl| {
            let (fps, duration_frames) = self
                .program
                .timelines
                .get(tl.timeline.0 as usize)
                .map_or((60.0, 0), |t| (t.fps.max(1.0), t.duration_frames));
            let total_us = ((duration_frames as f64 / fps as f64) * 1_000_000.0).ceil() as u64;
            tl.elapsed_us < total_us
        });
        !self.tweens.is_empty()
            || !self.shakes.is_empty()
            || has_running_timeline
            || self.transition.is_some()
            || self.window_reveal.is_some()
            || matches!(
                self.wait,
                WaitReason::Timer { .. } | WaitReason::Timeline { .. }
            )
            || self
                .dialogue
                .as_ref()
                .is_some_and(|d| !d.finished && !d.paused_for_click)
            || self.choice.as_ref().is_some_and(|c| c.timeout_us.is_some())
    }

    /// Exact microseconds until the next visual or timer event inside the VM (`None` = true idle sleep!).
    pub fn next_wakeup_delay_us(&self, text_speed: f32) -> Option<u64> {
        if self.outcome.is_some() {
            return None;
        }
        let has_running_timeline = self.timelines.iter().any(|tl| {
            let (fps, duration_frames) = self
                .program
                .timelines
                .get(tl.timeline.0 as usize)
                .map_or((60.0, 0), |t| (t.fps.max(1.0), t.duration_frames));
            let total_us = ((duration_frames as f64 / fps as f64) * 1_000_000.0).ceil() as u64;
            tl.elapsed_us < total_us
        });
        if !self.tweens.is_empty()
            || !self.shakes.is_empty()
            || has_running_timeline
            || self.transition.is_some()
            || self.window_reveal.is_some()
            || matches!(self.wait, WaitReason::Timeline { .. })
        {
            return Some(0);
        }

        let mut soonest: Option<u64> = None;
        let mut consider = |us: u64| {
            soonest = Some(soonest.map_or(us, |cur| cur.min(us)));
        };

        if let Some(dlg) = &self.dialogue {
            if !dlg.finished && !dlg.paused_for_click {
                if dlg.pause_remaining_us > 0 {
                    consider(dlg.pause_remaining_us);
                } else if text_speed <= 0.0 || dlg.reveal_us == 0 {
                    consider(0);
                } else {
                    let step_us = ((dlg.reveal_us as f64) / (text_speed as f64).max(0.1))
                        .max(1000.0) as u64;
                    consider(step_us.saturating_sub(dlg.elapsed_us).max(1_000));
                }
            }
        }

        if let WaitReason::Timer { remaining_us, .. } = self.wait {
            consider(remaining_us);
        }

        if let Some(choice) = &self.choice {
            if let Some(timeout) = choice.timeout_us {
                let rem = timeout.saturating_sub(choice.elapsed_us);
                // Choice countdown bar updates smoothly at ~20Hz without wasting 120Hz wakes.
                consider(rem.min(50_000));
            }
        }

        soonest
    }

    pub fn is_current_dialogue_read(&self) -> bool {
        self.dialogue.as_ref().is_some_and(|d| d.already_read)
    }

    pub fn dialogue_alpha(&self) -> f32 {
        if let Some(rev) = &self.window_reveal {
            rev.alpha() * self.dialogue_opacity
        } else if self.dialogue_visible {
            self.dialogue_opacity
        } else {
            0.0
        }
    }

    /// Jumps to a named label (used by `novelc dev --jump` / hot-reload).
    pub fn jump_to_label(&mut self, label_name: &str) -> bool {
        if let Some(label) = self.program.find_label(label_name) {
            self.pc = label.pc;
            self.last_label_id = Some(label.name);
            self.last_label_fallback = None;
            self.wait = WaitReason::None;
            self.dialogue = None;
            self.choice = None;
            true
        } else {
            false
        }
    }

    /// Live hot-reload: swaps the compiled program while preserving variable values
    /// (matched by name), persistent layers, audio, history, and relocating PC via label!
    pub fn hot_reload(&mut self, new_program: Arc<NirProgram>) {
        // Capture current variables by name.
        let old_vars: BTreeMap<String, Value> = self
            .program
            .vars
            .iter()
            .zip(self.vars.iter())
            .map(|(m, v)| (self.program.str(m.name).to_owned(), v.clone()))
            .collect();

        let next_vars = new_program
            .vars
            .iter()
            .map(|m| {
                old_vars
                    .get(new_program.str(m.name))
                    .cloned()
                    .unwrap_or_else(|| m.default.clone())
            })
            .collect();

        // Relocate PC using last visited label if possible, else clamp to new code length.
        let last_label_str = self.current_last_label();
        let relocated = last_label_str
            .as_deref()
            .and_then(|name| new_program.find_label(name));
        let new_pc = relocated
            .map(|l| l.pc)
            .unwrap_or_else(|| self.pc.min(new_program.ops.len().saturating_sub(1) as u32));
        let next_label_id = relocated.map(|l| l.name);

        self.label_by_pc = build_label_index(&new_program);
        self.program = new_program;
        self.vars = next_vars;
        self.pc = new_pc;
        self.last_label_id = next_label_id;
        self.last_label_fallback = if next_label_id.is_some() {
            None
        } else {
            last_label_str
        };
        self.wait = WaitReason::None;
        self.dialogue = None;
        self.choice = None;
        self.stack.clear();
    }

    /// Captures a lightweight in-memory rollback checkpoint in O(1) without cloning 500 history strings!
    pub fn checkpoint(&self) -> RollbackCheckpoint {
        RollbackCheckpoint {
            pc: self.pc,
            last_label_id: self.last_label_id,
            last_label_fallback: self.last_label_fallback.clone(),
            vars: self.vars.clone(),
            frames: self.frames.clone(),
            layers: self.layers.clone(),
            dialogue_visible: self.dialogue_visible,
            dialogue_style: self.dialogue_style,
            dialogue_portrait: self.dialogue_portrait,
            timelines: self.timelines.clone(),
            dialogue: self.dialogue.clone(),
            choice: self.choice.clone(),
            wait: self.wait.clone(),
            audio: self.audio.clone(),
            audio_paused: self.audio_paused,
            history_seq: self.history_seq,
            locale: self.locale.clone(),
            rng_word_pos: self.rng.get_word_pos(),
            outcome: self.outcome.clone(),
        }
    }

    /// Restores an in-memory rollback checkpoint, truncating history back to `cp.history_seq` in O(1).
    pub fn restore_checkpoint(&mut self, cp: RollbackCheckpoint) -> Vec<VmIntent> {
        let mut intents = Vec::new();
        for ch in &cp.audio {
            intents.push(VmIntent::PlayAudio {
                channel: ch.channel,
                bus: ch.bus,
                asset: ch.asset,
                gain: ch.gain * ch.envelope,
                looped: ch.looped,
                fade_in_us: 0,
                loop_region: ch.loop_region,
                character: ch.character.clone(),
            });
        }
        for (idx, &paused) in cp.audio_paused.iter().enumerate() {
            if paused {
                let bus = match idx {
                    0 => AudioBus::Bgm,
                    1 => AudioBus::Voice,
                    _ => AudioBus::Sfx,
                };
                intents.push(VmIntent::PauseAudioBus { bus, paused: true });
            }
        }

        if self.history_seq > cp.history_seq {
            let pop_count = (self.history_seq - cp.history_seq) as usize;
            for _ in 0..pop_count {
                if self.history.pop_back().is_none() {
                    break;
                }
            }
            self.history_seq = cp.history_seq;
        }

        self.pc = cp.pc;
        self.last_label_id = cp.last_label_id;
        self.last_label_fallback = cp.last_label_fallback;
        self.stack.clear();
        self.vars = cp.vars;
        self.frames = cp.frames;
        self.layers = cp.layers.clone();
        self.pending_layers = cp.layers;
        self.pending_dirty = false;
        self.tweens.clear();
        self.shakes.clear();
        self.timelines = cp.timelines;
        self.transition = None;
        self.window_reveal = None;
        self.dialogue_visible = cp.dialogue_visible;
        self.dialogue_opacity = 1.0;
        self.dialogue_shake = [0.0, 0.0];
        self.dialogue_style = cp.dialogue_style;
        self.dialogue_portrait = cp.dialogue_portrait;
        self.dialogue = cp.dialogue;
        self.choice = cp.choice;
        self.wait = cp.wait;
        self.audio = cp.audio;
        self.audio_paused = cp.audio_paused;
        self.locale = cp.locale;
        self.rng = ChaCha8Rng::from_seed(RNG_SEED);
        self.rng.set_word_pos(cp.rng_word_pos);
        self.outcome = cp.outcome;
        self.intents.clear();
        intents
    }

    /// Captures a compact, version-resilient snapshot for persistent save slots.
    pub fn snapshot(&self, unix_time: u64) -> SaveSnapshot {
        let vars = self
            .program
            .vars
            .iter()
            .zip(self.vars.iter())
            .map(|(m, v)| (self.program.str(m.name).to_owned(), v.clone()))
            .collect();
        let last_label = self.current_last_label();
        let summary = self
            .dialogue
            .as_ref()
            .map(|d| {
                let mut s = if d.speaker.is_empty() {
                    d.full_text.clone()
                } else {
                    format!("{}: {}", d.speaker, d.full_text)
                };
                if s.chars().count() > 48 {
                    s = s.chars().take(48).collect::<String>() + "…";
                }
                s
            })
            .or_else(|| last_label.clone())
            .unwrap_or_else(|| self.program.meta.title.clone());
        let skip = self.history.len().saturating_sub(MAX_HISTORY);
        let history: Vec<HistoryEntry> = self
            .history
            .iter()
            .skip(skip)
            .map(|e| (**e).clone())
            .collect();
        SaveSnapshot {
            format: SAVE_FORMAT_VERSION,
            game_id: self.program.meta.game_id.clone(),
            version: self.program.meta.version.clone(),
            saved_at_unix: unix_time,
            summary,
            pc: self.pc,
            last_label,
            vars,
            frames: self.frames.clone(),
            layers: self.layers.clone(),
            dialogue_visible: self.dialogue_visible,
            dialogue_style: self.dialogue_style,
            dialogue_portrait: self.dialogue_portrait,
            timelines: self.timelines.clone(),
            dialogue: self.dialogue.clone(),
            choice: self.choice.clone(),
            wait: self.wait.clone(),
            audio: self.audio.clone(),
            audio_paused: self.audio_paused,
            history,
            locale: self.locale.clone(),
            rng_word_pos: self.rng.get_word_pos(),
            outcome: self.outcome.clone(),
            thumbnail: None,
        }
    }

    /// Replaces the program Arc (e.g. after dynamically loading an external translation locale)
    /// without resetting VM state, and refreshes the currently displayed dialogue line if any.
    pub fn apply_translation_program(&mut self, program: Arc<NirProgram>, locale: String) {
        self.program = program;
        self.locale = locale;
        if let Some(dlg) = &mut self.dialogue {
            if let Some(entry) = self.program.text(&self.locale, dlg.text_id) {
                let full_text = entry.to_plain_text();
                if !full_text.is_empty() {
                    use unicode_segmentation::UnicodeSegmentation;
                    let len = full_text.len();
                    dlg.total_graphemes = full_text.graphemes(true).count();
                    dlg.full_text = full_text;
                    dlg.spans = vec![ResolvedSpan {
                        byte_start: 0,
                        byte_end: len,
                        emphasis: false,
                        color: None,
                        ruby: None,
                        speed: 1.0,
                        inline_image: None,
                    }];
                    if dlg.finished {
                        dlg.visible_graphemes = dlg.total_graphemes;
                    } else {
                        dlg.visible_graphemes = dlg.visible_graphemes.min(dlg.total_graphemes);
                    }
                    dlg.recompute_visible_bytes();
                }
            }
        }
    }

    /// Restores a snapshot cleanly, migrating variables by name and falling back to
    /// `last_label` if the script version changed.
    pub fn restore(
        program: Arc<NirProgram>,
        mut snap: SaveSnapshot,
        profile: ProfileData,
    ) -> Result<(Self, Vec<VmIntent>)> {
        if snap.format != SAVE_FORMAT_VERSION {
            return Err(NirError::new(
                "E_SAVE_VERSION",
                "restore",
                format!("unsupported save format {}", snap.format),
            ));
        }
        if snap.game_id != program.meta.game_id {
            return Err(NirError::new(
                "E_SAVE_GAME",
                "restore",
                format!(
                    "save belongs to {}, not {}",
                    snap.game_id, program.meta.game_id
                ),
            ));
        }
        let vars = program
            .vars
            .iter()
            .map(|m| {
                snap.vars
                    .get(program.str(m.name))
                    .cloned()
                    .unwrap_or_else(|| m.default.clone())
            })
            .collect();

        // If version matches and PC is in range, restore exact PC; otherwise relocate via label!
        let found_lbl = snap
            .last_label
            .as_deref()
            .and_then(|n| program.find_label(n));
        let pc = if snap.version == program.meta.version && (snap.pc as usize) <= program.ops.len()
        {
            snap.pc
        } else if let Some(lbl) = found_lbl {
            lbl.pc
        } else {
            snap.pc.min(program.ops.len().saturating_sub(1) as u32)
        };
        let last_label_id = found_lbl.map(|l| l.name);
        let last_label_fallback = if last_label_id.is_some() {
            None
        } else {
            snap.last_label
        };

        if let Some(dlg) = &mut snap.dialogue {
            dlg.recompute_visible_bytes();
        }

        let mut rng = ChaCha8Rng::from_seed(RNG_SEED);
        rng.set_word_pos(snap.rng_word_pos);

        // Emit intents to reconstruct active audio channels on host.
        let mut intents = Vec::new();
        for ch in &snap.audio {
            intents.push(VmIntent::PlayAudio {
                channel: ch.channel,
                bus: ch.bus,
                asset: ch.asset,
                gain: ch.gain * ch.envelope,
                looped: ch.looped,
                fade_in_us: 0,
                loop_region: ch.loop_region,
                character: ch.character.clone(),
            });
        }
        for (idx, &paused) in snap.audio_paused.iter().enumerate() {
            if paused {
                let bus = match idx {
                    0 => AudioBus::Bgm,
                    1 => AudioBus::Voice,
                    _ => AudioBus::Sfx,
                };
                intents.push(VmIntent::PauseAudioBus { bus, paused: true });
            }
        }

        let history_seq = snap.history.len() as u64;
        let history: VecDeque<Arc<HistoryEntry>> = snap.history.into_iter().map(Arc::new).collect();
        let label_by_pc = build_label_index(&program);

        let vm = Self {
            program,
            label_by_pc,
            pc,
            last_label_id,
            last_label_fallback,
            stack: Vec::with_capacity(32),
            vars,
            frames: if snap.frames.is_empty() {
                vec![CallFrame {
                    func: FuncId(0),
                    return_pc: u32::MAX,
                    locals: Vec::new(),
                }]
            } else {
                snap.frames
            },
            layers: snap.layers.clone(),
            pending_layers: snap.layers,
            pending_dirty: false,
            tweens: Vec::new(),
            shakes: Vec::new(),
            timelines: snap.timelines,
            transition: None,
            window_reveal: None,
            dialogue_visible: snap.dialogue_visible,
            dialogue_opacity: 1.0,
            dialogue_shake: [0.0, 0.0],
            dialogue_style: snap.dialogue_style,
            dialogue_portrait: snap.dialogue_portrait,
            dialogue: snap.dialogue,
            choice: snap.choice,
            wait: snap.wait,
            audio: snap.audio,
            audio_paused: snap.audio_paused,
            history,
            history_seq,
            locale: snap.locale,
            profile,
            rng,
            outcome: snap.outcome,
            intents: Vec::new(),
        };
        Ok((vm, intents))
    }

    /// Runs bytecode until the VM suspends (Say/Choice/Wait/Transition/End) or hits budget.
    pub fn step(&mut self, budget: u32) -> Result<Vec<VmIntent>> {
        self.intents.clear();
        let limit = budget.min(MAX_FUEL_PER_STEP);
        let mut used = 0u32;
        // Clone cheap Arc handles once so we can borrow `&Op` and `&label_by_pc` with zero Op cloning!
        let program = Arc::clone(&self.program);
        let label_by_pc = Arc::clone(&self.label_by_pc);

        while self.outcome.is_none() && self.wait == WaitReason::None && used < limit {
            let pc = self.pc as usize;
            let Some(op) = program.ops.get(pc) else {
                self.outcome = Some("end".into());
                break;
            };
            // O(1) label tracking with zero String allocation!
            if let Some(&name_id) = label_by_pc.get(&self.pc) {
                self.last_label_id = Some(name_id);
                self.last_label_fallback = None;
            }
            self.pc += 1;
            used += 1;
            self.exec_op(op)?;
        }

        if used >= MAX_FUEL_PER_STEP && self.wait == WaitReason::None && self.outcome.is_none() {
            return Err(NirError::new(
                "E_FUEL",
                format!("pc:{}", self.pc),
                "non-suspending loop exceeded step budget",
            ));
        }

        Ok(std::mem::take(&mut self.intents))
    }

    fn exec_op(&mut self, op: &Op) -> Result<()> {
        match op {
            Op::PushNull => self.stack.push(Value::Null),
            Op::PushBool(b) => self.stack.push(Value::Bool(*b)),
            Op::PushInt(n) => self.stack.push(Value::Int(*n)),
            Op::PushFloat(f) => self.stack.push(Value::Float(*f)),
            Op::PushStr(id) => self
                .stack
                .push(Value::Str(self.program.str(*id).to_owned())),
            Op::LoadVar(id) => {
                let v = self.vars.get(id.0 as usize).cloned().unwrap_or(Value::Null);
                self.stack.push(v);
            }
            Op::StoreVar(id) => {
                let v = self.pop()?;
                if let Some(slot) = self.vars.get_mut(id.0 as usize) {
                    *slot = v;
                }
            }
            Op::LoadLocal(slot) => {
                let v = self
                    .frames
                    .last()
                    .and_then(|f| f.locals.get(*slot as usize))
                    .cloned()
                    .unwrap_or(Value::Null);
                self.stack.push(v);
            }
            Op::StoreLocal(slot) => {
                let v = self.pop()?;
                if let Some(frame) = self.frames.last_mut() {
                    if let Some(dst) = frame.locals.get_mut(*slot as usize) {
                        *dst = v;
                    }
                }
            }
            Op::Pop => {
                let _ = self.stack.pop();
            }
            Op::BinOp(bop) => {
                let rhs = self.pop()?;
                let lhs = self.pop()?;
                self.stack.push(eval_binop(*bop, lhs, rhs)?);
            }
            Op::UnaryOp(uop) => {
                let v = self.pop()?;
                let res = match *uop {
                    UnaryOp::Not => Value::Bool(!v.as_bool()),
                    UnaryOp::Neg => match v {
                        Value::Int(n) => Value::Int(n.wrapping_neg()),
                        Value::Float(f) => Value::Float(-f),
                        _ => Value::Int(0),
                    },
                };
                self.stack.push(res);
            }
            Op::RandInt { dst, min, max } => {
                let (min, max) = (*min, *max);
                let val = if max <= min {
                    min
                } else {
                    let span = (max as i128 - min as i128 + 1) as u64;
                    let r = self.rng.next_u64() % span;
                    (min as i128 + r as i128) as i64
                };
                if let Some(slot) = self.vars.get_mut(dst.0 as usize) {
                    *slot = Value::Int(val);
                }
            }
            Op::Jump(target) => {
                self.pc = *target;
            }
            Op::JumpIfFalse(target) => {
                let cond = self.pop()?;
                if !cond.as_bool() {
                    self.pc = *target;
                }
            }
            Op::Call { func, argc } => {
                let (func, argc) = (*func, *argc);
                if self.frames.len() >= MAX_CALL_DEPTH {
                    return Err(NirError::new("E_STACK", "call", "call depth exceeded"));
                }
                let (entry_pc, local_count) = {
                    let fmeta = self
                        .program
                        .func(func)
                        .ok_or_else(|| NirError::new("E_FUNC", "call", "invalid function id"))?;
                    (fmeta.entry_pc, fmeta.local_count)
                };
                let mut locals = vec![Value::Null; (local_count as usize).max(argc as usize)];
                for i in (0..argc as usize).rev() {
                    locals[i] = self.pop()?;
                }
                self.frames.push(CallFrame {
                    func,
                    return_pc: self.pc,
                    locals,
                });
                self.pc = entry_pc;
            }
            Op::Ret { has_value } => {
                let ret_val = if *has_value { Some(self.pop()?) } else { None };
                let frame = self.frames.pop();
                match frame {
                    Some(f) if f.return_pc != u32::MAX => {
                        self.pc = f.return_pc;
                        if let Some(v) = ret_val {
                            self.stack.push(v);
                        }
                    }
                    _ => {
                        self.outcome = Some("end".into());
                    }
                }
            }
            Op::End { outcome } => {
                self.commit_staged_if_dirty(Transition::Cut, 0, false);
                self.outcome = Some(self.program.str(*outcome).to_owned());
            }

            // --- Persistent Scene Graph ---
            Op::LayerSet {
                layer,
                parent,
                asset,
                transform,
            } => {
                let (layer, parent, asset) = (*layer, *parent, *asset);
                self.ensure_pending_synced();
                if let Some(existing) = self.pending_layers.iter_mut().find(|l| l.id == layer) {
                    existing.parent = parent;
                    existing.asset = asset;
                    existing.transform = (**transform).clone();
                    existing.text = None;
                } else {
                    self.pending_layers.push(LayerState {
                        id: layer,
                        parent,
                        asset,
                        transform: (**transform).clone(),
                        shake_offset: [0.0, 0.0],
                        text: None,
                    });
                }
                self.pending_dirty = true;
            }
            Op::LayerAsset { layer, asset } => {
                let (layer, asset) = (*layer, *asset);
                self.ensure_pending_synced();
                if let Some(existing) = self.pending_layers.iter_mut().find(|l| l.id == layer) {
                    existing.asset = asset;
                    if asset.is_some() {
                        existing.transform.color = [1.0, 1.0, 1.0, 1.0];
                        existing.transform.width = 0.0;
                        existing.transform.height = 0.0;
                    }
                    existing.text = None;
                } else {
                    self.pending_layers.push(LayerState {
                        id: layer,
                        parent: None,
                        asset,
                        transform: Transform2D::default(),
                        shake_offset: [0.0, 0.0],
                        text: None,
                    });
                }
                self.pending_dirty = true;
            }
            Op::LayerPatch { layer, prop, value } => {
                let (layer, prop, value) = (*layer, *prop, *value);
                self.ensure_pending_synced();
                if let Some(existing) = self.pending_layers.iter_mut().find(|l| l.id == layer) {
                    apply_layer_prop(&mut existing.transform, prop, value);
                    self.pending_dirty = true;
                }
            }
            Op::LayerRemove { layer } => {
                let layer = *layer;
                self.ensure_pending_synced();
                self.pending_layers
                    .retain(|l| l.id != layer && l.parent != Some(layer));
                self.tweens
                    .retain(|t| !matches!(t.target, TweenTarget::Layer(id) if id == layer));
                self.shakes
                    .retain(|s| !matches!(s.target, TweenTarget::Layer(id) if id == layer));
                self.timelines.retain(|tl| tl.layer != layer);
                self.pending_dirty = true;
            }
            Op::LayerClear => {
                self.pending_layers.clear();
                self.tweens
                    .retain(|t| !matches!(t.target, TweenTarget::Layer(_)));
                self.shakes
                    .retain(|s| !matches!(s.target, TweenTarget::Layer(_)));
                self.timelines.clear();
                self.pending_dirty = true;
            }
            Op::StageCommit {
                transition,
                duration_us,
                wait,
            } => {
                self.commit_staged_if_dirty(*transition, *duration_us, *wait);
            }
            Op::Tween {
                target,
                prop,
                to,
                duration_us,
                easing,
                wait,
            } => {
                let (target, prop, to, duration_us, easing, wait) =
                    (*target, *prop, *to, *duration_us, *easing, *wait);
                // Flush pending layer additions so tween finds newly created layers!
                if self.pending_dirty {
                    self.commit_staged_if_dirty(Transition::Cut, 0, false);
                }
                let from = self.read_prop(target, prop);
                // Replace any existing tween targeting the exact same (target, prop).
                self.tweens
                    .retain(|t| !(t.target == target && t.prop == prop));
                if duration_us == 0 {
                    self.write_prop(target, prop, to);
                } else {
                    self.tweens.push(ActiveTween {
                        target,
                        prop,
                        from,
                        to,
                        elapsed_us: 0,
                        duration_us,
                        easing,
                    });
                    if wait {
                        self.wait = WaitReason::Tween;
                    }
                }
            }
            Op::Shake { target, spec, wait } => {
                let (target, spec, wait) = (*target, *spec, *wait);
                if self.pending_dirty {
                    self.commit_staged_if_dirty(Transition::Cut, 0, false);
                }
                self.shakes.retain(|s| s.target != target);
                if spec.duration_us > 0 {
                    let seed = self.rng.next_u32();
                    self.shakes.push(ActiveShake {
                        target,
                        spec,
                        elapsed_us: 0,
                        seed,
                    });
                    if wait {
                        self.wait = WaitReason::Shake;
                    }
                }
            }

            // --- Narrative ---
            Op::Say {
                speaker,
                speaker_color,
                text,
                voice,
                reveal_us,
                append,
            } => {
                let (speaker, speaker_color, text, voice, reveal_us, append) =
                    (*speaker, *speaker_color, *text, *voice, *reveal_us, *append);
                // Auto-commit any staged scene edits cleanly before displaying dialogue.
                if self.pending_dirty {
                    self.commit_staged_if_dirty(Transition::Cut, 0, false);
                }
                let speaker_str = speaker
                    .map(|id| self.program.str(id).to_owned())
                    .unwrap_or_default();
                let (mut full_text, mut spans, mut pauses) =
                    if let Some(entry) = self.program.text(&self.locale, text) {
                        self.instantiate_text(entry)
                    } else {
                        (String::new(), Vec::new(), Vec::new())
                    };
                let (prev_graphemes, prev_bytes) = if append {
                    if let Some(prev) = &self.dialogue {
                        let prefix_bytes = prev.full_text.len();
                        let prefix_graphemes = prev.total_graphemes;
                        for s in &mut spans {
                            s.byte_start += prefix_bytes;
                            s.byte_end += prefix_bytes;
                        }
                        for p in &mut pauses {
                            p.grapheme_index += prefix_graphemes;
                        }
                        let mut combined = prev.full_text.clone();
                        combined.push_str(&full_text);
                        full_text = combined;
                        let mut combined_spans = prev.spans.clone();
                        combined_spans.extend(spans);
                        spans = combined_spans;
                        let mut combined_pauses = prev.pauses.clone();
                        combined_pauses.extend(pauses);
                        pauses = combined_pauses;
                        (prefix_graphemes, prefix_bytes)
                    } else {
                        (0, 0)
                    }
                } else {
                    (0, 0)
                };

                let total_graphemes = full_text.graphemes(true).count();
                let reveal = if reveal_us > 0 {
                    reveal_us
                } else {
                    self.program.meta.default_reveal_us
                };
                let already_read = self.profile.read_texts.contains(&text.0);
                let finished = reveal == 0 || prev_graphemes >= total_graphemes;

                if let Some(v_asset) = voice {
                    let voice_channel = ChannelId(1);
                    self.play_audio_internal(
                        voice_channel,
                        AudioBus::Voice,
                        v_asset,
                        1.0,
                        false,
                        0,
                        None,
                        (!speaker_str.is_empty()).then(|| speaker_str.clone()),
                    );
                }

                let visible_bytes = if finished {
                    full_text.len()
                } else {
                    prev_bytes
                };
                let dlg = DialogueState {
                    text_id: text,
                    speaker: speaker_str.clone(),
                    speaker_color,
                    full_text: full_text.clone(),
                    spans,
                    pauses,
                    voice,
                    reveal_us: reveal,
                    elapsed_us: 0,
                    visible_graphemes: if finished {
                        total_graphemes
                    } else {
                        prev_graphemes
                    },
                    visible_bytes,
                    total_graphemes,
                    paused_for_click: false,
                    pause_remaining_us: 0,
                    finished,
                    already_read,
                };
                self.dialogue = Some(dlg);
                self.dialogue_visible = true;
                self.wait = WaitReason::Dialogue;
                self.push_history(HistoryEntry {
                    text_id: text,
                    speaker: speaker_str,
                    text: full_text,
                    voice,
                });
                self.intents.push(VmIntent::Checkpoint);
            }
            Op::TextWindow {
                visible,
                transition,
                duration_us,
            } => {
                let (visible, transition, duration_us) = (*visible, *transition, *duration_us);
                if duration_us == 0 || matches!(transition, Transition::Cut) {
                    self.dialogue_visible = visible;
                    self.window_reveal = None;
                } else {
                    let from_alpha = self.dialogue_alpha();
                    self.window_reveal = Some(WindowRevealState {
                        to_visible: visible,
                        transition,
                        from_alpha,
                        elapsed_us: 0,
                        duration_us,
                    });
                }
            }
            Op::Choice {
                items,
                result_var,
                cancel_pc,
                timeout_us,
                default_idx,
            } => {
                let (result_var, cancel_pc, timeout_us, default_idx) =
                    (*result_var, *cancel_pc, *timeout_us, *default_idx);
                if self.pending_dirty {
                    self.commit_staged_if_dirty(Transition::Cut, 0, false);
                }
                let mut offered = Vec::new();
                for (i, item) in items.iter().enumerate() {
                    if let Some(cvar) = item.cond_var {
                        let visible = self.vars.get(cvar.0 as usize).is_some_and(Value::as_bool);
                        if !visible {
                            continue;
                        }
                    }
                    let label = self
                        .program
                        .text(&self.locale, item.text)
                        .map(|e| self.instantiate_text(e).0)
                        .unwrap_or_default();
                    let disabled = item.disabled_var.is_some_and(|dvar| {
                        self.vars.get(dvar.0 as usize).is_some_and(Value::as_bool)
                    });
                    offered.push(OfferedChoice {
                        index: i as u16,
                        label,
                        image: item.image,
                        hover_image: item.hover_image,
                        disabled_image: item.disabled_image,
                        preview: item.preview,
                        rect: item.rect,
                        target_pc: item.target_pc,
                        value: item.value.clone(),
                        choice_index: Some(Value::Int(i as i64)),
                        disabled,
                    });
                }
                if offered.is_empty() {
                    if let Some(pc) = cancel_pc {
                        self.pc = pc;
                    }
                    return Ok(());
                }
                self.choice = Some(ChoiceState {
                    items: offered,
                    result_var,
                    index_var: None,
                    cancel_pc,
                    timeout_us,
                    elapsed_us: 0,
                    default_idx,
                    background: None,
                    menu_size: None,
                    hover_se: None,
                    click_se: None,
                    fade_out_us: 0,
                });
                self.wait = WaitReason::Choice;
                self.intents.push(VmIntent::Checkpoint);
            }
            Op::Wait {
                duration_us,
                skippable,
            } => {
                let (duration_us, skippable) = (*duration_us, *skippable);
                if self.pending_dirty {
                    self.commit_staged_if_dirty(Transition::Cut, 0, false);
                }
                if duration_us > 0 {
                    self.wait = WaitReason::Timer {
                        remaining_us: duration_us,
                        skippable,
                    };
                }
            }

            // --- Audio & Video ---
            Op::AudioPlay {
                channel,
                bus,
                asset,
                gain,
                looped,
                fade_in_us,
                loop_region,
            } => {
                self.play_audio_internal(
                    *channel,
                    *bus,
                    *asset,
                    *gain,
                    *looped,
                    *fade_in_us,
                    *loop_region,
                    None,
                );
            }
            Op::AudioStop {
                channel,
                fade_out_us,
            } => {
                let (channel, fade_out_us) = (*channel, *fade_out_us);
                self.audio.retain(|c| c.channel != channel);
                self.intents.push(VmIntent::StopAudio {
                    channel,
                    fade_out_us,
                });
            }
            Op::AudioPause { bus, paused } => {
                let (bus, paused) = (*bus, *paused);
                self.audio_paused[bus as usize] = paused;
                self.intents.push(VmIntent::PauseAudioBus { bus, paused });
            }
            Op::VideoPlay { asset, skippable } => {
                let (asset, skippable) = (*asset, *skippable);
                if self.pending_dirty {
                    self.commit_staged_if_dirty(Transition::Cut, 0, false);
                }
                self.wait = WaitReason::Video { asset, skippable };
                self.intents.push(VmIntent::PlayVideo { asset, skippable });
            }

            // --- Profile / Unlocks ---
            Op::ProfileSetFlag(key) => {
                let s = self.program.str(*key).to_owned();
                if self.profile.flags.insert(s) {
                    self.intents.push(VmIntent::ProfileChanged);
                }
            }
            Op::ProfileGetFlag { dst, key } => {
                let flag = self.profile.flags.contains(self.program.str(*key));
                if let Some(slot) = self.vars.get_mut(dst.0 as usize) {
                    *slot = Value::Bool(flag);
                }
            }
            Op::ProfileSetVal { key } => {
                let val = self.pop()?;
                let k = self.program.str(*key).to_owned();
                self.profile.values.insert(k, val);
                self.intents.push(VmIntent::ProfileChanged);
            }
            Op::ProfileGetVal { dst, key } => {
                if let Some(val) = self.profile.values.get(self.program.str(*key)).cloned() {
                    if let Some(slot) = self.vars.get_mut(dst.0 as usize) {
                        *slot = val;
                    }
                }
            }

            // --- Pure Built-ins, Dynamic Media, Timelines, Dynamic Menus & Modals ---
            Op::CallBuiltin { func, argc } => {
                let n = *argc as usize;
                let mut args = vec![Value::Null; n];
                for i in (0..n).rev() {
                    args[i] = self.pop()?;
                }
                let res = self.eval_builtin(*func, args);
                self.stack.push(res);
            }
            Op::ResolveAsset => {
                let raw = self.pop()?.to_display_string();
                let trimmed = raw.trim();
                let resolved = if trimmed.is_empty() {
                    Value::Null
                } else if trimmed.starts_with('$') || trimmed.starts_with('#') {
                    Value::Str(trimmed.to_owned())
                } else if let Some(id) = resolve_asset_path(&self.program.asset_lookup, trimmed) {
                    Value::Int(id.0 as i64)
                } else {
                    Value::Null
                };
                self.stack.push(resolved);
            }
            Op::LayerSetDyn {
                layer,
                parent,
                transform,
            } => {
                let val = self.pop()?;
                let (layer, parent) = (*layer, *parent);
                let mut tf = (**transform).clone();
                let asset = match &val {
                    Value::Int(id) if *id >= 0 => Some(AssetId(*id as u32)),
                    Value::Str(s) => {
                        if let Some(rgba) = parse_solid_color(s) {
                            tf.color = rgba;
                            None
                        } else {
                            resolve_asset_path(&self.program.asset_lookup, s)
                        }
                    }
                    _ => None,
                };
                self.ensure_pending_synced();
                if let Some(existing) = self.pending_layers.iter_mut().find(|l| l.id == layer) {
                    existing.parent = parent;
                    existing.asset = asset;
                    existing.transform = tf;
                    existing.text = None;
                } else {
                    self.pending_layers.push(LayerState {
                        id: layer,
                        parent,
                        asset,
                        transform: tf,
                        shake_offset: [0.0, 0.0],
                        text: None,
                    });
                }
                self.pending_dirty = true;
            }
            Op::LayerAssetDyn { layer } => {
                let val = self.pop()?;
                let layer = *layer;
                let mut solid_color = None;
                let asset = match &val {
                    Value::Int(id) if *id >= 0 => Some(AssetId(*id as u32)),
                    Value::Str(s) => {
                        if let Some(rgba) = parse_solid_color(s) {
                            solid_color = Some(rgba);
                            None
                        } else {
                            resolve_asset_path(&self.program.asset_lookup, s)
                        }
                    }
                    _ => None,
                };
                self.ensure_pending_synced();
                if let Some(existing) = self.pending_layers.iter_mut().find(|l| l.id == layer) {
                    existing.asset = asset;
                    if let Some(c) = solid_color {
                        existing.transform.color = c;
                    } else if asset.is_some() {
                        existing.transform.color = [1.0, 1.0, 1.0, 1.0];
                    }
                    existing.text = None;
                } else {
                    let mut tf = Transform2D::default();
                    if let Some(c) = solid_color {
                        tf.color = c;
                    }
                    self.pending_layers.push(LayerState {
                        id: layer,
                        parent: None,
                        asset,
                        transform: tf,
                        shake_offset: [0.0, 0.0],
                        text: None,
                    });
                }
                self.pending_dirty = true;
            }
            Op::LayerSetText {
                layer,
                parent,
                transform,
                text,
                font_size,
                color,
                align,
            } => {
                let text_str = match text {
                    Some(sid) => self.program.str(*sid).to_owned(),
                    None => self.pop()?.to_display_string(),
                };
                let (layer, parent, font_size, color, align) =
                    (*layer, *parent, *font_size, *color, *align);
                let ltext = LayerText {
                    text: text_str,
                    font_size,
                    color,
                    align,
                };
                self.ensure_pending_synced();
                if let Some(existing) = self.pending_layers.iter_mut().find(|l| l.id == layer) {
                    if let Some(tf) = transform {
                        existing.parent = parent;
                        existing.transform = (**tf).clone();
                    }
                    existing.asset = None;
                    existing.text = Some(ltext);
                } else {
                    self.pending_layers.push(LayerState {
                        id: layer,
                        parent,
                        asset: None,
                        transform: transform
                            .as_ref()
                            .map(|tf| (**tf).clone())
                            .unwrap_or_default(),
                        shake_offset: [0.0, 0.0],
                        text: Some(ltext),
                    });
                }
                self.pending_dirty = true;
            }
            Op::ShakeStop { target } => {
                let target = *target;
                if target == TweenTarget::Layer(LayerId(u32::MAX)) {
                    // Wildcard stop-all: clears all active layer and dialogue box shakes cleanly.
                    self.shakes.clear();
                    for l in self.layers.iter_mut().chain(self.pending_layers.iter_mut()) {
                        l.shake_offset = [0.0, 0.0];
                    }
                    self.dialogue_shake = [0.0, 0.0];
                } else {
                    self.shakes.retain(|s| s.target != target);
                    match target {
                        TweenTarget::Layer(lid) => {
                            for l in self.layers.iter_mut().chain(self.pending_layers.iter_mut()) {
                                if l.id == lid {
                                    l.shake_offset = [0.0, 0.0];
                                }
                            }
                        }
                        TweenTarget::DialogueBox => self.dialogue_shake = [0.0, 0.0],
                        TweenTarget::Audio(_) => {}
                    }
                }
                if self.wait == WaitReason::Shake && self.shakes.is_empty() {
                    self.wait = WaitReason::None;
                }
            }
            Op::AudioPlayDyn {
                channel,
                bus,
                gain,
                looped,
                fade_in_us,
            } => {
                let val = self.pop()?;
                let asset_opt = match &val {
                    Value::Int(id) if *id >= 0 => Some(AssetId(*id as u32)),
                    Value::Str(s) => resolve_asset_path(&self.program.asset_lookup, s),
                    _ => None,
                };
                if let Some(asset) = asset_opt {
                    self.play_audio_internal(
                        *channel,
                        *bus,
                        asset,
                        *gain,
                        *looped,
                        *fade_in_us,
                        None,
                        None,
                    );
                }
            }
            Op::AudioGain {
                channel,
                gain,
                duration_us,
                wait,
            } => {
                let (channel, gain, duration_us, wait) = (*channel, *gain, *duration_us, *wait);
                let target = TweenTarget::Audio(channel);
                self.tweens
                    .retain(|t| !(t.target == target && t.prop == TweenProp::Volume));
                if let Some(chan) = self.audio.iter_mut().find(|c| c.channel == channel) {
                    let current = chan.gain * chan.envelope;
                    if duration_us == 0 {
                        chan.gain = 1.0;
                        chan.envelope = gain.clamp(0.0, 4.0);
                        self.intents.push(VmIntent::SetAudioGain {
                            channel,
                            gain: chan.envelope,
                        });
                    } else {
                        chan.gain = 1.0;
                        chan.envelope = current;
                        self.tweens.push(ActiveTween {
                            target,
                            prop: TweenProp::Volume,
                            from: current,
                            to: gain.clamp(0.0, 4.0),
                            elapsed_us: 0,
                            duration_us,
                            easing: Easing::Linear,
                        });
                        if wait {
                            self.wait = WaitReason::Tween;
                        }
                    }
                }
            }
            Op::ChoiceList {
                result_var,
                index_var,
                cancelable,
                click_se,
                timeout_us,
            } => {
                let id_list = self.pop()?;
                let labels_val = self.pop()?;
                let (result_var, index_var, cancelable, click_se, timeout_us) =
                    (*result_var, *index_var, *cancelable, *click_se, *timeout_us);
                if self.pending_dirty {
                    self.commit_staged_if_dirty(Transition::Cut, 0, false);
                }
                let labels: Vec<String> = match labels_val {
                    Value::List(v) => v
                        .into_iter()
                        .map(|x| x.to_display_string())
                        .filter(|s| !s.is_empty())
                        .collect(),
                    other => other
                        .to_display_string()
                        .split("\r\n")
                        .flat_map(|s| s.split('\n'))
                        .map(str::to_owned)
                        .filter(|s| !s.is_empty())
                        .collect(),
                };
                if labels.is_empty() {
                    if let Some(var) = result_var {
                        if let Some(slot) = self.vars.get_mut(var.0 as usize) {
                            *slot = Value::Str(String::new());
                        }
                    }
                    if let Some(var) = index_var {
                        if let Some(slot) = self.vars.get_mut(var.0 as usize) {
                            *slot = Value::Int(-1);
                        }
                    }
                    return Ok(());
                }
                let ids: Vec<Value> = match id_list {
                    Value::List(v) => v,
                    Value::Str(s) if !s.is_empty() => s
                        .split("\r\n")
                        .flat_map(|p| p.split('\n'))
                        .filter(|p| !p.is_empty())
                        .map(|p| {
                            p.trim()
                                .parse::<i64>()
                                .map(Value::Int)
                                .unwrap_or_else(|_| Value::Str(p.to_owned()))
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                let next_pc = self.pc;
                let offered: Vec<OfferedChoice> = labels
                    .into_iter()
                    .enumerate()
                    .map(|(i, label)| {
                        let choice_index = ids.get(i).cloned().unwrap_or(Value::Int(i as i64));
                        OfferedChoice {
                            index: i as u16,
                            label: label.clone(),
                            image: None,
                            hover_image: None,
                            disabled_image: None,
                            preview: None,
                            rect: None,
                            target_pc: next_pc,
                            value: Some(Value::Str(label)),
                            choice_index: Some(choice_index),
                            disabled: false,
                        }
                    })
                    .collect();
                self.choice = Some(ChoiceState {
                    items: offered,
                    result_var,
                    index_var,
                    cancel_pc: cancelable.then_some(next_pc),
                    timeout_us,
                    elapsed_us: 0,
                    default_idx: Some(0),
                    background: None,
                    menu_size: None,
                    hover_se: None,
                    click_se,
                    fade_out_us: 0,
                });
                self.wait = WaitReason::Choice;
                self.intents.push(VmIntent::Checkpoint);
            }
            Op::ChoiceMenu {
                menu,
                result_var,
                index_var,
                cancelable,
                hover_se,
                click_se,
                fade_in_us: _,
                fade_out_us,
                timeout_us,
            } => {
                let disabled_val = self.pop()?;
                let hidden_val = self.pop()?;
                let (
                    menu,
                    result_var,
                    index_var,
                    cancelable,
                    hover_se,
                    click_se,
                    fade_out_us,
                    timeout_us,
                ) = (
                    *menu,
                    *result_var,
                    *index_var,
                    *cancelable,
                    *hover_se,
                    *click_se,
                    *fade_out_us,
                    *timeout_us,
                );
                if self.pending_dirty {
                    self.commit_staged_if_dirty(Transition::Cut, 0, false);
                }
                let hidden_set = value_to_name_set(&hidden_val);
                let disabled_set = value_to_name_set(&disabled_val);
                let Some(tmpl) = self.program.menus.get(menu.0 as usize) else {
                    return Ok(());
                };
                let next_pc = self.pc;
                let mut offered = Vec::new();
                for (orig_idx, item) in tmpl.items.iter().enumerate() {
                    let name = self.program.str(item.name).to_owned();
                    if hidden_set.contains(&name) {
                        continue;
                    }
                    let disabled = disabled_set.contains(&name);
                    let label = self.program.str(item.label).to_owned();
                    offered.push(OfferedChoice {
                        index: orig_idx as u16,
                        label: if label.is_empty() {
                            name.clone()
                        } else {
                            label
                        },
                        image: item.image,
                        hover_image: item.hover_image,
                        disabled_image: item.disabled_image,
                        preview: item.preview_image.map(|img| (img, item.preview_pos)),
                        rect: Some(item.rect),
                        target_pc: next_pc,
                        value: Some(Value::Str(name)),
                        choice_index: Some(Value::Int(orig_idx as i64)),
                        disabled,
                    });
                }
                let any_enabled = offered.iter().any(|c| !c.disabled);
                if offered.is_empty() || !any_enabled {
                    if let Some(var) = result_var {
                        if let Some(slot) = self.vars.get_mut(var.0 as usize) {
                            *slot = Value::Str(String::new());
                        }
                    }
                    if let Some(var) = index_var {
                        if let Some(slot) = self.vars.get_mut(var.0 as usize) {
                            *slot = Value::Int(-1);
                        }
                    }
                    return Ok(());
                }
                self.choice = Some(ChoiceState {
                    items: offered,
                    result_var,
                    index_var,
                    cancel_pc: cancelable.then_some(next_pc),
                    timeout_us,
                    elapsed_us: 0,
                    default_idx: Some(0),
                    background: tmpl.background,
                    menu_size: Some(tmpl.size),
                    hover_se,
                    click_se,
                    fade_out_us,
                });
                self.wait = WaitReason::Choice;
                self.intents.push(VmIntent::Checkpoint);
            }
            Op::TimelinePlay {
                timeline,
                layer,
                scrap_on_end,
                wait,
                click_skippable,
            } => {
                let (timeline, layer, scrap_on_end, wait, click_skippable) =
                    (*timeline, *layer, *scrap_on_end, *wait, *click_skippable);
                if self.pending_dirty {
                    self.commit_staged_if_dirty(Transition::Cut, 0, false);
                }
                self.timelines.retain(|tl| tl.layer != layer);
                let mut active = ActiveTimeline {
                    timeline,
                    layer,
                    elapsed_us: 0,
                    scrap_on_end,
                    triggered_sounds: Vec::new(),
                };
                self.trigger_timeline_sounds(&mut active, 0);
                self.timelines.push(active);
                if wait {
                    self.wait = WaitReason::Timeline {
                        layer,
                        skippable: click_skippable,
                    };
                }
            }
            Op::WaitPlay {
                layer,
                click_skippable,
            } => {
                let (layer, click_skippable) = (*layer, *click_skippable);
                if self.timelines.iter().any(|tl| tl.layer == layer) {
                    self.wait = WaitReason::Timeline {
                        layer,
                        skippable: click_skippable,
                    };
                }
            }
            Op::OpenModal(kind) => {
                let kind = *kind;
                if self.pending_dirty {
                    self.commit_staged_if_dirty(Transition::Cut, 0, false);
                }
                self.wait = WaitReason::Modal(kind);
                self.intents.push(VmIntent::OpenModal(kind));
            }
            Op::SetDialogueStyle(idx) => {
                self.dialogue_style = *idx;
            }
            Op::SetDialoguePortrait(asset) => {
                self.dialogue_portrait = *asset;
            }
            Op::SetDialoguePortraitDyn => {
                let val = self.pop()?;
                self.dialogue_portrait = match &val {
                    Value::Int(id) if *id >= 0 => Some(AssetId(*id as u32)),
                    Value::Str(s) => resolve_asset_path(&self.program.asset_lookup, s),
                    _ => None,
                };
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn play_audio_internal(
        &mut self,
        channel: ChannelId,
        bus: AudioBus,
        asset: AssetId,
        gain: f32,
        looped: bool,
        fade_in_us: u64,
        loop_region: Option<AudioLoopRegion>,
        character: Option<String>,
    ) {
        self.audio.retain(|c| c.channel != channel);
        self.audio.push(AudioChannelState {
            channel,
            bus,
            asset,
            gain,
            envelope: 1.0,
            looped,
            loop_region,
            character: character.clone(),
        });
        self.intents.push(VmIntent::PlayAudio {
            channel,
            bus,
            asset,
            gain,
            looped,
            fade_in_us,
            loop_region,
            character,
        });
    }

    fn ensure_pending_synced(&mut self) {
        if !self.pending_dirty {
            self.pending_layers = self.layers.clone();
        }
    }

    fn commit_staged_if_dirty(&mut self, transition: Transition, duration_us: u64, wait: bool) {
        if self.pending_dirty {
            let from = std::mem::replace(&mut self.layers, self.pending_layers.clone());
            self.pending_dirty = false;
            if duration_us > 0 && !matches!(transition, Transition::Cut) {
                self.transition = Some(StageTransitionState {
                    from_layers: from,
                    transition,
                    elapsed_us: 0,
                    duration_us,
                });
                if wait {
                    self.wait = WaitReason::StageTransition;
                }
            } else {
                self.transition = None;
            }
        } else if duration_us > 0 && !matches!(transition, Transition::Cut) && wait {
            self.transition = Some(StageTransitionState {
                from_layers: self.layers.clone(),
                transition,
                elapsed_us: 0,
                duration_us,
            });
            self.wait = WaitReason::StageTransition;
        }
    }

    fn instantiate_text(
        &self,
        entry: &TextEntry,
    ) -> (String, Vec<ResolvedSpan>, Vec<DialoguePausePoint>) {
        let mut full = String::new();
        let mut spans = Vec::new();
        let mut pauses = Vec::new();
        let mut cur_speed = 1.0f32;
        let mut grapheme_cursor = 0usize;

        for span in &entry.spans {
            match span {
                RichSpan::Text(s) => {
                    let start = full.len();
                    full.push_str(s);
                    let end = full.len();
                    grapheme_cursor += s.graphemes(true).count();
                    spans.push(ResolvedSpan {
                        byte_start: start,
                        byte_end: end,
                        emphasis: false,
                        color: None,
                        ruby: None,
                        speed: cur_speed,
                        inline_image: None,
                    });
                }
                RichSpan::Emphasis(s) => {
                    let start = full.len();
                    full.push_str(s);
                    let end = full.len();
                    grapheme_cursor += s.graphemes(true).count();
                    spans.push(ResolvedSpan {
                        byte_start: start,
                        byte_end: end,
                        emphasis: true,
                        color: None,
                        ruby: None,
                        speed: cur_speed,
                        inline_image: None,
                    });
                }
                RichSpan::Color { text, rgba } => {
                    let start = full.len();
                    full.push_str(text);
                    let end = full.len();
                    grapheme_cursor += text.graphemes(true).count();
                    spans.push(ResolvedSpan {
                        byte_start: start,
                        byte_end: end,
                        emphasis: false,
                        color: Some(*rgba),
                        ruby: None,
                        speed: cur_speed,
                        inline_image: None,
                    });
                }
                RichSpan::Ruby { base, ruby } => {
                    let start = full.len();
                    full.push_str(base);
                    let end = full.len();
                    grapheme_cursor += base.graphemes(true).count();
                    spans.push(ResolvedSpan {
                        byte_start: start,
                        byte_end: end,
                        emphasis: false,
                        color: None,
                        ruby: Some(ruby.clone()),
                        speed: cur_speed,
                        inline_image: None,
                    });
                }
                RichSpan::Pause { duration_us, click } => {
                    pauses.push(DialoguePausePoint {
                        grapheme_index: grapheme_cursor,
                        duration_us: *duration_us,
                        click: *click,
                        triggered: false,
                    });
                }
                RichSpan::InlineImage { asset, .. } => {
                    let start = full.len();
                    full.push('\u{3000}');
                    grapheme_cursor += 1;
                    spans.push(ResolvedSpan {
                        byte_start: start,
                        byte_end: full.len(),
                        emphasis: false,
                        color: None,
                        ruby: None,
                        speed: cur_speed,
                        inline_image: Some(*asset),
                    });
                }
                RichSpan::Var(var_id) => {
                    let text = self
                        .vars
                        .get(var_id.0 as usize)
                        .map(Value::to_display_string)
                        .unwrap_or_default();
                    let start = full.len();
                    full.push_str(&text);
                    grapheme_cursor += text.graphemes(true).count();
                    spans.push(ResolvedSpan {
                        byte_start: start,
                        byte_end: full.len(),
                        emphasis: false,
                        color: None,
                        ruby: None,
                        speed: cur_speed,
                        inline_image: None,
                    });
                }
                RichSpan::Speed(mult) => {
                    cur_speed = mult.clamp(0.1, 16.0);
                }
            }
        }
        (full, spans, pauses)
    }

    /// Advances all active clocks (tweens, shakes, transitions, reveal, timers).
    pub fn tick(&mut self, delta_us: u64, text_speed: f32, reduced_motion: bool) -> Vec<VmIntent> {
        self.tick_with_dirty(delta_us, text_speed, reduced_motion).0
    }

    /// Advances all active clocks and returns `(intents, visual_changed)` so `Runtime`
    /// never rebuilds `DrawPacket` on non-visual ticks (e.g. between typewriter glyphs or `Op::Wait`).
    pub fn tick_with_dirty(
        &mut self,
        delta_us: u64,
        text_speed: f32,
        reduced_motion: bool,
    ) -> (Vec<VmIntent>, bool) {
        self.intents.clear();
        if delta_us == 0 || self.outcome.is_some() {
            return (Vec::new(), false);
        }
        let mut visual_changed = false;

        // 1. Tweens
        let mut i = 0;
        while i < self.tweens.len() {
            self.tweens[i].elapsed_us = if reduced_motion {
                self.tweens[i].duration_us
            } else {
                self.tweens[i].elapsed_us.saturating_add(delta_us)
            };
            let t = &self.tweens[i];
            let progress = if t.duration_us == 0 {
                1.0
            } else {
                (t.elapsed_us as f64 / t.duration_us as f64).min(1.0) as f32
            };
            let val = interpolate(t.from, t.to, progress, t.easing);
            let target = t.target;
            let prop = t.prop;
            let done = t.elapsed_us >= t.duration_us;
            self.write_prop(target, prop, val);
            if !matches!(target, TweenTarget::Audio(_)) {
                visual_changed = true;
            }
            if done {
                self.tweens.remove(i);
            } else {
                i += 1;
            }
        }
        if self.wait == WaitReason::Tween && self.tweens.is_empty() {
            self.wait = WaitReason::None;
        }

        // 2. Shakes (only reset/redraw when shakes were active or clearing non-zero offsets)
        if !self.shakes.is_empty() {
            visual_changed = true;
            for layer in &mut self.layers {
                layer.shake_offset = [0.0, 0.0];
            }
            self.dialogue_shake = [0.0, 0.0];
            let mut s_idx = 0;
            while s_idx < self.shakes.len() {
                self.shakes[s_idx].elapsed_us = if reduced_motion {
                    self.shakes[s_idx].spec.duration_us
                } else {
                    self.shakes[s_idx].elapsed_us.saturating_add(delta_us)
                };
                let s = &self.shakes[s_idx];
                if s.elapsed_us >= s.spec.duration_us {
                    self.shakes.remove(s_idx);
                } else {
                    let offset = s.spec.sample(s.elapsed_us, s.seed);
                    match s.target {
                        TweenTarget::Layer(lid) => {
                            if let Some(layer) = self.layers.iter_mut().find(|l| l.id == lid) {
                                layer.shake_offset = offset;
                            }
                        }
                        TweenTarget::DialogueBox => self.dialogue_shake = offset,
                        TweenTarget::Audio(_) => {}
                    }
                    s_idx += 1;
                }
            }
            if self.wait == WaitReason::Shake && self.shakes.is_empty() {
                self.wait = WaitReason::None;
            }
        }

        // 2b. Timelines (60Hz keyframed sequences)
        if !self.timelines.is_empty() {
            let mut tl_idx = 0;
            while tl_idx < self.timelines.len() {
                let tl_id = self.timelines[tl_idx].timeline;
                let (fps, duration_frames) = self
                    .program
                    .timelines
                    .get(tl_id.0 as usize)
                    .map_or((60.0, 0), |t| (t.fps.max(1.0), t.duration_frames));
                let total_us = ((duration_frames as f64 / fps as f64) * 1_000_000.0).ceil() as u64;
                if self.timelines[tl_idx].elapsed_us >= total_us
                    && !self.timelines[tl_idx].scrap_on_end
                {
                    let done_layer = self.timelines[tl_idx].layer;
                    if matches!(self.wait, WaitReason::Timeline { layer, .. } if layer == done_layer)
                    {
                        self.wait = WaitReason::None;
                    }
                    tl_idx += 1;
                    continue;
                }
                visual_changed = true;
                self.timelines[tl_idx].elapsed_us = if reduced_motion {
                    total_us
                } else {
                    self.timelines[tl_idx]
                        .elapsed_us
                        .saturating_add(delta_us)
                        .min(total_us)
                };
                let cur_frame =
                    ((self.timelines[tl_idx].elapsed_us as f64 / 1_000_000.0) * fps as f64) as u32;
                let mut tl_tmp = self.timelines[tl_idx].clone();
                self.trigger_timeline_sounds(&mut tl_tmp, cur_frame);
                self.timelines[tl_idx] = tl_tmp;
                if self.timelines[tl_idx].elapsed_us >= total_us {
                    let done_layer = self.timelines[tl_idx].layer;
                    if self.timelines[tl_idx].scrap_on_end {
                        self.timelines.remove(tl_idx);
                        self.layers
                            .retain(|l| l.id != done_layer && l.parent != Some(done_layer));
                        self.pending_layers
                            .retain(|l| l.id != done_layer && l.parent != Some(done_layer));
                    } else {
                        tl_idx += 1;
                    }
                    if matches!(self.wait, WaitReason::Timeline { layer, .. } if layer == done_layer)
                    {
                        self.wait = WaitReason::None;
                    }
                } else {
                    tl_idx += 1;
                }
            }
        }

        // 3. Stage transition
        if let Some(tr) = &mut self.transition {
            visual_changed = true;
            tr.elapsed_us = if reduced_motion {
                tr.duration_us
            } else {
                tr.elapsed_us.saturating_add(delta_us)
            };
            if tr.elapsed_us >= tr.duration_us {
                self.transition = None;
                if self.wait == WaitReason::StageTransition {
                    self.wait = WaitReason::None;
                }
            }
        }

        // 4. Text window reveal
        if let Some(rev) = &mut self.window_reveal {
            visual_changed = true;
            rev.elapsed_us = if reduced_motion {
                rev.duration_us
            } else {
                rev.elapsed_us.saturating_add(delta_us)
            };
            if rev.elapsed_us >= rev.duration_us {
                self.dialogue_visible = rev.to_visible;
                self.window_reveal = None;
            }
        }

        // 5. Dialogue reveal (only mark visual_changed when visible_graphemes or finished actually changes!)
        if let Some(dlg) = &mut self.dialogue {
            if !dlg.finished && !dlg.paused_for_click {
                let prev_vis = dlg.visible_graphemes;
                let prev_fin = dlg.finished;
                let mut rem = delta_us;
                if dlg.pause_remaining_us > 0 {
                    let consume = rem.min(dlg.pause_remaining_us);
                    dlg.pause_remaining_us -= consume;
                    rem -= consume;
                }
                if rem > 0 && dlg.pause_remaining_us == 0 {
                    if text_speed <= 0.0 || dlg.reveal_us == 0 {
                        dlg.visible_graphemes = dlg.total_graphemes;
                        dlg.visible_bytes = dlg.full_text.len();
                        dlg.finished = true;
                    } else {
                        let step_us = ((dlg.reveal_us as f64) / (text_speed as f64).max(0.1))
                            .max(1000.0) as u64;
                        dlg.elapsed_us = dlg.elapsed_us.saturating_add(rem);
                        let advance = (dlg.elapsed_us / step_us) as usize;
                        dlg.elapsed_us %= step_us;
                        if advance > 0 {
                            let target = (dlg.visible_graphemes + advance).min(dlg.total_graphemes);
                            // Check inline pauses encountered along the way.
                            let mut stopped = target;
                            for pause in &mut dlg.pauses {
                                if !pause.triggered
                                    && pause.grapheme_index > dlg.visible_graphemes
                                    && pause.grapheme_index <= target
                                {
                                    pause.triggered = true;
                                    stopped = pause.grapheme_index;
                                    if pause.click {
                                        dlg.paused_for_click = true;
                                    } else {
                                        dlg.pause_remaining_us = pause.duration_us;
                                    }
                                    break;
                                }
                            }
                            let actual_delta = stopped.saturating_sub(dlg.visible_graphemes);
                            dlg.visible_graphemes = stopped;
                            if dlg.visible_graphemes >= dlg.total_graphemes && !dlg.paused_for_click
                            {
                                dlg.finished = true;
                            }
                            dlg.advance_visible_bytes(actual_delta);
                        }
                    }
                }
                if dlg.visible_graphemes != prev_vis || dlg.finished != prev_fin {
                    visual_changed = true;
                }
            }
        }

        // 6. Wait timer (pure wait does NOT alter visuals until it finishes & steps VM)
        if let WaitReason::Timer {
            remaining_us,
            skippable,
        } = self.wait
        {
            if delta_us >= remaining_us {
                self.wait = WaitReason::None;
            } else {
                self.wait = WaitReason::Timer {
                    remaining_us: remaining_us - delta_us,
                    skippable,
                };
            }
        }

        // 7. Choice timeout
        if let Some(choice) = &mut self.choice {
            if let Some(timeout) = choice.timeout_us {
                choice.elapsed_us = choice.elapsed_us.saturating_add(delta_us);
                if choice.elapsed_us >= timeout {
                    let idx = choice.default_idx.unwrap_or(0) as usize;
                    self.choose_internal(idx);
                    visual_changed = true;
                }
            }
        }

        (std::mem::take(&mut self.intents), visual_changed)
    }

    /// Handles player advance input (click / Space / Enter).
    pub fn advance(&mut self, stop_voice: bool) -> Vec<VmIntent> {
        self.intents.clear();
        match self.wait.clone() {
            WaitReason::Dialogue => {
                if let Some(dlg) = &mut self.dialogue {
                    if dlg.paused_for_click {
                        dlg.paused_for_click = false;
                        return Vec::new();
                    }
                    if !dlg.finished {
                        // First click finishes revealing current text immediately!
                        dlg.visible_graphemes = dlg.total_graphemes;
                        dlg.visible_bytes = dlg.full_text.len();
                        dlg.pause_remaining_us = 0;
                        dlg.finished = true;
                        return Vec::new();
                    }
                    // Mark text read in persistent profile.
                    if self.profile.read_texts.insert(dlg.text_id.0) {
                        self.intents.push(VmIntent::ProfileChanged);
                    }
                    if stop_voice && dlg.voice.is_some() {
                        let ch = ChannelId(1);
                        self.audio.retain(|c| c.channel != ch);
                        self.intents.push(VmIntent::StopAudio {
                            channel: ch,
                            fade_out_us: 50_000,
                        });
                    }
                }
                self.wait = WaitReason::None;
            }
            WaitReason::StageTransition => {
                self.transition = None;
                self.wait = WaitReason::None;
            }
            WaitReason::Tween => {
                let pending = std::mem::take(&mut self.tweens);
                for t in pending {
                    self.write_prop(t.target, t.prop, t.to);
                }
                self.wait = WaitReason::None;
            }
            WaitReason::Shake => {
                self.shakes.clear();
                for l in &mut self.layers {
                    l.shake_offset = [0.0, 0.0];
                }
                self.dialogue_shake = [0.0, 0.0];
                self.wait = WaitReason::None;
            }
            WaitReason::Timer {
                skippable: true, ..
            } => {
                self.wait = WaitReason::None;
            }
            WaitReason::Video {
                skippable: true, ..
            } => {
                self.intents.push(VmIntent::StopVideo);
                self.wait = WaitReason::None;
            }
            WaitReason::Timeline {
                layer,
                skippable: true,
            } => {
                if let Some(pos) = self.timelines.iter().position(|t| t.layer == layer) {
                    if self.timelines[pos].scrap_on_end {
                        let done = self.timelines.remove(pos);
                        self.layers
                            .retain(|l| l.id != done.layer && l.parent != Some(done.layer));
                        self.pending_layers
                            .retain(|l| l.id != done.layer && l.parent != Some(done.layer));
                    } else {
                        let tl_id = self.timelines[pos].timeline;
                        let (fps, duration_frames) = self
                            .program
                            .timelines
                            .get(tl_id.0 as usize)
                            .map_or((60.0, 0), |t| (t.fps.max(1.0), t.duration_frames));
                        let total_us =
                            ((duration_frames as f64 / fps as f64) * 1_000_000.0).ceil() as u64;
                        self.timelines[pos].elapsed_us = total_us;
                    }
                }
                self.wait = WaitReason::None;
            }
            _ => {}
        }
        std::mem::take(&mut self.intents)
    }

    pub fn notify_video_finished(&mut self) {
        if matches!(self.wait, WaitReason::Video { .. }) {
            self.wait = WaitReason::None;
        }
    }

    pub fn notify_modal_closed(&mut self) {
        if matches!(self.wait, WaitReason::Modal(_)) {
            self.wait = WaitReason::None;
        }
    }

    pub fn notify_audio_ended(&mut self, channel: ChannelId) {
        self.audio.retain(|c| !(c.channel == channel && !c.looped));
    }

    pub fn choose(&mut self, offered_idx: usize) -> Vec<VmIntent> {
        self.intents.clear();
        self.choose_internal(offered_idx);
        std::mem::take(&mut self.intents)
    }

    fn choose_internal(&mut self, offered_idx: usize) {
        let Some(choice) = self.choice.take() else {
            return;
        };
        let Some(item) = choice.items.get(offered_idx) else {
            self.choice = Some(choice);
            return;
        };
        if item.disabled {
            self.choice = Some(choice);
            return;
        }
        if let Some(se) = choice.click_se {
            self.play_audio_internal(ChannelId(7), AudioBus::Sfx, se, 1.0, false, 0, None, None);
        }
        if let (Some(var), Some(val)) = (choice.result_var, &item.value) {
            if let Some(slot) = self.vars.get_mut(var.0 as usize) {
                *slot = val.clone();
            }
        }
        if let (Some(var), Some(idx_val)) = (choice.index_var, &item.choice_index) {
            if let Some(slot) = self.vars.get_mut(var.0 as usize) {
                *slot = idx_val.clone();
            }
        }
        self.pc = item.target_pc;
        self.wait = WaitReason::None;
    }

    pub fn cancel_choice(&mut self) {
        if let Some(choice) = self.choice.take() {
            if let Some(cancel_pc) = choice.cancel_pc {
                if let Some(var) = choice.result_var {
                    if let Some(slot) = self.vars.get_mut(var.0 as usize) {
                        *slot = Value::Str(String::new());
                    }
                }
                if let Some(var) = choice.index_var {
                    if let Some(slot) = self.vars.get_mut(var.0 as usize) {
                        *slot = Value::Int(-1);
                    }
                }
                self.pc = cancel_pc;
                self.wait = WaitReason::None;
            } else {
                self.choice = Some(choice);
            }
        }
    }

    /// Collects all assets actively needed by current layers, transitions, audio, and choices.
    pub fn active_assets(&self) -> BTreeSet<AssetId> {
        let mut set = BTreeSet::new();
        for l in self.layers.iter().chain(self.pending_layers.iter()) {
            if let Some(a) = l.asset {
                set.insert(a);
            }
        }
        if let Some(tr) = &self.transition {
            for l in &tr.from_layers {
                if let Some(a) = l.asset {
                    set.insert(a);
                }
            }
            if let Some(mask) = tr.transition.mask_asset() {
                set.insert(mask);
            }
        }
        for ch in &self.audio {
            set.insert(ch.asset);
        }
        if let Some(c) = &self.choice {
            if let Some(bg) = c.background {
                set.insert(bg);
            }
            if let Some(se) = c.hover_se {
                set.insert(se);
            }
            if let Some(se) = c.click_se {
                set.insert(se);
            }
            for item in &c.items {
                if let Some(a) = item.image {
                    set.insert(a);
                }
                if let Some(a) = item.hover_image {
                    set.insert(a);
                }
                if let Some(a) = item.disabled_image {
                    set.insert(a);
                }
                if let Some((a, _)) = item.preview {
                    set.insert(a);
                }
            }
        }
        for tl in &self.timelines {
            if let Some(def) = self.program.timelines.get(tl.timeline.0 as usize) {
                for clip in &def.clips {
                    match &clip.kind {
                        TimelineClipKind::Image { asset, .. }
                        | TimelineClipKind::Sound { asset, .. } => {
                            set.insert(*asset);
                        }
                        TimelineClipKind::Box { .. } => {}
                    }
                }
            }
        }
        if let Some(p) = self.dialogue_portrait {
            set.insert(p);
        }
        if let Some(dlg) = &self.dialogue {
            for sp in &dlg.spans {
                if let Some(img) = sp.inline_image {
                    set.insert(img);
                }
            }
        }
        if let Some(style) = self
            .program
            .ui
            .dialogue_styles
            .get(self.dialogue_style as usize)
        {
            if let Some(bg) = style.dialogue_bg {
                set.insert(bg);
            }
        }
        if let Some(bg) = self.program.ui.dialogue_bg {
            set.insert(bg);
        }
        for &f in self
            .program
            .meta
            .ui_fonts
            .iter()
            .chain(self.program.meta.text_fonts.iter())
        {
            set.insert(f);
        }
        // Keep any delta keyframe base pinned alongside its active child patch!
        let bases: Vec<AssetId> = set
            .iter()
            .filter_map(|&id| self.program.asset(id)?.delta.as_ref().map(|d| d.base))
            .collect();
        set.extend(bases);
        set
    }

    /// Bounded multi-branch static lookahead over the next `max_ops` bytecode instructions
    /// (follows both arms of `JumpIfFalse` up to depth 2, plus `Choice`/`ChoiceMenu` targets
    /// and pulls delta keyframe bases ahead of their patches!).
    pub fn predict_assets(&self, max_ops: usize) -> Vec<AssetId> {
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();
        let mut visited = BTreeSet::new();
        let mut queue: VecDeque<(usize, u8)> = VecDeque::new();
        queue.push_back((self.pc as usize, 0));

        let push_asset = |id: AssetId, seen: &mut BTreeSet<AssetId>, out: &mut Vec<AssetId>| {
            if let Some(base) = self
                .program
                .asset(id)
                .and_then(|m| m.delta.as_ref())
                .map(|d| d.base)
            {
                if seen.insert(base) {
                    out.push(base);
                }
            }
            if seen.insert(id) {
                out.push(id);
            }
        };

        let mut steps = 0usize;
        while let Some((mut pc, depth)) = queue.pop_front() {
            while steps < max_ops {
                if !visited.insert(pc) {
                    break;
                }
                let Some(op) = self.program.ops.get(pc) else {
                    break;
                };
                steps += 1;
                match op {
                    Op::LayerSet { asset: Some(a), .. } | Op::LayerAsset { asset: Some(a), .. } => {
                        push_asset(*a, &mut seen, &mut out);
                    }
                    Op::StageCommit { transition, .. } => {
                        if let Some(m) = transition.mask_asset() {
                            push_asset(m, &mut seen, &mut out);
                        }
                    }
                    Op::Say { voice: Some(v), .. } => push_asset(*v, &mut seen, &mut out),
                    Op::AudioPlay { asset, .. } | Op::VideoPlay { asset, .. } => {
                        push_asset(*asset, &mut seen, &mut out);
                    }
                    Op::SetDialoguePortrait(Some(a)) => push_asset(*a, &mut seen, &mut out),
                    Op::TimelinePlay { timeline, .. } => {
                        if let Some(def) = self.program.timelines.get(timeline.0 as usize) {
                            for clip in &def.clips {
                                match &clip.kind {
                                    TimelineClipKind::Image { asset, .. }
                                    | TimelineClipKind::Sound { asset, .. } => {
                                        push_asset(*asset, &mut seen, &mut out);
                                    }
                                    TimelineClipKind::Box { .. } => {}
                                }
                            }
                        }
                    }
                    Op::ChoiceMenu {
                        menu,
                        hover_se,
                        click_se,
                        ..
                    } => {
                        if let Some(se) = hover_se {
                            push_asset(*se, &mut seen, &mut out);
                        }
                        if let Some(se) = click_se {
                            push_asset(*se, &mut seen, &mut out);
                        }
                        if let Some(tmpl) = self.program.menus.get(menu.0 as usize) {
                            if let Some(bg) = tmpl.background {
                                push_asset(bg, &mut seen, &mut out);
                            }
                            for item in &tmpl.items {
                                for a in [
                                    item.image,
                                    item.hover_image,
                                    item.disabled_image,
                                    item.preview_image,
                                ]
                                .into_iter()
                                .flatten()
                                {
                                    push_asset(a, &mut seen, &mut out);
                                }
                            }
                        }
                    }
                    Op::Jump(target) => {
                        pc = *target as usize;
                        continue;
                    }
                    Op::JumpIfFalse(target) => {
                        if depth < 2 {
                            queue.push_back((*target as usize, depth + 1));
                        }
                    }
                    Op::Call { func, .. } => {
                        if let Some(f) = self.program.func(*func) {
                            if depth < 2 {
                                queue.push_back((pc + 1, depth + 1));
                            }
                            pc = f.entry_pc as usize;
                            continue;
                        }
                        break;
                    }
                    Op::Choice { items, .. } => {
                        for item in items {
                            if let Some(a) = item.image {
                                push_asset(a, &mut seen, &mut out);
                            }
                            if let Some(a) = item.hover_image {
                                push_asset(a, &mut seen, &mut out);
                            }
                            if depth < 2 {
                                queue.push_back((item.target_pc as usize, depth + 1));
                            }
                        }
                        break;
                    }
                    Op::End { .. } | Op::Ret { .. } => break,
                    _ => {}
                }
                pc += 1;
            }
            if steps >= max_ops {
                break;
            }
        }
        out
    }

    fn push_history(&mut self, entry: HistoryEntry) {
        if self.history.len() >= MAX_HISTORY + HISTORY_ROLLBACK_SLACK {
            self.history.pop_front();
        }
        self.history.push_back(Arc::new(entry));
        self.history_seq = self.history_seq.wrapping_add(1);
    }

    fn pop(&mut self) -> Result<Value> {
        self.stack
            .pop()
            .ok_or_else(|| NirError::new("E_STACK", format!("pc:{}", self.pc), "stack underflow"))
    }

    fn read_prop(&self, target: TweenTarget, prop: TweenProp) -> f32 {
        match target {
            TweenTarget::Layer(lid) => self
                .layers
                .iter()
                .find(|l| l.id == lid)
                .map_or(0.0, |l| read_layer_prop(&l.transform, prop)),
            TweenTarget::DialogueBox => self.dialogue_opacity,
            TweenTarget::Audio(ch) => self
                .audio
                .iter()
                .find(|c| c.channel == ch)
                .map_or(1.0, |c| c.envelope),
        }
    }

    fn write_prop(&mut self, target: TweenTarget, prop: TweenProp, value: f32) {
        match target {
            TweenTarget::Layer(lid) => {
                if let Some(layer) = self.layers.iter_mut().find(|l| l.id == lid) {
                    apply_layer_prop(&mut layer.transform, prop, value);
                }
            }
            TweenTarget::DialogueBox => {
                self.dialogue_opacity = value.clamp(0.0, 1.0);
            }
            TweenTarget::Audio(ch) => {
                if let Some(chan) = self.audio.iter_mut().find(|c| c.channel == ch) {
                    chan.envelope = value.clamp(0.0, 4.0);
                    let eff = chan.gain * chan.envelope;
                    self.intents.push(VmIntent::SetAudioGain {
                        channel: ch,
                        gain: eff,
                    });
                }
            }
        }
    }

    fn trigger_timeline_sounds(&mut self, active: &mut ActiveTimeline, cur_frame: u32) {
        let clips: Vec<(u32, AssetId, f32, AudioBus)> = self
            .program
            .timelines
            .get(active.timeline.0 as usize)
            .map(|def| {
                def.clips
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, c)| match &c.kind {
                        TimelineClipKind::Sound { asset, gain, bus }
                            if cur_frame >= c.start_frame
                                && !active.triggered_sounds.contains(&(idx as u32)) =>
                        {
                            Some((idx as u32, *asset, *gain, *bus))
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        for (idx, asset, gain, bus) in clips {
            active.triggered_sounds.push(idx);
            let ch = ChannelId(16 + (idx % 16));
            self.play_audio_internal(ch, bus, asset, gain, false, 0, None, None);
        }
    }

    fn eval_builtin(&mut self, func: BuiltinFn, args: Vec<Value>) -> Value {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Null);
        match func {
            BuiltinFn::ToInt => Value::Int(arg(0).as_i64().unwrap_or(0)),
            BuiltinFn::ToStr => Value::Str(arg(0).to_display_string()),
            BuiltinFn::CharLen => Value::Int(arg(0).to_display_string().chars().count() as i64),
            BuiltinFn::StrRepeat => {
                let s = arg(0).to_display_string();
                let n = arg(1).as_i64().unwrap_or(0).clamp(0, 1024) as usize;
                Value::Str(s.repeat(n))
            }
            BuiltinFn::AddDelimiter => {
                let delim = arg(0).to_display_string();
                let s = arg(1).to_display_string();
                if s.is_empty() {
                    Value::Str(String::new())
                } else {
                    Value::Str(format!("{s}{delim}"))
                }
            }
            BuiltinFn::ListPush => {
                let mut list = match arg(0) {
                    Value::List(v) => v,
                    Value::Null => Vec::new(),
                    Value::Str(s) if s.is_empty() => Vec::new(),
                    other => vec![other],
                };
                if list.len() < 4096 {
                    list.push(arg(1));
                }
                Value::List(list)
            }
            BuiltinFn::ListLen => match arg(0) {
                Value::List(v) => Value::Int(v.len() as i64),
                Value::Null => Value::Int(0),
                Value::Str(s) if s.is_empty() => Value::Int(0),
                _ => Value::Int(1),
            },
            BuiltinFn::ListGet => {
                let idx = arg(1).as_i64().unwrap_or(-1);
                if idx < 0 {
                    return Value::Str(String::new());
                }
                match arg(0) {
                    Value::List(v) => v
                        .get(idx as usize)
                        .cloned()
                        .unwrap_or_else(|| Value::Str(String::new())),
                    other if idx == 0 => other,
                    _ => Value::Str(String::new()),
                }
            }
            BuiltinFn::ListSet => {
                let idx = arg(1).as_i64().unwrap_or(-1);
                let mut list = match arg(0) {
                    Value::List(v) => v,
                    Value::Null => Vec::new(),
                    other => vec![other],
                };
                if idx >= 0 && (idx as usize) < 4096 {
                    let i = idx as usize;
                    if list.len() <= i {
                        list.resize(i + 1, Value::Null);
                    }
                    list[i] = arg(2);
                }
                Value::List(list)
            }
            BuiltinFn::ListJoin => {
                let delim = if args.len() >= 2 {
                    arg(1).to_display_string()
                } else {
                    "\r\n".to_owned()
                };
                match arg(0) {
                    Value::List(v) => Value::Str(
                        v.iter()
                            .map(Value::to_display_string)
                            .collect::<Vec<_>>()
                            .join(&delim),
                    ),
                    other => Value::Str(other.to_display_string()),
                }
            }
            BuiltinFn::ListSplit => {
                let src = arg(0).to_display_string();
                let delim = if args.len() >= 2 {
                    arg(1).to_display_string()
                } else {
                    "\r\n".to_owned()
                };
                if src.is_empty() {
                    Value::List(Vec::new())
                } else if delim.is_empty() {
                    Value::List(vec![Value::Str(src)])
                } else {
                    Value::List(
                        src.split(&delim)
                            .map(|s| Value::Str(s.to_owned()))
                            .collect(),
                    )
                }
            }
            BuiltinFn::ListCompact => match arg(0) {
                Value::List(v) => Value::List(
                    v.into_iter()
                        .filter(|el| !el.to_display_string().is_empty())
                        .collect(),
                ),
                other => other,
            },
            BuiltinFn::Min => {
                let a = arg(0).as_i64().unwrap_or(0);
                let b = arg(1).as_i64().unwrap_or(0);
                Value::Int(a.min(b))
            }
            BuiltinFn::Max => {
                let a = arg(0).as_i64().unwrap_or(0);
                let b = arg(1).as_i64().unwrap_or(0);
                Value::Int(a.max(b))
            }
            BuiltinFn::RandInt => {
                let n = arg(0).as_i64().unwrap_or(1);
                if n <= 1 {
                    Value::Int(0)
                } else {
                    Value::Int((self.rng.next_u64() % (n as u64)) as i64)
                }
            }
        }
    }
}

fn value_to_name_set(val: &Value) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    match val {
        Value::List(v) => {
            for item in v {
                let s = item.to_display_string();
                for part in s.split("\r\n").flat_map(|p| p.split('\n')) {
                    if !part.is_empty() {
                        set.insert(part.to_owned());
                    }
                }
            }
        }
        other => {
            let s = other.to_display_string();
            for part in s.split("\r\n").flat_map(|p| p.split('\n')) {
                if !part.is_empty() {
                    set.insert(part.to_owned());
                }
            }
        }
    }
    set
}

fn resolve_asset_path(lookup: &BTreeMap<String, AssetId>, raw: &str) -> Option<AssetId> {
    let mut norm = raw.replace('\\', "/");
    while norm.starts_with("./") {
        norm = norm[2..].to_owned();
    }
    while norm.starts_with('/') {
        norm.remove(0);
    }
    let lower = norm.to_lowercase();
    if let Some(&id) = lookup.get(&lower) {
        return Some(id);
    }
    if let Some(dot) = lower.rfind('.') {
        if let Some(&id) = lookup.get(&lower[..dot]) {
            return Some(id);
        }
    }
    for prefix in ["グラフィック/", "グラフィック/顔/", "サウンド/"] {
        let cand = format!("{prefix}{lower}");
        if let Some(&id) = lookup.get(&cand) {
            return Some(id);
        }
        if let Some(dot) = cand.rfind('.') {
            if let Some(&id) = lookup.get(&cand[..dot]) {
                return Some(id);
            }
        }
    }
    None
}

fn parse_solid_color(s: &str) -> Option<[f32; 4]> {
    let hex = s.strip_prefix('$').or_else(|| s.strip_prefix('#'))?;
    if hex.len() == 6 {
        let r = u8::from_str_radix(&hex[0..2], 16).ok()? as f32 / 255.0;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()? as f32 / 255.0;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()? as f32 / 255.0;
        Some([r, g, b, 1.0])
    } else if hex.len() == 8 {
        let r = u8::from_str_radix(&hex[0..2], 16).ok()? as f32 / 255.0;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()? as f32 / 255.0;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()? as f32 / 255.0;
        let a = u8::from_str_radix(&hex[6..8], 16).ok()? as f32 / 255.0;
        Some([r, g, b, a])
    } else {
        None
    }
}

fn read_layer_prop(t: &Transform2D, prop: TweenProp) -> f32 {
    match prop {
        TweenProp::X => t.x,
        TweenProp::Y => t.y,
        TweenProp::ScaleX => t.scale[0],
        TweenProp::ScaleY => t.scale[1],
        TweenProp::Rotation => t.rotation_deg,
        TweenProp::Opacity => t.opacity,
        TweenProp::Volume => 1.0,
    }
}

fn apply_layer_prop(t: &mut Transform2D, prop: TweenProp, v: f32) {
    if !v.is_finite() {
        return;
    }
    match prop {
        TweenProp::X => t.x = v,
        TweenProp::Y => t.y = v,
        TweenProp::ScaleX => t.scale[0] = v.max(0.0),
        TweenProp::ScaleY => t.scale[1] = v.max(0.0),
        TweenProp::Rotation => t.rotation_deg = v,
        TweenProp::Opacity => t.opacity = v.clamp(0.0, 1.0),
        TweenProp::Volume => {}
    }
}

fn eval_binop(op: BinOp, lhs: Value, rhs: Value) -> Result<Value> {
    match op {
        BinOp::Concat => Ok(Value::Str(format!(
            "{}{}",
            lhs.to_display_string(),
            rhs.to_display_string()
        ))),
        BinOp::BitAnd => Ok(Value::Int(
            lhs.as_i64().unwrap_or(0) & rhs.as_i64().unwrap_or(0),
        )),
        BinOp::BitOr => Ok(Value::Int(
            lhs.as_i64().unwrap_or(0) | rhs.as_i64().unwrap_or(0),
        )),
        BinOp::BitXor => Ok(Value::Int(
            lhs.as_i64().unwrap_or(0) ^ rhs.as_i64().unwrap_or(0),
        )),
        BinOp::Shl => {
            Ok(Value::Int(lhs.as_i64().unwrap_or(0).wrapping_shl(
                rhs.as_i64().unwrap_or(0).clamp(0, 63) as u32,
            )))
        }
        BinOp::Shr => {
            Ok(Value::Int(lhs.as_i64().unwrap_or(0).wrapping_shr(
                rhs.as_i64().unwrap_or(0).clamp(0, 63) as u32,
            )))
        }
        BinOp::And => Ok(Value::Bool(lhs.as_bool() && rhs.as_bool())),
        BinOp::Or => Ok(Value::Bool(lhs.as_bool() || rhs.as_bool())),
        BinOp::Eq => Ok(Value::Bool(values_eq(&lhs, &rhs))),
        BinOp::Ne => Ok(Value::Bool(!values_eq(&lhs, &rhs))),
        BinOp::Add => match (lhs, rhs) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a.saturating_add(b))),
            (Value::Str(a), b) => Ok(Value::Str(format!("{a}{}", b.to_display_string()))),
            (a, Value::Str(b)) => Ok(Value::Str(format!("{}{b}", a.to_display_string()))),
            (a, b) => Ok(Value::Float(
                a.as_f64().unwrap_or(0.0) + b.as_f64().unwrap_or(0.0),
            )),
        },
        BinOp::Sub => match (lhs, rhs) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a.saturating_sub(b))),
            (a, b) => Ok(Value::Float(
                a.as_f64().unwrap_or(0.0) - b.as_f64().unwrap_or(0.0),
            )),
        },
        BinOp::Mul => match (lhs, rhs) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a.saturating_mul(b))),
            (a, b) => Ok(Value::Float(
                a.as_f64().unwrap_or(0.0) * b.as_f64().unwrap_or(0.0),
            )),
        },
        BinOp::Div => match (lhs, rhs) {
            (Value::Int(a), Value::Int(b)) => {
                if b == 0 {
                    Err(NirError::new("E_MATH", "div", "division by zero"))
                } else {
                    Ok(Value::Int(a / b))
                }
            }
            (a, b) => {
                let bv = b.as_f64().unwrap_or(0.0);
                if bv == 0.0 {
                    Err(NirError::new("E_MATH", "div", "division by zero"))
                } else {
                    Ok(Value::Float(a.as_f64().unwrap_or(0.0) / bv))
                }
            }
        },
        BinOp::Rem => match (lhs, rhs) {
            (Value::Int(a), Value::Int(b)) => {
                if b == 0 {
                    Err(NirError::new("E_MATH", "rem", "remainder by zero"))
                } else {
                    Ok(Value::Int(a % b))
                }
            }
            (a, b) => {
                let bv = b.as_f64().unwrap_or(0.0);
                if bv == 0.0 {
                    Err(NirError::new("E_MATH", "rem", "remainder by zero"))
                } else {
                    Ok(Value::Float(a.as_f64().unwrap_or(0.0) % bv))
                }
            }
        },
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            let ord = match (&lhs, &rhs) {
                (Value::Int(a), Value::Int(b)) => a.cmp(b),
                (Value::Str(a), Value::Str(b)) => a.cmp(b),
                _ => lhs
                    .as_f64()
                    .unwrap_or(0.0)
                    .total_cmp(&rhs.as_f64().unwrap_or(0.0)),
            };
            let ok = match op {
                BinOp::Lt => ord.is_lt(),
                BinOp::Le => ord.is_le(),
                BinOp::Gt => ord.is_gt(),
                BinOp::Ge => ord.is_ge(),
                _ => false,
            };
            Ok(Value::Bool(ok))
        }
    }
}

fn values_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Int(x), Value::Float(y)) | (Value::Float(y), Value::Int(x)) => (*x as f64) == *y,
        (Value::Int(x), Value::Bool(y)) | (Value::Bool(y), Value::Int(x)) => *x == i64::from(*y),
        (Value::Int(x), Value::Str(s)) | (Value::Str(s), Value::Int(x)) => {
            s.trim().parse::<i64>().is_ok_and(|y| y == *x)
        }
        (Value::Null, Value::Str(s)) | (Value::Str(s), Value::Null) => s.is_empty(),
        (Value::Null, Value::Int(0)) | (Value::Int(0), Value::Null) => true,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_program() -> Arc<NirProgram> {
        Arc::new(NirProgram {
            features: 0,
            meta: GameMeta {
                game_id: "org.nir.test".into(),
                title: "Test".into(),
                version: "1.0".into(),
                stage_width: 1280,
                stage_height: 720,
                default_locale: "en".into(),
                locales: vec!["en".into()],
                entry_func: FuncId(0),
                save_slots: 6,
                default_reveal_us: 20_000,
                title_bg: None,
                title_bgm: None,
                ui_fonts: vec![],
                text_fonts: vec![],
            },
            strings: vec![
                "main".into(),
                "affection".into(),
                "Aki".into(),
                "start".into(),
                "good".into(),
            ],
            vars: vec![VarMeta {
                name: StrId(1),
                default: Value::Int(0),
            }],
            funcs: vec![FuncMeta {
                name: StrId(0),
                entry_pc: 0,
                param_count: 0,
                local_count: 0,
            }],
            labels: vec![LabelMeta {
                name: StrId(3),
                pc: 0,
                file: StrId(0),
                line: 1,
            }],
            ops: vec![
                // 0: set bg layer
                Op::LayerSet {
                    layer: LayerId(0),
                    parent: None,
                    asset: Some(AssetId(0)),
                    transform: Box::new(Transform2D::default()),
                },
                // 1: commit dissolve 200ms
                Op::StageCommit {
                    transition: Transition::Dissolve,
                    duration_us: 200_000,
                    wait: true,
                },
                // 2: say greeting
                Op::Say {
                    speaker: Some(StrId(2)),
                    speaker_color: None,
                    text: TextId(0),
                    voice: Some(AssetId(1)),
                    reveal_us: 10_000,
                    append: false,
                },
                // 3: affection += 2
                Op::LoadVar(VarId(0)),
                Op::PushInt(2),
                Op::BinOp(BinOp::Add),
                Op::StoreVar(VarId(0)),
                // 7: say score interpolation
                Op::Say {
                    speaker: Some(StrId(2)),
                    speaker_color: None,
                    text: TextId(1),
                    voice: None,
                    reveal_us: 0,
                    append: false,
                },
                // 8: end
                Op::End { outcome: StrId(4) },
            ],
            texts: BTreeMap::from([(
                "en".into(),
                vec![
                    TextEntry::plain("Hello!"),
                    TextEntry {
                        spans: vec![RichSpan::Text("Score: ".into()), RichSpan::Var(VarId(0))],
                    },
                ],
            )]),
            ui: UiTheme::default(),
            assets: vec![],
            asset_lookup: BTreeMap::new(),
            timelines: vec![],
            menus: vec![],
        })
    }

    #[test]
    fn vm_persistent_layers_reveal_snapshot_and_hot_reload() {
        let prog = sample_program();
        let mut vm = Vm::new(prog.clone(), None).unwrap();

        // Lookahead finds bg asset 0 and voice asset 1 before executing!
        let predicted = vm.predict_assets(16);
        assert_eq!(predicted, vec![AssetId(0), AssetId(1)]);

        // Step stops on StageCommit transition
        vm.step(1000).unwrap();
        assert_eq!(vm.wait, WaitReason::StageTransition);
        assert_eq!(vm.layers.len(), 1);
        assert_eq!(vm.layers[0].asset, Some(AssetId(0)));

        // Advance clock past 200ms dissolve
        vm.tick(250_000, 1.0, false);
        assert_eq!(vm.wait, WaitReason::None);

        // Next step hits Say("Hello!") with voice
        let intents = vm.step(1000).unwrap();
        assert!(intents.iter().any(|i| matches!(
            i,
            VmIntent::PlayAudio {
                asset: AssetId(1),
                ..
            }
        )));
        assert_eq!(vm.wait, WaitReason::Dialogue);
        assert_eq!(vm.dialogue.as_ref().unwrap().visible_text(), "");

        // Tick 30ms -> reveals 3 chars ("Hel")
        vm.tick(30_000, 1.0, false);
        assert_eq!(vm.dialogue.as_ref().unwrap().visible_text(), "Hel");

        // First advance completes reveal; second advance moves on
        vm.advance(false);
        assert!(vm.dialogue.as_ref().unwrap().finished);
        assert_eq!(vm.wait, WaitReason::Dialogue);
        vm.advance(true);
        assert_eq!(vm.wait, WaitReason::None);
        assert!(vm.profile.read_texts.contains(&0));

        // Next step updates affection=2 and shows interpolated text "Score: 2"
        vm.step(1000).unwrap();
        assert_eq!(vm.vars[0], Value::Int(2));
        assert_eq!(vm.dialogue.as_ref().unwrap().full_text, "Score: 2");

        // Snapshot & restore roundtrip
        let snap = vm.snapshot(123456);
        let (mut restored, _) = Vm::restore(prog.clone(), snap, vm.profile.clone()).unwrap();
        assert_eq!(restored.vars[0], Value::Int(2));
        assert_eq!(restored.layers.len(), 1);
        restored.advance(false); // complete text reveal
        restored.advance(false); // dismiss finished dialogue
        restored.step(1000).unwrap();
        assert_eq!(restored.outcome.as_deref(), Some("good"));
    }

    #[test]
    fn vm_builtins_dynamic_assets_menus_timelines_and_modals() {
        let mut lookup = BTreeMap::new();
        lookup.insert("gfx/hero.png".into(), AssetId(10));
        lookup.insert("sfx/click.wav".into(), AssetId(11));

        let prog = Arc::new(NirProgram {
            features: 0,
            meta: GameMeta {
                game_id: "org.nir.ext".into(),
                title: "Ext".into(),
                version: "1.0".into(),
                stage_width: 1280,
                stage_height: 720,
                default_locale: "en".into(),
                locales: vec!["en".into()],
                entry_func: FuncId(0),
                save_slots: 4,
                default_reveal_us: 0,
                title_bg: None,
                title_bgm: None,
                ui_fonts: vec![],
                text_fonts: vec![],
            },
            strings: vec![
                "main".into(),          // 0
                "list".into(),          // 1
                "choice_val".into(),    // 2
                "choice_idx".into(),    // 3
                "gfx\\HERO.PNG".into(), // 4
                "A".into(),             // 5
                "B".into(),             // 6
                "hidden".into(),        // 7
                "Start".into(),         // 8
                "Bonus".into(),         // 9
                "done".into(),          // 10
            ],
            vars: vec![
                VarMeta {
                    name: StrId(1),
                    default: Value::Null,
                },
                VarMeta {
                    name: StrId(2),
                    default: Value::Str(String::new()),
                },
                VarMeta {
                    name: StrId(3),
                    default: Value::Int(-1),
                },
            ],
            funcs: vec![FuncMeta {
                name: StrId(0),
                entry_pc: 0,
                param_count: 0,
                local_count: 2,
            }],
            labels: vec![],
            ops: vec![
                // ListPush + ListCompact + ResolveAsset + LayerSetDyn
                Op::LoadVar(VarId(0)),
                Op::PushStr(StrId(5)),
                Op::CallBuiltin {
                    func: BuiltinFn::ListPush,
                    argc: 2,
                },
                Op::PushStr(StrId(6)),
                Op::CallBuiltin {
                    func: BuiltinFn::ListPush,
                    argc: 2,
                },
                Op::StoreVar(VarId(0)),
                Op::PushStr(StrId(4)),
                Op::ResolveAsset,
                Op::LayerSetDyn {
                    layer: LayerId(0),
                    parent: None,
                    transform: Box::new(Transform2D::default()),
                },
                Op::LayerSetText {
                    layer: LayerId(1),
                    parent: None,
                    transform: Some(Box::new(Transform2D::default())),
                    text: Some(StrId(5)),
                    font_size: 24.0,
                    color: [1.0, 1.0, 1.0, 1.0],
                    align: TextAlign::Center,
                },
                // Play 30-frame 60Hz timeline with scrap_on_end + wait
                Op::TimelinePlay {
                    timeline: TimelineId(0),
                    layer: LayerId(2),
                    scrap_on_end: true,
                    wait: true,
                    click_skippable: false,
                },
                // ChoiceMenu filtering out "Bonus" via hidden_names
                Op::PushStr(StrId(9)),
                Op::PushNull,
                Op::ChoiceMenu {
                    menu: MenuId(0),
                    result_var: Some(VarId(1)),
                    index_var: Some(VarId(2)),
                    cancelable: false,
                    hover_se: None,
                    click_se: Some(AssetId(11)),
                    fade_in_us: 0,
                    fade_out_us: 0,
                    timeout_us: None,
                },
                // OpenModal(Gallery)
                Op::OpenModal(ModalKind::Gallery),
                Op::End { outcome: StrId(10) },
            ],
            texts: BTreeMap::from([("en".into(), vec![])]),
            ui: UiTheme::default(),
            assets: vec![],
            asset_lookup: lookup,
            timelines: vec![TimelineDef {
                fps: 60.0,
                size: [1280, 720],
                duration_frames: 30,
                clips: vec![TimelineClip {
                    start_frame: 0,
                    duration_frames: 30,
                    z_order: 1,
                    kind: TimelineClipKind::Image {
                        asset: AssetId(10),
                        pivot: [0.0, 0.0],
                        pos_start: [0.0, 0.0],
                        pos_end: [100.0, 0.0],
                        pos_easing: [Easing::Linear, Easing::Linear],
                        angle: [0.0, 0.0],
                        angle_easing: Easing::Linear,
                        scale_x: [1.0, 1.0],
                        scale_y: [1.0, 1.0],
                        scale_easing: [Easing::Linear, Easing::Linear],
                        opacity: [0.0, 1.0],
                        clip_start: None,
                        clip_end: None,
                    },
                }],
            }],
            menus: vec![MenuTemplate {
                size: [1280, 720],
                background: None,
                items: vec![
                    MenuItemDef {
                        name: StrId(8),
                        label: StrId(8),
                        rect: [100.0, 100.0, 200.0, 50.0],
                        image: Some(AssetId(10)),
                        hover_image: None,
                        disabled_image: None,
                        preview_image: None,
                        preview_pos: [0.0, 0.0],
                    },
                    MenuItemDef {
                        name: StrId(9),
                        label: StrId(9),
                        rect: [100.0, 170.0, 200.0, 50.0],
                        image: Some(AssetId(10)),
                        hover_image: None,
                        disabled_image: None,
                        preview_image: None,
                        preview_pos: [0.0, 0.0],
                    },
                ],
            }],
        });

        let mut vm = Vm::new(prog, None).unwrap();
        vm.step(1000).unwrap();
        assert_eq!(vm.layers.len(), 2);
        assert_eq!(vm.layers[0].asset, Some(AssetId(10)));
        assert_eq!(vm.layers[1].text.as_ref().unwrap().text, "A");
        assert!(matches!(
            vm.wait,
            WaitReason::Timeline {
                layer: LayerId(2),
                ..
            }
        ));

        // Advance past 500ms timeline -> finishes & advances to ChoiceMenu
        vm.tick(600_000, 1.0, false);
        assert!(vm.timelines.is_empty());
        assert_eq!(vm.wait, WaitReason::None);

        vm.step(1000).unwrap();
        assert_eq!(vm.wait, WaitReason::Choice);
        // "Bonus" was hidden via stack arg, so only "Start" is offered!
        let c = vm.choice.as_ref().unwrap();
        assert_eq!(c.items.len(), 1);
        assert_eq!(c.items[0].label, "Start");
        let intents = vm.choose(0);
        assert!(intents.iter().any(|i| matches!(
            i,
            VmIntent::PlayAudio {
                asset: AssetId(11),
                ..
            }
        )));
        assert_eq!(vm.vars[1], Value::Str("Start".into()));
        assert_eq!(vm.vars[2], Value::Int(0));

        // Next step hits OpenModal(Gallery)
        let intents2 = vm.step(1000).unwrap();
        assert!(intents2.contains(&VmIntent::OpenModal(ModalKind::Gallery)));
        assert_eq!(vm.wait, WaitReason::Modal(ModalKind::Gallery));
        vm.notify_modal_closed();
        vm.step(1000).unwrap();
        assert_eq!(vm.outcome.as_deref(), Some("done"));
    }

    #[test]
    fn predict_assets_follows_branches_and_orders_delta_base_first() {
        let mk_img = |id: u32, delta: Option<ImageDelta>| AssetMeta {
            id: AssetId(id),
            name: StrId(0),
            kind: AssetKind::Image,
            codec: MediaCodec::Webp,
            width: 800,
            height: 600,
            trim: None,
            duration_us: 0,
            sample_rate: 0,
            channels: 0,
            loop_region: None,
            offset: 0,
            size: 100,
            delta,
        };
        let prog = Arc::new(NirProgram {
            features: FEATURE_DELTA_IMAGE,
            meta: GameMeta {
                game_id: "org.nir.predict".into(),
                title: "Predict".into(),
                version: "1.0".into(),
                stage_width: 800,
                stage_height: 600,
                default_locale: "en".into(),
                locales: vec!["en".into()],
                entry_func: FuncId(0),
                save_slots: 4,
                default_reveal_us: 0,
                title_bg: None,
                title_bgm: None,
                ui_fonts: vec![],
                text_fonts: vec![],
            },
            strings: vec!["main".into()],
            vars: vec![],
            funcs: vec![FuncMeta {
                name: StrId(0),
                entry_pc: 0,
                param_count: 0,
                local_count: 0,
            }],
            labels: vec![],
            ops: vec![
                // pc 0: branch to pc 3 (else) or fall through to pc 1 (then)
                Op::PushBool(true),
                Op::JumpIfFalse(3),
                // then arm: uses delta child AssetId(1) whose base is AssetId(0)
                Op::LayerSet {
                    layer: LayerId(0),
                    parent: None,
                    asset: Some(AssetId(1)),
                    transform: Box::new(Transform2D::default()),
                },
                // else arm: uses AssetId(2)
                Op::LayerSet {
                    layer: LayerId(0),
                    parent: None,
                    asset: Some(AssetId(2)),
                    transform: Box::new(Transform2D::default()),
                },
                Op::End { outcome: StrId(0) },
            ],
            texts: BTreeMap::from([("en".into(), vec![])]),
            ui: UiTheme::default(),
            assets: vec![
                mk_img(0, None),
                mk_img(
                    1,
                    Some(ImageDelta {
                        base: AssetId(0),
                        x: 16,
                        y: 16,
                        tile_mask: vec![],
                    }),
                ),
                mk_img(2, None),
            ],
            asset_lookup: BTreeMap::new(),
            timelines: vec![],
            menus: vec![],
        });

        let vm = Vm::new(prog, None).unwrap();
        let pred = vm.predict_assets(16);
        assert!(pred.contains(&AssetId(0)), "delta base must be predicted");
        assert!(
            pred.contains(&AssetId(1)),
            "then-branch delta asset must be predicted"
        );
        assert!(
            pred.contains(&AssetId(2)),
            "else-branch asset must be predicted"
        );
        let p0 = pred.iter().position(|&a| a == AssetId(0)).unwrap();
        let p1 = pred.iter().position(|&a| a == AssetId(1)).unwrap();
        assert!(
            p0 < p1,
            "delta base must be prefetched ahead of delta patch"
        );
    }

    #[test]
    fn shake_stop_wildcard_clears_layers_and_dialogue_box() {
        let prog = Arc::new(NirProgram {
            features: 0,
            meta: GameMeta {
                game_id: "org.nir.shake".into(),
                title: "Shake".into(),
                version: "1.0".into(),
                stage_width: 800,
                stage_height: 600,
                default_locale: "en".into(),
                locales: vec!["en".into()],
                entry_func: FuncId(0),
                save_slots: 4,
                default_reveal_us: 0,
                title_bg: None,
                title_bgm: None,
                ui_fonts: vec![],
                text_fonts: vec![],
            },
            strings: vec!["main".into()],
            vars: vec![],
            funcs: vec![FuncMeta {
                name: StrId(0),
                entry_pc: 0,
                param_count: 0,
                local_count: 0,
            }],
            labels: vec![],
            ops: vec![
                Op::LayerSet {
                    layer: LayerId(1),
                    parent: None,
                    asset: None,
                    transform: Box::new(Transform2D::default()),
                },
                Op::Shake {
                    target: TweenTarget::Layer(LayerId(1)),
                    spec: ShakeSpec {
                        amplitude: [12.0, 8.0],
                        freq_hz: 20.0,
                        duration_us: 2_000_000,
                        decay: false,
                        mode: ShakeMode::Wave,
                    },
                    wait: false,
                },
                Op::Shake {
                    target: TweenTarget::DialogueBox,
                    spec: ShakeSpec {
                        amplitude: [10.0, 6.0],
                        freq_hz: 20.0,
                        duration_us: 2_000_000,
                        decay: false,
                        mode: ShakeMode::Wave,
                    },
                    wait: false,
                },
                Op::Wait {
                    duration_us: 100_000,
                    skippable: false,
                },
                Op::ShakeStop {
                    target: TweenTarget::Layer(LayerId(u32::MAX)),
                },
                Op::End { outcome: StrId(0) },
            ],
            texts: BTreeMap::from([("en".into(), vec![])]),
            ui: UiTheme::default(),
            assets: vec![],
            asset_lookup: BTreeMap::new(),
            timelines: vec![],
            menus: vec![],
        });

        let mut vm = Vm::new(prog, None).unwrap();
        vm.step(64).unwrap();
        assert_eq!(vm.shakes.len(), 2);
        // Tick partway through the shake so offsets become non-zero
        vm.tick(12_500, 1.0, false);
        assert!(vm.layers[0].shake_offset[0].abs() > 0.1 || vm.dialogue_shake[0].abs() > 0.1);
        // Finish the Wait and hit wildcard ShakeStop
        vm.tick(100_000, 1.0, false);
        vm.step(64).unwrap();
        assert!(vm.shakes.is_empty());
        assert_eq!(vm.layers[0].shake_offset, [0.0, 0.0]);
        assert_eq!(vm.dialogue_shake, [0.0, 0.0]);
    }
}
