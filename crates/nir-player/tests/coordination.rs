use nir_format::*;
use nir_player::*;
fn player() -> Player {
    Player::new(
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap(),
        "release".into(),
        "Test".into(),
    )
    .unwrap()
}
fn action(p: &mut Player, a: UiAction) -> Vec<AppCommand> {
    p.pump(
        vec![AppEvent::Action {
            action: a,
            interaction: p.current_interaction(),
            sequence: p.core().state().last_input + 1,
            session: p.generation.session,
        }],
        1000,
    )
}
fn ready(p: &mut Player, commands: Vec<AppCommand>) -> Vec<AppCommand> {
    let mut q = commands;
    let mut other = vec![];
    for _ in 0..30 {
        if q.is_empty() {
            return other;
        }
        let mut next = vec![];
        for c in q {
            match c {
                AppCommand::GetAssets {
                    request, assets, ..
                } => {
                    for asset in assets {
                        next.extend(p.pump(vec![AppEvent::AssetReady { request, asset }], 1000));
                    }
                }
                AppCommand::PreparePresentation { request } => {
                    next.extend(p.pump(vec![AppEvent::PresentationReady { request }], 1000))
                }
                _ => other.push(c),
            }
        }
        q = next;
    }
    panic!("prepare did not converge")
}
fn playing() -> Player {
    let mut p = player();
    let c = p.pump(vec![], 1000);
    ready(&mut p, c);
    let c = action(&mut p, UiAction::NewGame);
    ready(&mut p, c);
    p
}
#[test]
fn stale_asset_callbacks_cannot_commit() {
    let mut p = player();
    let initial = p.pump(vec![], 1000);
    let request = initial
        .iter()
        .find_map(|c| {
            if let AppCommand::GetAssets { request, .. } = c {
                Some(*request)
            } else {
                None
            }
        })
        .unwrap();
    p.viewport_changed().unwrap();
    let c = p.pump(
        vec![AppEvent::AssetReady {
            request,
            asset: "font.reader".into(),
        }],
        1000,
    );
    assert!(p.is_loading());
    ready(&mut p, c);
    assert!(!p.is_loading());
}
#[test]
fn menu_and_background_pause_owners_are_independent() {
    let mut p = playing();
    action(&mut p, UiAction::Menu);
    p.pump(vec![AppEvent::Hidden(true)], 1000);
    action(&mut p, UiAction::Close);
    assert!(p.paused());
    let tick = p.core().state().tick_us;
    p.pump(
        vec![AppEvent::Tick {
            delta_us: 1_000_000,
        }],
        1000,
    );
    assert_eq!(tick, p.core().state().tick_us);
    p.pump(vec![AppEvent::Hidden(false)], 1000);
    assert!(!p.paused());
}
#[test]
fn incompatible_restore_keeps_current_session() {
    let mut p = playing();
    let before = p.core().snapshot();
    let mut bad = before.clone();
    bad.release = "wrong".into();
    let digest = nir_content::digest(&serde_json::to_vec(&bad).unwrap());
    p.pump(
        vec![AppEvent::Loaded {
            envelope: Box::new(SaveEnvelope {
                format: 1,
                slot: 0,
                revision: 1,
                snapshot: bad,
                digest,
            }),
        }],
        1000,
    );
    assert_eq!(p.core().state().release, before.release);
    assert_eq!(p.core().state().tick_us, before.tick_us);
    assert!(p.error.is_some());
}
#[test]
fn save_job_survives_new_session() {
    let mut p = playing();
    let cmds = action(&mut p, UiAction::Save { slot: 0 });
    let job = cmds
        .iter()
        .find_map(|c| {
            if let AppCommand::Save { job, .. } = c {
                Some(*job)
            } else {
                None
            }
        })
        .unwrap();
    let before = p.generation.session;
    let c = action(&mut p, UiAction::NewGame);
    ready(&mut p, c);
    assert!(p.generation.session > before);
    p.pump(
        vec![AppEvent::Saved {
            job,
            slot: 0,
            revision: 1,
        }],
        1000,
    );
    assert!(p.status.contains("保存"));
}
#[test]
fn restore_is_candidate_then_paused_commit() {
    let mut p = playing();
    p.pump(vec![AppEvent::Tick { delta_us: 200_000 }], 1000);
    let snapshot = p.core().snapshot();
    let digest = nir_content::digest(&serde_json::to_vec(&snapshot).unwrap());
    let epoch = p.generation.session;
    let cmds = p.pump(
        vec![AppEvent::Loaded {
            envelope: Box::new(SaveEnvelope {
                format: 1,
                slot: 0,
                revision: 1,
                snapshot,
                digest,
            }),
        }],
        1000,
    );
    assert_eq!(p.generation.session, epoch);
    ready(&mut p, cmds);
    assert!(p.generation.session > epoch);
    assert!(p.paused());
    action(&mut p, UiAction::Continue);
    assert!(!p.paused());
}
#[test]
fn device_rebuild_does_not_restart_story() {
    let mut p = playing();
    p.pump(vec![AppEvent::Tick { delta_us: 240_000 }], 1000);
    let before = p.core().snapshot();
    p.pump(vec![AppEvent::DeviceLost], 1000);
    let c = p.pump(vec![AppEvent::DeviceReady], 1000);
    ready(&mut p, c);
    assert_eq!(p.core().state().tick_us, before.tick_us);
    assert_eq!(p.core().state().variables, before.variables);
    assert_eq!(p.core().state().history.len(), before.history.len());
    assert!(p.paused());
}
#[test]
fn consecutive_rollbacks_keep_the_earlier_checkpoints() {
    let mut p = playing();
    for _ in 0..20 {
        let c = action(&mut p, UiAction::Advance);
        ready(&mut p, c);
        let c = p.pump(
            vec![AppEvent::Tick {
                delta_us: 1_000_000,
            }],
            1000,
        );
        ready(&mut p, c);
        if p.core()
            .dialogue()
            .is_some_and(|(_, d)| d.text_id == "arrival")
        {
            break;
        }
    }
    let epoch = p.generation.session;
    let c = action(&mut p, UiAction::Rollback);
    ready(&mut p, c);
    assert!(p.generation.session > epoch);
    let epoch = p.generation.session;
    let c = action(&mut p, UiAction::Rollback);
    ready(&mut p, c);
    assert!(p.generation.session > epoch);
    assert!(p.paused());
    assert!(p.error.is_none(), "{:?}", p.error);
}

