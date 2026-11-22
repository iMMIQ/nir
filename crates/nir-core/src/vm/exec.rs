use super::*;

impl Core {
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
    pub(super) fn process(&mut self, input: CoreInput) -> Result<()> {
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
    pub(super) fn trim_history(&mut self) {
        const MAX_HISTORY_BYTES: usize = 4 * 1024 * 1024;
        const MAX_HISTORY_LEN: usize = 1000;
        let mut drop_count = 0usize;
        let len = self.state.history.len();
        while len - drop_count > MAX_HISTORY_LEN || self.history_bytes > MAX_HISTORY_BYTES {
            self.history_bytes = self
                .history_bytes
                .saturating_sub(history_bytes(&self.state.history[drop_count]));
            drop_count += 1;
        }
        if drop_count > 0 {
            self.state.history.drain(0..drop_count);
        }
    }
    pub(super) fn push_history(&mut self, entry: HistoryEntry) {
        // This is derived reading data, not a gameplay limit. Oversized rows
        // are discarded whole without evicting an otherwise valid backlog.
        let added = history_bytes(&entry);
        if added > 4 * 1024 * 1024 {
            return;
        }
        self.state.history.push(entry);
        self.history_bytes = self.history_bytes.saturating_add(added);
        self.trim_history();
    }
    pub(super) fn choice_history(
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
    pub(super) fn run(&mut self) -> Result<()> {
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
        let prog = self.program.clone();
        let mut checked_block: Option<(String, String, usize)> = None;
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
            // Within an already-verified non-terminator block, `self.program`
            // does not change during `run()`, so skip redundant residency locks.
            let frame = self.frame();
            let same_verified_block = checked_block.as_ref().is_some_and(|(f, b, op_len)| {
                f == &frame.function && b == &frame.block && frame.op < *op_len
            });
            if !same_verified_block {
                if let Some(module) = self.missing_content()? {
                    self.intents.push(CoreIntent::PrepareContent {
                        module,
                        locale: self.state.locale.clone(),
                    });
                    return Ok(());
                }
                let frame = self.frame();
                let op_len = prog.program().functions[&frame.function].blocks[&frame.block]
                    .ops
                    .len();
                checked_block = if frame.op < op_len {
                    Some((frame.function.clone(), frame.block.clone(), op_len))
                } else {
                    None
                };
            }
            self.work_remaining -= 1;
            self.state.unsuspended_ops += 1;
            if self.state.unsuspended_ops > 1_000_000 {
                return Err(self.error("E_FUEL", "non-suspending loop"));
            }
            let frame = self.frame();
            match prog.instruction(&frame.function, &frame.block, frame.op) {
                crate::validate::Instruction::Op(op) => {
                    self.execute_op(&op.operation)?;
                    if self.tracing {
                        self.trace(format!("op:{}", op.id));
                    }
                    self.frame_mut().op += 1;
                    self.sync_op_id();
                }
                crate::validate::Instruction::Term(t) => {
                    checked_block = None;
                    self.terminate(t.clone())?;
                }
            }
        }
        Ok(())
    }
    pub(super) fn missing_content(&self) -> Result<Option<String>> {
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
    pub(super) fn execute_op(&mut self, op: &Operation) -> Result<()> {
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
                            let before = history_bytes(entry);
                            entry.voices.push(voice);
                            self.history_bytes = self
                                .history_bytes
                                .saturating_add(history_bytes(entry).saturating_sub(before));
                        }
                    }
                    // History is a bounded derived view, never a new reason
                    // for valid authored dialogue to fault. Evict a record
                    // exceeding its budget rather than publish partial audio.
                    let had_excess = self
                        .state
                        .history
                        .iter()
                        .any(|h| h.voices.len() > MAX_HISTORY_VOICES);
                    if had_excess {
                        self.state
                            .history
                            .retain(|h| h.voices.len() <= MAX_HISTORY_VOICES);
                        self.history_bytes = self.state.history.iter().map(history_bytes).sum();
                    }
                    self.trim_history();
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
    pub(super) fn terminate(&mut self, t: Terminator) -> Result<()> {
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
}
