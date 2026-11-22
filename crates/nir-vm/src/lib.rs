//! High-performance NIR v2 flat-bytecode VM with Persistent Scene Graph,
//! unified Tween/Shake/Transition evaluation, hot-reload, and lookahead prefetching.
#![forbid(unsafe_code)]

pub mod state;
pub use state::*;

use nir_ir::*;
use rand_chacha::ChaCha8Rng;
use rand_core::{RngCore, SeedableRng};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use unicode_segmentation::UnicodeSegmentation;

const MAX_HISTORY: usize = 500;
const MAX_CALL_DEPTH: usize = 64;
const MAX_FUEL_PER_STEP: u32 = 200_000;
const RNG_SEED: [u8; 32] = [0x4E, 0x49, 0x52, 0x32, 42, 7, 13, 99, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24];

#[derive(Clone)]
pub struct Vm {
    program: Arc<NirProgram>,
    pub pc: u32,
    last_label: Option<String>,
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
    pub transition: Option<StageTransitionState>,
    pub window_reveal: Option<WindowRevealState>,
    pub dialogue_visible: bool,
    pub dialogue_opacity: f32,
    pub dialogue_shake: [f32; 2],
    pub dialogue: Option<DialogueState>,
    pub choice: Option<ChoiceState>,
    pub wait: WaitReason,
    pub audio: Vec<AudioChannelState>,
    pub audio_paused: [bool; 3],
    pub history: Vec<HistoryEntry>,
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

    pub fn new_at(
        program: Arc<NirProgram>,
        func: FuncId,
        locale: Option<String>,
    ) -> Result<Self> {
        let fmeta = program
            .func(func)
            .ok_or_else(|| NirError::new("E_VM", "entry", "missing entry function"))?;
        let locale = locale
            .filter(|l| program.texts.contains_key(l) || program.meta.locales.contains(l))
            .unwrap_or_else(|| program.meta.default_locale.clone());
        let vars = program.vars.iter().map(|v| v.default.clone()).collect();
        let entry_pc = fmeta.entry_pc;
        let locals = vec![Value::Null; fmeta.local_count as usize];
        Ok(Self {
            program,
            pc: entry_pc,
            last_label: None,
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
            transition: None,
            window_reveal: None,
            dialogue_visible: true,
            dialogue_opacity: 1.0,
            dialogue_shake: [0.0, 0.0],
            dialogue: None,
            choice: None,
            wait: WaitReason::None,
            audio: Vec::new(),
            audio_paused: [false; 3],
            history: Vec::new(),
            locale,
            profile: ProfileData::default(),
            rng: ChaCha8Rng::from_seed(RNG_SEED),
            outcome: None,
            intents: Vec::new(),
        })
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
        !self.tweens.is_empty()
            || !self.shakes.is_empty()
            || self.transition.is_some()
            || self.window_reveal.is_some()
            || matches!(self.wait, WaitReason::Timer { .. })
            || self
                .dialogue
                .as_ref()
                .is_some_and(|d| !d.finished && !d.paused_for_click)
            || self
                .choice
                .as_ref()
                .is_some_and(|c| c.timeout_us.is_some())
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
            self.last_label = Some(label_name.to_owned());
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
        let new_pc = self
            .last_label
            .as_deref()
            .and_then(|name| new_program.find_label(name))
            .map(|l| l.pc)
            .unwrap_or_else(|| self.pc.min(new_program.ops.len().saturating_sub(1) as u32));

        self.program = new_program;
        self.vars = next_vars;
        self.pc = new_pc;
        self.wait = WaitReason::None;
        self.dialogue = None;
        self.choice = None;
        self.stack.clear();
    }