#[test]
fn device_loss_during_candidate_restore_keeps_the_candidate() {
    let mut p = playing();
    let snapshot = p.core().snapshot();
    let digest = nir_content::digest(&serde_json::to_vec(&snapshot).unwrap());
    p.pump(vec![AppEvent::Tick { delta_us: 300_000 }], 1000);
    let epoch = p.generation.session;
    p.pump(
        vec![AppEvent::Loaded {
            envelope: Box::new(SaveEnvelope {
                format: 1,
                slot: 0,
                revision: 1,
                snapshot: snapshot.clone(),
                digest,
            }),
        }],
        1000,
    );
    p.pump(vec![AppEvent::DeviceLost], 1000);
    let c = p.pump(vec![AppEvent::DeviceReady], 1000);
    ready(&mut p, c);
    assert!(p.generation.session > epoch);
    assert_eq!(p.core().state().tick_us, snapshot.tick_us);
    assert!(p.paused());
    assert!(!p.is_loading());
}

#[test]
fn invalid_locale_does_not_replace_the_open_dialogue() {
    let mut p = playing();
    let locale = p.preferences.locale.clone();
    let state = p.core().snapshot();
    action(
        &mut p,
        UiAction::Locale {
            locale: "missing".into(),
        },
    );
    assert_eq!(p.preferences.locale, locale);
    assert_eq!(p.core().state().history.len(), state.history.len());
    assert!(p.error.as_ref().unwrap().contains("E_LOCALE"));
}

