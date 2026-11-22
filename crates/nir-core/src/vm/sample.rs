use super::*;

impl Core {
    pub(super) fn track_is_current(&self, task: &Task, target: &TweenTarget) -> bool {
        !matches!(target, TweenTarget::SceneNode { .. })
            || task.scene_generation == self.state.scene_generation
    }
    pub(super) fn sample_timelines(&self, nodes: &mut [Node]) {
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
        let has_running_scene_effect = self.state.tasks.values().any(|t| {
            t.state == TaskState::Running
                && matches!(
                    t.effect,
                    Effect::SpriteTimeline { .. }
                        | Effect::SpriteWave { .. }
                        | Effect::SpriteShake { .. }
                        | Effect::Clip { .. }
                        | Effect::SourceMotion { .. }
                        | Effect::Tween {
                            target: TweenTarget::SceneNode { .. },
                            ..
                        }
                )
        });
        if !has_running_scene_effect {
            return nodes;
        }
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
    pub(super) fn window_coverage(&self) -> f32 {
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
    pub(super) fn task_needs_clock(&self, task: &Task) -> bool {
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
}
