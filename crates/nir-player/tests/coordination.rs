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
        1000,
    );
    ready(&mut p, c);
    assert_eq!(p.core().state().variables["affection"], Value::I32(1));
    assert!(p.error.is_none(), "{:?}", p.error);
}