#[test]
fn choice_input_in_the_deadline_pump_precedes_timeout() {
    for budget in [2, 1000] {
        choice_at_deadline(budget);
    }
}
fn choice_at_deadline(budget: u32) {
    let mut program: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    let choice = program.choices.get_mut("route").unwrap();
    choice.timeout_us = Some(Micros(300_000));
    choice.default = Some("stay".into());
    let mut p = Player::new(program, "timed".into(), "Test".into()).unwrap();
    let c = p.pump(vec![], 1000);
    ready(&mut p, c);
    let c = action(&mut p, UiAction::NewGame);
    ready(&mut p, c);
    for _ in 0..40 {
        if p.core().state().choice.is_some() {
            break;
        }
        let c = action(&mut p, UiAction::Advance);
        ready(&mut p, c);
        if p.core().state().choice.is_some() {
            break;
        }
        let events = p
            .core()
            .state()
            .tasks
            .values()
            .filter(|t| {
                t.state == nir_core::TaskState::Running
                    && matches!(t.effect, Effect::Audio { looped: false, .. })
            })
            .map(|t| AppEvent::AudioEnded {
                task: t.id,
                session: p.generation.session,
            })
            .chain([AppEvent::Tick { delta_us: 100_000 }])
            .collect();
        let c = p.pump(events, 1000);
        ready(&mut p, c);
    }
    assert!(p.core().state().choice.is_some());
    let c = p.pump(
        vec![
            AppEvent::Tick { delta_us: 300_000 },
            AppEvent::Action {
                action: UiAction::Choose {
                    option: "walk".into(),
                },
                interaction: p.current_interaction(),
                sequence: p.core().state().last_input + 1,
                session: p.generation.session,
            },
        ],
        budget,
    );
    ready(&mut p, c);
    for _ in 0..20 {
        let c = p.pump(vec![], budget);
        assert!(p.work_used() <= budget);
        ready(&mut p, c);
        if p.core().state().variables["affection"] == Value::I32(1) {
            break;
        }
    }
    assert_eq!(p.core().state().variables["affection"], Value::I32(1));
    assert!(p.error.is_none(), "{:?}", p.error);
}

#[test]
fn same_reason_tokens_release_only_their_own_pause() {
    let mut p = playing();
    let one = p.acquire_pause("plugin");
    let two = p.acquire_pause("plugin");
    assert!(p
        .pump(vec![], 10)
        .iter()
        .any(|c| matches!(c, AppCommand::AudioPause { paused: true })));
    drop(one);
    action(&mut p, UiAction::Menu);
    action(&mut p, UiAction::Close);
    assert!(p.paused());
    drop(two);
    assert!(p
        .pump(vec![], 10)
        .iter()
        .any(|c| matches!(c, AppCommand::AudioPause { paused: false })));
    assert!(!p.paused());
}

#[test]
fn external_pause_survives_new_game_and_token_outlives_player() {
    let mut p = playing();
    let token = p.acquire_pause("host");
    let c = action(&mut p, UiAction::NewGame);
    ready(&mut p, c);
    assert!(p.paused());
    drop(p);
    drop(token);
}

#[test]
fn work_is_retained_and_terminal_events_have_admission_room() {
    let mut p = playing();
    let mut events = vec![AppEvent::Tick { delta_us: 1 }; 128];
    events.push(AppEvent::LoadFailed("terminal".into()));
    p.pump(events, 0);
    assert_eq!(p.pending_events(), 129);
    assert_eq!(p.work_used(), 0);
    p.pump(vec![], 2);
    assert_eq!(p.diagnostic.as_ref().unwrap().message, "terminal");
    assert!(p.status.starts_with("E_STORAGE"));
    assert!(p.pending_events() > 0);
    assert!(p.work_used() <= 2);
    for _ in 0..200 {
        p.pump(vec![], 4);
        assert!(p.work_used() <= 4);
    }
    assert_eq!(p.pending_events(), 0);
}

