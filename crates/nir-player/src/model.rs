use super::*;

impl Player {
    pub fn menu_depth(&self) -> usize {
        self.menu_session.depth()
    }
    pub fn model(&self) -> UiModel {
        self.model_for(&self.core, self.screen)
    }
    pub(super) fn model_for(&self, c: &Core, screen: Screen) -> UiModel {
        self.model_for_locale(c, screen, &self.effective_ui_locale)
    }
    pub(super) fn model_for_locale(&self, c: &Core, screen: Screen, ui_locale: &str) -> UiModel {
        let screen = if self.menu_peek && screen == Screen::Menu {
            Screen::Story
        } else {
            screen
        };
        let ui_plan = &c.program().locale_config.ui[ui_locale];
        let text_plan = &c.program().locale_config.text[&self.effective_text_locale];
        // Menus opened from the title use its retained background, even when
        // Core still contains the previous story or a restore is preparing.
        let title_context = screen == Screen::Title
            || (self.return_screen == Screen::Title
                && matches!(
                    screen,
                    Screen::Menu | Screen::Settings | Screen::Saves | Screen::History
                ));
        // While a reveal is in flight the window stays projectable in either
        // direction: the committed hidden flag only lands at its deadline.
        let window_transition = c
            .window_reveal()
            .filter(|_| !self.preferences.reduced_motion)
            .map(|(style, to_visible, progress)| WindowTransition {
                style: style.clone(),
                to_visible,
                progress,
            });
        let window_reveal_live = window_transition.is_some();
        UiModel {
            advance_wait: c.advance_wait().is_some(),
            transition_style: c.transition_style(),
            image_menu: self.active_menu_id().unwrap_or(&self.image_menu).to_owned(),
            authored_menu: self.active_menu_id().is_some()
                && !(screen == Screen::Menu && self.error.is_some() && self.pending_menu.is_none()),
            menu_instance: self.menu_session.instance,
            menu_revision: self.menu_session.revision,
            menu_depth: self.menu_session.depth(),
            menu_navigation_pending: self.pending_menu.is_some(),
            menu_locals: self.menu_session.locals.clone(),
            hovered_image: self.hovered_image.clone(),
            profile: self.profile.clone(),
            title: self.title.clone(),
            screen,
            nodes: if title_context {
                self.title_nodes()
            } else {
                c.sample_scene()
            },
            transition: if title_context || self.preferences.reduced_motion {
                None
            } else {
                c.transition()
            },
            stage: [
                c.program().stage.width as f32,
                c.program().stage.height as f32,
            ],
            dialogue_appearance: c.sample_dialogue_appearance(),
            dialogue_decorations: c.state().dialogue_decorations.clone(),
            interface_hidden: self.interface_hidden && screen == Screen::Story,
            hidden_dialogue: c.state().dialogue_hidden
                && c.dialogue().is_some()
                && window_transition.is_none(),
            window_transition,
            menu_transition: self
                .menu_effects
                .transition(self.ui_clock_us.0, self.preferences.reduced_motion),
            menu_element_animations: if self.preferences.reduced_motion {
                Default::default()
            } else {
                self.menu_effects.element_animations(self.ui_clock_us.0)
            },
            dialogue: c
                .dialogue()
                .filter(|_| !c.state().dialogue_hidden || window_reveal_live)
                .map(|(_, d)| DialogueView {
                    full_text: d.full_text(),
                    images: d.inline_images(),
                    visible_text: d.visible_text(),
                    speaker: d.speaker.clone(),
                    ready: d.awaiting_advance,
                    gate: d.at_gate,
                    locale: d.locale.clone(),
                    font_plan_digest: d.font_plan_digest.clone(),
                    font_assets: c.program().locale_config.text[&d.locale].fonts.clone(),
                    emphasis: {
                        let mut offset = 0;
                        d.spans
                            .iter()
                            .filter_map(|s| {
                                let start = offset;
                                offset += s.text.len();
                                s.emphasis.then_some((start, offset))
                            })
                            .collect()
                    },
                    ruby: {
                        let mut offset = 0;
                        d.spans
                            .iter()
                            .filter_map(|s| {
                                let start = offset;
                                offset += s.text.len();
                                s.ruby
                                    .as_ref()
                                    .map(|reading| (start, offset, reading.clone()))
                            })
                            .collect()
                    },
                }),
            choices: c
                .state()
                .choice
                .as_ref()
                .map(|choice_state| {
                    choice_state
                        .options
                        .iter()
                        .map(|o| ChoiceView {
                            id: o.id.clone(),
                            image: c
                                .program()
                                .choices
                                .get(&choice_state.id)
                                .and_then(|choice| {
                                    choice.options.iter().find(|option| option.id == o.id)
                                })
                                .and_then(|option| option.image.clone()),
                            label: o.label.clone(),
                            enabled: o.enabled,
                            selected: choice_state.selected.as_deref() == Some(o.id.as_str()),
                            locale: choice_state.locale.clone(),
                            font_plan_digest: choice_state.font_plan_digest.clone(),
                            font_assets: c.program().locale_config.text[&choice_state.locale]
                                .fonts
                                .clone(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            choice_cancellable: c
                .state()
                .choice
                .as_ref()
                .is_some_and(|c| c.on_cancel.is_some()),
            prefs: self.preferences.clone(),
            ui_locale: ui_locale.into(),
            ui_fonts: ui_plan.fonts.clone(),
            ui_font_plan_digest: ui_plan.digest.clone(),
            text_locale: self.effective_text_locale.clone(),
            available_ui_locales: ["zh-Hans", "en"]
                .into_iter()
                .filter(|locale| c.program().locale_config.ui.contains_key(*locale))
                .map(str::to_owned)
                .collect(),
            available_text_locales: ["zh-Hans", "en", "ja"]
                .into_iter()
                .filter(|locale| c.program().locale_config.text.contains_key(*locale))
                .map(str::to_owned)
                .collect(),
            text_fonts: text_plan.fonts.clone(),
            text_font_plan_digest: text_plan.digest.clone(),
            locale_pending: self.locale_pending(),
            locale_error: self.locale_error.clone(),
            preflight_texts: vec![],
            theme: {
                let base = &c.program().theme;
                if screen == Screen::Story {
                    if let Some(style) = c
                        .state()
                        .dialogue_style
                        .as_ref()
                        .and_then(|id| base.dialogue_styles.get(id))
                    {
                        let mut theme = (**base).clone();
                        theme.dialogue = style.dialogue.clone();
                        theme.text = style.text;
                        std::sync::Arc::new(theme)
                    } else {
                        std::sync::Arc::clone(base)
                    }
                } else {
                    std::sync::Arc::clone(base)
                }
            },
            history: if screen == Screen::History {
                Self::history_rows(c, self.history_offset, 3)
                    .into_iter()
                    .map(|row| row.entry)
                    .collect()
            } else {
                vec![]
            },
            history_total: c.state().history.len(),
            history_voice: self.history_voice_model(),
            character_voices: if screen == Screen::Settings {
                Self::character_voice_views(c)
            } else {
                vec![]
            },
            menu_history: self.menu_history_model(c, screen),
            menu_history_flow: self.menu_history_flow_model(screen),
            menu_opacity: self
                .menu_effects
                .opacity(self.ui_clock_us.0, self.preferences.reduced_motion),
            slots: self.slots.clone(),
            save_confirmation: self.save_confirmation.as_ref().map(|c| (c.token, c.slot)),
            busy_slots: self.save_jobs.values().map(|(slot, _)| *slot).collect(),
            menu_disabled: c.state().menu_disabled,
            can_save: !c.state().menu_disabled
                && !self.story_modal_load()
                && self.screen != Screen::Title
                && self.return_screen != Screen::Title
                && self.slot_load.is_none(),
            can_import: !cfg!(target_os = "android"),
            replay_active: self.replay_live(),
            menu_reading_modes: self.menu_reading_modes(),
            menu_story: self.menu_story_values(),
            paused: self.paused(),
            can_continue: self.pauses.is_only_named("restored"),
            loading: self.is_loading(),
            status: self.status.clone(),
            fault: self.error.clone(),
            retrying: self.retrying(),
            fault_recovery: self
                .diagnostic
                .as_ref()
                .and_then(|d| d.details.as_ref())
                .map(|d| d.recovery.clone())
                .unwrap_or_default(),
            auto: self.auto,
            skip: self.skip || self.held_skip,
            outcome: c.state().outcome.clone(),
            history_offset: self.history_offset,
        }
    }
    pub fn preview(&self) -> UiModel {
        let mut model = self.preview_inner();
        // Prepare the visible state too: restoring a transient mask must not
        // expose text that was skipped by resource/glyph preparation.
        model.interface_hidden = false;
        model
    }
    pub(super) fn preview_inner(&self) -> UiModel {
        if let Some(p) = &self.prepare {
            match p.purpose {
                Purpose::Activation => {
                    let mut c = self.core.clone();
                    c.step(
                        CoreInput::Prepared {
                            activation: p.job.activation,
                        },
                        0,
                    );
                    return self.model_for(&c, Screen::Story);
                }
                Purpose::Menu | Purpose::Device => {
                    if let Some(model) = self.menu_navigation_preview() {
                        return model;
                    }
                }
                Purpose::Restore | Purpose::Rollback => {
                    if let Some(c) = &self.candidate {
                        return self.model_for(c, Screen::Story);
                    }
                }
                _ => {}
            }
        }
        self.model()
    }
}
