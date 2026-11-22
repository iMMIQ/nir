use super::*;

impl Core {
    pub fn restore(program: ValidatedProgram, s: Snapshot, release: &str) -> Result<Self> {
        Self::restore_inner(program, s, release, false)
    }
    pub(super) fn task_audio_paused(&self, task: &Task) -> bool {
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
    pub(super) fn restore_inner(
        program: ValidatedProgram,
        s: Snapshot,
        release: &str,
        canonical_verified: bool,
    ) -> Result<Self> {
        let fail = |msg: &str| Diagnostic::new("E_SNAPSHOT", "restore", msg);
        let p = program.program();
        if s.menu_disabled && !p.capabilities.contains(Capability::PlayerMenuAccess) {
            return Err(fail("menu access capability missing"));
        }
        if !s.audio_paused.is_empty() && !p.capabilities.contains(Capability::AudioPause) {
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
            && (!p.capabilities.contains(Capability::DialogueDecoration)
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
                        && !p.capabilities.contains(Capability::StoryTypedResult))
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
                || (!h.voices.is_empty() && !p.capabilities.contains(Capability::TextVoiceBinding))
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
                        || c.effect != defs[index].effect
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
            if c.effects != pending.effects {
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
            if !p.capabilities.contains(Capability::TextWindowTransition)
                || !reveal.style.valid()
                || reveal.duration_us.0 == 0
                || reveal.duration_us.0 > 60_000_000
                || reveal.started_us.0 > s.tick_us.0
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
                if !p.capabilities.contains(Capability::TextVoiceBinding)
                    || (reading.wait == VoiceWaitPolicy::SampledRemaining
                        && (!p.capabilities.contains(Capability::TextVoiceTimer)
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
                && !p.capabilities.contains(Capability::StageSpriteTimeline)
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
                    && (!p.capabilities.contains(Capability::StageSpriteLifecycle)
                        || n.parent.is_some()
                        || n.timeline_binding.is_none())
                {
                    return Err(fail("invalid inherited animated existence in snapshot"));
                }
                if let Some(transform) = n.sprite_transform {
                    if !p.capabilities.contains(Capability::StageSpriteTransform)
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
                    && (!p.capabilities.contains(Capability::StageSpriteWave)
                        || n.offset.iter().any(|v| !v.is_finite() || v.abs() > 8192.))
                {
                    return Err(fail("invalid sprite offset in snapshot"));
                }
                if !n.preserve_pose.is_empty()
                    && (!p.capabilities.contains(Capability::StageSpriteContinuity)
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
        let mut total_history_bytes: usize = s.history.iter().map(history_bytes).sum();
        let mut drop_count = 0usize;
        while total_history_bytes > 4 * 1024 * 1024 && drop_count < s.history.len() {
            total_history_bytes =
                total_history_bytes.saturating_sub(history_bytes(&s.history[drop_count]));
            drop_count += 1;
        }
        if drop_count > 0 {
            s.history.drain(0..drop_count);
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
            tracing: true,
            history_bytes: total_history_bytes,
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

pub(super) fn valid_reveal_interval(authored: Micros, frozen: Micros) -> bool {
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
pub(super) fn def_matches_task(def: &EffectDef, task: &Task) -> bool {
    (def.id == task.name && def.scope == task.scope && def.effect == task.effect)
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