#[test]
fn inbox_overflow_is_explicit_and_bounded() {
    let mut p = playing();
    p.pump(
        vec![AppEvent::LoadFailed("late".into()); EVENT_CAPACITY + 10],
        0,
    );
    assert_eq!(p.pending_events(), EVENT_CAPACITY);
    assert!(p.error.as_ref().unwrap().starts_with("E_EVENT_QUEUE"));
    assert!(p.paused());
}

#[test]
fn replacing_preparation_emits_one_cancellation_and_ignores_its_terminal() {
    let mut p = player();
    let commands = p.pump(vec![], 100);
    let old = commands
        .iter()
        .find_map(|c| match c {
            AppCommand::GetAssets { request, .. } => Some(*request),
            _ => None,
        })
        .unwrap();
    p.viewport_changed().unwrap();
    let commands = p.pump(
        vec![AppEvent::AssetFailed {
            request: old,
            message: "old failure".into(),
        }],
        100,
    );
    assert_eq!(
        commands
            .iter()
            .filter(|c| matches!(c, AppCommand::CancelAssets { request } if *request == old))
            .count(),
        1
    );
    assert!(p.error.is_none());
    ready(&mut p, commands);
    assert!(!p.is_loading());
}

#[test]
fn failed_preparation_cannot_be_committed_by_late_success() {
    let mut p = player();
    let c = p.pump(vec![], 100);
    let (request, assets) = c
        .into_iter()
        .find_map(|c| match c {
            AppCommand::GetAssets {
                request, assets, ..
            } => Some((request, assets)),
            _ => None,
        })
        .unwrap();
    let c = p.pump(
        vec![AppEvent::AssetFailed {
            request,
            message: "network failed".into(),
        }],
        100,
    );
    assert!(c
        .iter()
        .any(|c| matches!(c, AppCommand::CancelAssets { request: r } if *r == request)));
    let mut events: Vec<_> = assets
        .into_iter()
        .map(|asset| AppEvent::AssetReady { request, asset })
        .collect();
    events.push(AppEvent::PresentationReady { request });
    p.pump(events, 100);
    assert!(p.paused());
    assert!(p.is_loading());
    assert!(!p.accepts(request));
    assert_eq!(p.diagnostic.as_ref().unwrap().message, "network failed");
    assert!(p.error.as_ref().unwrap().starts_with("E_PREPARE"));
    let c = action(&mut p, UiAction::Retry);
    ready(&mut p, c);
    assert!(!p.is_loading());
    assert!(p.error.is_none());
}

#[test]
fn late_failure_does_not_overwrite_successful_save_status() {
    let mut p = playing();
    let c = action(&mut p, UiAction::Save { slot: 0 });
    let job = c
        .into_iter()
        .find_map(|c| match c {
            AppCommand::Save { job, .. } => Some(job),
            _ => None,
        })
        .unwrap();
    p.pump(
        vec![AppEvent::Saved {
            job,
            slot: 0,
            revision: 1,
        }],
        100,
    );
    let status = p.status.clone();
    p.pump(
        vec![AppEvent::SaveFailed {
            job,
            message: "duplicate".into(),
        }],
        100,
    );
    assert_eq!(p.status, status);
}

#[test]
fn many_events_cannot_multiply_the_story_work_budget() {
    let mut program: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    let f = program.functions.get_mut("main").unwrap();
    let b = f.blocks.get_mut(&f.entry).unwrap();
    b.ops.clear();
    b.terminator = Terminator::Goto {
        target: f.entry.clone(),
    };
    let mut p = Player::new(program, "loop".into(), "Test".into()).unwrap();
    let c = p.pump(vec![], 100);
    ready(&mut p, c);
    let events = vec![
        AppEvent::Action {
            action: UiAction::NewGame,
            interaction: 0,
            sequence: 1,
            session: p.generation.session,
        },
        AppEvent::Tick { delta_us: 100 },
        AppEvent::Tick { delta_us: 100 },
    ];
    p.pump(events, 7);
    assert_eq!(p.work_used(), 7);
    assert_eq!(p.core().state().unsuspended_ops, 6);
    assert!(p.pending_events() > 0);
    for _ in 0..10 {
        let before = p.core().state().unsuspended_ops;
        p.pump(vec![], 7);
        assert!(p.core().state().unsuspended_ops - before <= 7);
        assert_eq!(p.work_used(), 7);
    }
}

