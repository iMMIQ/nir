use super::*;

impl Player {
    pub(super) fn action(
        &mut self,
        a: UiAction,
        interaction: u32,
        sequence: u32,
        budget: &mut u32,
    ) -> Result<()> {
        // Input cannot mutate the committed page or enqueue another target
        // while navigation prepares. Close abandons only the pending target.
        if self.pending_menu.is_some() {
            match a {
                UiAction::Close => {
                    self.abandon_menu_navigation()?;
                    return Ok(());
                }
                UiAction::Title | UiAction::NewGame => self.abandon_menu_navigation()?,
                UiAction::Retry => {}
                _ => return Ok(()),
            }
        }
        // A close fade locks the old page's input for its finite duration.
        if self.menu_effects.closing.is_some() {
            return Ok(());
        }
        self.sync_menu_state()?;
        let a = match a {
            UiAction::ConfirmSave { token } => {
                let Some(c) = self.save_confirmation.as_ref().filter(|c| c.token == token) else {
                    return Ok(());
                };
                if self.is_loading() || self.slot_load.is_some() {
                    return Ok(());
                }
                let slot = c.slot;
                self.save_confirmation = None;
                UiAction::Save { slot }
            }
            UiAction::CancelSave { token } => {
                if self
                    .save_confirmation
                    .as_ref()
                    .is_some_and(|c| c.token == token)
                {
                    self.save_confirmation = None;
                }
                return Ok(());
            }
            UiAction::Close if self.save_confirmation.is_some() => {
                self.save_confirmation = None;
                return Ok(());
            }
            UiAction::Title | UiAction::Load { .. } if self.save_confirmation.is_some() => {
                self.save_confirmation = None;
                a
            }
            _ if self.save_confirmation.is_some() => return Ok(()),
            _ => a,
        };
        let a = if let UiAction::MenuValue {
            instance,
            revision,
            control,
            value,
        } = a
        {
            let Some(action) = self.resolve_menu_value(instance, revision, &control, value)? else {
                return Ok(());
            };
            action
        } else {
            a
        };
        let resolved_menu = matches!(&a, UiAction::MenuControl { .. });
        let a = if let UiAction::MenuControl {
            instance,
            revision,
            control,
        } = a
        {
            let Some(action) =
                self.resolve_menu_control(instance, revision, &control, interaction, sequence)?
            else {
                return Ok(());
            };
            action
        } else {
            a
        };
        if self.menu_peek && matches!(a, UiAction::Close) {
            self.set_interface_hidden(false);
            return Ok(());
        }
        if resolved_menu
            && matches!(a, UiAction::ToggleAuto | UiAction::ToggleSkip)
            && self.screen == Screen::Story
        {
            self.finish_story_modal(budget)?;
        }
        let cancelled_slot_restore = self.slot_restore;
        if matches!(
            &a,
            UiAction::Close
                | UiAction::Title
                | UiAction::NewGame
                | UiAction::ImageMenuEntry { .. }
                | UiAction::ImageMenuReplay { .. }
                | UiAction::ImageMenu { .. }
                | UiAction::Menu
                | UiAction::Settings
                | UiAction::Saves
                | UiAction::History
                | UiAction::Load { .. }
                | UiAction::Import
                | UiAction::Rollback
        ) {
            self.cancel_slot_restore();
            self.slot_load = None;
        }
        if self.interface_hidden
            && matches!(
                a,
                UiAction::Advance
                    | UiAction::Continue
                    | UiAction::Menu
                    | UiAction::ToggleAuto
                    | UiAction::ToggleSkip
                    | UiAction::HoldSkip { pressed: true }
                    | UiAction::ToggleInterface
                    | UiAction::RestoreInterface
            )
        {
            self.set_interface_hidden(false);
            return Ok(());
        }
        match a {
            UiAction::MenuValue { .. }
            | UiAction::MenuControl { .. }
            | UiAction::ConfirmSave { .. }
            | UiAction::CancelSave { .. } => unreachable!("resolved above"),
            UiAction::ToggleInterface => {
                if self.screen == Screen::Story
                    && !self.paused()
                    && !self.is_loading()
                    && self.core.state().choice.is_none()
                    && self.core.dialogue().is_some()
                {
                    self.set_interface_hidden(true);
                }
            }
            UiAction::RestoreInterface => {
                self.set_interface_hidden(false);
            }
            UiAction::ImageMenu { menu } => {
                let allowed = self.active_menu_id().and_then(|id|self.core.program().theme.image_menus.get(id)).is_some_and(|current|
                    current.controls().any(|(id,action,requires)| (resolved_menu || (self.screen==Screen::Title && !current.uses_state() && !current.uses_services() && current.buttons.iter().any(|b|b.id==id))) && matches!(action, nir_format::ImageMenuAction::Menu { menu: target } if target == &menu)
                        && requires.is_none_or(|key| self.profile.contains(key))));
                if matches!(self.screen, Screen::Title | Screen::Menu)
                    && allowed
                    && self.core.program().theme.image_menus.contains_key(&menu)
                {
                    self.select_menu_page(&menu)?;
                }
            }
            UiAction::HoverImage { id } => {
                self.hovered_image = id.filter(|id| {
                    if self.screen == Screen::Story {
                        return self.core.state().choice.as_ref().is_some_and(|choice| {
                            choice
                                .options
                                .iter()
                                .any(|option| option.id == *id && option.enabled)
                        });
                    }
                    self.active_menu_id()
                        .and_then(|menu| self.core.program().theme.image_menus.get(menu))
                        .is_some_and(|menu| menu.controls().any(|(control, _, _)| control == id))
                });
            }
            UiAction::NewGame | UiAction::ImageMenuEntry { .. } => {
                let function = match &a {
                    UiAction::ImageMenuEntry { function } => {
                        let allowed = self.screen == Screen::Title && self.core.program().theme.image_menus
                            .get(&self.image_menu).is_some_and(|menu| menu.controls().any(|(id,action,requires)|
                                (resolved_menu || (!menu.uses_state() && !menu.uses_services() && menu.buttons.iter().any(|b|b.id==id))) && matches!(action, nir_format::ImageMenuAction::Entry { function: target } if target == function)
                                && requires.is_none_or(|key| self.profile.contains(key))));
                        if !allowed {
                            return Ok(());
                        }
                        function.clone()
                    }
                    _ => {
                        self.image_menu = "title".into();
                        self.core.program().entry.clone()
                    }
                };
                if self.prepare.is_some() {
                    return Ok(());
                }
                let restart_locale = self.locale_pending();
                self.cancel_content(false);
                self.restore_work = None;
                self.candidate = None;
                // Starting a fresh story abandons any replay transaction:
                // its frozen state and half-entered candidate with it.
                self.replay_work = None;
                self.prefetch_attempted = None;
                self.generation.session += 1;
                self.set_interface_hidden(false);
                self.held_skip = false;
                self.failed_admission = None;
                self.reset_audio();
                self.core = Core::new_at(
                    self.validated.clone(),
                    self.release.clone(),
                    self.effective_text_locale.clone(),
                    &function,
                )?;
                self.screen = Screen::Story;
                self.return_screen = Screen::Story;
                self.pauses.retain(|r| r == "hidden");
                self.ui_pauses.retain(|r| r == "hidden");
                self.checkpoints.clear();
                self.error = None;
                self.diagnostic = None;
                self.status.clear();
                if restart_locale {
                    self.start_locale_switch()?;
                }
                // Freeze the first dialogue only after a pending language
                // transaction commits its effective context for this session.
                if !self.locale_pending() {
                    self.step(CoreInput::None, budget)?;
                }
            }
            UiAction::ImageMenuReplay { function } => {
                let allowed = matches!(self.screen, Screen::Title | Screen::Menu)
                    && self
                        .active_menu_id()
                        .and_then(|id| self.core.program().theme.image_menus.get(id))
                        .is_some_and(|menu| {
                            menu.controls().any(|(id, action, requires)| {
                                (resolved_menu
                                    || (!menu.uses_state()
                                        && !menu.uses_services()
                                        && menu.buttons.iter().any(|b| b.id == id)))
                                    && matches!(
                                        action,
                                        nir_format::ImageMenuAction::Replay {
                                            function: target
                                        } if target == &function
                                    )
                                    && requires.is_none_or(|key| self.profile.contains(key))
                            })
                        });
                if !allowed {
                    return Ok(());
                }
                self.begin_replay(&function, budget)?;
            }
            UiAction::ExitReplay => {
                if self.replay_live() {
                    self.begin_replay_return()?;
                }
            }
            UiAction::MenuHistoryScroll { .. } => {}
            // The Engine handles these only after current layout/clip checks.
            UiAction::MenuHistoryVoice { .. } => {}
            UiAction::Scroll {
                region: ScrollRegion::Menu | ScrollRegion::Saves | ScrollRegion::Settings,
                ..
            } => {}
            UiAction::Scroll { .. } => {
                if interaction != self.current_interaction() {
                    return Ok(());
                }
                // Browsing revealed text takes control back from automatic reading.
                self.auto = false;
                self.skip = false;
                self.held_skip = false;
                self.auto_elapsed = 0;
                self.auto_wait_delay = None;
                self.auto_voice_wait = None;
            }
            UiAction::Advance => {
                if self.screen == Screen::Story && !self.paused() {
                    self.auto_elapsed = 0;
                    self.auto_wait_delay = None;
                    self.auto_voice_wait = None;
                    self.step(
                        CoreInput::AdvanceReading {
                            interaction,
                            sequence,
                            stop_voice: !self.preferences.voice_continue,
                        },
                        budget,
                    )?;
                }
            }
            UiAction::Choose { option } => {
                if self.screen == Screen::Story && !self.paused() {
                    self.skip = false;
                    self.held_skip = false;
                    self.step(
                        CoreInput::Choose {
                            interaction,
                            option,
                            sequence,
                        },
                        budget,
                    )?;
                }
            }
            UiAction::CancelChoice => {
                // Only the pending interaction's own declaration authorizes a
                // cancel; hosts offer the affordance from the same state.
                if self.screen == Screen::Story
                    && !self.paused()
                    && self
                        .core
                        .state()
                        .choice
                        .as_ref()
                        .is_some_and(|c| c.interaction == interaction && c.on_cancel.is_some())
                {
                    self.skip = false;
                    self.held_skip = false;
                    self.step(
                        CoreInput::CancelChoice {
                            interaction,
                            sequence,
                        },
                        budget,
                    )?;
                }
            }
            UiAction::SelectChoice { option } => {
                // A focus-following cursor move on a typed-result interaction.
                // It observes a suspended interaction, so sequence identity is
                // irrelevant by construction.
                if self.screen == Screen::Story && !self.paused() {
                    let typed = self
                        .core
                        .state()
                        .choice
                        .as_ref()
                        .filter(|c| c.interaction == interaction && c.result.is_some())
                        .map(|c| c.interaction);
                    if let Some(interaction) = typed {
                        self.step(
                            CoreInput::SelectChoice {
                                interaction,
                                option,
                                sequence: 0,
                            },
                            budget,
                        )?;
                    }
                }
            }
            UiAction::Continue => {
                self.pauses.remove("restored");
                if self.prepare.is_none() {
                    if let Some(pending) = &self.core.state().pending {
                        self.begin_prepare(
                            Purpose::Activation,
                            pending.id,
                            self.validated.cue_assets(&pending.cue),
                        )?;
                    }
                }
                self.screen = Screen::Story;
            }
            UiAction::Menu | UiAction::Settings | UiAction::History | UiAction::Saves => {
                if self.screen == Screen::Story && self.core.state().menu_disabled {
                    return Ok(());
                }
                if matches!(a, UiAction::Menu) && self.screen != Screen::Menu {
                    self.overlay_menu = None;
                    // Force a fresh overlay when reopening from the story.
                    if self.screen != Screen::Title {
                        self.menu_session.menu.clear();
                    }
                }
                if matches!(self.screen, Screen::Title | Screen::Story | Screen::Ended) {
                    self.return_screen = self.screen;
                }
                self.held_skip = false;
                self.set_interface_hidden(false);
                self.screen = match a {
                    UiAction::Settings => Screen::Settings,
                    UiAction::History => Screen::History,
                    UiAction::Saves => Screen::Saves,
                    _ => Screen::Menu,
                };
                self.pauses.insert_barrier("menu".into());
                if self.screen == Screen::Saves
                    || self
                        .active_menu_id()
                        .and_then(|id| self.core.program().theme.image_menus.get(id))
                        .is_some_and(ImageMenu::uses_storage)
                {
                    self.commands.push(AppCommand::ListSaves);
                }
            }
            UiAction::Close => {
                if self.pop_menu()? {
                    return Ok(());
                }
                if self.begin_menu_close(
                    DeferredExitKind::CloseScreen {
                        cancelled_slot_restore,
                    },
                    interaction,
                    sequence,
                ) {
                    return Ok(());
                }
                self.commit_close(cancelled_slot_restore, budget)?;
            }
            UiAction::Title => {
                self.image_menu = "title".into();
                self.hovered_image = None;
                let restart_locale = self.locale_pending();
                self.cancel_content(false);
                self.reset_audio();
                self.cancel_preparation();
                self.candidate = None;
                self.restore_work = None;
                // Returning to the title abandons any replay transaction —
                // entering, live or returning — together with its frozen state.
                self.replay_work = None;
                self.prefetch_attempted = None;
                self.device_resume = None;
                self.pauses.retain(|r| r == "hidden");
                self.ui_pauses.retain(|r| r == "hidden");
                self.screen = Screen::Title;
                self.return_screen = Screen::Title;
                self.generation.session += 1;
                self.set_interface_hidden(false);
                self.held_skip = false;
                self.error = None;
                self.diagnostic = None;
                self.status.clear();
                self.begin_prepare(Purpose::Boot, 0, self.title_assets())?;
                if restart_locale {
                    self.start_locale_switch()?;
                }
            }
            UiAction::ToggleAuto => {
                self.auto = !self.auto;
                self.skip = false;
                self.held_skip = false;
                self.auto_elapsed = 0;
                self.auto_wait_delay = None;
                self.auto_voice_wait = None;
                if self.auto {
                    self.read_policy(0, budget)?;
                }
            }
            UiAction::HoldSkip { pressed } => {
                self.held_skip = pressed
                    && self.screen == Screen::Story
                    && !self.paused()
                    && self.core.state().choice.is_none();
                if self.held_skip {
                    self.auto = false;
                }
            }
            UiAction::ToggleSkip => {
                self.held_skip = false;
                self.skip = !self.skip;
                self.auto = false;
            }
            UiAction::UiLocale { locale } => {
                if !self.core.program().locale_config.ui.contains_key(&locale) {
                    return Err(Diagnostic::new(
                        "E_LOCALE",
                        "ui_locale",
                        "unsupported UI locale",
                    ));
                }
                self.preferences.ui_locale = locale;
                self.start_locale_switch()?;
                self.persist_preferences();
            }
            UiAction::TextLocale { locale } => {
                if !self.core.program().locale_config.text.contains_key(&locale) {
                    return Err(Diagnostic::new(
                        "E_LOCALE",
                        "text_locale",
                        "unsupported text locale",
                    ));
                }
                self.preferences.text_locale = locale;
                self.start_locale_switch()?;
                self.persist_preferences();
            }
            UiAction::LocaleRetry => {
                self.start_locale_switch()?;
            }
            UiAction::LocaleCancel => {
                self.cancel_locale_switch();
            }
            UiAction::FontSize { delta } => {
                let current = self.preferences.font_scale;
                self.preferences.font_scale = finite_clamp(current + delta, 0.8, 1.5, current);
                self.generation.typography += 1;
                self.persist_preferences();
                self.restart_preparation()?;
                if self.locale_pending() {
                    self.start_locale_switch()?;
                }
            }
            UiAction::TextSpeed { delta } => {
                self.preferences.text_speed =
                    finite_clamp(self.preferences.text_speed + delta, 0.25, 4., 1.);
                self.persist_preferences();
            }
            UiAction::AutoWait { delta } => {
                self.preferences.auto_wait_scale =
                    finite_clamp(self.preferences.auto_wait_scale + delta, 0.25, 4., 1.);
                self.persist_preferences();
            }
            UiAction::AutoWaitVoice { enabled } => {
                self.preferences.auto_wait_voice = enabled;
                self.persist_preferences();
            }
            UiAction::VoiceContinue { enabled } => {
                self.preferences.voice_continue = enabled;
                self.persist_preferences();
            }
            UiAction::Volume { bus, delta } => {
                let v = match bus {
                    AudioBus::Bgm => &mut self.preferences.bgm_volume,
                    AudioBus::Voice => &mut self.preferences.voice_volume,
                    AudioBus::Sfx => &mut self.preferences.sfx_volume,
                };
                *v = finite_clamp(*v + delta, 0., 1., *v);
                self.persist_preferences();
            }
            UiAction::CharacterVolume { character, delta } => {
                self.edit_character_voice(character, Some(delta), None);
            }
            UiAction::CharacterMute { character, muted } => {
                self.edit_character_voice(character, None, Some(muted));
            }
            UiAction::ReducedMotion => {
                self.preferences.reduced_motion = !self.preferences.reduced_motion;
                self.persist_preferences();
            }
            UiAction::Save { slot } => {
                // Replay sessions never touch storage: saves belong to the
                // frozen session, which returns with its own history intact.
                if self.replay_live()
                    || self.story_modal_load()
                    || self.core.state().menu_disabled
                    || self.return_screen == Screen::Title
                    || slot >= self.save_slots()
                    || self
                        .slots
                        .iter()
                        .any(|row| row.slot == slot && row.error.is_some())
                    || self
                        .save_jobs
                        .values()
                        .any(|(saved_slot, _)| *saved_slot == slot)
                {
                    return Ok(());
                }
                let s = self.core.snapshot();
                let bytes = serde_json::to_vec(&s).unwrap();
                if bytes.len() > MAX_INPUT_BYTES - 1024 {
                    return Err(Diagnostic::new(
                        "E_SAVE_LIMIT",
                        "save",
                        "snapshot exceeds import limit",
                    ));
                }
                let revision = self.slot_revisions.get(&slot).copied().unwrap_or(0);
                let next_revision = revision.checked_add(1).ok_or_else(|| {
                    Diagnostic::new("E_SAVE_LIMIT", "save", "slot revision exhausted")
                })?;
                self.request = self.request.checked_add(1).ok_or_else(|| {
                    Diagnostic::new("E_REQUEST_LIMIT", "save", "request identity exhausted")
                })?;
                let job = self.request;
                self.save_jobs.insert(job, (slot, self.generation.session));
                let envelope = SaveEnvelope {
                    format: 1,
                    slot,
                    revision: next_revision,
                    digest: nir_content::digest(&bytes),
                    snapshot: s,
                };
                self.commands.push(AppCommand::Save {
                    slot,
                    expected_revision: revision,
                    job,
                    envelope: Box::new(envelope),
                });
                self.status = if self.effective_ui_locale == "en" {
                    "Saving…"
                } else {
                    "正在保存…"
                }
                .into();
            }
            UiAction::Load { slot } => {
                // A load swaps the session underneath any replay transaction;
                // entering replays keep their frozen page until they settle.
                if self.replay_work.is_some() || slot >= self.save_slots() {
                    return Ok(());
                }
                self.request = self.request.checked_add(1).ok_or_else(|| {
                    Diagnostic::new("E_REQUEST_LIMIT", "load", "request identity exhausted")
                })?;
                let job = self.request;
                self.slot_load = Some((
                    job,
                    slot,
                    self.generation.session,
                    self.menu_session.instance,
                ));
                self.commands.push(AppCommand::Load { slot, job });
            }
            UiAction::Export => {
                // The frozen session exports on return; a replay core is not
                // a savable story state.
                if self.replay_live() {
                    return Ok(());
                }
                self.request = self.request.checked_add(1).ok_or_else(|| {
                    Diagnostic::new("E_REQUEST_LIMIT", "export", "request identity exhausted")
                })?;
                let job = self.request;
                self.export_job = Some(job);
                let snapshot = self.core.snapshot();
                let snapshot_json = serde_json::to_string(&snapshot).unwrap();
                let digest = nir_content::digest(snapshot_json.as_bytes());
                let json = format!(
                    r#"{{"format":1,"slot":0,"revision":0,"digest":"{digest}","snapshot":{snapshot_json}}}"#
                );
                self.commands.push(AppCommand::Export { job, json });
            }
            UiAction::Import => {
                // An import resolves as a load: never under a replay.
                if self.replay_work.is_some() {
                    return Ok(());
                }
                self.commands.push(AppCommand::Import);
            }
            UiAction::Rollback => {
                // Rollback inside a live replay rewinds the replay's own
                // checkpoints. Entering and returning transactions would swap
                // cores underneath the frozen session's candidate.
                let replay_settled = self
                    .replay_work
                    .as_ref()
                    .is_none_or(|w| w.phase == ReplayPhase::Active);
                if replay_settled && self.checkpoints.len() > 1 {
                    let s = self.checkpoints[self.checkpoints.len() - 2].clone();
                    self.restore_with_purpose(s, true)?;
                }
            }
            UiAction::HistoryPage { delta } => {
                self.stop_history_voice();
                self.history_offset = (self.history_offset as i64 + delta as i64)
                    .clamp(0, self.core.state().history.len().saturating_sub(1) as i64)
                    as usize;
            }
            UiAction::HistoryVoice { entry } => {
                if interaction == self.current_interaction() {
                    self.start_history_voice(entry);
                }
            }
            UiAction::StopHistoryVoice => {
                if interaction == self.current_interaction() {
                    self.stop_history_voice();
                }
            }
            UiAction::Retry => {
                if self.recovery_work_pending() {
                    return Ok(());
                }
                self.clear_fault_status();
                if let Some(job) = self
                    .content
                    .values()
                    .find(|p| p.failed && !matches!(p.purpose, ContentPurpose::Locale))
                    .cloned()
                {
                    self.begin_content(job.purpose, job.objects)?;
                } else if self.replay_entering() && self.candidate.is_some() {
                    // Retry the replay candidate's own media in isolation;
                    // the generic candidate branch below would misread it as
                    // a restore.
                    self.begin_replay_media()?;
                } else if self.candidate.is_some() {
                    let rollback = self.restore_work.as_ref().is_some_and(|work| work.rollback);
                    let purpose = if rollback {
                        Purpose::Rollback
                    } else {
                        Purpose::Restore
                    };
                    let assets = self
                        .candidate
                        .as_ref()
                        .map(|candidate| self.state_assets(candidate))
                        .unwrap_or_default();
                    self.begin_prepare(purpose, 0, assets)?;
                } else {
                    self.restart_preparation()?;
                }
            }
        }
        self.sync_menu_state()?;
        self.update_history_voice();
        Ok(())
    }
    pub(super) fn persist_preferences(&mut self) {
        self.preference_recovery.edited(&self.preferences);
        self.commands.push(AppCommand::PersistPreferences {
            preferences: self.preferences.clone(),
        });
    }
    pub(super) fn restart_preparation(&mut self) -> Result<()> {
        if let Some((purpose, activation, assets)) = self.failed_admission.clone() {
            return self.begin_prepare(purpose, activation, assets);
        }
        if let Some(prep) = self.prepare.as_ref() {
            let (purpose, activation) = (prep.purpose, prep.job.activation);
            let assets = self.retained_assets();
            self.begin_prepare(purpose, activation, assets)?;
        }
        Ok(())
    }
    pub(super) fn advance_story_time(&mut self, delta_us: u64, budget: &mut u32) -> Result<()> {
        if self.needs_story_clock() {
            let before = self.core.state().tick_us.0;
            self.step(CoreInput::Time { delta_us }, budget)?;
            self.read_policy(self.core.state().tick_us.0 - before, budget)?;
        }
        Ok(())
    }
    pub(super) fn read_policy(&mut self, delta: u64, budget: &mut u32) -> Result<()> {
        if self.interface_hidden || self.paused() || self.core.state().choice.is_some() {
            self.skip = false;
            self.held_skip = false;
            return Ok(());
        }
        let Some((_, d)) = self.core.dialogue() else {
            return Ok(());
        };
        let anchor = (
            self.generation.session,
            d.interaction,
            d.reading.as_ref().map_or(0, |r| r.revision),
        );
        if self.auto_anchor != Some(anchor) {
            self.auto_elapsed = 0;
            self.auto_wait_delay = None;
            self.auto_voice_wait = None;
            self.auto_anchor = Some(anchor);
        }
        let read = self
            .profile
            .contains(&format!("read:{}:{}", d.text_id, d.meaning_revision));
        if (self.skip || self.held_skip) && !read {
            self.skip = false;
            self.held_skip = false;
        }
        if (self.skip || self.held_skip) && read && !d.at_gate {
            let token = d.interaction;
            let sequence = self.core.state().last_input.saturating_add(1);
            self.step(
                CoreInput::AdvanceReading {
                    interaction: token,
                    sequence,
                    stop_voice: true,
                },
                budget,
            )?;
        } else if self.auto && d.awaiting_advance {
            // Freeze this player's preference at the reading boundary, like
            // the timer. Menu changes apply to the next automatic cycle.
            let wait_voice = *self
                .auto_voice_wait
                .get_or_insert(self.preferences.auto_wait_voice);
            let voice = wait_voice
                && if let Some(reading) = &d.reading {
                    reading.voice.is_some_and(|id| {
                        self.core
                            .state()
                            .tasks
                            .get(&id)
                            .is_some_and(|t| t.state == TaskState::Running)
                    })
                } else {
                    self.core.state().tasks.values().any(|t| {
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
                };
            let sampled = wait_voice
                && d.reading
                    .as_ref()
                    .is_some_and(|r| r.wait == VoiceWaitPolicy::SampledRemaining);
            if voice
                && d.reading
                    .as_ref()
                    .is_some_and(|r| r.wait == VoiceWaitPolicy::AfterVoice)
            {
                self.auto_elapsed = 0;
                self.auto_wait_delay = None;
            } else {
                let starting = self.auto_wait_delay.is_none();
                self.auto_wait_delay.get_or_insert_with(|| {
                    let base = self.core.program().player.auto_delay(
                        d.full_text().chars().count(),
                        self.preferences.auto_wait_scale,
                    );
                    let remaining = if sampled
                        && self.preferences.voice_volume > 0.
                        && d.reading
                            .as_ref()
                            .and_then(|r| r.voice)
                            .and_then(|id| self.core.state().tasks.get(&id))
                            .is_none_or(|task| {
                                self.preferences.character_voice_gain(&task.voice_character) > 0.
                            }) {
                        d.reading
                            .as_ref()
                            .and_then(|r| r.voice)
                            .and_then(|id| self.core.state().tasks.get(&id))
                            .filter(|t| t.state == TaskState::Running)
                            .and_then(|t| {
                                if let Effect::Audio { asset, .. } = &t.effect {
                                    self.core.program().asset(asset).map(|a| {
                                        a.duration_us.0.saturating_sub(
                                            t.audio_position_us.unwrap_or(t.elapsed_us).0,
                                        )
                                    })
                                } else {
                                    None
                                }
                            })
                            .unwrap_or(0)
                    } else {
                        0
                    };
                    base.saturating_add(remaining)
                });
                // A coarse clock turn can reveal the final character or stop
                // the voice partway through. Charge only the time after the
                // actual reading boundary, not the entire incoming turn.
                let elapsed = if !starting {
                    delta
                } else if sampled {
                    0 // Sampled policy freezes remaining voice time at now.
                } else {
                    let voice_end = if wait_voice
                        && d.reading
                            .as_ref()
                            .is_some_and(|r| r.wait == VoiceWaitPolicy::AfterVoice)
                    {
                        d.reading
                            .as_ref()
                            .and_then(|r| r.voice)
                            .and_then(|id| self.core.state().tasks.get(&id))
                            .map_or(0, |t| t.started_us.0.saturating_add(t.elapsed_us.0))
                    } else {
                        0
                    };
                    delta.min(
                        self.core
                            .state()
                            .tick_us
                            .0
                            .saturating_sub(d.last_reveal_us.0.max(voice_end)),
                    )
                };
                self.auto_elapsed = self.auto_elapsed.saturating_add(elapsed);
            }
            if (!voice || sampled)
                && self
                    .auto_wait_delay
                    .is_some_and(|delay| self.auto_elapsed >= delay)
            {
                let token = d.interaction;
                self.auto_elapsed = 0;
                self.auto_wait_delay = None;
                self.auto_voice_wait = None;
                self.step(
                    CoreInput::AdvanceReading {
                        interaction: token,
                        sequence: self.core.state().last_input.saturating_add(1),
                        stop_voice: !self.preferences.voice_continue,
                    },
                    budget,
                )?;
            }
        } else {
            self.auto_elapsed = 0;
            self.auto_wait_delay = None;
            self.auto_voice_wait = None;
        }
        Ok(())
    }
}
