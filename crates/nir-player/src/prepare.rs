use super::*;

impl Player {
    pub(super) fn begin_prepare(
        &mut self,
        purpose: Purpose,
        activation: u32,
        assets: BTreeSet<String>,
    ) -> Result<()> {
        self.failed_admission = None;
        match self.admit_prepare(purpose, activation, assets.clone()) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.cancel_preparation();
                self.failed_admission = Some((purpose, activation, assets));
                self.pauses.insert_barrier("prepare".into());
                Err(error.classified(
                    ErrorDomain::Prepare,
                    "prepare",
                    "admission",
                    vec![Recovery::Retry, Recovery::KeepCurrent, Recovery::Exit],
                ))
            }
        }
    }
    pub(super) fn admit_prepare(
        &mut self,
        purpose: Purpose,
        activation: u32,
        mut assets: BTreeSet<String>,
    ) -> Result<()> {
        if self.deferred_prepare.take().is_some() {
            self.pauses.remove("prepare");
        }
        // Old and candidate resources are admitted together; never pin half a cue.
        assets.extend(if !self.story_context_active() {
            self.title_assets()
        } else {
            self.state_assets(&self.core)
        });
        let needs = self.asset_content_requirements(&assets)?;
        if !needs.is_empty() {
            return self.begin_content(
                ContentPurpose::Media {
                    purpose,
                    activation,
                    assets,
                },
                needs,
            );
        }
        self.request = self
            .request
            .checked_add(1)
            .ok_or_else(|| Diagnostic::new("E_LIMIT", "request", "counter"))?;
        let costs = self.costs(&assets)?;
        // Stop/cancel intents precede this preparation in the owner's command
        // queue. Only current scene/tasks and the candidate need to stay pinned;
        // a cancelled voice/BGM must not occupy the previous active lease.
        if let Some(active) = &mut self.active {
            active.retain(&assets);
        }
        let job = match PrepareJob::new(activation, self.generation, costs.clone(), &self.ledger) {
            Ok(job) => job,
            Err(_) if self.media_lookahead.is_some() || !self.media_retired.is_empty() => {
                self.cancel_media_lookahead();
                self.deferred_prepare = Some((self.generation, purpose, activation, assets));
                self.pauses.insert_barrier("prepare".into());
                self.observe("prepare_waiting_for_media_cancel", None);
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let mut job = job;
        let promoted = matches!(purpose, Purpose::Activation)
            && self.media_lookahead.as_ref().is_some_and(|s| {
                s.job.generation == self.generation && s.assets.is_subset(&assets)
            });
        let request = self.request;
        let speculative_assets = self
            .media_lookahead
            .as_ref()
            .map(|s| s.assets.clone())
            .unwrap_or_default();
        let promoted_from = if promoted {
            let spec = self.media_lookahead.take().unwrap();
            for ready in spec.assets.difference(&spec.job.missing) {
                job.ready(ready, self.generation);
            }
            self.commands.push(AppCommand::PromoteAssets {
                request: spec.request,
                session: self.generation.session,
            });
            Some(spec.request)
        } else {
            self.cancel_media_lookahead();
            None
        };
        let required_fetch: BTreeSet<_> = if promoted_from.is_some() {
            assets.difference(&speculative_assets).cloned().collect()
        } else {
            assets.clone()
        };
        self.cancel_preparation();
        self.prepare = Some(Preparation {
            request,
            promoted_from,
            assets: assets.clone(),
            purpose,
            job,
            preflight: false,
            failed: false,
        });
        self.pauses.insert_barrier("prepare".into());
        self.observe("prepare_requested", Some(request));
        if !required_fetch.is_empty() {
            self.commands.push(AppCommand::GetAssets {
                request,
                session: self.generation.session,
                device: self.generation.device,
                descriptors: self.describe_assets(&required_fetch)?,
                assets: required_fetch.into_iter().collect(),
                priority: PreparePriority::Required,
            });
        }
        if self
            .prepare
            .as_ref()
            .is_some_and(|p| p.job.missing.is_empty())
        {
            self.prepare.as_mut().unwrap().preflight = true;
            self.commands
                .push(AppCommand::PreparePresentation { request });
        }
        Ok(())
    }
    pub(super) fn cancel_preparation(&mut self) {
        self.failed_admission = None;
        // An entering replay survives a cancelled preparation: its retry and
        // device-resume paths re-prepare the same candidate. Real abandonment
        // (Title, NewGame) clears the whole transaction explicitly.
        if let Some(p) = self.prepare.take() {
            if !p.failed {
                self.observe("prepare_cancelled", Some(p.request));
                self.commands
                    .push(AppCommand::CancelAssets { request: p.request });
                if let Some(old) = p.promoted_from {
                    self.commands
                        .push(AppCommand::CancelAssets { request: old });
                }
            }
        }
    }
    pub(super) fn cancel_media_lookahead(&mut self) {
        if let Some(spec) = self.media_lookahead.take() {
            self.commands.push(AppCommand::CancelAssets {
                request: spec.request,
            });
            self.observe("media_lookahead_cancelled", Some(spec.request));
            self.media_retired.insert(spec.request, spec.job);
        }
    }
    pub(super) fn maybe_prefetch_media(&mut self) {
        let eligible = self.screen == Screen::Story
            && self.story_context_active()
            && !self.paused()
            && self.prepare.is_none()
            && self.candidate.is_none()
            && self.restore_work.is_none()
            && self.locale_candidate.is_none()
            && self.preferences.ui_locale == self.effective_ui_locale
            && self.preferences.text_locale == self.effective_text_locale
            && !self.content.values().any(|job| {
                !matches!(
                    job.purpose,
                    ContentPurpose::Locale | ContentPurpose::Prefetch
                )
            })
            && self.validated.program().player.prefetch_media;
        let cue = if eligible {
            self.core.predict_next_cue()
        } else {
            None
        };
        let fingerprint = cue.as_ref().map(|cue| {
            format!(
                "{}:{}:{}:{}:{}:{}",
                self.generation.session,
                self.generation.device,
                self.effective_text_locale,
                self.core.location(),
                self.core
                    .state()
                    .waiting
                    .as_ref()
                    .and_then(|w| serde_json::to_string(w).ok())
                    .unwrap_or_default(),
                cue
            )
        });
        if self
            .media_lookahead
            .as_ref()
            .is_some_and(|s| Some(&s.fingerprint) != fingerprint.as_ref())
        {
            self.cancel_media_lookahead();
        }
        if fingerprint
            .as_ref()
            .is_some_and(|f| self.media_attempted.as_ref() != Some(f))
        {
            self.media_attempted = None;
        }
        let Some(fingerprint) = fingerprint else {
            return;
        };
        if self.media_attempted.as_ref() == Some(&fingerprint)
            || self.media_lookahead.is_some()
            || !self.media_retired.is_empty()
        {
            return;
        }
        self.media_attempted = Some(fingerprint.clone());
        let cue = cue.unwrap();
        let recipe = self.validated.cue_assets(&cue);
        if recipe.iter().any(|id| self.validated.asset(id).is_none()) {
            return;
        }
        let resident = self.retained_assets();
        let assets: BTreeSet<_> = recipe
            .into_iter()
            .filter(|id| {
                !resident.contains(id)
                    && self
                        .validated
                        .asset(id)
                        .is_some_and(|a| matches!(a.kind, AssetKind::Image | AssetKind::Audio))
            })
            .collect();
        if assets.is_empty() {
            return;
        }
        // Only the incremental cost of media absent from the active group is
        // speculative. The ledger still jointly accounts for shared assets.
        let costs = match self.costs(&assets) {
            Ok(c) => c,
            Err(_) => return,
        };
        let incremental: u64 = costs
            .values()
            .fold(0u64, |total, cost| total.saturating_add(*cost));
        if incremental > 32 * 1024 * 1024 {
            return;
        }
        let Some(request) = self.request.checked_add(1) else {
            return;
        };
        let Ok(job) = PrepareJob::new(0, self.generation, costs, &self.ledger) else {
            return;
        };
        self.request = request;
        let descriptors = match self.describe_assets(&assets) {
            Ok(d) => d,
            Err(_) => return,
        };
        self.media_lookahead = Some(MediaLookahead {
            request,
            fingerprint,
            assets: assets.clone(),
            job,
        });
        self.commands.push(AppCommand::GetAssets {
            request,
            session: self.generation.session,
            device: self.generation.device,
            assets: assets.into_iter().collect(),
            descriptors,
            priority: PreparePriority::Near,
        });
        self.observe("media_lookahead_requested", Some(request));
    }
    pub(super) fn step(&mut self, input: CoreInput, budget: &mut u32) -> Result<()> {
        self.core.set_tracing(self.tracing);
        self.core.merge_profile_facts(&self.profile)?;
        if !self.replay_live() {
            self.core.set_profile_values(&self.profile_values)?;
        }
        self.core.set_text_speed(self.preferences.text_speed)?;
        let before_location = self.core.location();
        let output = self.core.step(input, *budget);
        *budget -= output.work_used;
        if before_location != output.location {
            self.touch_snapshot_content(self.core.state())?;
        }
        if output.remaining_time_us > 0 {
            self.inbox.push_front((
                self.generation.session,
                AppEvent::ContinueStoryTime {
                    delta_us: output.remaining_time_us,
                },
            ));
        }
        let mut profile_batch: Option<usize> = None;
        for intent in output.intents {
            // Trace remains in its original order. Other intents delimit
            // progress batches, including audio and preparation barriers.
            if !matches!(
                &intent,
                CoreIntent::ProfileMerge { .. } | CoreIntent::Trace { .. }
            ) {
                profile_batch = None;
            }
            match intent {
                CoreIntent::PrepareContent { module, locale } => {
                    let needs = self.content_requirements(&module, Some(&locale), true)?;
                    // Independent media completions may wake the VM while its
                    // PC is at the same barrier. They do not restart a download
                    // or implicitly retry a failed preparation.
                    if !self.content.values().any(|p| {
                        !matches!(p.purpose, ContentPurpose::Locale | ContentPurpose::Prefetch)
                    }) {
                        self.begin_content(ContentPurpose::Execution, needs)?;
                    }
                }
                CoreIntent::Prepare { activation, cue } => self.begin_prepare(
                    Purpose::Activation,
                    activation,
                    self.validated.cue_assets(&cue),
                )?,
                CoreIntent::AudioStart {
                    task,
                    asset,
                    bus,
                    looped,
                    loop_region,
                    position_us,
                    gain,
                } => self.commands.push(AppCommand::AudioStart {
                    domain: TimeDomain::Story,
                    task,
                    asset,
                    bus,
                    looped,
                    loop_region,
                    position_us,
                    gain,
                    envelope: 1.,
                    character: self
                        .core
                        .state()
                        .tasks
                        .get(&task)
                        .map(|task| task.voice_character.clone())
                        .unwrap_or_default(),
                    session: self.generation.session,
                }),
                CoreIntent::AudioCharacter { task, character } => {
                    if let Some(AppCommand::AudioStart { character: queued, .. }
                        | AppCommand::AudioCharacter { character: queued, .. }) =
                        self.commands.iter_mut().rev().find(|command| matches!(command,
                            AppCommand::AudioStart { domain:TimeDomain::Story,task:id,session,.. }
                            | AppCommand::AudioCharacter { domain:TimeDomain::Story,task:id,session,.. }
                            if *id==task && *session==self.generation.session)) {
                        *queued = character;
                        continue;
                    }
                    self.commands.push(AppCommand::AudioCharacter {
                        domain: TimeDomain::Story,
                        task,
                        character,
                        session: self.generation.session,
                    });
                }
                CoreIntent::AudioEnvelope {
                    owner,
                    elapsed_us,
                    task,
                    from,
                    to,
                    duration_us,
                } => {
                    self.commands.push(AppCommand::AudioEnvelope {
                        owner,
                        elapsed_us,
                        domain: TimeDomain::Story,
                        session: self.generation.session,
                        task,
                        from,
                        to,
                        duration_us,
                    });
                }
                CoreIntent::AudioStop { task } => self.commands.push(AppCommand::AudioStop {
                    domain: TimeDomain::Story,
                    session: self.generation.session,
                    task,
                }),
                CoreIntent::ProfileMerge { key } => {
                    // Replay cores run isolated: their profile writes die with
                    // the replay and never mark the owner's progress.
                    if !self.replay_live() && self.profile.insert(key.clone()) {
                        // Use a stable index so intervening diagnostic trace
                        // does not cause repeated scans or extra disk writes.
                        if let Some(AppCommand::PersistProfile { keys }) =
                            profile_batch.map(|index| &mut self.commands[index])
                        {
                            keys.insert(key);
                        } else {
                            profile_batch = Some(self.commands.len());
                            self.commands.push(AppCommand::PersistProfile {
                                keys: BTreeSet::from([key]),
                            });
                        }
                    }
                }
                CoreIntent::ProfileValueAssign { key, value } => {
                    if !self.replay_live() && self.profile_values.get(&key) != Some(&value) {
                        self.profile_values.insert(key.clone(), value.clone());
                        self.profile_value_edits.insert(key.clone(), value.clone());
                        self.commands.push(AppCommand::PersistProfileValues {
                            values: BTreeMap::from([(key, value)]),
                        });
                    }
                }
                CoreIntent::Checkpoint => {
                    self.checkpoints.push(self.core.snapshot());
                }
                CoreIntent::Trace { event, at } => {
                    self.commands.push(AppCommand::Trace { event, at })
                }
            }
        }
        if let Some(e) = &self.core.state().fault {
            self.report(e.clone(), true);
            self.pauses.insert("fault".into());
        }
        if self.core.state().fault.is_some()
            || (!self.menu_peek
                && (self.core.state().choice.is_some() || self.core.dialogue().is_none()))
        {
            self.set_interface_hidden(false);
        }
        if self.core.state().outcome.is_some() {
            if self.replay_live() {
                // A finished replay returns to its frozen session instead of
                // the title: the launching page is the replay's home.
                self.begin_replay_return()?;
            } else if self.core.program().theme.return_to_title {
                let root = self
                    .core
                    .state()
                    .frames
                    .first()
                    .map(|frame| frame.function.as_str());
                let menu = self.core.program().theme.image_menus.iter().find_map(|(id, menu)|
                    menu.controls().any(|(_,action,_)| matches!(action, nir_format::ImageMenuAction::Entry { function } if Some(function.as_str()) == root)).then(|| id.clone()))
                    .unwrap_or_else(|| "title".into());
                self.action(UiAction::Title, 0, 0, budget)?;
                self.image_menu = menu;
            } else {
                self.screen = Screen::Ended;
            }
            self.auto = false;
            self.skip = false;
            self.held_skip = false;
        }
        Ok(())
    }
    /// A reveal committed by a mid-block operation owns no cue recipe, so its
    /// mask may sit outside every admitted set when it spawns. One top-up per
    /// reveal fetches it; the story clock holds at the prepare pause until the
    /// media lands, so the reveal never runs half-masked.
    pub(super) fn maybe_prepare_window_mask(&mut self) -> Result<()> {
        let Some(reveal) = self.core.state().window_reveal.clone() else {
            self.window_mask_attempt = None;
            return Ok(());
        };
        let Some(asset) = reveal.style.asset().map(str::to_owned) else {
            self.window_mask_attempt = None;
            return Ok(());
        };
        if self.window_mask_attempt == Some((reveal.started_us, asset.clone())) {
            return Ok(());
        }
        self.window_mask_attempt = Some((reveal.started_us, asset.clone()));
        if self.screen == Screen::Story
            && self.prepare.is_none()
            && self.candidate.is_none()
            && self.restore_work.is_none()
            && !self.replay_live()
        {
            self.begin_prepare(Purpose::TopUp, 0, BTreeSet::from([asset]))?;
        }
        Ok(())
    }
    pub(super) fn cancel_prefetch_content(&mut self) -> bool {
        let requests: Vec<_> = self
            .content
            .iter()
            .filter(|(_, job)| matches!(job.purpose, ContentPurpose::Prefetch))
            .map(|(request, _)| *request)
            .collect();
        for request in &requests {
            self.content.remove(request);
            self.commands
                .push(AppCommand::CancelContent { request: *request });
        }
        !requests.is_empty()
    }
    pub(super) fn maybe_prefetch_content(&mut self) {
        let eligible = self.screen == Screen::Story
            && self.story_context_active()
            && !self.paused()
            && self.prepare.is_none()
            && self.candidate.is_none()
            && self.restore_work.is_none()
            && self.validated.program().player.prefetch_content
            && !self.content.values().any(|job| {
                !matches!(
                    job.purpose,
                    ContentPurpose::Locale | ContentPurpose::Prefetch
                )
            });
        if !eligible {
            if self.cancel_prefetch_content() {
                self.prefetch_attempted = None;
            }
            return;
        }
        let Some(module) = self.core.prefetch_module() else {
            if self.cancel_prefetch_content() {
                self.prefetch_attempted = None;
            }
            self.prefetch_attempted = None;
            return;
        };
        let state = self.core.state();
        let Some(frame) = state.frames.last() else {
            self.prefetch_attempted = None;
            return;
        };
        let wait = state
            .waiting
            .as_ref()
            .and_then(|waiting| serde_json::to_string(waiting).ok())
            .unwrap_or_default();
        let attempt = PrefetchAttempt {
            wait: format!("{}:{wait}", self.core.location()),
            function: frame.function.clone(),
            module,
            locale: self.effective_text_locale.clone(),
            session: self.generation.session,
        };
        if self.prefetch_attempted.as_ref() != Some(&attempt) {
            self.cancel_prefetch_content();
            self.prefetch_attempted = None;
        }
        if self.prefetch_attempted.as_ref() == Some(&attempt) {
            return;
        }
        if self
            .content
            .values()
            .any(|job| matches!(job.purpose, ContentPurpose::Prefetch))
        {
            self.prefetch_attempted = Some(attempt);
            return;
        }
        let objects = match self.content_requirements(&attempt.module, Some(&attempt.locale), true)
        {
            Ok(objects) => objects,
            Err(_) => {
                self.prefetch_attempted = Some(attempt);
                return;
            }
        };
        if objects.is_empty() {
            self.prefetch_attempted = Some(attempt);
            return;
        }
        if self
            .begin_content(ContentPurpose::Prefetch, objects)
            .is_err()
        {
            // Speculation must not affect the running story on admission or
            // request-limit failures.
            self.prefetch_attempted = Some(attempt);
            return;
        }
        if self
            .content
            .values()
            .any(|job| matches!(job.purpose, ContentPurpose::Prefetch))
        {
            self.prefetch_attempted = Some(attempt);
        }
    }
}