#[test]
fn preparation_diagnostic_keeps_identity_and_hides_internal_cause() {
    let mut p = player();
    let initial = p.pump(vec![], 1000);
    let request = initial
        .iter()
        .find_map(|c| match c {
            AppCommand::GetAssets { request, .. } => Some(*request),
            _ => None,
        })
        .unwrap();
    let mut d = Diagnostic::new("E_RESOURCE_FETCH", "f/b/1", "secret URL or browser cause")
        .classified(
            ErrorDomain::Prepare,
            "prepare",
            "fetch",
            vec![Recovery::Retry, Recovery::KeepCurrent],
        );
    d.details
        .as_mut()
        .unwrap()
        .references
        .push("asset.background".into());
    let commands = p.pump(
        vec![AppEvent::AssetFault {
            request,
            diagnostic: Box::new(d),
        }],
        1000,
    );
    let diagnostic = p.diagnostic.as_ref().unwrap();
    assert_eq!(diagnostic.details.as_ref().unwrap().request, Some(request));
    assert_eq!(
        diagnostic.details.as_ref().unwrap().session,
        Some(p.generation.session)
    );
    assert!(commands
        .iter()
        .any(|c| matches!(c, AppCommand::Diagnostic { .. })));
    assert!(!p.error.as_ref().unwrap().contains("secret"));
    assert!(p.paused());
    let late = p.pump(vec![AppEvent::PresentationReady { request }], 1000);
    assert!(late.iter().any(
        |c| matches!(c,AppCommand::Observation{stage,..} if stage=="stale_presentation_discarded")
    ));
    assert_eq!(p.diagnostic.as_ref().unwrap().code, "E_RESOURCE_FETCH");
    p.pump(
        vec![AppEvent::LoadFailed("independent storage error".into())],
        100,
    );
    assert_eq!(p.diagnostic.as_ref().unwrap().code, "E_RESOURCE_FETCH");
    assert!(p.model().fault_recovery.contains(&Recovery::Retry));
}

#[test]
fn save_diagnostic_retains_the_origin_session_after_replacement() {
    let mut p = playing();
    let origin = p.generation.session;
    let job = action(&mut p, UiAction::Save { slot: 0 })
        .into_iter()
        .find_map(|c| match c {
            AppCommand::Save { job, .. } => Some(job),
            _ => None,
        })
        .unwrap();
    let commands = action(&mut p, UiAction::Title);
    ready(&mut p, commands);
    assert_ne!(p.generation.session, origin);
    p.pump(
        vec![AppEvent::SaveFailed {
            job,
            message: "quota".into(),
        }],
        100,
    );
    let d = p.diagnostic.as_ref().unwrap();
    assert_eq!(d.details.as_ref().unwrap().session, Some(origin));
    assert_eq!(d.details.as_ref().unwrap().request, Some(job));
    assert!(p.error.is_none());
}

#[test]
fn work_defaults_yield_to_player_preferences_and_survive_new_sessions() {
    let mut program: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    program.player.font_scale = 1.2;
    program.player.bgm_volume = 0.1;
    program.player.reduced_motion = true;
    let mut p = Player::new(program, "release".into(), "Test".into()).unwrap();
    assert_eq!(p.preferences.font_scale, 1.2);
    assert_eq!(p.preferences.bgm_volume, 0.1);
    assert!(p.preferences.reduced_motion);
    let saved = Preferences {
        font_scale: 1.4,
        bgm_volume: 0.7,
        reduced_motion: false,
        ..Default::default()
    };
    let commands = p.pump(vec![AppEvent::Preferences(saved)], 1000);
    ready(&mut p, commands);
    let commands = action(&mut p, UiAction::NewGame);
    ready(&mut p, commands);
    assert_eq!(p.preferences.font_scale, 1.4);
    assert_eq!(p.preferences.bgm_volume, 0.7);
    assert!(!p.preferences.reduced_motion);
}

