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
                AppCommand::PrepareLocale { request, .. } => {
                    next.extend(p.pump(vec![AppEvent::LocaleReady { request }], 1000))
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
/// A rain.json variant whose story opens on a typed interaction: the route
/// options carry i32 values written to `picked`, and `mode` picks between
/// plain, typed, and typed-plus-cancel destinations.
fn typed_program(mode: &str) -> Program {
    let mut p: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    if mode != "plain" {
        p.requires.push("story.typed-result.v1".into());
    }
    p.variables.insert(
        "picked".into(),
        serde_json::from_value(serde_json::json!({"type":"i32","value":0})).unwrap(),
    );
    for (option, value) in p.choices.get_mut("route").unwrap().options.iter_mut().zip([1, 2]) {
        option.value =
            Some(serde_json::from_value(serde_json::json!({"type":"i32","value":value})).unwrap());
    }
    let f = p.functions.get_mut("main").unwrap();
    f.entry = "test".into();
    f.blocks.insert(
        "test".into(),
        serde_json::from_value(serde_json::json!({"terminator":{
            "type":"interact","choice":"route",
            "branches":{"walk":"after_walk","stay":"after_stay"},"on_empty":"failed",
            "result":(mode != "plain").then_some("picked"),
            "on_cancel":(mode == "cancel").then_some("gave_up")}}))
        .unwrap(),
    );
    for (block, outcome) in [("after_walk", "walk"), ("after_stay", "stay"), ("gave_up", "gave_up")] {
        f.blocks.insert(
            block.into(),
            serde_json::from_value(serde_json::json!({"terminator":{"type":"end","outcome":outcome}}))
                .unwrap(),
        );
    }
    p
}
fn typed_playing(mode: &str) -> Player {
    let mut p = Player::new(typed_program(mode), "release".into(), "Test".into()).unwrap();
    let c = p.pump(vec![], 1000);
    ready(&mut p, c);
    let c = action(&mut p, UiAction::NewGame);
    ready(&mut p, c);
    assert!(p.core().state().choice.is_some(), "typed interaction pending");
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
fn typed_interaction_save_and_load_restores_the_selection() {
    let mut p = typed_playing("typed");
    let interaction = p.current_interaction();
    // The semantic cursor moves through the same action path the engine's
    // focus sync uses; hover and keyboard focus stay presentation-only.
    let c = action(&mut p, UiAction::SelectChoice { option: "stay".into() });
    ready(&mut p, c);
    assert_eq!(
        p.core().state().choice.as_ref().unwrap().selected.as_deref(),
        Some("stay")
    );
    let snapshot = p.core().snapshot();
    let digest = nir_content::digest(&serde_json::to_vec(&snapshot).unwrap());
    let epoch = p.generation.session;
    let c = p.pump(
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
    ready(&mut p, c);
    assert!(p.generation.session > epoch);
    assert!(p.paused());
    action(&mut p, UiAction::Continue);
    // The pending interaction and its cursor restore together; restored
    // interactions receive fresh identities on top of the session change.
    let choice = p.core().state().choice.as_ref().unwrap();
    assert_ne!(choice.interaction, interaction);
    assert_eq!(choice.selected.as_deref(), Some("stay"));
    assert_eq!(choice.result.as_deref(), Some("picked"));
    let c = action(&mut p, UiAction::Choose { option: "stay".into() });
    ready(&mut p, c);
    assert_eq!(p.core().state().variables["picked"], Value::I32(2));
    assert_eq!(p.core().state().outcome.as_deref(), Some("stay"));
}

#[test]
fn rollback_after_a_typed_commit_rewinds_the_write() {
    let mut p = typed_playing("typed");
    let c = action(&mut p, UiAction::Choose { option: "stay".into() });
    ready(&mut p, c);
    assert_eq!(p.core().state().variables["picked"], Value::I32(2));
    assert_eq!(p.core().state().outcome.as_deref(), Some("stay"));
    // The offer checkpoint precedes the commit, so rollback re-suspends the
    // interaction with the typed variable back at its prior value.
    let epoch = p.generation.session;
    let c = action(&mut p, UiAction::Rollback);
    ready(&mut p, c);
    assert!(p.generation.session > epoch);
    assert!(p.paused());
    assert!(p.error.is_none(), "{:?}", p.error);
    assert_eq!(p.core().state().variables["picked"], Value::I32(0));
    assert_eq!(p.core().state().outcome, None);
    let choice = p.core().state().choice.as_ref().expect("interaction re-offered");
    assert_eq!(choice.selected.as_deref(), Some("walk"));
    action(&mut p, UiAction::Continue);
    let c = action(&mut p, UiAction::Choose { option: "stay".into() });
    ready(&mut p, c);
    assert_eq!(p.core().state().variables["picked"], Value::I32(2));
    assert_eq!(p.core().state().outcome.as_deref(), Some("stay"));
}

#[test]
fn player_cancel_branches_without_a_typed_write() {
    let mut p = typed_playing("cancel");
    let before = p.core().state().last_input;
    let c = action(&mut p, UiAction::CancelChoice);
    ready(&mut p, c);
    assert_eq!(p.core().state().variables["picked"], Value::I32(0));
    assert_eq!(p.core().state().outcome.as_deref(), Some("gave_up"));
    assert_eq!(p.core().state().last_input, before + 1);

    // Without a declared cancel target the affordance is refused: the
    // interaction stays pending and nothing is written.
    let mut p = typed_playing("typed");
    let c = action(&mut p, UiAction::CancelChoice);
    ready(&mut p, c);
    assert!(p.core().state().choice.is_some());
    assert_eq!(p.core().state().outcome, None);
    assert_eq!(p.core().state().variables["picked"], Value::I32(0));
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
    let locale = p.preferences.text_locale.clone();
    let state = p.core().snapshot();
    action(
        &mut p,
        UiAction::TextLocale {
            locale: "missing".into(),
        },
    );
    assert_eq!(p.preferences.text_locale, locale);
    assert_eq!(p.core().state().history.len(), state.history.len());
    assert!(p.error.as_ref().unwrap().contains("E_LOCALE"));
}

#[test]
fn ui_and_text_locales_commit_independently_and_freeze_open_text() {
    let mut p = playing();
    let original = p.core().dialogue().unwrap().1.locale.clone();
    let mut commands = action(
        &mut p,
        UiAction::UiLocale {
            locale: "en".into(),
        },
    );
    ready(&mut p, std::mem::take(&mut commands));
    assert_eq!(p.effective_ui_locale, "en");
    assert_eq!(p.effective_text_locale, "zh-Hans");
    assert_eq!(p.core().dialogue().unwrap().1.locale, original);

    commands = action(
        &mut p,
        UiAction::TextLocale {
            locale: "en".into(),
        },
    );
    ready(&mut p, commands);
    assert_eq!(p.effective_ui_locale, "en");
    assert_eq!(p.effective_text_locale, "en");
    assert_eq!(p.core().dialogue().unwrap().1.locale, original);

    let original_id = p.core().dialogue().unwrap().1.text_id.clone();
    let mut next = None;
    for _ in 0..100 {
        if let Some((_, d)) = p.core().dialogue() {
            if d.text_id != original_id {
                next = Some(d.locale.clone());
                break;
            }
            let commands = p.pump(
                vec![AppEvent::Tick {
                    delta_us: 60_000_000,
                }],
                1000,
            );
            ready(&mut p, commands);
            let commands = action(&mut p, UiAction::Advance);
            ready(&mut p, commands);
        } else if p.core().state().choice.is_some() {
            let option = p
                .core()
                .state()
                .choice
                .as_ref()
                .unwrap()
                .options
                .iter()
                .find(|option| option.enabled)
                .unwrap()
                .id
                .clone();
            let commands = action(&mut p, UiAction::Choose { option });
            ready(&mut p, commands);
        } else if let Some(task) = p.core().state().tasks.values().find(|task| {
            task.state == nir_core::TaskState::Running
                && matches!(task.effect, nir_format::Effect::Audio { looped: false, .. })
        }) {
            let commands = p.pump(
                vec![AppEvent::AudioEnded {
                    domain: TimeDomain::Story,
                    task: task.id,
                    session: p.generation.session,
                }],
                1000,
            );
            ready(&mut p, commands);
        } else if p
            .core()
            .state()
            .tasks
            .values()
            .any(|task| task.state == nir_core::TaskState::Running)
        {
            let commands = p.pump(
                vec![AppEvent::Tick {
                    delta_us: 1_000_000,
                }],
                1000,
            );
            ready(&mut p, commands);
        } else {
            break;
        }
    }
    assert_eq!(next.as_deref(), Some("en"));
}

#[test]
fn locale_failure_keeps_effective_context_and_retry_uses_a_new_generation() {
    let mut p = playing();
    let old_ui = p.effective_ui_locale.clone();
    let old_text = p.effective_text_locale.clone();
    let commands = action(
        &mut p,
        UiAction::TextLocale {
            locale: "en".into(),
        },
    );
    let request = commands
        .iter()
        .find_map(|c| match c {
            AppCommand::GetAssets { request, .. } => Some(*request),
            _ => None,
        })
        .unwrap();
    let preview = p.locale_preview(request).unwrap();
    assert_eq!(preview.ui_locale, old_ui);
    assert_eq!(preview.text_locale, "en");
    assert_eq!(
        preview.text_font_plan_digest,
        p.core().program().locale_config.text["en"].digest
    );
    p.pump(
        vec![AppEvent::LocaleFailed {
            request,
            message: "E_FONT_PLAN".into(),
        }],
        1000,
    );
    assert_eq!(p.effective_ui_locale, old_ui);
    assert_eq!(p.effective_text_locale, old_text);
    assert_eq!(p.core().dialogue().unwrap().1.locale, old_text);
    assert!(p.model().locale_error.is_some());
    assert!(
        p.error.is_none(),
        "language errors must not fault the story session"
    );
    p.pump(vec![AppEvent::LocaleReady { request }], 1000);
    assert_eq!(
        p.effective_text_locale, old_text,
        "late success after a failed attempt is ignored"
    );

    let retry = action(&mut p, UiAction::LocaleRetry);
    let retried = retry
        .iter()
        .find_map(|c| match c {
            AppCommand::GetAssets { request, .. } => Some(*request),
            _ => None,
        })
        .unwrap();
    assert!(retried > request);
    ready(&mut p, retry);
    assert_eq!(p.effective_text_locale, "en");

    let next = action(
        &mut p,
        UiAction::UiLocale {
            locale: "en".into(),
        },
    );
    let failed_request = next
        .iter()
        .find_map(|c| match c {
            AppCommand::GetAssets { request, .. } => Some(*request),
            _ => None,
        })
        .unwrap();
    p.pump(
        vec![AppEvent::LocaleFailed {
            request: failed_request,
            message: "E_FONT_PLAN".into(),
        }],
        1000,
    );
    let cancelled = action(&mut p, UiAction::LocaleCancel);
    ready(&mut p, cancelled);
    assert_eq!(p.effective_ui_locale, old_ui);
    assert_eq!(p.effective_text_locale, "en");
    assert_eq!(p.preferences.ui_locale, old_ui);
    assert!(!p.paused());
}

#[test]
fn locale_resource_failure_isolated_from_story_preparation() {
    let mut p = playing();
    let before = p.core().snapshot();
    let commands = action(
        &mut p,
        UiAction::TextLocale {
            locale: "en".into(),
        },
    );
    let request = commands
        .iter()
        .find_map(|command| match command {
            AppCommand::GetAssets {
                request, assets, ..
            } if assets == &["font.reader".to_owned()] => Some(*request),
            _ => None,
        })
        .unwrap();
    assert!(p.accepts_resource(request));
    assert!(!commands
        .iter()
        .any(|command| matches!(command, AppCommand::PrepareLocale { .. })));
    let failure = p.pump(
        vec![AppEvent::AssetFailed {
            request,
            message: "E_FONT_FETCH".into(),
        }],
        1000,
    );
    assert!(failure.iter().any(
        |command| matches!(command, AppCommand::CancelAssets { request: id } if *id == request)
    ));
    assert_eq!(
        serde_json::to_value(p.core().snapshot()).unwrap(),
        serde_json::to_value(before).unwrap()
    );
    assert_eq!(p.effective_text_locale, "zh-Hans");
    assert!(p.error.is_none());
    assert!(!p.paused());
    assert!(!p.accepts_resource(request));
    p.pump(
        vec![AppEvent::AssetReady {
            request,
            asset: "font.reader".into(),
        }],
        1000,
    );
    assert_eq!(p.effective_text_locale, "zh-Hans");
    let retry = action(&mut p, UiAction::LocaleRetry);
    ready(&mut p, retry);
    assert_eq!(p.effective_text_locale, "en");
}

#[test]
fn device_recovery_reissues_pending_locale_with_a_new_resource_generation() {
    let mut p = playing();
    let commands = action(
        &mut p,
        UiAction::TextLocale {
            locale: "en".into(),
        },
    );
    let old_request = commands
        .iter()
        .find_map(|command| match command {
            AppCommand::GetAssets { request, .. } => Some(*request),
            _ => None,
        })
        .unwrap();
    assert!(p.paused());
    let lost = p.pump(vec![AppEvent::DeviceLost], 1000);
    assert!(lost.iter().any(
        |command| matches!(command, AppCommand::CancelAssets { request } if *request == old_request)
    ));
    assert!(!p.accepts_resource(old_request));
    let commands = p.pump(vec![AppEvent::DeviceReady], 1000);
    let new_request = commands
        .iter()
        .filter_map(|command| match command {
            AppCommand::GetAssets { request, .. } if *request != old_request => Some(*request),
            _ => None,
        })
        .max()
        .unwrap();
    assert!(new_request > old_request);
    assert!(p.accepts_resource(new_request));
    p.pump(
        vec![AppEvent::AssetReady {
            request: old_request,
            asset: "font.reader".into(),
        }],
        1000,
    );
    ready(&mut p, commands);
    assert_eq!(p.effective_text_locale, "en");
    assert_eq!(p.core().dialogue().unwrap().1.locale, "zh-Hans");
    assert!(p.paused(), "device recovery keeps its own pause owner");
}

#[test]
fn latest_locale_candidate_wins_out_of_order_completion() {
    let mut p = playing();
    let first = action(
        &mut p,
        UiAction::TextLocale {
            locale: "en".into(),
        },
    );
    let first_request = first
        .iter()
        .find_map(|c| match c {
            AppCommand::GetAssets { request, .. } => Some(*request),
            _ => None,
        })
        .unwrap();
    let latest = action(
        &mut p,
        UiAction::UiLocale {
            locale: "en".into(),
        },
    );
    let latest_request = latest
        .iter()
        .find_map(|c| match c {
            AppCommand::GetAssets { request, .. } => Some(*request),
            _ => None,
        })
        .unwrap();
    assert!(latest_request > first_request);
    p.pump(
        vec![AppEvent::LocaleReady {
            request: first_request,
        }],
        1000,
    );
    assert_eq!(p.effective_ui_locale, "zh-Hans");
    assert_eq!(p.effective_text_locale, "zh-Hans");
    ready(&mut p, latest);
    assert_eq!(p.effective_ui_locale, "en");
    assert_eq!(p.effective_text_locale, "en");
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
                domain: TimeDomain::Story,
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
    assert!(p.pump(vec![], 10).iter().any(|c| matches!(
        c,
        AppCommand::AudioPause {
            domain: TimeDomain::Story,
            paused: true
        }
    )));
    drop(one);
    action(&mut p, UiAction::Menu);
    action(&mut p, UiAction::Close);
    assert!(p.paused());
    drop(two);
    assert!(p.pump(vec![], 10).iter().any(|c| matches!(
        c,
        AppCommand::AudioPause {
            domain: TimeDomain::Story,
            paused: false
        }
    )));
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
        text_speed: 2.,
        auto_wait_scale: 0.5,
        bgm_volume: 0.7,
        reduced_motion: false,
        ..Default::default()
    };
    let commands = p.pump(vec![AppEvent::Preferences(saved)], 1000);
    ready(&mut p, commands);
    let commands = action(&mut p, UiAction::NewGame);
    ready(&mut p, commands);
    assert_eq!(p.preferences.text_speed, 2.);
    assert_eq!(p.preferences.auto_wait_scale, 0.5);
    if let Some((id, dialogue)) = p.core().dialogue() {
        let Effect::Dialogue { reveal_us, .. } = p.core().state().tasks[&id].effect else {
            panic!()
        };
        assert_eq!(
            dialogue.reveal_interval_us,
            Some(Micros((reveal_us.0 as f64 / 2.).round() as u64))
        );
    } else {
        panic!("new session must have a dialogue");
    }
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
            selected: false,
            locale: m.text_locale.clone(),
            font_plan_digest: m.text_font_plan_digest.clone(),
            font_assets: m.text_fonts.clone(),
        },
        ChoiceView {
            id: "stay".into(),
            label: "Stay".into(),
            enabled: false,
            selected: false,
            locale: m.text_locale.clone(),
            font_plan_digest: m.text_font_plan_digest.clone(),
            font_assets: m.text_fonts.clone(),
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

#[test]
fn fixed_auto_delay_ignores_length_freezes_scale_and_pauses_with_story() {
    let mut program: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    program.player.auto_delay_policy = AutoDelayPolicy::Fixed;
    program.player.auto_delay_us = Micros(250_000);
    assert_eq!(program.player.auto_delay(1, 1.), 250_000);
    assert_eq!(program.player.auto_delay(10_000, 1.), 250_000);
    assert_eq!(program.player.auto_delay(1, 2.), 500_000);
    assert!(Player::new(program.clone(), "r".into(), "t".into()).is_err());
    program.requires.push("player.auto-delay-policy.v1".into());
    let mut p = Player::new(program, "r".into(), "t".into()).unwrap();
    let c = p.pump(vec![], 1000);
    ready(&mut p, c);
    let c = action(&mut p, UiAction::NewGame);
    ready(&mut p, c);
    action(&mut p, UiAction::Advance);
    let interaction = p.current_interaction();
    action(&mut p, UiAction::ToggleAuto);
    p.pump(vec![AppEvent::Tick { delta_us: 100_000 }], 1000);
    assert_eq!(p.current_interaction(), interaction);
    action(&mut p, UiAction::AutoWait { delta: 3. });
    action(&mut p, UiAction::Menu);
    p.pump(
        vec![AppEvent::Tick {
            delta_us: 1_000_000,
        }],
        1000,
    );
    action(&mut p, UiAction::Close);
    p.pump(vec![AppEvent::Tick { delta_us: 149_999 }], 1000);
    assert_eq!(p.current_interaction(), interaction);
    let c = p.pump(vec![AppEvent::Tick { delta_us: 1 }], 1000);
    ready(&mut p, c);
    assert_ne!(p.current_interaction(), interaction);
}
#[test]
fn zero_auto_delay_is_explicit_and_legacy_default_still_includes_text_length() {
    let mut player = PlayerDefaults::default();
    assert_eq!(player.auto_delay(10, 1.), 1_400_000);
    player.auto_delay_us = Micros(0);
    assert!(validate_ui_config(&Theme::default(), &player).is_err());
    player.auto_delay_policy = AutoDelayPolicy::Fixed;
    assert!(validate_ui_config(&Theme::default(), &player).is_ok());
    assert_eq!(player.auto_delay(10_000, 4.), 0);
}

fn reading_text() -> nir_presentation::TextEngine {
    let mut engine = nir_presentation::TextEngine::default();
    engine
        .add_font_asset(
            "font.reader",
            include_bytes!("../../../examples/rain-letters/assets/source/reader.otf").to_vec(),
        )
        .unwrap();
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
    m.ui_locale = "en".into();
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
            selected: false,
            locale: m.text_locale.clone(),
            font_plan_digest: m.text_font_plan_digest.clone(),
            font_assets: m.text_fonts.clone(),
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
    m.history = vec![nir_presentation::HistoryView {
        speaker: String::new(),
        text: "雨后书简。\n".repeat(80),
        locale: "zh-Hans".into(),
        font_plan_digest: m.text_font_plan_digest.clone(),
        font_assets: m.text_fonts.clone(),
    }];
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
    // The Player now supplies the visible window, not the entire history.
    // Moving to an empty window must still discard the previous text scroll.
    m.history.clear();
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

#[test]
fn dialogue_opacity_multiplies_author_colors_without_fading_scene() {
    use nir_presentation::{project, Messages};
    let p = playing();
    let mut model = p.model();
    model.loading = false;
    model.paused = false;
    let messages = Messages::default();
    for background in [None, Some("test-box".into())] {
        model.theme.dialogue.background = background;
        model.dialogue_appearance = DialogueAppearance::default();
        let before = project(&model, 1280., 800., &messages);
        model.dialogue_appearance = DialogueAppearance {
            opacity: 0.5,
            background_opacity: 0.4,
            text_opacity: 0.6,
        };
        let after = project(&model, 1280., 800., &messages);
        assert_eq!(before.quads.len(), after.quads.len());
        assert_eq!(before.texts.len(), after.texts.len());
        let dialogue = model.dialogue.as_ref().unwrap();
        let old_text = before
            .texts
            .iter()
            .find(|t| t.text == dialogue.full_text)
            .unwrap();
        let new_text = after
            .texts
            .iter()
            .find(|t| t.text == dialogue.full_text)
            .unwrap();
        assert!((new_text.color[3] - old_text.color[3] * 0.3).abs() < 0.00001);
        let changed: Vec<_> = before
            .quads
            .iter()
            .zip(&after.quads)
            .filter(|(a, b)| a.color != b.color)
            .collect();
        assert!(!changed.is_empty());
        for (a, b) in changed {
            assert!((b.color[3] - a.color[3] * 0.2).abs() < 0.00001);
        }
        for (a, b) in before.quads.iter().zip(&after.quads) {
            if a.asset.is_some() && model.nodes.iter().any(|n| n.asset == a.asset) {
                assert_eq!(a.color, b.color);
            }
        }
        assert_eq!(before.semantics.len(), after.semantics.len());
    }
}

/// A story whose entry block commits a styled window reveal and then parks on
/// the intro dialogue, so the reveal runs while the reader holds input.
fn reveal_player(style: StageTransition, duration_us: u64, reduced_motion: bool) -> Player {
    let mut program: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    program.requires.push("text.window-transition.v1".into());
    let f = program.functions.get_mut("main").unwrap();
    f.entry = "test".into();
    f.blocks.insert(
        "test".into(),
        serde_json::from_value(serde_json::json!({
            "ops":[{"id":"reveal","operation":{
                "type":"dialogue_visibility","visible":false,
                "transition":serde_json::to_value(&style).unwrap(),
                "duration_us":duration_us.to_string()}}],
            "terminator":{"type":"activate","cue":"intro","next":"hold"}
        }))
        .unwrap(),
    );
    for (block, body) in [
        (
            "hold",
            serde_json::json!({"terminator":{
                "type":"await","conditions":[{"task":"line","milestone":{"type":"finished"}}],
                "next":"done","on_cancelled":"done","on_failed":"done"}}),
        ),
        (
            "done",
            serde_json::json!({"terminator":{"type":"end","outcome":"done"}}),
        ),
    ] {
        f.blocks
            .insert(block.into(), serde_json::from_value(body).unwrap());
    }
    let mut p = Player::new(program, "release".into(), "Test".into()).unwrap();
    p.preferences.reduced_motion = reduced_motion;
    let commands = p.pump(vec![], 1000);
    ready(&mut p, commands);
    let commands = action(&mut p, UiAction::NewGame);
    ready(&mut p, commands);
    action(&mut p, UiAction::Advance);
    p
}

#[test]
fn window_reveal_projection_follows_the_story_clock_and_commits_at_the_deadline() {
    let mut p = reveal_player(StageTransition::Dissolve, 1_000_000, false);
    let commands = p.pump(vec![AppEvent::Tick { delta_us: 500_000 }], 1000);
    ready(&mut p, commands);
    let m = p.model();
    let w = m.window_transition.as_ref().expect("reveal live in the model");
    assert_eq!(w.style, StageTransition::Dissolve);
    assert!(!w.to_visible);
    assert!((w.progress - 0.5).abs() < 0.001);
    assert!(!m.hidden_dialogue, "the committed flag lands at the deadline");
    assert!(m.dialogue.is_some(), "the view stays projectable while live");
    let commands = p.pump(vec![AppEvent::Tick { delta_us: 600_000 }], 1000);
    ready(&mut p, commands);
    let m = p.model();
    assert!(m.window_transition.is_none());
    assert!(m.hidden_dialogue);
    assert!(m.dialogue.is_none());
}

#[test]
fn reduced_motion_suppresses_the_reveal_and_jumps_at_the_deadline() {
    let mut p = reveal_player(StageTransition::Dissolve, 1_000_000, true);
    let commands = p.pump(vec![AppEvent::Tick { delta_us: 500_000 }], 1000);
    ready(&mut p, commands);
    let m = p.model();
    assert!(m.window_transition.is_none());
    assert!(
        !m.hidden_dialogue,
        "committed flag still deferred under reduced motion"
    );
    let commands = p.pump(vec![AppEvent::Tick { delta_us: 600_000 }], 1000);
    ready(&mut p, commands);
    assert!(p.model().hidden_dialogue);
}

#[test]
fn mid_reveal_save_and_load_restores_the_flight() {
    let mut p = reveal_player(StageTransition::Dissolve, 1_000_000, false);
    let commands = p.pump(vec![AppEvent::Tick { delta_us: 500_000 }], 1000);
    ready(&mut p, commands);
    let envelope = slot_envelope(&p, 0);
    let commands = p.pump(vec![AppEvent::Loaded { envelope }], 1000);
    ready(&mut p, commands);
    assert!(p.paused());
    action(&mut p, UiAction::Continue);
    let m = p.model();
    let w = m.window_transition.expect("reveal restored mid-flight");
    assert_eq!(w.style, StageTransition::Dissolve);
    assert!((w.progress - 0.5).abs() < 0.001);
    assert!(!m.hidden_dialogue);
}

#[test]
fn window_mask_reveals_fetch_their_mask_once_and_hold_the_story_clock() {
    let style = StageTransition::Mask {
        asset: "bg.river".into(),
        channel: MaskChannel::Alpha,
        invert: false,
        softness: 0.2,
    };
    let mut program: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    program.requires.push("text.window-transition.v1".into());
    let f = program.functions.get_mut("main").unwrap();
    f.entry = "test".into();
    f.blocks.insert(
        "test".into(),
        serde_json::from_value(serde_json::json!({
            "ops":[{"id":"reveal","operation":{
                "type":"dialogue_visibility","visible":false,
                "transition":serde_json::to_value(&style).unwrap(),
                "duration_us":"1000000"}}],
            "terminator":{"type":"activate","cue":"intro","next":"hold"}
        }))
        .unwrap(),
    );
    for (block, body) in [
        (
            "hold",
            serde_json::json!({"terminator":{
                "type":"await","conditions":[{"task":"line","milestone":{"type":"finished"}}],
                "next":"done","on_cancelled":"done","on_failed":"done"}}),
        ),
        (
            "done",
            serde_json::json!({"terminator":{"type":"end","outcome":"done"}}),
        ),
    ] {
        f.blocks
            .insert(block.into(), serde_json::from_value(body).unwrap());
    }
    let mut p = Player::new(program, "release".into(), "Test".into()).unwrap();
    let commands = p.pump(vec![], 1000);
    ready(&mut p, commands);
    // Drive the new session but withhold the mask top-up request once it
    // appears: the reveal must not run while its media is outstanding.
    let mut queue = action(&mut p, UiAction::NewGame);
    let mut mask_request: Option<(u32, Vec<String>)> = None;
    for _ in 0..40 {
        let mut next = vec![];
        for c in queue {
            match c {
                AppCommand::GetAssets { request, assets, .. } => {
                    if assets.iter().any(|a| a == "bg.river") {
                        assert!(mask_request.replace((request, assets)).is_none(),
                            "the mask is fetched by exactly one top-up");
                    } else {
                        for asset in assets {
                            next.extend(
                                p.pump(vec![AppEvent::AssetReady { request, asset }], 1000)
                            );
                        }
                    }
                }
                AppCommand::PreparePresentation { request } => next
                    .extend(p.pump(vec![AppEvent::PresentationReady { request }], 1000)),
                AppCommand::PrepareLocale { request, .. } => {
                    next.extend(p.pump(vec![AppEvent::LocaleReady { request }], 1000))
                }
                _ => {}
            }
        }
        queue = next;
        if queue.is_empty() {
            break;
        }
    }
    let (request, withheld) =
        mask_request.expect("the reveal mask is fetched by a top-up");
    assert!(
        p.paused(),
        "the story clock holds while the mask is outstanding"
    );
    let before = p.core().state().tick_us;
    p.pump(vec![AppEvent::Tick { delta_us: 5_000_000 }], 1000);
    assert_eq!(
        p.core().state().tick_us, before,
        "no story time passes before the media lands"
    );
    let commands = p.pump(
        withheld
            .into_iter()
            .map(|asset| AppEvent::AssetReady { request, asset })
            .collect(),
        1000,
    );
    ready(&mut p, commands);
    action(&mut p, UiAction::Advance);
    assert!(p.retained_assets().contains("bg.river"));
    let commands = p.pump(vec![AppEvent::Tick { delta_us: 500_000 }], 1000);
    ready(&mut p, commands);
    let m = p.model();
    let w = m.window_transition
        .as_ref()
        .expect("reveal resumed after the fetch");
    assert_eq!(w.style, style);
    assert!((w.progress - 0.5).abs() < 0.001);
    // One top-up per reveal: the completed fetch is not re-issued.
    let later = p.pump(vec![], 1000);
    assert!(!later.iter().any(|c| match c {
        AppCommand::GetAssets { assets, .. } => assets.contains(&"bg.river".to_owned()),
        _ => false,
    }));
}

#[test]
fn window_reveal_dissolve_folds_coverage_into_window_items_only() {
    use nir_presentation::{project, Messages, WindowTransition};
    let p = interface_player(HidePolicy::ContinueStory, false);
    let mut model = p.model();
    model.loading = false;
    model.paused = false;
    let messages = Messages::default();
    let before = project(&model, 1280., 800., &messages);
    assert!(model.dialogue.is_some());
    model.window_transition = Some(WindowTransition {
        style: StageTransition::Dissolve,
        to_visible: false,
        progress: 0.25,
    });
    let after = project(&model, 1280., 800., &messages);
    assert_eq!(before.quads.len(), after.quads.len());
    assert_eq!(before.texts.len(), after.texts.len());
    assert!(after.window_layers.is_none(), "dissolve needs no diversion");
    // The window items (box, accent, window texts) take the coverage; the
    // backdrop quad and the HUD controls outside the range keep their color.
    let mut faded_quads = 0;
    for (a, b) in before.quads.iter().zip(&after.quads) {
        assert_eq!(b.asset, a.asset);
        if a.color != b.color {
            faded_quads += 1;
            assert!((b.color[3] - a.color[3] * 0.75).abs() < 0.00001);
        }
    }
    assert!(faded_quads > 0, "the window box must fade");
    assert!(faded_quads < before.quads.len(), "HUD quads keep their color");
    let mut faded_texts = 0;
    for (a, b) in before.texts.iter().zip(&after.texts) {
        assert_eq!(b.text, a.text);
        if a.color != b.color {
            faded_texts += 1;
            assert!((b.color[3] - a.color[3] * 0.75).abs() < 0.00001);
        }
    }
    assert!(faded_texts > 0, "the window text must fade");
    assert!(
        faded_texts < before.texts.len(),
        "HUD button labels keep their color"
    );
}

#[test]
fn window_reveal_wipe_diverts_window_layers_behind_a_sentinel() {
    use nir_presentation::{project, Messages, WindowTransition};
    let wipe = StageTransition::Wipe {
        direction: WipeDirection::LeftToRight,
        softness: 0.,
    };
    let p = interface_player(HidePolicy::ContinueStory, false);
    let mut model = p.model();
    model.loading = false;
    model.paused = false;
    let messages = Messages::default();
    let before = project(&model, 1280., 800., &messages);
    model.window_transition = Some(WindowTransition {
        style: wipe.clone(),
        to_visible: true,
        progress: 0.5,
    });
    let after = project(&model, 1280., 800., &messages);
    let layers = after.window_layers.as_ref().expect("spatial styles divert");
    assert_eq!(layers.style, wipe);
    assert!(layers.to_visible);
    assert_eq!(layers.progress, 0.5);
    // The diverted window run left the main list exactly once, replaced in
    // place by a full-surface sentinel at the window's z-position.
    let at = after
        .quads
        .iter()
        .position(|q| q.asset.as_deref() == Some("@window"))
        .expect("a sentinel quad marks the window");
    let sentinel = &after.quads[at];
    assert_eq!(sentinel.rect, [0., 0., 1280., 800.]);
    let diverted = layers.quads.len();
    assert!(diverted > 0);
    assert_eq!(&before.quads[..at], &after.quads[..at]);
    assert_eq!(&layers.quads, &before.quads[at..at + diverted]);
    assert_eq!(&after.quads[at + 1..], &before.quads[at + diverted..]);
    assert_eq!(
        after.quads.len(),
        before.quads.len() - diverted + 1
    );
    // Window texts stay in the packet for layout, routed to the window pass.
    assert_eq!(after.texts, before.texts);
    assert!(!layers.texts.is_empty());
    assert!(layers
        .texts
        .windows(2)
        .all(|pair| pair[0] + 1 == pair[1]));
    assert!(layers.texts.iter().any(|&i| before.texts[i].region.is_some()));
    assert!(after
        .semantics
        .iter()
        .any(|s| s.action == UiAction::Advance));
}

#[test]
fn foreground_and_story_pause_owners_and_clocks_are_independent() {
    let mut p = playing();
    let story_before = p.core().state().tick_us;
    let ui_before = p.foreground_clock();
    let commands = action(&mut p, UiAction::Menu);
    assert!(commands.iter().any(|c| matches!(
        c,
        AppCommand::AudioPause {
            domain: TimeDomain::Story,
            paused: true
        }
    )));
    assert!(!p.domain_paused(TimeDomain::ForegroundUi));
    assert!(!p.needs_clock());
    let clock = p.acquire_foreground_clock().unwrap();
    assert!(p.needs_clock());
    p.pump(vec![AppEvent::Tick { delta_us: 100_000 }], 1000);
    assert_eq!(p.core().state().tick_us, story_before);
    assert_eq!(p.foreground_clock().0, ui_before.0 + 100_000);
    let ui_token = p.acquire_domain_pause(TimeDomain::ForegroundUi, "overlay");
    let second = p.acquire_domain_pause(TimeDomain::ForegroundUi, "overlay");
    p.pump(vec![AppEvent::Hidden(true)], 1000);
    action(&mut p, UiAction::Close);
    assert!(p.domain_paused(TimeDomain::Story));
    p.pump(vec![AppEvent::Hidden(false)], 1000);
    assert!(!p.domain_paused(TimeDomain::Story));
    assert!(p.domain_paused(TimeDomain::ForegroundUi));
    drop(ui_token);
    assert!(p.domain_paused(TimeDomain::ForegroundUi));
    let before = p.foreground_clock();
    p.pump(vec![AppEvent::Tick { delta_us: 100_000 }], 1000);
    assert_eq!(p.foreground_clock(), before);
    assert!(p.core().state().tick_us > story_before);
    drop(second);
    p.pump(vec![AppEvent::Tick { delta_us: 100_000 }], 1000);
    assert_eq!(p.foreground_clock().0, before.0 + 100_000);
    action(&mut p, UiAction::Menu);
    assert!(p.needs_clock());
    drop(clock);
    assert!(!p.needs_clock());
}

#[test]
fn foreground_completion_with_a_story_task_number_cannot_end_or_fail_story_audio() {
    let mut p = playing();
    let task = p
        .core()
        .state()
        .tasks
        .values()
        .find(|t| matches!(t.effect, Effect::Audio { .. }))
        .unwrap()
        .id;
    let before = p.core().state().tasks[&task].state;
    p.pump(
        vec![
            AppEvent::AudioEnded {
                domain: TimeDomain::ForegroundUi,
                task,
                session: p.generation.session,
            },
            AppEvent::AudioFailed {
                domain: TimeDomain::ForegroundUi,
                task,
                session: p.generation.session,
                message: "retired UI voice".into(),
            },
        ],
        1000,
    );
    assert_eq!(p.core().state().tasks[&task].state, before);
    assert!(p.error.is_none());
}

#[test]
fn foreground_clock_demand_is_bounded_and_released_by_owner() {
    let p = player();
    let mut leases: Vec<_> = (0..MAX_TASKS)
        .map(|_| p.acquire_foreground_clock().unwrap())
        .collect();
    assert!(p.acquire_foreground_clock().is_none());
    leases.pop();
    assert!(p.acquire_foreground_clock().is_some());
}

fn voice_bound_player(binding: Option<(Option<&str>, VoiceWaitPolicy)>) -> Player {
    let mut program: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    program.functions.get_mut("main").unwrap().entry = "intro".into();
    program.player.auto_delay_us = Micros(100_000);
    let asset = program
        .assets
        .iter()
        .find(|(_, a)| a.kind == AssetKind::Audio)
        .unwrap()
        .0
        .clone();
    for id in ["spoken", "unrelated"] {
        program
            .cues
            .get_mut("intro")
            .unwrap()
            .effects
            .push(EffectDef {
                id: id.into(),
                scope: Scope::Session,
                effect: Effect::Audio {
                    asset: asset.clone(),
                    bus: AudioBus::Voice,
                    looped: false,
                    gain: 1.,
                },
            });
    }
    if let Some((voice, wait)) = binding {
        if wait == VoiceWaitPolicy::SampledRemaining {
            program.requires.extend([
                "text.voice-timer.v1".into(),
                "player.auto-delay-policy.v1".into(),
            ]);
            program.player.auto_delay_policy = AutoDelayPolicy::Fixed;
            program.assets.get_mut(&asset).unwrap().duration_us = Micros(2_000_000);
        }
        program.requires.push("text.voice-binding.v1".into());
        program
            .functions
            .get_mut("main")
            .unwrap()
            .blocks
            .get_mut("wait_intro")
            .unwrap()
            .ops
            .push(Op {
                id: "bind-page-voice".into(),
                operation: Operation::DialogueVoice {
                    task: "line".into(),
                    voice: voice.map(str::to_owned),
                    wait,
                },
            });
    }
    let mut p = Player::new(program, "release".into(), "Test".into()).unwrap();
    let commands = p.pump(vec![], 1000);
    ready(&mut p, commands);
    let commands = action(&mut p, UiAction::NewGame);
    ready(&mut p, commands);
    action(&mut p, UiAction::Advance);
    assert!(p.core().dialogue().unwrap().1.awaiting_advance);
    p
}

#[test]
fn sampled_voice_timer_freezes_remaining_position_and_mute_state() {
    for (muted, ended_early) in [(false, false), (false, true), (true, false)] {
        let mut p = voice_bound_player(Some((Some("spoken"), VoiceWaitPolicy::SampledRemaining)));
        let task = p.core().state().handles["spoken"];
        p.observe_audio_positions(
            TimeDomain::Story,
            p.generation.session,
            &[AudioPosition {
                task,
                position_us: Micros(1_250_000),
                envelope: None,
            }],
        )
        .unwrap();
        if muted {
            action(
                &mut p,
                UiAction::Volume {
                    bus: AudioBus::Voice,
                    delta: -1.,
                },
            );
        }
        let interaction = p.current_interaction();
        action(&mut p, UiAction::ToggleAuto);
        if ended_early {
            p.pump(
                vec![AppEvent::AudioEnded {
                    domain: TimeDomain::Story,
                    task,
                    session: p.generation.session,
                }],
                1000,
            );
        }
        // Changes after sampling must neither shorten nor extend the timer.
        action(
            &mut p,
            UiAction::Volume {
                bus: AudioBus::Voice,
                delta: if muted { 1. } else { -1. },
            },
        );
        let delay = if muted { 100_000 } else { 850_000 };
        for delta in [delay / 2, delay - delay / 2 - 1] {
            p.pump(vec![AppEvent::Tick { delta_us: delta }], 1000);
        }
        assert_eq!(
            p.current_interaction(),
            interaction,
            "muted={muted}, ended={ended_early}"
        );
        let c = p.pump(vec![AppEvent::Tick { delta_us: 1 }], 1000);
        ready(&mut p, c);
        assert_ne!(
            p.current_interaction(),
            interaction,
            "muted={muted}, ended={ended_early}"
        );
    }
}
#[test]
fn auto_uses_the_bound_voice_and_distinguishes_serial_and_parallel_delay() {
    for wait in [VoiceWaitPolicy::AfterVoice, VoiceWaitPolicy::Parallel] {
        let mut p = voice_bound_player(Some((Some("spoken"), wait)));
        let token = p.current_interaction();
        action(&mut p, UiAction::ToggleAuto);
        for _ in 0..20 {
            p.pump(vec![AppEvent::Tick { delta_us: 250_000 }], 1000);
        }
        assert_eq!(p.current_interaction(), token);
        let spoken = p.core().state().handles["spoken"];
        let unrelated = p.core().state().handles["unrelated"];
        p.pump(
            vec![AppEvent::AudioEnded {
                domain: TimeDomain::Story,
                task: spoken,
                session: p.generation.session,
            }],
            1000,
        );
        p.pump(vec![AppEvent::Tick { delta_us: 1 }], 1000);
        if wait == VoiceWaitPolicy::AfterVoice {
            assert_eq!(
                p.current_interaction(),
                token,
                "post-voice delay must not have elapsed concurrently"
            );
            for _ in 0..20 {
                p.pump(vec![AppEvent::Tick { delta_us: 250_000 }], 1000);
                if p.current_interaction() != token {
                    break;
                }
            }
        }
        assert_ne!(p.current_interaction(), token);
        assert_eq!(
            p.core().state().tasks[&unrelated].state,
            nir_core::TaskState::Running
        );
    }
}
#[test]
fn explicit_no_voice_does_not_wait_for_ambient_voice_but_legacy_still_does() {
    for binding in [None, Some((None, VoiceWaitPolicy::Parallel))] {
        let mut p = voice_bound_player(binding);
        let token = p.current_interaction();
        action(&mut p, UiAction::ToggleAuto);
        for _ in 0..20 {
            p.pump(vec![AppEvent::Tick { delta_us: 250_000 }], 1000);
            if p.current_interaction() != token {
                break;
            }
        }
        assert_eq!(p.current_interaction() == token, binding.is_none());
    }
}
#[test]
fn fully_revealed_dialogue_keeps_audio_offsets_advancing_and_menu_freezes_them() {
    let mut p = voice_bound_player(None);
    let id = p.core().state().handles["spoken"];
    assert!(p.needs_clock());
    let before = p.core().state().tasks[&id].elapsed_us.0;
    for _ in 0..4 {
        p.pump(vec![AppEvent::Tick { delta_us: 250_000 }], 1000);
    }
    assert_eq!(p.core().state().tasks[&id].elapsed_us.0, before + 1_000_000);
    action(&mut p, UiAction::Menu);
    p.pump(vec![AppEvent::Tick { delta_us: 250_000 }], 1000);
    assert_eq!(p.core().state().tasks[&id].elapsed_us.0, before + 1_000_000);
    let commands = action(&mut p, UiAction::Save { slot: 1 });
    let snapshot = commands
        .iter()
        .find_map(|c| match c {
            AppCommand::Save { envelope, .. } => Some(&envelope.snapshot),
            _ => None,
        })
        .unwrap();
    assert_eq!(snapshot.tasks[&id].elapsed_us.0, before + 1_000_000);
}

#[test]
fn automatic_wait_freezes_preferences_until_the_next_cycle() {
    let mut p = voice_bound_player(Some((None, VoiceWaitPolicy::Parallel)));
    let token = p.current_interaction();
    let base = p.core().program().player.auto_delay_us.0
        + p.core().dialogue().unwrap().1.full_text().chars().count() as u64 * 20_000;
    p.preferences.auto_wait_scale = 4.;
    action(&mut p, UiAction::ToggleAuto);
    p.pump(vec![AppEvent::Tick { delta_us: 1 }], 1000);
    action(&mut p, UiAction::AutoWait { delta: -3.75 });
    for _ in 0..(base / 100_000 + 2) {
        p.pump(vec![AppEvent::Tick { delta_us: 100_000 }], 1000);
    }
    assert_eq!(
        p.current_interaction(),
        token,
        "active timer keeps the original 4x duration"
    );
    action(&mut p, UiAction::ToggleAuto);
    action(&mut p, UiAction::ToggleAuto);
    for _ in 0..(base / 100_000 + 2) {
        p.pump(vec![AppEvent::Tick { delta_us: 100_000 }], 1000);
        if p.current_interaction() != token {
            break;
        }
    }
    assert_ne!(
        p.current_interaction(),
        token,
        "new cycle uses the updated duration"
    );
}

#[test]
fn settings_scrolling_keeps_new_preferences_and_close_reachable() {
    use nir_presentation::{Messages, ReadingState, Screen};
    let p = playing();
    for (width, height) in [(390., 844.), (844., 390.), (1280., 720.)] {
        let mut m = p.model();
        m.screen = Screen::Settings;
        m.loading = false;
        let mut state = ReadingState::default();
        let mut text = reading_text();
        let messages = Messages::default();
        let mut found_speed = false;
        let mut found_wait = false;
        for _ in 0..30 {
            let packet = state.project(&m, (1, 1), width, height, &messages, &mut text);
            assert!(packet.semantics.iter().any(|n| n.action == UiAction::Close));
            for n in &packet.semantics {
                assert!(n.rect[1] >= 0. && n.rect[1] + n.rect[3] <= height);
                found_speed |= matches!(n.action, UiAction::TextSpeed { .. });
                found_wait |= matches!(n.action, UiAction::AutoWait { .. });
            }
            if !state.scroll(ScrollRegion::Settings, 1, &packet) {
                break;
            }
        }
        assert!(found_speed && found_wait, "{width}x{height}");
    }
}

#[test]
fn held_skip_release_is_independent_of_toggle_and_cleared_by_menu_and_focus_loss() {
    let mut p = voice_bound_player(Some((None, VoiceWaitPolicy::Parallel)));
    action(&mut p, UiAction::HoldSkip { pressed: true });
    assert!(p.model().skip);
    action(&mut p, UiAction::HoldSkip { pressed: false });
    assert!(!p.model().skip);
    action(&mut p, UiAction::ToggleSkip);
    action(&mut p, UiAction::HoldSkip { pressed: true });
    action(&mut p, UiAction::HoldSkip { pressed: false });
    assert!(p.model().skip, "release must not disable latched skip");
    action(&mut p, UiAction::ToggleSkip);
    action(&mut p, UiAction::HoldSkip { pressed: true });
    action(&mut p, UiAction::Menu);
    assert!(!p.model().skip);
    action(&mut p, UiAction::HoldSkip { pressed: true });
    assert!(!p.model().skip, "menu cannot start held skip");
    action(&mut p, UiAction::Close);
    action(&mut p, UiAction::HoldSkip { pressed: true });
    p.pump(vec![AppEvent::Hidden(true)], 1000);
    p.pump(vec![AppEvent::Hidden(false)], 1000);
    assert!(!p.model().skip);
}

#[test]
fn held_skip_cannot_advance_unread_dialogue() {
    let mut p = voice_bound_player(Some((None, VoiceWaitPolicy::Parallel)));
    let token = p.current_interaction();
    action(&mut p, UiAction::HoldSkip { pressed: true });
    p.pump(vec![AppEvent::Tick { delta_us: 1000 }], 1000);
    assert_eq!(p.current_interaction(), token);
    assert!(!p.model().skip);
}

#[test]
fn held_skip_advances_a_read_dialogue() {
    let mut p = voice_bound_player(Some((None, VoiceWaitPolicy::Parallel)));
    let token = p.current_interaction();
    let d = p.core().dialogue().unwrap().1;
    let key = format!("read:{}:{}", d.text_id, d.meaning_revision);
    p.pump(vec![AppEvent::Profile([key].into_iter().collect())], 1000);
    action(&mut p, UiAction::HoldSkip { pressed: true });
    p.pump(vec![AppEvent::Tick { delta_us: 1000 }], 1000);
    assert_ne!(p.current_interaction(), token);
}

#[test]
fn long_tick_preserves_elapsed_audio_time_and_counts_ui_time_once_across_budget_yields() {
    for budget in [1, 1000] {
        let mut p = voice_bound_player(None);
        let audio = p.core().state().handles["spoken"];
        let before = p.core().state().tasks[&audio].elapsed_us.0;
        let ui = p.foreground_clock().0;
        p.pump(vec![AppEvent::Tick { delta_us: 900_000 }], budget);
        for _ in 0..10 {
            p.pump(vec![], 1000);
        }
        assert_eq!(
            p.core().state().tasks[&audio].elapsed_us.0 - before,
            900_000
        );
        assert_eq!(p.foreground_clock().0 - ui, 900_000);
    }
}

#[test]
fn new_session_discards_old_clock_continuation_without_charging_ui_again() {
    let mut p = voice_bound_player(None);
    let ui = p.foreground_clock().0;
    p.pump(vec![AppEvent::Tick { delta_us: 900_000 }], 1);
    let session = p.generation.session;
    action(&mut p, UiAction::NewGame);
    p.pump(vec![], 1000);
    assert_ne!(p.generation.session, session);
    assert_eq!(p.core().state().tick_us.0, 0);
    assert_eq!(p.foreground_clock().0 - ui, 900_000);
}

#[test]
fn shared_pointer_router_blocks_disabled_controls_and_only_advances_story_background() {
    use nir_presentation::{pointer_action, DrawPacket, Screen, SemanticNode};
    let p = playing();
    let mut m = p.model();
    m.loading = false;
    m.paused = false;
    m.screen = Screen::Story;
    let mut packet = DrawPacket::default();
    packet.width = 800.;
    packet.height = 600.;
    assert_eq!(
        pointer_action(&packet, &m, 20., 20., 0),
        Some(UiAction::Advance)
    );
    assert_eq!(
        pointer_action(&packet, &m, 20., 20., 2),
        Some(UiAction::Menu)
    );
    assert_eq!(pointer_action(&packet, &m, 20., 20., 1), None);
    assert_eq!(pointer_action(&packet, &m, -1., 20., 0), None);
    assert_eq!(pointer_action(&packet, &m, f32::NAN, 20., 0), None);
    for enabled in [true, false] {
        packet.semantics.push(SemanticNode {
            value: None,
            id: packet.semantics.len() as u32,
            label: "control".into(),
            action: UiAction::Menu,
            enabled,
            rect: [0., 0., 100., 100.],
            locale: "en".into(),
        });
    }
    assert_eq!(
        packet.hit(20., 20.),
        None,
        "disabled top control blocks the enabled one underneath"
    );
    assert_eq!(pointer_action(&packet, &m, 20., 20., 0), None);
    m.paused = true;
    assert_eq!(pointer_action(&packet, &m, 200., 20., 0), None);
    m.paused = false;
    m.dialogue = None;
    assert_eq!(pointer_action(&packet, &m, 200., 20., 0), None);
    for screen in [
        Screen::Menu,
        Screen::Settings,
        Screen::Saves,
        Screen::History,
    ] {
        m.screen = screen;
        assert_eq!(pointer_action(&packet, &m, 200., 20., 0), None);
        assert_eq!(
            pointer_action(&packet, &m, 200., 20., 2),
            Some(UiAction::Close)
        );
    }
    m.screen = Screen::Title;
    assert_eq!(pointer_action(&packet, &m, 200., 20., 2), None);
    m.loading = true;
    packet.semantics[1].enabled = true;
    packet.semantics[1].action = UiAction::LocaleCancel;
    assert_eq!(
        pointer_action(&packet, &m, 20., 20., 0),
        Some(UiAction::LocaleCancel),
        "loading must not trap visible cancellation controls"
    );
}

#[test]
fn shared_primary_router_obeys_available_title_actions_and_choice_focus() {
    use nir_presentation::{
        pointer_action, primary_action, ChoiceView, DrawPacket, Screen, SemanticNode,
    };
    let p = playing();
    let mut m = p.model();
    m.loading = false;
    m.paused = false;
    m.screen = Screen::Title;
    let mut packet = DrawPacket::default();
    packet.width = 800.;
    packet.height = 600.;
    assert_eq!(primary_action(&packet, &m), None);
    packet.semantics.push(SemanticNode {
        value: None,
        id: 0,
        label: "new".into(),
        action: UiAction::NewGame,
        enabled: false,
        rect: [0., 0., 10., 10.],
        locale: "en".into(),
    });
    assert_eq!(primary_action(&packet, &m), None);
    packet.semantics[0].enabled = true;
    assert_eq!(primary_action(&packet, &m), Some(UiAction::NewGame));
    packet.semantics.clear();
    m.screen = Screen::Story;
    assert_eq!(primary_action(&packet, &m), Some(UiAction::Advance));
    m.choices.push(ChoiceView {
        id: "one".into(),
        label: "one".into(),
        enabled: true,
        selected: false,
        locale: "en".into(),
        font_plan_digest: String::new(),
        font_assets: vec![],
    });
    assert_eq!(primary_action(&packet, &m), None);
    assert_eq!(pointer_action(&packet, &m, 100., 100., 0), None);
    m.choices.clear();
    m.dialogue = None;
    m.hidden_dialogue = true;
    assert_eq!(
        primary_action(&packet, &m),
        Some(UiAction::Advance),
        "script-hidden dialogue keeps its existing continuation route"
    );
    m.paused = true;
    assert_eq!(primary_action(&packet, &m), None);
    packet.semantics.push(SemanticNode {
        value: None,
        id: 1,
        label: "continue".into(),
        action: UiAction::Continue,
        enabled: true,
        rect: [0., 0., 10., 10.],
        locale: "en".into(),
    });
    assert_eq!(primary_action(&packet, &m), Some(UiAction::Continue));
    m.screen = Screen::Menu;
    assert_eq!(primary_action(&packet, &m), None);
    m.loading = true;
    assert_eq!(primary_action(&packet, &m), None);
}

fn interface_player(policy: HidePolicy, script_hidden: bool) -> Player {
    let mut program: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    program.functions.get_mut("main").unwrap().entry = "intro".into();
    program.player.hide_policy = policy;
    if policy == HidePolicy::PauseStory {
        program.requires.push("player.hide-policy.v1".into());
    }
    if script_hidden {
        program.requires.push("text.visibility.v1".into());
        program
            .functions
            .get_mut("main")
            .unwrap()
            .blocks
            .get_mut("wait_intro")
            .unwrap()
            .ops
            .push(Op {
                id: "script-hidden".into(),
                operation: Operation::DialogueVisibility {
                    visible: false,
                    transition: None,
                    duration_us: Micros(0),
                },
            });
    }
    let mut p = Player::new(program, "release".into(), "Test".into()).unwrap();
    let commands = p.pump(vec![], 1000);
    ready(&mut p, commands);
    let commands = action(&mut p, UiAction::NewGame);
    ready(&mut p, commands);
    action(&mut p, UiAction::Advance);
    p
}

#[test]
fn temporary_hide_masks_ui_without_advancing_on_restore_or_overwriting_script_visibility() {
    use nir_presentation::{project, Messages};
    for script_hidden in [false, true] {
        let mut p = interface_player(HidePolicy::ContinueStory, script_hidden);
        let token = p.current_interaction();
        action(&mut p, UiAction::ToggleAuto);
        let snapshot = serde_json::to_value(p.core().snapshot()).unwrap();
        action(&mut p, UiAction::ToggleInterface);
        assert!(p.interface_hidden());
        assert!(
            !p.preview().interface_hidden,
            "preparation must include the restored presentation"
        );
        assert!(!p.paused());
        assert!(!p.auto && !p.model().skip);
        assert_eq!(serde_json::to_value(p.core().snapshot()).unwrap(), snapshot);
        let packet = project(&p.model(), 1280., 720., &Messages::default());
        assert!(packet.texts.is_empty());
        assert_eq!(packet.semantics.len(), 1);
        assert_eq!(packet.semantics[0].action, UiAction::RestoreInterface);
        p.pump(vec![AppEvent::Tick { delta_us: 300_000 }], 1000);
        assert_eq!(p.current_interaction(), token);
        assert!(p.interface_hidden());
        action(&mut p, UiAction::Advance);
        assert!(!p.interface_hidden());
        assert_eq!(p.current_interaction(), token);
        assert_eq!(p.core().state().dialogue_hidden, script_hidden);
    }
}

#[test]
fn explicit_hide_pause_releases_only_its_own_owner() {
    let mut p = interface_player(HidePolicy::PauseStory, false);
    action(&mut p, UiAction::ToggleInterface);
    assert!(p.paused());
    let tick = p.core().state().tick_us;
    p.pump(vec![AppEvent::Tick { delta_us: 300_000 }], 1000);
    assert_eq!(p.core().state().tick_us, tick);
    p.pump(vec![AppEvent::Hidden(true)], 1000);
    action(&mut p, UiAction::RestoreInterface);
    assert!(!p.interface_hidden());
    assert!(
        p.paused(),
        "restoring interface must not release background pause"
    );
    p.pump(vec![AppEvent::Hidden(false)], 1000);
    assert!(!p.paused());
}

#[test]
fn hide_policy_requires_capability_and_mask_does_not_survive_new_session() {
    let mut program: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    program.player.hide_policy = HidePolicy::PauseStory;
    assert_eq!(
        nir_core::ValidatedProgram::new(program).unwrap_err().code,
        "E_CAPABILITY"
    );
    let mut p = interface_player(HidePolicy::ContinueStory, false);
    action(&mut p, UiAction::ToggleInterface);
    let commands = action(&mut p, UiAction::NewGame);
    ready(&mut p, commands);
    assert!(!p.interface_hidden());
    action(&mut p, UiAction::ToggleInterface);
    assert!(p.interface_hidden());
    p.pump(vec![AppEvent::Tick { delta_us: 0 }; 200], 0);
    assert!(
        !p.interface_hidden(),
        "blocking recovery UI must not remain masked"
    );
}

#[test]
fn custom_dialogue_rect_has_a_static_hint_without_changing_text_bounds() {
    use nir_presentation::{project, Messages};
    let p = interface_player(HidePolicy::ContinueStory, false);
    let mut model = p.model();
    model.theme.dialogue.rect = Some([10., 500., 1200., 180.]);
    let messages = Messages::default();
    for (width, height) in [(1280., 720.), (390., 844.), (844., 390.)] {
        let packet = project(&model, width, height, &messages);
        let hint = packet
            .texts
            .iter()
            .find(|t| t.text == messages.text(&model.ui_locale, "advance-hint"))
            .unwrap();
        assert!(hint.x >= 0. && hint.y >= 0. && hint.y + hint.line_height <= height);
        let text = packet
            .texts
            .iter()
            .find(|t| t.region == Some(ScrollRegion::Dialogue))
            .unwrap();
        assert!(hint.y >= text.y + text.height || hint.y + hint.line_height <= text.y);
    }
    model.theme.dialogue.rect = Some([0., 0., 1280., 720.]);
    let packet = project(&model, 1280., 720., &messages);
    let hint = packet
        .texts
        .iter()
        .find(|t| t.text == messages.text(&model.ui_locale, "advance-hint"))
        .unwrap();
    let text = packet
        .texts
        .iter()
        .find(|t| t.region == Some(ScrollRegion::Dialogue))
        .unwrap();
    assert!(
        hint.y >= text.y + text.height,
        "full-screen default padding can hold the hint without covering text"
    );
}

#[test]
fn default_interface_hide_keeps_audio_clock_running() {
    let mut p = voice_bound_player(None);
    let audio = p.core().state().handles["spoken"];
    let before = p.core().state().tasks[&audio].elapsed_us.0;
    let commands = action(&mut p, UiAction::ToggleInterface);
    assert!(!commands.iter().any(|c| matches!(
        c,
        AppCommand::AudioPause {
            domain: TimeDomain::Story,
            paused: true
        }
    )));
    p.pump(vec![AppEvent::Tick { delta_us: 300_000 }], 1000);
    assert_eq!(
        p.core().state().tasks[&audio].elapsed_us.0,
        before + 300_000
    );
}

#[test]
fn audio_observations_are_session_scoped_and_restore_the_device_playhead() {
    for observed in [false, true] {
        let mut p = voice_bound_player(None);
        let id = p.core().state().handles["spoken"];
        let epoch = p.generation.session;
        let before = p.core().snapshot();
        let positions = [AudioPosition {
            envelope: None,
            task: id,
            position_us: Micros(800_000),
        }];
        p.observe_audio_positions(TimeDomain::ForegroundUi, epoch, &positions)
            .unwrap();
        p.observe_audio_positions(TimeDomain::Story, epoch + 1, &positions)
            .unwrap();
        assert_eq!(p.core().state().tasks[&id].audio_position_us, None);
        if observed {
            p.observe_audio_positions(TimeDomain::Story, epoch, &positions)
                .unwrap();
        }
        assert_eq!(p.core().state().tick_us, before.tick_us);
        assert_eq!(
            p.core().state().tasks[&id].elapsed_us,
            before.tasks[&id].elapsed_us
        );
        action(&mut p, UiAction::Menu);
        let commands = action(&mut p, UiAction::Save { slot: 1 });
        let envelope = commands
            .into_iter()
            .find_map(|c| match c {
                AppCommand::Save { envelope, .. } => Some(envelope),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            envelope.snapshot.tasks[&id].audio_position_us,
            observed.then_some(Micros(800_000))
        );
        let commands = p.pump(vec![AppEvent::Loaded { envelope }], 1000);
        let commands = ready(&mut p, commands);
        let expected = if observed {
            Micros(800_000)
        } else {
            before.tasks[&id].elapsed_us
        };
        assert!(commands.iter().any(|c| matches!(c, AppCommand::AudioStart { task, position_us, .. } if *task == id && *position_us == expected)));
        assert!(p.paused());
    }
}

#[test]
fn domain_dispatch_does_not_charge_menu_return_time_to_story() {
    let mut p = voice_bound_player(None);
    let before = p.core().state().tick_us;
    let ui = p.foreground_clock();
    action(&mut p, UiAction::Menu);
    p.pump(
        vec![AppEvent::TickDomains {
            story_us: 0,
            foreground_us: 800_000,
        }],
        1000,
    );
    action(&mut p, UiAction::Close);
    p.pump(
        vec![AppEvent::TickDomains {
            story_us: 0,
            foreground_us: 800_000,
        }],
        1000,
    );
    assert_eq!(p.core().state().tick_us, before);
    assert_eq!(p.foreground_clock().0, ui.0 + 1_600_000);
    p.pump(
        vec![AppEvent::TickDomains {
            story_us: 50_000,
            foreground_us: 50_000,
        }],
        1000,
    );
    assert_eq!(p.core().state().tick_us.0, before.0 + 50_000);
    assert_eq!(p.foreground_clock().0, ui.0 + 1_650_000);
}

#[test]
fn transition_masks_leave_the_active_asset_set_and_reenter_on_restore() {
    let mut program: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    let image = program
        .assets
        .values()
        .find(|a| a.kind == AssetKind::Image)
        .unwrap()
        .clone();
    program.assets.insert("mask.pattern".into(), image);
    program.requires.push("stage.mask.v1".into());
    program.functions.get_mut("main").unwrap().entry = "intro".into();
    let scene = program.scenes.keys().next().unwrap().clone();
    program
        .cues
        .get_mut("intro")
        .unwrap()
        .effects
        .push(EffectDef {
            id: "mask".into(),
            scope: Scope::Session,
            effect: Effect::StagePresent {
                scene,
                duration_us: Micros(1000),
                transition: StageTransition::Mask {
                    asset: "mask.pattern".into(),
                    channel: MaskChannel::Alpha,
                    invert: false,
                    softness: 0.2,
                },
            },
        });
    let mut p = Player::new(program, "release".into(), "Test".into()).unwrap();
    let commands = p.pump(vec![], 1000);
    ready(&mut p, commands);
    let commands = action(&mut p, UiAction::NewGame);
    ready(&mut p, commands);
    assert!(p.retained_assets().contains("mask.pattern"));
    let snapshot = p.core().snapshot();
    p.pump(vec![AppEvent::Tick { delta_us: 1000 }], 1000);
    assert!(!p.retained_assets().contains("mask.pattern"));
    let digest = nir_content::digest(&serde_json::to_vec(&snapshot).unwrap());
    let commands = p.pump(
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
    ready(&mut p, commands);
    assert!(p.retained_assets().contains("mask.pattern"));
    assert!(p.paused());
}

fn slot_load_job(p: &mut Player, slot: u32) -> u32 {
    action(p, UiAction::Load { slot })
        .into_iter()
        .find_map(|c| {
            if let AppCommand::Load { job, .. } = c {
                Some(job)
            } else {
                None
            }
        })
        .unwrap()
}
fn slot_envelope(p: &Player, slot: u32) -> Box<SaveEnvelope> {
    let snapshot = p.core().snapshot();
    Box::new(SaveEnvelope {
        format: 1,
        slot,
        revision: 1,
        digest: nir_content::digest(&serde_json::to_vec(&snapshot).unwrap()),
        snapshot,
    })
}
#[test]
fn slot_load_rejects_replaced_requests_and_back_then_reopen() {
    let mut p = playing();
    action(&mut p, UiAction::Saves);
    let envelope = slot_envelope(&p, 0);
    let first = slot_load_job(&mut p, 0);
    let second = slot_load_job(&mut p, 0);
    let before = p.generation.session;
    let commands = p.pump(
        vec![AppEvent::SlotLoaded {
            job: first,
            envelope: envelope.clone(),
        }],
        1000,
    );
    assert!(!commands.iter().any(|c| matches!(
        c,
        AppCommand::GetAssets { .. } | AppCommand::PreparePresentation { .. }
    )));
    assert_eq!(p.generation.session, before);
    action(&mut p, UiAction::Close);
    action(&mut p, UiAction::Saves);
    let commands = p.pump(
        vec![AppEvent::SlotLoaded {
            job: second,
            envelope: envelope.clone(),
        }],
        1000,
    );
    ready(&mut p, commands);
    assert_eq!(p.generation.session, before);
    let third = slot_load_job(&mut p, 0);
    p.pump(
        vec![AppEvent::SlotLoadFailed {
            job: second,
            message: "late failure".into(),
        }],
        1000,
    );
    assert!(p.error.is_none());
    let commands = p.pump(
        vec![AppEvent::SlotLoaded {
            job: third,
            envelope,
        }],
        1000,
    );
    ready(&mut p, commands);
    assert_eq!(p.generation.session, before + 1);
}
#[test]
fn slot_load_rejects_wrong_slot_and_consumes_the_request() {
    let mut p = playing();
    action(&mut p, UiAction::Saves);
    let job = slot_load_job(&mut p, 0);
    let envelope = slot_envelope(&p, 1);
    p.pump(vec![AppEvent::SlotLoaded { job, envelope }], 1000);
    assert_eq!(p.diagnostic.as_ref().unwrap().code, "E_SAVE_SLOT");
    let before = p.generation.session;
    let envelope = slot_envelope(&p, 0);
    let commands = p.pump(vec![AppEvent::SlotLoaded { job, envelope }], 1000);
    ready(&mut p, commands);
    assert_eq!(p.generation.session, before);
    assert!(!action(&mut p, UiAction::Load { slot: 3 })
        .iter()
        .any(|c| matches!(c, AppCommand::Load { .. })));
}

#[test]
fn closing_during_slot_candidate_preparation_keeps_story_and_rejects_ready() {
    let mut p = playing();
    action(&mut p, UiAction::Saves);
    let envelope = slot_envelope(&p, 0);
    let job = slot_load_job(&mut p, 0);
    let before = p.generation.session;
    let commands = p.pump(vec![AppEvent::SlotLoaded { job, envelope }], 1000);
    assert!(p.is_loading());
    let cancelled = action(&mut p, UiAction::Close);
    assert!(cancelled
        .iter()
        .any(|c| matches!(c, AppCommand::CancelAssets { .. })));
    ready(&mut p, commands);
    assert_eq!(p.generation.session, before);
    assert_eq!(p.screen, nir_presentation::Screen::Story);
    assert!(!p.is_loading());
    assert!(!p.paused());
}

#[test]
fn closing_a_failed_slot_restore_clears_only_its_diagnostic() {
    for corrupt in [false, true] {
        let mut p = playing();
        action(&mut p, UiAction::Saves);
        let job = slot_load_job(&mut p, 0);
        let mut envelope = slot_envelope(&p, 0);
        if corrupt {
            envelope.digest = "invalid".into();
        }
        let commands = p.pump(vec![AppEvent::SlotLoaded { job, envelope }], 1000);
        if !corrupt {
            let request = commands
                .iter()
                .find_map(|c| {
                    if let AppCommand::GetAssets { request, .. } = c {
                        Some(*request)
                    } else {
                        None
                    }
                })
                .unwrap();
            p.pump(
                vec![AppEvent::AssetFailed {
                    request,
                    message: "restore image unavailable".into(),
                }],
                1000,
            );
        }
        assert!(p.error.is_some());
        action(&mut p, UiAction::Close);
        assert!(p.error.is_none());
        assert!(p.diagnostic.is_none());
        assert!(!p.paused());
        assert!(!p.is_loading());
    }
    let mut p = playing();
    action(&mut p, UiAction::Saves);
    let job = slot_load_job(&mut p, 0);
    let envelope = slot_envelope(&p, 0);
    p.pump(vec![AppEvent::SlotLoaded { job, envelope }], 1000);
    p.pump(
        vec![AppEvent::HostFailed("unrelated host error".into())],
        1000,
    );
    action(&mut p, UiAction::Close);
    assert_eq!(p.diagnostic.as_ref().unwrap().code, "E_HOST");
}

#[test]
fn restore_media_failure_does_not_fail_original_pending_activation() {
    let saved = playing();
    let envelope = slot_envelope(&saved, 0);
    let mut p = player();
    let commands = p.pump(vec![], 1000);
    ready(&mut p, commands);
    action(&mut p, UiAction::NewGame);
    assert!(p.core().state().pending.is_some());
    action(&mut p, UiAction::Saves);
    let original = serde_json::to_value(p.core().snapshot()).unwrap();
    let job = slot_load_job(&mut p, 0);
    let commands = p.pump(vec![AppEvent::SlotLoaded { job, envelope }], 1000);
    let request = commands
        .iter()
        .find_map(|c| {
            if let AppCommand::GetAssets { request, .. } = c {
                Some(*request)
            } else {
                None
            }
        })
        .unwrap();
    p.pump(
        vec![AppEvent::AssetFailed {
            request,
            message: "candidate failure".into(),
        }],
        1000,
    );
    assert_eq!(serde_json::to_value(p.core().snapshot()).unwrap(), original);
    let commands = action(&mut p, UiAction::Close);
    ready(&mut p, commands);
    assert!(p.core().dialogue().is_some());
    assert!(!p.is_loading());
}

#[test]
fn invalid_slot_envelope_does_not_cancel_existing_story_preparation() {
    let saved = playing();
    let mut envelope = slot_envelope(&saved, 0);
    envelope.digest = "invalid".into();
    let mut p = player();
    let boot = p.pump(vec![], 1000);
    ready(&mut p, boot);
    let original = action(&mut p, UiAction::NewGame);
    action(&mut p, UiAction::Saves);
    let job = slot_load_job(&mut p, 0);
    p.pump(vec![AppEvent::SlotLoaded { job, envelope }], 1000);
    let close = action(&mut p, UiAction::Close);
    assert!(!close
        .iter()
        .any(|c| matches!(c, AppCommand::CancelAssets { .. })));
    ready(&mut p, original);
    assert!(p.core().dialogue().is_some());
    assert!(p.error.is_none());
}

/// A dialogue cue with a parallel composition beside it: the VM parks on the
/// dialogue wait while the bell rings and the background fades concurrently.
/// The story changes local state (affection) before activating the cue, and a
/// second short cue follows so the chain is mid-flight across checkpoints.
fn compose_program() -> Program {
    let mut p: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    p.requires.push("tween.target.v1".into());
    p.requires.push("task.compose.v1".into());
    p.cues.insert(
        "compose".into(),
        serde_json::from_value(serde_json::json!({
            "effects": [
                {"id":"stage","scope":"scene","effect":{"type":"stage_present","scene":"station","duration_us":"0"}},
                {"id":"line","scope":"interaction","effect":{"type":"dialogue","text":"intro","speaker":"","reveal_us":"10000000"}},
                {"id":"chain","scope":"session","effect":{"type":"parallel_all","children":[
                    {"id":"ring","scope":"session","effect":{"type":"audio","asset":"audio.bell","bus":"bgm","looped":false}},
                    {"id":"fade","scope":"session","effect":{"type":"tween","target":{"type":"scene_node","node":"background","property":"opacity"},"to":0.2,"duration_us":"10000000"}}
                ]}}
            ]
        }))
        .unwrap(),
    );
    p.cues.insert(
        "beat".into(),
        serde_json::from_value(serde_json::json!({
            "effects":[{"id":"beat","scope":"session","effect":{"type":"delay","duration_us":"4000000"}}]
        }))
        .unwrap(),
    );
    p.cues.insert(
        "beat2".into(),
        serde_json::from_value(serde_json::json!({
            "effects":[{"id":"beat2","scope":"session","effect":{"type":"delay","duration_us":"100000"}}]
        }))
        .unwrap(),
    );
    let main = p.functions.get_mut("main").unwrap();
    main.entry = "test".into();
    for (block, body) in [
        (
            "test",
            serde_json::json!({
                "ops":[{"id":"affection.set","operation":{"type":"assign","target":"affection","value":{"type":"const","value":{"type":"i32","value":5}}}}],
                "terminator":{"type":"activate","cue":"compose","next":"hold"}
            }),
        ),
        (
            "hold",
            serde_json::json!({
                "terminator":{"type":"await","conditions":[{"task":"line","milestone":{"type":"finished"}}],"next":"second","on_cancelled":"second","on_failed":"second"}
            }),
        ),
        (
            "second",
            serde_json::json!({"terminator":{"type":"activate","cue":"beat","next":"wait_beat"}}),
        ),
        (
            "wait_beat",
            serde_json::json!({
                "terminator":{"type":"await","conditions":[{"task":"beat","milestone":{"type":"finished"}}],"next":"third","on_cancelled":"third","on_failed":"third"}
            }),
        ),
        (
            "third",
            serde_json::json!({"terminator":{"type":"activate","cue":"beat2","next":"wait_beat2"}}),
        ),
        (
            "wait_beat2",
            serde_json::json!({
                "terminator":{"type":"await","conditions":[{"task":"beat2","milestone":{"type":"finished"}}],"next":"wait_chain","on_cancelled":"wait_chain","on_failed":"wait_chain"}
            }),
        ),
        (
            "wait_chain",
            serde_json::json!({
                "terminator":{"type":"await","conditions":[{"task":"chain","milestone":{"type":"finished"}}],"next":"done","on_cancelled":"done","on_failed":"done"}
            }),
        ),
        (
            "done",
            serde_json::json!({"terminator":{"type":"end","outcome":"done"}}),
        ),
    ] {
        main.blocks
            .insert(block.into(), serde_json::from_value(body).unwrap());
    }
    p
}

fn audio_starts(commands: &[AppCommand]) -> Vec<(u32, Micros, u32)> {
    commands
        .iter()
        .filter_map(|c| {
            if let AppCommand::AudioStart {
                task,
                position_us,
                session,
                ..
            } = c
            {
                Some((*task, *position_us, *session))
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn parallel_chain_mid_flight_save_and_load_resume_without_replay() {
    let mut p = Player::new(compose_program(), "release".into(), "Test".into()).unwrap();
    let commands = p.pump(vec![], 1000);
    ready(&mut p, commands);
    let commands = action(&mut p, UiAction::NewGame);
    ready(&mut p, commands);
    // The VM parks on the dialogue wait while both children run beside it.
    let state = p.core().state();
    assert!(state.waiting.is_some());
    assert_eq!(state.tasks[&state.handles["chain"]].cursor, 2);
    assert_eq!(state.tasks[&state.handles["ring"]].state, nir_core::TaskState::Running);
    assert_eq!(state.tasks[&state.handles["fade"]].state, nir_core::TaskState::Running);
    // Local state changed and the chain is mid-flight.
    p.pump(vec![AppEvent::Tick { delta_us: 1_000_000 }], 1000);
    let state = p.core().state();
    assert_eq!(state.variables["affection"], Value::I32(5));
    assert_eq!(state.tasks[&state.handles["fade"]].elapsed_us, Micros(1_000_000));

    // The save envelope freezes the mid-flight chain and the changed state.
    let commands = action(&mut p, UiAction::Save { slot: 0 });
    let (job, envelope) = commands
        .into_iter()
        .find_map(|c| {
            if let AppCommand::Save {
                job, envelope, ..
            } = c
            {
                Some((job, envelope))
            } else {
                None
            }
        })
        .unwrap();
    let saved = &envelope.snapshot;
    assert_eq!(saved.variables["affection"], Value::I32(5));
    assert_eq!(saved.tasks[&saved.handles["chain"]].cursor, 2);
    assert_eq!(
        saved.tasks[&saved.handles["ring"]].elapsed_us,
        Micros(1_000_000)
    );
    let saved_fade = &saved.tasks[&saved.handles["fade"]];
    assert_eq!((saved_fade.elapsed_us, saved_fade.captured), (Micros(1_000_000), 1.));
    p.pump(
        vec![AppEvent::Saved {
            job,
            slot: 0,
            revision: 1,
        }],
        1000,
    );

    // Loading the slot restores the composition and resumes it exactly once.
    let session = p.generation.session;
    let job = slot_load_job(&mut p, 0);
    let commands = p.pump(
        vec![AppEvent::SlotLoaded {
            job,
            envelope,
        }],
        1000,
    );
    let commands = ready(&mut p, commands);
    assert!(p.generation.session > session);
    assert!(p.paused());
    let ring = p.core().state().handles["ring"];
    // One AudioStart total: the restore resume carrying the saved offset.
    assert_eq!(
        audio_starts(&commands),
        vec![(ring, Micros(1_000_000), p.generation.session)]
    );
    // The restored chain continues from its frozen values, not from a restart.
    let state = p.core().state();
    assert_eq!(state.variables["affection"], Value::I32(5));
    let fade = &state.tasks[&state.handles["fade"]];
    assert_eq!((fade.elapsed_us, fade.captured), (Micros(1_000_000), 1.));
    assert_eq!(state.tasks[&state.handles["ring"]].state, nir_core::TaskState::Running);
    assert_eq!(state.tasks[&state.handles["chain"]].cursor, 2);

    // Release the restored pause; the chain finishes without any replay.
    action(&mut p, UiAction::Continue);
    let commands = p.pump(
        vec![AppEvent::AudioEnded {
            domain: TimeDomain::Story,
            task: ring,
            session: p.generation.session,
        }],
        1000,
    );
    let commands = ready(&mut p, commands);
    assert!(audio_starts(&commands).is_empty());
    let commands = p.pump(vec![AppEvent::Tick { delta_us: 9_000_000 }], 1000);
    ready(&mut p, commands);
    let state = p.core().state();
    assert_eq!(state.tasks[&state.handles["fade"]].state, nir_core::TaskState::Finished);
    assert_eq!(
        state
            .scene
            .iter()
            .find(|n| n.id == "background")
            .unwrap()
            .opacity,
        0.2
    );
    assert!(state.tasks[&state.handles["chain"]]
        .milestones
        .contains(&nir_format::Milestone::Finished));
    // The parked dialogue resolves on reader input and the story ends.
    let commands = action(&mut p, UiAction::Advance);
    ready(&mut p, commands);
    let commands = action(&mut p, UiAction::Advance);
    ready(&mut p, commands);
    let commands = p.pump(vec![AppEvent::Tick { delta_us: 9_000_000 }], 1000);
    ready(&mut p, commands);
    // The beat cue's activation stops the clock mid-tick; the next host frame
    // resumes it, exactly like a render loop would.
    let commands = p.pump(vec![AppEvent::Tick { delta_us: 1_000_000 }], 1000);
    ready(&mut p, commands);
    assert_eq!(p.core().state().outcome.as_deref(), Some("done"));
}

#[test]
fn parallel_chain_rolls_back_mid_flight_without_replay() {
    let mut p = Player::new(compose_program(), "release".into(), "Test".into()).unwrap();
    let commands = p.pump(vec![], 1000);
    ready(&mut p, commands);
    let commands = action(&mut p, UiAction::NewGame);
    ready(&mut p, commands);
    p.pump(vec![AppEvent::Tick { delta_us: 2_000_000 }], 1000);
    // Finish the dialogue so the beat cue activates and checkpoints a state
    // with the chain mid-flight at the two-second mark.
    let commands = action(&mut p, UiAction::Advance);
    ready(&mut p, commands);
    let commands = action(&mut p, UiAction::Advance);
    ready(&mut p, commands);
    assert!(p.core().state().waiting.is_some());
    assert_eq!(
        p.core().state().tasks[&p.core().state().handles["fade"]].elapsed_us,
        Micros(2_000_000)
    );
    // Run past that checkpoint so the rollback really rewinds live progress.
    let commands = p.pump(vec![AppEvent::Tick { delta_us: 4_000_000 }], 1000);
    ready(&mut p, commands);
    let commands = p.pump(vec![], 1000);
    ready(&mut p, commands);
    assert_eq!(
        p.core().state().tasks[&p.core().state().handles["fade"]].elapsed_us,
        Micros(6_000_000)
    );

    // Rolling back returns to that checkpoint: the chain rewinds to its
    // frozen two-second values and the running audio resumes exactly once.
    let session = p.generation.session;
    let commands = action(&mut p, UiAction::Rollback);
    let commands = ready(&mut p, commands);
    assert!(p.generation.session > session);
    assert!(p.paused());

    let ring = p.core().state().handles["ring"];
    assert_eq!(
        audio_starts(&commands),
        vec![(ring, Micros(2_000_000), p.generation.session)]
    );
    let state = p.core().state();
    let fade = &state.tasks[&state.handles["fade"]];
    assert_eq!((fade.elapsed_us, fade.captured), (Micros(2_000_000), 1.));
    assert_eq!(state.tasks[&state.handles["ring"]].state, nir_core::TaskState::Running);
    assert_eq!(state.tasks[&state.handles["chain"]].cursor, 2);
    assert_eq!(state.variables["affection"], Value::I32(5));

    // The rewound chain completes exactly once and the story still ends.
    action(&mut p, UiAction::Continue);
    let commands = p.pump(
        vec![AppEvent::AudioEnded {
            domain: TimeDomain::Story,
            task: ring,
            session: p.generation.session,
        }],
        1000,
    );
    let commands = ready(&mut p, commands);
    assert!(audio_starts(&commands).is_empty());
    let commands = p.pump(vec![AppEvent::Tick { delta_us: 8_000_000 }], 1000);
    ready(&mut p, commands);
    let commands = p.pump(vec![AppEvent::Tick { delta_us: 10_000_000 }], 1000);
    ready(&mut p, commands);
    let state = p.core().state();
    assert_eq!(state.tasks[&state.handles["fade"]].state, nir_core::TaskState::Finished);
    assert!(state.tasks[&state.handles["chain"]]
        .milestones
        .contains(&nir_format::Milestone::Finished));
    assert_eq!(state.outcome.as_deref(), Some("done"));
}
