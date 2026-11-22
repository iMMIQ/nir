use super::*;

impl Core {
    pub(super) fn commit(&mut self) -> Result<()> {
        let old = self.state.clone();
        let n = self.intents.len();
        if let Err(e) = self.commit_inner() {
            self.state = old;
            self.intents.truncate(n);
            return Err(e);
        }
        Ok(())
    }
    pub(super) fn commit_inner(&mut self) -> Result<()> {
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
    pub(super) fn commit_effect(
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
    pub(super) fn advance_compositions(&mut self) -> Result<()> {
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
    pub(super) fn advance_composition(&mut self, id: u32) -> Result<bool> {
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
    pub(super) fn spawn_child(&mut self, parent: u32, def: &EffectDef) -> Result<bool> {
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
    pub(super) fn resolve_wait(&mut self) -> Result<bool> {
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
    pub(super) fn advance_reading(
        &mut self,
        interaction: u32,
        sequence: u32,
        stop_voice: bool,
    ) -> Result<()> {
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
    pub(super) fn finish_task(&mut self, id: u32, status: TaskState) -> Result<()> {
        let reason = match status {
            TaskState::Finished => TaskEndReason::Completed,
            TaskState::Cancelled => TaskEndReason::CancelledByControl,
            TaskState::Failed => TaskEndReason::Failed,
            TaskState::Running => return Err(self.error("E_TASK", "invalid terminal state")),
        };
        self.end_task(id, reason)
    }
    pub(super) fn end_task(&mut self, id: u32, reason: TaskEndReason) -> Result<()> {
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
    pub(super) fn resume_text_pause(&mut self, id: u32) -> Result<()> {
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
    pub(super) fn reveal(&mut self, id: u32, until_gate: bool) -> Result<()> {
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
    pub(super) fn advance_time(&mut self, delta: u64) -> Result<()> {
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
}