#[test]
fn theme_components_preserve_semantics_gates_and_viewport_bounds() {
    use nir_presentation::{project, ChoiceView, Messages, Screen};
    let p = playing();
    let mut m = p.model();
    m.screen = Screen::Story;
    m.loading = false;
    m.paused = false;
    m.choices = vec![
        ChoiceView {
            id: "walk".into(),
            label: "Walk".into(),
            enabled: true,
        },
        ChoiceView {
            id: "stay".into(),
            label: "Stay".into(),
            enabled: false,
        },
    ];
    let messages = Messages::default();
    for (width, height) in [(1280., 800.), (390., 844.), (844., 390.)] {
        for component in [DialogueComponent::Bottom, DialogueComponent::Top] {
            for choice in [ChoiceComponent::Standard, ChoiceComponent::Compact] {
                m.theme.slots.dialogue = component;
                m.theme.slots.choice = choice;
                m.prefs.font_scale = 1.5;
                m.dialogue.as_mut().unwrap().gate = true;
                let packet = project(&m, width, height, &messages);
                let advance = packet
                    .semantics
                    .iter()
                    .find(|s| s.action == UiAction::Advance)
                    .unwrap();
                assert!(!advance.enabled);
                assert!(packet
                    .semantics
                    .iter()
                    .any(|s| s.action == UiAction::Menu && s.enabled));
                let choices: Vec<_> = packet
                    .semantics
                    .iter()
                    .filter(|s| matches!(s.action, UiAction::Choose { .. }))
                    .collect();
                assert_eq!(choices.len(), 2);
                assert!(choices[0].enabled && !choices[1].enabled);
                assert_eq!(
                    packet.hit(choices[0].rect[0] + 2., choices[0].rect[1] + 2.),
                    Some(UiAction::Choose {
                        option: "walk".into()
                    })
                );
                for s in &packet.semantics {
                    assert!(
                        s.rect[0] >= 0.
                            && s.rect[1] >= 0.
                            && s.rect[0] + s.rect[2] <= width
                            && s.rect[1] + s.rect[3] <= height,
                        "{:?}",
                        s.rect
                    );
                }
                let text = packet.texts.iter().find(|t| t.visible.is_some()).unwrap();
                assert_eq!(text.size, (23. - if width < 650. { 4. } else { 0. }) * 1.5);
            }
        }
    }
}

#[test]
fn author_auto_delay_controls_reading_after_reveal_and_pauses() {
    let mut program: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    program.player.auto_delay_us = Micros(3_000_000);
    let mut p = Player::new(program, "release".into(), "Test".into()).unwrap();
    let c = p.pump(vec![], 1000);
    ready(&mut p, c);
    let c = action(&mut p, UiAction::NewGame);
    ready(&mut p, c);
    action(&mut p, UiAction::Advance);
    let interaction = p.current_interaction();
    assert!(p.core().dialogue().unwrap().1.awaiting_advance);
    action(&mut p, UiAction::ToggleAuto);
    for _ in 0..8 {
        p.pump(vec![AppEvent::Tick { delta_us: 250_000 }], 1000);
    }
    assert_eq!(p.current_interaction(), interaction);
    action(&mut p, UiAction::Menu);
    for _ in 0..20 {
        p.pump(vec![AppEvent::Tick { delta_us: 250_000 }], 1000);
    }
    assert_eq!(p.current_interaction(), interaction);
    action(&mut p, UiAction::Close);
    for _ in 0..16 {
        let c = p.pump(vec![AppEvent::Tick { delta_us: 250_000 }], 1000);
        ready(&mut p, c);
    }
    assert_ne!(p.current_interaction(), interaction);
}

