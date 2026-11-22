use super::*;

impl Player {
    pub fn pump(&mut self, events: Vec<AppEvent>, budget: u32) -> Vec<AppCommand> {
        // Admission leaves room for resource, audio and storage terminal events.
        for event in events {
            let input = matches!(
                event,
                AppEvent::Action { .. }
                    | AppEvent::Tick { .. }
                    | AppEvent::TickDomains { .. }
                    | AppEvent::ContinueStoryTime { .. }
            );
            let limit = if input {
                INPUT_CAPACITY
            } else {
                EVENT_CAPACITY
            };
            if self.inbox.len() >= limit {
                self.report(
                    Diagnostic::new("E_EVENT_QUEUE", "pump", "event admission limit").classified(
                        ErrorDomain::Host,
                        "admit",
                        "queue",
                        vec![Recovery::Exit],
                    ),
                    true,
                );
                self.pauses.insert("queue-overflow".into());
                self.ui_pauses.insert("queue-overflow".into());
                continue;
            }
            self.inbox.push_back((self.generation.session, event));
        }
        // Stable partition across retained work, so a choice beats same-turn time.
        self.inbox.make_contiguous().sort_by_key(|(_, e)| {
            matches!(
                e,
                AppEvent::Tick { .. }
                    | AppEvent::TickDomains { .. }
                    | AppEvent::ContinueStoryTime { .. }
            )
        });
        let mut remaining = budget.min(100_000);
        let limit = remaining;
        while remaining > 0 {
            let Some((session, event)) = self.inbox.pop_front() else {
                break;
            };
            remaining -= 1;
            if matches!(
                event,
                AppEvent::Tick { .. }
                    | AppEvent::TickDomains { .. }
                    | AppEvent::ContinueStoryTime { .. }
            ) && session != self.generation.session
            {
                self.observe("stale_tick_discarded", None);
                continue;
            }
            if let Err(e) = self
                .event(event, &mut remaining)
                .and_then(|_| self.sync_menu_state())
            {
                self.report(e, true);
            }
        }
        // Device completion never runs the VM through a paused reading scene.
        while remaining > 0 && self.screen == Screen::Story && !self.paused() {
            let Some((task, input)) = self.deferred_audio_events.pop_first() else {
                break;
            };
            remaining -= 1;
            // Closing a scene or cancelling a sound may retire it before the
            // held device event is delivered. It must not touch its successor.
            if !self.core.state().tasks.get(&task).is_some_and(|t| {
                t.state == TaskState::Running && matches!(t.effect, Effect::Audio { .. })
            }) {
                continue;
            }
            if let Err(error) = self.step(input, &mut remaining) {
                self.report(error, true);
            }
        }
        if self.prepare.is_none() && self.screen == Screen::Story && self.pauses.is_empty() {
            if let Err(e) = self.step(CoreInput::None, &mut remaining) {
                self.report(e, true);
            }
        }
        // Advance finite UI effect fades, then commit any page that became
        // prepared. Runs after the event loop so prepare completions inside
        // this turn are visible without waiting for the next host frame.
        if let Err(e) = self.update_menu_effects(&mut remaining) {
            self.report(e, true);
        }
        if let Err(e) = self.open_story_modal() {
            self.report(e, true);
        }
        if let Err(e) = self.sync_menu_state() {
            self.report(e, true);
        }
        self.update_history_voice();
        if let Err(e) = self.prepare_active_menu() {
            self.report(e, true);
        }
        if let Err(e) = self.maybe_prepare_window_mask() {
            self.report(e, true);
        }
        self.maybe_prefetch_content();
        self.maybe_prefetch_media();
        if let Err(error) = self.refresh_content_lease() {
            self.report(error, true);
        }
        // Preparation or a budget continuation can publish a choice without
        // a clock input. Stop skip in that same turn so the visible reading
        // state cannot advertise fast-forward while waiting for a choice.
        if self.core.state().choice.is_some() {
            self.skip = false;
            self.held_skip = false;
        }
        self.work_used = limit - remaining;
        for domain in [TimeDomain::Story, TimeDomain::ForegroundUi] {
            let after = self.domain_paused(domain);
            if self.audio_paused.get(&domain) != Some(&after) {
                self.audio_paused.insert(domain, after);
                self.commands.push(AppCommand::AudioPause {
                    domain,
                    paused: after,
                });
            }
        }
        for bus in [AudioBus::Bgm, AudioBus::Voice, AudioBus::Sfx] {
            let paused = self.overlay_bus_paused(bus);
            if self.audio_bus_paused.get(&bus).copied().unwrap_or(false) != paused {
                self.audio_bus_paused.insert(bus, paused);
                self.commands.push(AppCommand::AudioBusPause {
                    domain: TimeDomain::Story,
                    bus,
                    paused,
                });
            }
        }
        std::mem::take(&mut self.commands)
    }
    pub(super) fn event(&mut self, e: AppEvent, budget: &mut u32) -> Result<()> {
        if let AppEvent::ContentReady { request, objects } = e {
            if let Err(error) = self.complete_content(request, objects) {
                self.fail_content(request, error.to_string());
            }
            return Ok(());
        }
        if let AppEvent::ContentFailed { request, message } = e {
            self.fail_content(request, message);
            return Ok(());
        }
        if let AppEvent::ContentSkipped {
            request,
            code,
            message,
        } = e
        {
            if self.accepts_content(request) {
                let job = self.content.remove(&request).unwrap();
                self.commands.push(AppCommand::CancelContent { request });
                if matches!(job.purpose, ContentPurpose::Prefetch) {
                    self.observe("prefetch_skipped", Some(request));
                } else if matches!(job.purpose, ContentPurpose::HistoryVoice) {
                    self.fail_history_voice();
                } else {
                    // A prefetch can be promoted after the host's speculative
                    // size preflight has already rejected it. Retry the same
                    // exact objects under a fresh required request envelope.
                    self.observe("promoted_prefetch_skipped", Some(request));
                    if let Err(error) = self.begin_content(job.purpose, job.objects) {
                        self.pauses.insert_barrier("content".into());
                        self.report(
                            Diagnostic::new(
                                "E_MODULE_PREPARE",
                                "content",
                                format!("{code}: {message}; retry failed: {error}"),
                            ),
                            true,
                        );
                    }
                }
            }
            return Ok(());
        }
        match e {
            AppEvent::ContentReady { .. }
            | AppEvent::ContentFailed { .. }
            | AppEvent::ContentSkipped { .. } => unreachable!(),
            AppEvent::Action {
                action,
                interaction,
                sequence,
                session,
            } => {
                if session == self.generation.session {
                    self.observe_from(
                        "input_dispatched",
                        None,
                        Some(session),
                        None,
                        Some(sequence),
                    );
                    self.action(action, interaction, sequence, budget)?;
                } else {
                    self.observe_from(
                        "stale_input_discarded",
                        None,
                        Some(session),
                        None,
                        Some(sequence),
                    );
                }
            }
            AppEvent::Tick { delta_us } => {
                self.event(
                    AppEvent::TickDomains {
                        story_us: delta_us,
                        foreground_us: delta_us,
                    },
                    budget,
                )?;
            }
            AppEvent::TickDomains {
                story_us,
                foreground_us,
            } => {
                if !self.domain_paused(TimeDomain::ForegroundUi) {
                    self.ui_clock_us.0 =
                        self.ui_clock_us
                            .0
                            .checked_add(foreground_us)
                            .ok_or_else(|| {
                                Diagnostic::new("E_TIME", "clock", "foreground clock overflow")
                            })?;
                }
                self.advance_story_time(story_us, budget)?;
            }
            AppEvent::ContinueStoryTime { delta_us } => {
                self.advance_story_time(delta_us, budget)?;
            }
            AppEvent::AssetReady { request, asset } => {
                if self.accepts_history_voice_resource(request) {
                    self.history_voice_asset_ready(request, &asset)?;
                    return Ok(());
                }
                if !self.accepts_resource(request) {
                    self.observe("stale_asset_discarded", Some(request));
                }
                if let Some(spec) = self
                    .media_lookahead
                    .as_mut()
                    .filter(|s| s.request == request)
                {
                    spec.job.ready(&asset, self.generation);
                }
                if let Some(p) = self.prepare.as_mut().filter(|p| {
                    (p.request == request || p.promoted_from == Some(request)) && !p.failed
                }) {
                    p.job.ready(&asset, self.generation);
                    if p.job.missing.is_empty() && !p.preflight {
                        p.preflight = true;
                        self.commands
                            .push(AppCommand::PreparePresentation { request: p.request });
                    }
                }
                if let (Some(candidate), Some(job)) =
                    (self.locale_candidate.as_mut(), self.locale_job.as_mut())
                {
                    if candidate.request == request && self.locale_error.is_none() {
                        job.ready(&asset, self.generation);
                        if job.missing.is_empty() && !candidate.preflight {
                            candidate.preflight = true;
                            self.commands.push(AppCommand::PrepareLocale {
                                request,
                                ui_locale: candidate.ui_locale.clone(),
                                text_locale: candidate.text_locale.clone(),
                            });
                        }
                    }
                }
            }
            AppEvent::AssetsCancelled { request } => {
                self.media_retired.remove(&request);
                if self.media_retired.is_empty() {
                    if let Some((generation, purpose, activation, assets)) =
                        self.deferred_prepare.take()
                    {
                        self.pauses.remove("prepare");
                        if generation == self.generation {
                            self.begin_prepare(purpose, activation, assets)?;
                        }
                    }
                    if let Some(request) = self.deferred_locale.take() {
                        if self
                            .locale_candidate
                            .as_ref()
                            .is_some_and(|c| c.request == request)
                        {
                            self.start_locale_switch()?;
                        }
                    }
                }
            }
            AppEvent::AssetFailed { request, message } => {
                if self.accepts_history_voice_resource(request) {
                    self.fail_history_voice();
                    return Ok(());
                }
                if self
                    .media_lookahead
                    .as_ref()
                    .is_some_and(|s| s.request == request)
                {
                    self.cancel_media_lookahead();
                    return Ok(());
                }
                if self.accepts_locale(request) {
                    self.locale_failed(request, message);
                } else {
                    self.asset_fault(
                        request,
                        Diagnostic::new("E_PREPARE", self.core.location(), message),
                    );
                }
            }
            AppEvent::AssetFault {
                request,
                diagnostic,
            } => {
                if self.accepts_history_voice_resource(request) {
                    self.fail_history_voice();
                    return Ok(());
                }
                if self
                    .media_lookahead
                    .as_ref()
                    .is_some_and(|s| s.request == request)
                {
                    self.cancel_media_lookahead();
                    return Ok(());
                }
                if self.accepts_locale(request) {
                    self.locale_failed(request, diagnostic.to_string());
                } else {
                    self.asset_fault(request, *diagnostic);
                }
            }
            AppEvent::PresentationReady { request } => self.complete(request, budget)?,
            AppEvent::LocaleReady { request } => {
                if let Some(candidate) = self
                    .locale_candidate
                    .clone()
                    .filter(|c| c.request == request && c.preflight && self.locale_error.is_none())
                {
                    let lease = self.locale_job.take().and_then(|job| job.finish().ok());
                    if lease
                        .as_ref()
                        .is_some_and(|lease| lease.valid(request, self.generation))
                    {
                        self.core.set_locale(&candidate.text_locale)?;
                        self.effective_ui_locale = candidate.ui_locale;
                        self.effective_text_locale = candidate.text_locale;
                        self.locale_candidate = None;
                        self.locale_error = None;
                        self.pauses.remove("locale");
                        self.prefetch_attempted = None;
                        self.touch_snapshot_content(self.core.state())?;
                        self.restore_locale_changed()?;
                        self.status = self
                            .messages
                            .text(&self.effective_ui_locale, "language-applied");
                    } else {
                        self.locale_failed(request, "E_STALE_LEASE".into());
                    }
                }
            }
            AppEvent::LocaleFailed { request, message } => {
                self.locale_failed(request, message);
            }
            AppEvent::AudioEnded {
                domain,
                task,
                session,
            } => {
                if self.history_voice_audio_event(domain, task, session, false)? {
                    return Ok(());
                }
                if domain == TimeDomain::Story && session == self.generation.session {
                    if self.deferred_audio_events.contains_key(&task) {
                        return Ok(());
                    }
                    if self.paused() {
                        if self.core.state().tasks.get(&task).is_some_and(|t| {
                            t.state == TaskState::Running
                                && matches!(t.effect, Effect::Audio { looped: false, .. })
                        }) {
                            self.defer_audio_event(task, CoreInput::AudioEnded { task })?;
                        }
                    } else {
                        self.step(CoreInput::AudioEnded { task }, budget)?;
                    }
                } else if domain == TimeDomain::ForegroundUi
                    && session == self.generation.session
                    && self.menu_effects.sounds.remove(&task).is_some()
                {
                    self.observe("ui_sound_ended", None);
                }
            }
            AppEvent::AudioFailed {
                domain,
                task,
                session,
                message,
            } => {
                if self.history_voice_audio_event(domain, task, session, true)? {
                    return Ok(());
                }
                if domain == TimeDomain::Story && session == self.generation.session {
                    if !self.core.state().tasks.get(&task).is_some_and(|t| {
                        t.state == TaskState::Running && matches!(t.effect, Effect::Audio { .. })
                    }) || self.deferred_audio_events.contains_key(&task)
                    {
                        return Ok(());
                    }
                    // Device messages are held while a reader is in menus or
                    // another pause. Keep retained failure text bounded too.
                    let message: String = message.chars().take(1024).collect();
                    let mut d = Diagnostic::new("E_AUDIO", self.core.location(), &message)
                        .classified(
                            ErrorDomain::Host,
                            "audio",
                            "playback",
                            vec![Recovery::KeepCurrent],
                        );
                    d.details.as_mut().unwrap().task = Some(task);
                    self.report(d, false);
                    let input = CoreInput::TaskFailed { task, message };
                    if self.paused() {
                        self.defer_audio_event(task, input)?;
                    } else {
                        self.step(input, budget)?;
                    }
                } else if domain == TimeDomain::ForegroundUi
                    && session == self.generation.session
                    && self.menu_effects.sounds.remove(&task).is_some()
                {
                    // Menu page effect voices are best effort; a failed one
                    // never faults the session or cancels the transition.
                    self.observe_from("ui_sound_failed", None, Some(session), Some(task), None);
                } else if domain == TimeDomain::ForegroundUi
                    && session == self.generation.session
                    && self
                        .menu_effects
                        .music
                        .is_some_and(|voice| voice.task == task && voice.session == session)
                {
                    // A failed replacement cannot strand the retained Story music.
                    self.menu_effects.music = None;
                    self.observe_from("ui_music_failed", None, Some(session), Some(task), None);
                } else {
                    self.observe_from(
                        "stale_audio_discarded",
                        None,
                        Some(session),
                        Some(task),
                        None,
                    );
                }
            }
            AppEvent::AudioPositions {
                domain,
                session,
                positions,
            } => {
                self.observe_audio_positions(domain, session, &positions)?;
            }
            AppEvent::Hidden(hidden) => {
                if hidden {
                    self.held_skip = false;
                    self.pauses.insert("hidden".into());
                    self.ui_pauses.insert("hidden".into());
                } else {
                    self.pauses.remove("hidden");
                    self.ui_pauses.remove("hidden");
                }
            }
            AppEvent::SlotLoaded { job, envelope } => {
                if let Some(slot) = self.accept_slot_load(job) {
                    self.slot_restore = true;
                    if envelope.slot != slot {
                        return Err(Diagnostic::new(
                            "E_SAVE_SLOT",
                            "load",
                            "slot does not match request",
                        ));
                    }
                    if self
                        .slots
                        .iter()
                        .any(|row| row.slot == slot && row.error.is_some())
                    {
                        self.commands.push(AppCommand::ListSaves);
                    }
                    self.event(AppEvent::Loaded { envelope }, budget)?;
                }
            }
            AppEvent::SlotLoadFailed { job, message } => {
                if self.accept_slot_load(job).is_some() {
                    self.event(AppEvent::LoadFailed(message), budget)?;
                }
            }
            AppEvent::Loaded { envelope } => {
                if envelope.format != 1
                    || nir_content::digest(&serde_json::to_vec(&envelope.snapshot).unwrap())
                        != envelope.digest
                {
                    return Err(Diagnostic::new(
                        "E_SAVE_DIGEST",
                        "load",
                        "snapshot checksum",
                    ));
                }
                self.restore(envelope.snapshot).map_err(|d| {
                    d.classified(
                        ErrorDomain::Storage,
                        "load",
                        "validate",
                        vec![Recovery::KeepCurrent],
                    )
                })?;
            }
            AppEvent::HostFailed(m) => self.report(
                Diagnostic::new("E_HOST", "dispatch", m).classified(
                    ErrorDomain::Host,
                    "dispatch",
                    "owner",
                    vec![Recovery::Reload],
                ),
                false,
            ),
            AppEvent::PersistenceFailed { kind, message } => {
                self.persistence_failure(kind, message, "persist", "transaction");
            }
            AppEvent::PersistenceReadFailed { kind, message } => {
                self.persistence_failure(kind, message, "load_metadata", "read");
            }
            AppEvent::PersistenceStored(kind) => {
                self.persistence_failures
                    .remove(&(kind, PersistencePhase::Write));
                // Warnings are global to preferences/progress, independent of
                // the story session. Completing one does not hide another.
                self.clear_storage_warning(kind.location());
            }
            AppEvent::Exported { job } => {
                if self.export_job == Some(job) {
                    self.export_job = None;
                    self.clear_storage_warning("export");
                }
            }
            AppEvent::ExportCancelled { job } => {
                if self.export_job == Some(job) {
                    self.export_job = None;
                }
            }
            AppEvent::ExportFailed { job, message } => {
                if self.export_job == Some(job) {
                    self.export_job = None;
                    self.report(
                        Diagnostic::new("E_STORAGE", "export", message).classified(
                            ErrorDomain::Storage,
                            "export",
                            "file",
                            vec![Recovery::Retry, Recovery::KeepCurrent],
                        ),
                        false,
                    );
                }
            }
            AppEvent::LoadFailed(m) => self.report(
                Diagnostic::new("E_STORAGE", "load", m).classified(
                    ErrorDomain::Storage,
                    "load",
                    "transaction",
                    vec![Recovery::Retry, Recovery::KeepCurrent],
                ),
                false,
            ),
            AppEvent::Saved {
                job,
                slot,
                revision,
            } => {
                if self
                    .save_jobs
                    .get(&job)
                    .is_some_and(|(saved_slot, _)| *saved_slot == slot)
                {
                    self.save_jobs.remove(&job);
                    self.unconfirmed_saves.remove(&job);
                    self.failed_saves.remove(&slot);
                    self.clear_storage_warning("save");
                    self.slot_revisions.insert(slot, revision);
                    self.status = if let Some(pending) = self
                        .unconfirmed_saves
                        .values()
                        .next()
                        .or_else(|| self.failed_saves.values().next())
                    {
                        self.messages.diagnostic(pending, &self.effective_ui_locale)
                    } else if self.effective_ui_locale == "en" {
                        "Saved".into()
                    } else {
                        "已保存".into()
                    };
                    self.commands.push(AppCommand::ListSaves);
                }
            }
            AppEvent::SavePending { job, message } => {
                if let Some((_, session)) = self.save_jobs.get(&job).copied() {
                    let mut d = Diagnostic::new("E_STORAGE_UNCERTAIN", "save", message).classified(
                        ErrorDomain::Storage,
                        "save",
                        "confirmation",
                        vec![Recovery::KeepCurrent],
                    );
                    d.details.as_mut().unwrap().request = Some(job);
                    d.details.as_mut().unwrap().session = Some(session);
                    self.unconfirmed_saves.insert(job, d.clone());
                    self.report(d, false);
                } else {
                    self.observe("stale_save_notice_discarded", Some(job));
                }
            }
            AppEvent::SaveFailed { job, message } => {
                self.save_fault(job, Diagnostic::new("E_STORAGE", "save", message))
            }
            AppEvent::SaveFault { job, diagnostic } => self.save_fault(job, *diagnostic),
            AppEvent::Slots(slots, revisions) => {
                let mut failures = Vec::new();
                for slot in 0..self.save_slots() {
                    let revision = revisions.get(&slot).copied().unwrap_or(0);
                    let row = slots
                        .iter()
                        .find(|row| row.slot == slot)
                        .cloned()
                        .unwrap_or(SlotView {
                            slot,
                            ..Default::default()
                        });
                    if let Some(message) = &row.error {
                        // Unknown is never revision zero: retain the last known
                        // revision while the row blocks writes and offers retry.
                        failures.push(format!("slot {slot}: {message}"));
                    } else {
                        if revision < self.slot_revisions.get(&slot).copied().unwrap_or(0) {
                            continue;
                        }
                        self.slot_revisions.insert(slot, revision);
                    }
                    if let Some(old) = self.slots.iter_mut().find(|row| row.slot == slot) {
                        *old = row;
                    }
                }
                if !failures.is_empty() {
                    // The row itself communicates the failure. A background
                    // refresh must not replace successful save feedback.
                    let status = self.status.clone();
                    self.report(
                        Diagnostic::new("E_STORAGE", "list", failures.join("; ")).classified(
                            ErrorDomain::Storage,
                            "list",
                            "read",
                            vec![Recovery::Retry, Recovery::KeepCurrent],
                        ),
                        false,
                    );
                    self.status = status;
                }
            }
            AppEvent::Preferences(mut p) => {
                let locale_config = &self.core.program().locale_config;
                if !locale_config.ui.contains_key(&p.ui_locale) {
                    p.ui_locale = locale_config.default_ui.clone();
                }
                if !locale_config.text.contains_key(&p.text_locale) {
                    p.text_locale = locale_config.default_text.clone();
                }
                p.text_speed = finite_clamp(p.text_speed, 0.25, 4., 1.);
                p.auto_wait_scale = finite_clamp(p.auto_wait_scale, 0.25, 4., 1.);
                p.font_scale = finite_clamp(p.font_scale, 0.8, 1.5, 1.);
                p.bgm_volume = finite_clamp(p.bgm_volume, 0., 1., 0.3);
                p.voice_volume = finite_clamp(p.voice_volume, 0., 1., 0.8);
                p.sfx_volume = finite_clamp(p.sfx_volume, 0., 1., 0.5);
                p.character_voices
                    .retain(|id, _| self.core.program().text_identity(id).is_some());
                p.normalize_character_voices();
                self.preferences = p;
                self.preference_recovery.applied(&self.preferences);
                self.commands.push(AppCommand::ApplyPreferences {
                    preferences: self.preferences.clone(),
                });
                self.start_locale_switch()?;
            }
            AppEvent::Profile(keys) => self.profile.extend(keys),
            AppEvent::ProfileValues(mut values) | AppEvent::ProfileValuesRecovered(mut values) => {
                values.extend(self.profile_value_edits.clone());
                if !self.replay_live() {
                    self.core.set_profile_values(&values)?;
                }
                self.profile_values = values;
                self.persistence_failures
                    .remove(&(PersistenceKind::ProfileValues, PersistencePhase::Read));
                self.clear_storage_warning("profile_values");
                if !self.profile_value_edits.is_empty() {
                    self.commands.push(AppCommand::PersistProfileValues {
                        values: self.profile_value_edits.clone(),
                    });
                }
            }
            AppEvent::PreferencesRecovered(p) => {
                let merged = self.preference_recovery.merge(p, &self.preferences);
                self.event(AppEvent::Preferences(merged), budget)?;
                self.persistence_failures
                    .remove(&(PersistenceKind::Preferences, PersistencePhase::Read));
                self.clear_storage_warning("preferences");
                if self.preference_recovery.has_edits()
                    || self
                        .persistence_failures
                        .contains_key(&(PersistenceKind::Preferences, PersistencePhase::Write))
                {
                    self.persist_preferences();
                }
            }
            AppEvent::ProfileRecovered(keys) => {
                self.profile.extend(keys);
                self.persistence_failures
                    .remove(&(PersistenceKind::Profile, PersistencePhase::Read));
                self.clear_storage_warning("profile");
            }
            AppEvent::DeviceLost => {
                self.stop_history_voice();
                self.observe("device_lost", None);
                if self.locale_pending() {
                    self.invalidate_locale_candidate();
                    self.pauses.remove("locale");
                }
                self.generation.device += 1;
                self.device_resume = self.prepare.as_ref().map(|p| p.purpose);
                self.cancel_preparation();
                self.pauses.insert("device".into());
                self.ui_pauses.insert("device".into());
                self.commands.push(AppCommand::AudioPause {
                    domain: TimeDomain::Story,
                    paused: true,
                });
            }
            AppEvent::DeviceReady => {
                self.observe("device_ready", None);
                if let Some(work) = &self.restore_work {
                    if let Some(candidate) = &self.candidate {
                        let purpose = if work.rollback {
                            Purpose::Rollback
                        } else {
                            Purpose::Restore
                        };
                        let assets = self.state_assets(candidate);
                        self.begin_prepare(purpose, 0, assets)?;
                    } else if self.content.values().any(|job| {
                        matches!(
                            job.purpose,
                            ContentPurpose::RestoreValidation | ContentPurpose::RestoreBodies(_)
                        )
                    }) {
                        // Keep the old device pause until staged restore has a
                        // complete candidate whose media can be prepared.
                    } else {
                        self.resume_restore_work()?;
                    }
                    if self.locale_error.is_none()
                        && (self.preferences.ui_locale != self.effective_ui_locale
                            || self.preferences.text_locale != self.effective_text_locale)
                    {
                        self.start_locale_switch()?;
                    }
                    return Ok(());
                }
                let purpose = match self.device_resume.take() {
                    Some(Purpose::Restore) => Purpose::Restore,
                    Some(Purpose::Rollback) => Purpose::Rollback,
                    // The entering replay re-prepares its candidate after the
                    // device came back; its frozen session still owns the page.
                    Some(Purpose::Replay) => Purpose::Replay,
                    _ => Purpose::Device,
                };
                if purpose == Purpose::Replay {
                    // The replay candidate's activation is its own pending
                    // cue, which the generic resume path cannot know.
                    self.begin_replay_media()?;
                } else {
                    self.begin_prepare(purpose, 0, self.retained_assets())?;
                }
                if self.locale_error.is_none()
                    && (self.preferences.ui_locale != self.effective_ui_locale
                        || self.preferences.text_locale != self.effective_text_locale)
                {
                    self.start_locale_switch()?;
                }
            }
        }
        Ok(())
    }
    pub(super) fn save_fault(&mut self, job: u32, d: Diagnostic) {
        let Some((slot, session)) = self.save_jobs.remove(&job) else {
            self.observe("stale_save_failure_discarded", Some(job));
            return;
        };
        self.unconfirmed_saves.remove(&job);
        let mut d = d.classified(
            ErrorDomain::Storage,
            "save",
            "transaction",
            vec![Recovery::KeepCurrent],
        );
        d.details.as_mut().unwrap().request = Some(job);
        d.details.as_mut().unwrap().session = Some(session);
        self.failed_saves.insert(slot, d.clone());
        self.report(d, false);
    }
    pub(super) fn asset_fault(&mut self, request: u32, mut d: Diagnostic) {
        let request = self
            .prepare
            .as_ref()
            .and_then(|p| (p.promoted_from == Some(request)).then_some(p.request))
            .unwrap_or(request);
        if !self.accepts(request) {
            self.observe("stale_failure_discarded", Some(request));
            return;
        }
        self.prepare.as_mut().unwrap().failed = true;
        self.commands.push(AppCommand::CancelAssets { request });
        if let Some(old) = self.prepare.as_ref().and_then(|p| p.promoted_from) {
            self.commands
                .push(AppCommand::CancelAssets { request: old });
        }
        if d.details.is_none() {
            d = d.classified(
                ErrorDomain::Prepare,
                "prepare",
                "resource",
                vec![Recovery::Retry, Recovery::KeepCurrent, Recovery::Exit],
            );
        }
        d.details.as_mut().unwrap().request = Some(request);
        let message = self.messages.diagnostic(&d, &self.effective_ui_locale);
        self.report(d, true);
        self.observe("prepare_failed", Some(request));
        if let Some(p) = self.core.state().pending.as_ref().filter(|_| {
            self.prepare
                .as_ref()
                .is_some_and(|p| matches!(p.purpose, Purpose::Activation))
        }) {
            self.core.step(
                CoreInput::PreparationFailed {
                    activation: p.id,
                    message,
                },
                0,
            );
        }
    }
    pub(super) fn complete(&mut self, request: u32, budget: &mut u32) -> Result<()> {
        if !self.accepts(request) {
            self.observe("stale_presentation_discarded", Some(request));
            return Ok(());
        }
        let prep = self.prepare.take().unwrap();
        if let Some(old) = prep.promoted_from {
            self.commands
                .push(AppCommand::CancelAssets { request: old });
        }
        let purpose = prep.purpose;
        let restart_locale =
            matches!(purpose, Purpose::Restore | Purpose::Rollback) && self.locale_pending();
        let lease = prep.job.finish()?;
        if !lease.valid(lease.activation, self.generation) {
            return Err(Diagnostic::new(
                "E_STALE_LEASE",
                "commit",
                "generation changed",
            ));
        }
        if matches!(purpose, Purpose::Menu | Purpose::Device)
            && self.pending_menu.is_some()
            && self.pending_menu_assets().is_subset(&prep.assets)
        {
            self.commit_menu_navigation()?;
        }
        if matches!(self.screen, Screen::Title | Screen::Menu) {
            if let Some(id) = self.active_menu_id() {
                if self.core.program().theme.image_menus[id]
                    .prepared_assets()
                    .is_subset(&prep.assets)
                {
                    self.prepared_menu = Some(self.menu_asset_stamp(id));
                }
            }
        }
        self.observe("lease_ready", Some(request));
        self.pauses.remove("prepare");
        self.pauses.remove("device");
        self.ui_pauses.remove("device");
        self.clear_fault_status();
        self.error = None;
        self.diagnostic = None;
        let commit_location = self.core.location();
        let commit_cue = self.core.state().pending.as_ref().map(|p| p.cue.clone());
        let commit_generation = self.generation;
        self.observe("commit_started", Some(request));
        match purpose {
            Purpose::Boot | Purpose::Menu | Purpose::TopUp => {}
            Purpose::Activation => self.step(
                CoreInput::Prepared {
                    activation: lease.activation,
                },
                budget,
            )?,
            Purpose::Restore | Purpose::Rollback => {
                let candidate = self
                    .candidate
                    .take()
                    .ok_or_else(|| Diagnostic::new("E_RESTORE", "commit", "no candidate"))?;
                if purpose == Purpose::Restore
                    && self
                        .replay_work
                        .as_ref()
                        .is_some_and(|w| w.phase == ReplayPhase::Returning)
                {
                    // The frozen session returns whole: its checkpoints, menu
                    // page and navigation locals never noticed the replay.
                    self.commit_replay_return(candidate)?;
                } else {
                    let mut candidate = candidate;
                    candidate.set_locale(&self.effective_text_locale)?;
                    self.generation.session += 1;
                    self.set_interface_hidden(false);
                    self.held_skip = false;
                    self.failed_admission = None;
                    self.reset_audio();
                    self.slot_restore = false;
                    self.core = candidate;
                    self.restore_work = None;
                    self.touch_snapshot_content(self.core.state())?;
                    self.screen = Screen::Story;
                    self.return_screen = Screen::Story;
                    self.pauses.remove("menu");
                    self.pauses.remove("fault");
                    self.pauses.insert("restored".into());
                    if matches!(purpose, Purpose::Rollback) {
                        self.checkpoints.pop();
                    } else {
                        self.checkpoints.clear();
                        self.checkpoints.push(self.core.snapshot());
                    }
                    self.auto = false;
                    self.skip = false;
                    self.held_skip = false;
                    self.restart_audio();
                }
            }
            Purpose::Replay => self.commit_replay_enter(lease.activation, budget)?,
            Purpose::Device => {
                self.pauses.remove("device");
                self.ui_pauses.remove("device");
                // Rebuilding renderer resources leaves the audio backend and
                // session tasks intact. Resume their existing pause policy;
                // only an actual snapshot restore needs new sound instances
                // and a reading confirmation.
            }
        }
        // Committing this cue can already reach the next preparation barrier.
        // Its catalog may still be loading, and its media has its own future
        // reservation. Transfer only resources authenticated/prepared by this
        // lease into the active group; keep the old group until replacement.
        let assets = self
            .retained_assets()
            .intersection(lease.assets())
            .cloned()
            .collect();
        let active = self.ledger.reserve(&self.costs(&assets)?)?;
        self.active = Some(active);
        drop(lease);
        self.commands.push(AppCommand::Observation {
            origin_session: None,
            task: None,
            sequence: None,
            stage: "prepare_commit".into(),
            request: Some(request),
            location: commit_location,
            cue: commit_cue,
            session: commit_generation.session,
            device: commit_generation.device,
        });
        if restart_locale {
            self.start_locale_switch()?;
        }
        self.restore_storage_warning();
        Ok(())
    }
    pub(super) fn restart_audio(&mut self) {
        for t in self
            .core
            .state()
            .tasks
            .values()
            .filter(|t| t.state == TaskState::Running)
        {
            if let Effect::Audio {
                asset,
                bus,
                looped,
                gain,
                loop_region,
            } = &t.effect
            {
                self.commands.push(AppCommand::AudioStart {
                    domain: TimeDomain::Story,
                    task: t.id,
                    asset: asset.clone(),
                    bus: *bus,
                    looped: *looped,
                    loop_region: *loop_region,
                    gain: *gain,
                    envelope: self.core.audio_envelope(t.id).0,
                    character: t.voice_character.clone(),
                    position_us: t.audio_position_us.unwrap_or(t.elapsed_us),
                    session: self.generation.session,
                });
                let (from, to, duration_us) = self.core.audio_envelope(t.id);
                let (owner, elapsed_us) = self.core.audio_envelope_checkpoint(t.id);
                self.commands.push(AppCommand::AudioEnvelope {
                    owner,
                    elapsed_us,
                    domain: TimeDomain::Story,
                    session: self.generation.session,
                    task: t.id,
                    from,
                    to,
                    duration_us,
                });
            }
        }
        self.commands.push(AppCommand::AudioPause {
            domain: TimeDomain::Story,
            paused: true,
        });
    }
    pub(super) fn cancel_slot_restore(&mut self) {
        if !std::mem::take(&mut self.slot_restore) {
            return;
        }
        let owns_error = self.diagnostic.as_ref().is_some_and(|d| {
            d.location == "load"
                || d.details.as_ref().is_some_and(|details| {
                    details.operation == "load"
                        || details.request.is_some_and(|request| {
                            self.prepare.as_ref().is_some_and(|p| {
                                p.request == request && matches!(p.purpose, Purpose::Restore)
                            }) || self.content.get(&request).is_some_and(|p| {
                                matches!(
                                    p.purpose,
                                    ContentPurpose::Restore(_, false)
                                        | ContentPurpose::RestoreValidation
                                        | ContentPurpose::RestoreBodies(false)
                                        | ContentPurpose::Media {
                                            purpose: Purpose::Restore,
                                            ..
                                        }
                                )
                            })
                        })
                        || (details.operation == "prepare"
                            && details.stage == "admission"
                            && self
                                .failed_admission
                                .as_ref()
                                .is_some_and(|(purpose, _, _)| matches!(purpose, Purpose::Restore)))
                })
        });
        if owns_error {
            self.error = None;
            self.diagnostic = None;
            self.status.clear();
        }
        // Invalid envelopes have not acquired a candidate lane: leave any
        // pre-existing Story or menu preparation intact in that case.
        if self.content.values().any(|p| {
            matches!(
                p.purpose,
                ContentPurpose::Restore(_, false)
                    | ContentPurpose::RestoreValidation
                    | ContentPurpose::RestoreBodies(false)
                    | ContentPurpose::Media {
                        purpose: Purpose::Restore,
                        ..
                    }
            )
        }) {
            self.cancel_content(false);
        }
        let owns_prepare = self
            .prepare
            .as_ref()
            .is_some_and(|p| matches!(p.purpose, Purpose::Restore))
            || self
                .failed_admission
                .as_ref()
                .is_some_and(|(p, _, _)| matches!(p, Purpose::Restore));
        let owns_deferred = self
            .deferred_prepare
            .as_ref()
            .is_some_and(|(_, p, _, _)| matches!(p, Purpose::Restore));
        if owns_prepare {
            self.cancel_preparation();
        }
        if owns_deferred {
            self.deferred_prepare = None;
        }
        if owns_prepare || owns_deferred {
            self.pauses.remove("prepare");
        }
        self.candidate = None;
        self.restore_work = None;
        if matches!(self.device_resume, Some(Purpose::Restore)) {
            self.device_resume = None;
        }
    }
    pub(super) fn accept_slot_load(&mut self, job: u32) -> Option<u32> {
        let (expected, slot, session, instance) = self.slot_load?;
        if expected != job {
            return None;
        }
        self.slot_load = None;
        if session != self.generation.session || instance != self.menu_session.instance {
            self.observe("stale_slot_load_discarded", Some(job));
            return None;
        }
        Some(slot)
    }
    pub(super) fn restore(&mut self, s: Snapshot) -> Result<()> {
        self.restore_with_purpose(s, false)
    }
    pub(super) fn restore_with_purpose(&mut self, s: Snapshot, rollback: bool) -> Result<()> {
        if self.validated.runtime_root().is_some() {
            return self.start_staged_restore(s, rollback);
        }
        self.cancel_content(false);
        let needs = self.restore_content_requirements(&s)?;
        if !needs.is_empty() {
            return self.begin_content(ContentPurpose::Restore(Box::new(s), rollback), needs);
        }
        let mut candidate = Core::restore(self.validated.clone(), s, &self.release)?;
        // Preferences are independent of saves. Frozen current instances retain
        // their saved language; future instances use the current preference.
        candidate.set_locale(&self.effective_text_locale)?;
        let assets = self.state_assets(&candidate);
        // Keep the validated candidate and restore kind before admission. A
        // failed admission, media retry or device recovery must resume the
        // same transaction, including its rollback checkpoint policy.
        self.candidate = Some(candidate);
        self.restore_work = Some(RestoreWork {
            session: None,
            proof: None,
            active_batches: VecDeque::new(),
            rollback,
        });
        self.begin_prepare(
            if rollback {
                Purpose::Rollback
            } else {
                Purpose::Restore
            },
            0,
            assets,
        )
    }
}