    /// Captures a compact, version-resilient snapshot.
    pub fn snapshot(&self, unix_time: u64) -> SaveSnapshot {
        let vars = self
            .program
            .vars
            .iter()
            .zip(self.vars.iter())
            .map(|(m, v)| (self.program.str(m.name).to_owned(), v.clone()))
            .collect();
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
            .or_else(|| self.last_label.clone())
            .unwrap_or_else(|| self.program.meta.title.clone());
        SaveSnapshot {
            format: SAVE_FORMAT_VERSION,
            game_id: self.program.meta.game_id.clone(),
            version: self.program.meta.version.clone(),
            saved_at_unix: unix_time,
            summary,
            pc: self.pc,
            last_label: self.last_label.clone(),
            vars,
            frames: self.frames.clone(),
            layers: self.layers.clone(),
            dialogue_visible: self.dialogue_visible,
            dialogue: self.dialogue.clone(),
            choice: self.choice.clone(),
            wait: self.wait.clone(),
            audio: self.audio.clone(),
            audio_paused: self.audio_paused,
            history: self.history.clone(),
            locale: self.locale.clone(),
            rng_word_pos: self.rng.get_word_pos(),
            outcome: self.outcome.clone(),
        }
    }

    /// Restores a snapshot cleanly, migrating variables by name and falling back to
    /// `last_label` if the script version changed.
    pub fn restore(
        program: Arc<NirProgram>,
        snap: SaveSnapshot,
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
                format!("save belongs to {}, not {}", snap.game_id, program.meta.game_id),
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
        let pc = if snap.version == program.meta.version
            && (snap.pc as usize) <= program.ops.len()
        {
            snap.pc
        } else if let Some(lbl) = snap
            .last_label
            .as_deref()
            .and_then(|n| program.find_label(n))
        {
            lbl.pc
        } else {
            snap.pc.min(program.ops.len().saturating_sub(1) as u32)
        };

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

        let vm = Self {
            program,
            pc,
            last_label: snap.last_label,
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
            transition: None,
            window_reveal: None,
            dialogue_visible: snap.dialogue_visible,
            dialogue_opacity: 1.0,
            dialogue_shake: [0.0, 0.0],
            dialogue: snap.dialogue,
            choice: snap.choice,
            wait: snap.wait,
            audio: snap.audio,
            audio_paused: snap.audio_paused,
            history: snap.history,
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

        while self.outcome.is_none() && self.wait == WaitReason::None && used < limit {
            let pc = self.pc as usize;
            let Some(op) = self.program.ops.get(pc).cloned() else {
                self.outcome = Some("end".into());
                break;
            };
            // Track latest visited label for hot-reload / save migration.
            if let Some(lbl) = self.program.labels.iter().find(|l| l.pc == self.pc) {
                self.last_label = Some(self.program.str(lbl.name).to_owned());
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

    fn exec_op(&mut self, op: Op) -> Result<()> {
        match op {
            Op::PushNull => self.stack.push(Value::Null),
            Op::PushBool(b) => self.stack.push(Value::Bool(b)),
            Op::PushInt(n) => self.stack.push(Value::Int(n)),
            Op::PushFloat(f) => self.stack.push(Value::Float(f)),
            Op::PushStr(id) => self.stack.push(Value::Str(self.program.str(id).to_owned())),
            Op::LoadVar(id) => {
                let v = self
                    .vars
                    .get(id.0 as usize)
                    .cloned()
                    .unwrap_or(Value::Null);
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
                    .and_then(|f| f.locals.get(slot as usize))
                    .cloned()
                    .unwrap_or(Value::Null);
                self.stack.push(v);
            }
            Op::StoreLocal(slot) => {
                let v = self.pop()?;
                if let Some(frame) = self.frames.last_mut() {
                    if let Some(dst) = frame.locals.get_mut(slot as usize) {
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
                self.stack.push(eval_binop(bop, lhs, rhs)?);
            }
            Op::UnaryOp(uop) => {
                let v = self.pop()?;
                let res = match uop {
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
                self.pc = target;
            }
            Op::JumpIfFalse(target) => {
                let cond = self.pop()?;
                if !cond.as_bool() {
                    self.pc = target;
                }
            }
            Op::Call { func, argc } => {
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
                let ret_val = if has_value { Some(self.pop()?) } else { None };
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
                self.outcome = Some(self.program.str(outcome).to_owned());
            }

            // --- Persistent Scene Graph ---
            Op::LayerSet {
                layer,
                parent,
                asset,
                transform,
            } => {
                self.ensure_pending_synced();
                if let Some(existing) = self.pending_layers.iter_mut().find(|l| l.id == layer) {
                    existing.parent = parent;
                    if asset.is_some() {
                        existing.asset = asset;
                    }
                    existing.transform = *transform;
                } else {
                    self.pending_layers.push(LayerState {
                        id: layer,
                        parent,
                        asset,
                        transform: *transform,
                        shake_offset: [0.0, 0.0],
                    });
                }
                self.pending_dirty = true;
            }
            Op::LayerAsset { layer, asset } => {
                self.ensure_pending_synced();
                if let Some(existing) = self.pending_layers.iter_mut().find(|l| l.id == layer) {
                    existing.asset = asset;
                } else {
                    self.pending_layers.push(LayerState {
                        id: layer,
                        parent: None,
                        asset,
                        transform: Transform2D::default(),
                        shake_offset: [0.0, 0.0],
                    });
                }
                self.pending_dirty = true;
            }
            Op::LayerPatch { layer, prop, value } => {
                self.ensure_pending_synced();
                if let Some(existing) = self.pending_layers.iter_mut().find(|l| l.id == layer) {
                    apply_layer_prop(&mut existing.transform, prop, value);
                    self.pending_dirty = true;
                }
            }
            Op::LayerRemove { layer } => {
                self.ensure_pending_synced();
                self.pending_layers
                    .retain(|l| l.id != layer && l.parent != Some(layer));
                self.tweens
                    .retain(|t| !matches!(t.target, TweenTarget::Layer(id) if id == layer));
                self.shakes
                    .retain(|s| !matches!(s.target, TweenTarget::Layer(id) if id == layer));
                self.pending_dirty = true;
            }
            Op::LayerClear => {
                self.pending_layers.clear();
                self.tweens
                    .retain(|t| !matches!(t.target, TweenTarget::Layer(_)));
                self.shakes
                    .retain(|s| !matches!(s.target, TweenTarget::Layer(_)));
                self.pending_dirty = true;
            }
            Op::StageCommit {
                transition,
                duration_us,
                wait,
            } => {
                self.commit_staged_if_dirty(transition, duration_us, wait);
            }
            Op::Tween {
                target,
                prop,
                to,
                duration_us,
                easing,
                wait,
            } => {
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
                // Auto-commit any staged scene edits cleanly before displaying dialogue.
                if self.pending_dirty {
                    self.commit_staged_if_dirty(Transition::Cut, 0, false);
                }
                let speaker_str = speaker
                    .map(|id| self.program.str(id).to_owned())
                    .unwrap_or_default();
                let entry = self
                    .program
                    .text(&self.locale, text)
                    .cloned()
                    .unwrap_or_else(|| TextEntry::plain(""));

                let (mut full_text, mut spans, mut pauses) = self.instantiate_text(&entry);
                let prev_graphemes = if append {
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
                        prefix_graphemes
                    } else {
                        0
                    }
                } else {
                    0
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
                if self.pending_dirty {
                    self.commit_staged_if_dirty(Transition::Cut, 0, false);
                }
                let mut offered = Vec::new();
                for (i, item) in items.iter().enumerate() {
                    if let Some(cvar) = item.cond_var {
                        let visible = self
                            .vars
                            .get(cvar.0 as usize)
                            .is_some_and(Value::as_bool);
                        if !visible {
                            continue;
                        }
                    }
                    let label = self
                        .program
                        .text(&self.locale, item.text)
                        .map(|e| self.instantiate_text(e).0)
                        .unwrap_or_default();
                    offered.push(OfferedChoice {
                        index: i as u16,
                        label,
                        image: item.image,
                        hover_image: item.hover_image,
                        rect: item.rect,
                        target_pc: item.target_pc,
                        value: item.value.clone(),
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
                    cancel_pc,
                    timeout_us,
                    elapsed_us: 0,
                    default_idx,
                });
                self.wait = WaitReason::Choice;
                self.intents.push(VmIntent::Checkpoint);
            }
            Op::Wait {
                duration_us,
                skippable,
            } => {
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
                    channel,
                    bus,
                    asset,
                    gain,
                    looped,
                    fade_in_us,
                    loop_region,
                    None,
                );
            }
            Op::AudioStop {
                channel,
                fade_out_us,
            } => {
                self.audio.retain(|c| c.channel != channel);
                self.intents.push(VmIntent::StopAudio {
                    channel,
                    fade_out_us,
                });
            }
            Op::AudioPause { bus, paused } => {
                self.audio_paused[bus as usize] = paused;
                self.intents.push(VmIntent::PauseAudioBus { bus, paused });
            }
            Op::VideoPlay { asset, skippable } => {
                if self.pending_dirty {
                    self.commit_staged_if_dirty(Transition::Cut, 0, false);
                }
                self.wait = WaitReason::Video { asset, skippable };
                self.intents.push(VmIntent::PlayVideo { asset, skippable });
            }

            // --- Profile / Unlocks ---
            Op::ProfileSetFlag(key) => {
                let s = self.program.str(key).to_owned();
                if self.profile.flags.insert(s) {
                    self.intents.push(VmIntent::ProfileChanged);
                }
            }
            Op::ProfileGetFlag { dst, key } => {
                let flag = self.profile.flags.contains(self.program.str(key));
                if let Some(slot) = self.vars.get_mut(dst.0 as usize) {
                    *slot = Value::Bool(flag);
                }
            }
            Op::ProfileSetVal { key } => {
                let val = self.pop()?;
                let k = self.program.str(key).to_owned();
                self.profile.values.insert(k, val);
                self.intents.push(VmIntent::ProfileChanged);
            }
            Op::ProfileGetVal { dst, key } => {
                let val = self
                    .profile
                    .values
                    .get(self.program.str(key))
                    .cloned()
                    .unwrap_or(Value::Null);
                if let Some(slot) = self.vars.get_mut(dst.0 as usize) {
                    *slot = val;
                }
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
                RichSpan::InlineImage { .. } => {
                    let start = full.len();
                    full.push('\u{fffc}');
                    grapheme_cursor += 1;
                    spans.push(ResolvedSpan {
                        byte_start: start,
                        byte_end: full.len(),
                        emphasis: false,
                        color: None,
                        ruby: None,
                        speed: cur_speed,
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
        self.intents.clear();
        if delta_us == 0 || self.outcome.is_some() {
            return Vec::new();
        }

        // 1. Tweens
        let mut completed_tweens = Vec::new();
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
            if done {
                completed_tweens.push(self.tweens.remove(i));
            } else {
                i += 1;
            }
        }
        if self.wait == WaitReason::Tween && self.tweens.is_empty() {
            self.wait = WaitReason::None;
        }

        // 2. Shakes
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

        // 3. Stage transition
        if let Some(tr) = &mut self.transition {
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

        // 5. Dialogue reveal
        if let Some(dlg) = &mut self.dialogue {
            if !dlg.finished && !dlg.paused_for_click {
                let mut rem = delta_us;
                if dlg.pause_remaining_us > 0 {
                    let consume = rem.min(dlg.pause_remaining_us);
                    dlg.pause_remaining_us -= consume;
                    rem -= consume;
                }
                if rem > 0 && dlg.pause_remaining_us == 0 {
                    if text_speed <= 0.0 || dlg.reveal_us == 0 {
                        dlg.visible_graphemes = dlg.total_graphemes;
                        dlg.finished = true;
                    } else {
                        let step_us = ((dlg.reveal_us as f64) / (text_speed as f64).max(0.1))
                            .max(1000.0) as u64;
                        dlg.elapsed_us = dlg.elapsed_us.saturating_add(rem);
                        let advance = (dlg.elapsed_us / step_us) as usize;
                        dlg.elapsed_us %= step_us;
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
                        dlg.visible_graphemes = stopped;
                        if dlg.visible_graphemes >= dlg.total_graphemes && !dlg.paused_for_click {
                            dlg.finished = true;
                        }
                    }
                }
            }
        }

        // 6. Wait timer
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
                }
            }
        }

        std::mem::take(&mut self.intents)
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
            WaitReason::Timer { skippable: true, .. } => {
                self.wait = WaitReason::None;
            }
            WaitReason::Video { skippable: true, .. } => {
                self.intents.push(VmIntent::StopVideo);
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

    pub fn notify_audio_ended(&mut self, channel: ChannelId) {
        self.audio
            .retain(|c| !(c.channel == channel && !c.looped));
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
        if let (Some(var), Some(val)) = (choice.result_var, &item.value) {
            if let Some(slot) = self.vars.get_mut(var.0 as usize) {
                *slot = val.clone();
            }
        }
        self.pc = item.target_pc;
        self.wait = WaitReason::None;
    }

    pub fn cancel_choice(&mut self) {
        if let Some(cancel_pc) = self.choice.as_ref().and_then(|c| c.cancel_pc) {
            self.choice = None;
            self.pc = cancel_pc;
            self.wait = WaitReason::None;
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
            for item in &c.items {
                if let Some(a) = item.image {
                    set.insert(a);
                }
                if let Some(a) = item.hover_image {
                    set.insert(a);
                }
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
        set
    }

    /// Static lookahead over the next `max_ops` straight-line bytecode instructions
    /// so `nir-runtime` can asynchronously prefetch upcoming CGs, sprites, and voice clips!
    pub fn predict_assets(&self, max_ops: usize) -> Vec<AssetId> {
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();
        let mut pc = self.pc as usize;
        let mut visited = BTreeSet::new();
        for _ in 0..max_ops {
            if !visited.insert(pc) {
                break;
            }
            let Some(op) = self.program.ops.get(pc) else {
                break;
            };
            let mut add = |id: AssetId| {
                if seen.insert(id) {
                    out.push(id);
                }
            };
            match op {
                Op::LayerSet { asset: Some(a), .. } | Op::LayerAsset { asset: Some(a), .. } => {
                    add(*a)
                }
                Op::StageCommit { transition, .. } => {
                    if let Some(m) = transition.mask_asset() {
                        add(m);
                    }
                }
                Op::Say { voice: Some(v), .. } => add(*v),
                Op::AudioPlay { asset, .. } | Op::VideoPlay { asset, .. } => add(*asset),
                Op::Jump(target) => {
                    pc = *target as usize;
                    continue;
                }
                Op::Call { func, .. } => {
                    if let Some(f) = self.program.func(*func) {
                        pc = f.entry_pc as usize;
                        continue;
                    }
                    break;
                }
                Op::Choice { items, .. } => {
                    for item in items {
                        if let Some(a) = item.image {
                            add(a);
                        }
                        if let Some(a) = item.hover_image {
                            add(a);
                        }
                    }
                    break;
                }
                Op::End { .. } | Op::Ret { .. } => break,
                _ => {}
            }
            pc += 1;
        }
        out
    }

    fn push_history(&mut self, entry: HistoryEntry) {
        if self.history.len() >= MAX_HISTORY {
            self.history.remove(0);
        }
        self.history.push(entry);
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
                    chan.envelope = value.clamp(0.0, 1.0);
                    let eff = chan.gain * chan.envelope;
                    self.intents.push(VmIntent::SetAudioGain {
                        channel: ch,
                        gain: eff,
                    });
                }
            }
        }
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
            strings: vec!["main".into(), "affection".into(), "Aki".into(), "start".into(), "good".into()],
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
                        spans: vec![
                            RichSpan::Text("Score: ".into()),
                            RichSpan::Var(VarId(0)),
                        ],
                    },
                ],
            )]),
            ui: UiTheme::default(),
            assets: vec![],
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
        assert!(intents
            .iter()
            .any(|i| matches!(i, VmIntent::PlayAudio { asset: AssetId(1), .. })));
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
}