fn reading_text() -> nir_presentation::TextEngine {
    let mut engine = nir_presentation::TextEngine::default();
    engine.add_font(
        include_bytes!("../../../examples/rain-letters/assets/source/reader.otf").to_vec(),
    );
    engine
}
#[test]
fn long_dialogue_browses_only_revealed_text_and_preserves_it_on_reflow() {
    use nir_presentation::{Messages, ReadingState};
    let p = playing();
    let mut m = p.model();
    m.loading = false;
    m.paused = false;
    m.prefs.font_scale = 1.5;
    m.prefs.locale = "en".into();
    let full = "末班电车刚刚离开。雨后书简。\n".repeat(35);
    m.dialogue.as_mut().unwrap().full_text = full.clone();
    m.dialogue.as_mut().unwrap().visible_text = "末班电车刚刚离开。".into();
    let mut text = reading_text();
    let mut reading = ReadingState::default();
    let messages = Messages::default();
    let initial = reading.project(&m, (1, 2), 390., 844., &messages, &mut text);
    assert!(initial.scrolls.is_empty());
    reading.hold_dialogue();
    m.dialogue.as_mut().unwrap().visible_text =
        full.lines().take(10).collect::<Vec<_>>().join("\n");
    m.dialogue.as_mut().unwrap().gate = true;
    let first = reading.project(&m, (1, 2), 390., 844., &messages, &mut text);
    let v = &first.scrolls[0];
    assert_eq!(v.offset, 0.);
    assert!(v.max > 100.);
    let gated_max = v.max;
    text.layout(&first);
    for node in first
        .semantics
        .iter()
        .filter(|n| matches!(n.action, UiAction::Scroll { .. }))
    {
        let run = first.texts.iter().find(|r| r.text == node.label).unwrap();
        let key = nir_presentation::TextEngine::key(run);
        let bottom = text.buffers[&key]
            .layout_runs()
            .map(|l| l.line_top + l.line_height)
            .fold(0., f32::max);
        assert!(bottom <= run.height, "page label must not be clipped");
        assert!(node.rect[0] + node.rect[2] <= v.rect[0] + v.rect[2]);
    }
    assert!(first
        .semantics
        .iter()
        .any(|s| matches!(s.action, UiAction::Advance) && !s.enabled));
    assert!(reading.scroll(ScrollRegion::Dialogue, 1, &first));
    let second = reading.project(&m, (1, 2), 390., 844., &messages, &mut text);
    assert!(second.scrolls[0].offset > 0.);
    let wider = reading.project(&m, (1, 2), 640., 800., &messages, &mut text);
    assert!(wider.scrolls[0].offset > 0.);
    assert!(wider.scrolls[0].offset <= wider.scrolls[0].max);
    assert_eq!(wider.announcement, full);
    m.dialogue.as_mut().unwrap().visible_text = full.clone();
    let rest = reading.project(&m, (1, 2), 390., 844., &messages, &mut text);
    assert!(rest.scrolls[0].max > gated_max * 2.);
    assert_eq!(
        rest.texts
            .iter()
            .find(|r| r.visible.is_some())
            .unwrap()
            .text,
        full
    );
    // Returning to automatic reading follows the reveal frontier again.
    m.auto = true;
    let auto = reading.project(&m, (1, 2), 390., 844., &messages, &mut text);
    assert_eq!(auto.scrolls[0].offset, auto.scrolls[0].max);
    m.auto = false;
    assert!(reading.scroll(ScrollRegion::Dialogue, -1, &auto));
    let manual = reading.project(&m, (1, 2), 390., 844., &messages, &mut text);
    assert!(manual.scrolls[0].offset < manual.scrolls[0].max);
    // A restored/new instance follows its current reveal position, not the prior viewport.
    let restored = reading.project(&m, (2, 3), 390., 844., &messages, &mut text);
    assert_eq!(restored.scrolls[0].offset, restored.scrolls[0].max);
}
#[test]
fn measured_choice_list_exposes_every_stable_option_with_bounded_hit_regions() {
    use nir_presentation::{ChoiceView, Messages, ReadingState};
    let p = playing();
    let mut m = p.model();
    m.dialogue = None;
    m.loading = false;
    m.paused = false;
    m.prefs.font_scale = 1.5;
    m.choices = (0..24)
        .map(|i| ChoiceView {
            id: format!("option-{i}"),
            label: if i == 3 {
                "沿着河边，一起走回去。".repeat(80)
            } else {
                format!("{i} 沿着河边，一起走回去。留在车站，读完这封信。")
            },
            enabled: i != 7,
        })
        .collect();
    let mut text = reading_text();
    let mut reading = ReadingState::default();
    let messages = Messages::default();
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..160 {
        let packet = reading.project(&m, (1, 3), 390., 600., &messages, &mut text);
        assert!(packet.quads.len() < 20);
        for node in &packet.semantics {
            assert!(node.rect[1] >= 0. && node.rect[1] + node.rect[3] <= 600.);
            if let UiAction::Choose { option } = &node.action {
                seen.insert(option.clone());
                assert_eq!(node.enabled, option != "option-7");
                if node.enabled {
                    assert_eq!(
                        packet.hit(node.rect[0] + 2., node.rect[1] + 2.),
                        Some(node.action.clone())
                    );
                }
            }
        }
        let view = packet
            .scrolls
            .iter()
            .find(|v| v.region == ScrollRegion::Choices)
            .unwrap();
        if view.offset >= view.max {
            break;
        }
        reading.scroll(ScrollRegion::Choices, 1, &packet);
    }
    assert_eq!(seen.len(), 24);
}
#[test]
fn history_allows_browsing_inside_a_long_entry() {
    use nir_presentation::{Messages, ReadingState, Screen};
    let p = playing();
    let mut m = p.model();
    m.screen = Screen::History;
    m.loading = false;
    m.history = vec![(String::new(), "雨后书简。\n".repeat(80))];
    let mut text = reading_text();
    let mut reading = ReadingState::default();
    let messages = Messages::default();
    let first = reading.project(&m, (1, 2), 390., 844., &messages, &mut text);
    assert_eq!(first.scrolls[0].region, ScrollRegion::History);
    assert_eq!(first.scrolls[0].offset, 0.);
    reading.scroll(ScrollRegion::History, 1, &first);
    let next = reading.project(&m, (1, 2), 390., 844., &messages, &mut text);
    assert!(next.scrolls[0].offset > 0.);
    m.history_offset = 3;
    let older = reading.project(&m, (1, 2), 390., 844., &messages, &mut text);
    assert!(older.scrolls.is_empty());
}

#[test]
fn mixed_line_endings_and_styled_paragraphs_keep_original_reveal_offsets() {
    use nir_presentation::{Messages, ReadingState, TextEngine};
    let p = playing();
    let mut m = p.model();
    let full = "雨\r后\r\n书\n\r简\n末\u{2029}班".repeat(30);
    let mut text = reading_text();
    let messages = Messages::default();
    for emphasis in [vec![], vec![(0, "雨".len())]] {
        let d = m.dialogue.as_mut().unwrap();
        d.full_text = full.clone();
        d.visible_text = "雨\r后".into();
        d.emphasis = emphasis;
        let packet = ReadingState::default().project(&m, (1, 2), 390., 844., &messages, &mut text);
        assert!(packet.scrolls.is_empty());
        let run = packet.texts.iter().find(|r| r.visible.is_some()).unwrap();
        let offsets = TextEngine::line_offsets(run);
        let buffer = &text.buffers[&TextEngine::key(run)];
        assert_eq!(buffer.lines.len(), offsets.len());
        for (line, offset) in buffer.lines.iter().zip(offsets) {
            assert!(full[offset..].starts_with(line.text()));
        }
        m.dialogue.as_mut().unwrap().visible_text = full.clone();
        let packet = ReadingState::default().project(&m, (1, 2), 390., 844., &messages, &mut text);
        assert!(packet.scrolls[0].max > 1000.);
    }
}
