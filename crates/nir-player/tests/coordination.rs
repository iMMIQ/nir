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

#[test]
fn profile_persistence_sends_new_keys_without_resending_loaded_progress() {
    use std::collections::BTreeSet;
    let mut program: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    let function = program.functions.get_mut("main").unwrap();
    let original = function.entry.clone();
    function.entry = "profile_delta".into();
    let new_keys: BTreeSet<String> = (0..64).map(|i| format!("unlocked.{i:03}")).collect();
    let loaded_keys: BTreeSet<String> = (0..128).map(|i| format!("previous.{i:03}")).collect();
    let mut ops = vec![];
    for key in loaded_keys
        .iter()
        .chain(new_keys.iter())
        .chain(new_keys.iter())
    {
        ops.push(serde_json::json!({"id":format!("profile.{}", ops.len()),"operation":{"type":"profile_merge","key":key}}));
    }
    function.blocks.insert(
        "profile_delta".into(),
        serde_json::from_value(serde_json::json!({
            "ops":ops,"terminator":{"type":"goto","target":original}
        }))
        .unwrap(),
    );
    let mut p = Player::new(program, "release".into(), "Test".into()).unwrap();
    let initial = p.pump(vec![AppEvent::Profile(loaded_keys.clone())], 1000);
    assert!(!initial
        .iter()
        .any(|c| matches!(c, AppCommand::PersistProfile { .. })));
    ready(&mut p, initial);
    let commands = action(&mut p, UiAction::NewGame);
    let commands = ready(&mut p, commands);
    let writes: Vec<_> = commands
        .iter()
        .filter_map(|c| match c {
            AppCommand::PersistProfile { keys } => Some(keys),
            _ => None,
        })
        .collect();
    let total_keys: usize = writes.iter().map(|keys| keys.len()).sum();
    assert_eq!(
        writes.len(),
        1,
        "adjacent progress deltas share one durable write"
    );
    assert_eq!(
        total_keys,
        new_keys.len(),
        "only newly learned progress crosses the host boundary"
    );
    assert_eq!(
        writes
            .into_iter()
            .flat_map(|keys| keys.iter().cloned())
            .collect::<BTreeSet<_>>(),
        new_keys
    );
    let mut stored = loaded_keys;
    for command in commands {
        if let AppCommand::PersistProfile { keys } = command {
            stored.extend(keys);
        }
    }
    assert_eq!(
        stored, p.profile,
        "both hosts merge deltas into previously stored progress"
    );
    assert!(p.error.is_none());
}
#[test]
fn profile_batches_preserve_an_intervening_audio_command() {
    let mut program: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    let mut ops = vec![];
    for prefix in ["before", "after"] {
        if prefix == "after" {
            ops.push(serde_json::json!({"id":"stop.music","operation":{
                "type":"task_control","task":"music","action":"cancel"
            }}));
        }
        for index in 0..32 {
            ops.push(
                serde_json::json!({"id":format!("{prefix}.{index}"),"operation":{
                    "type":"profile_merge","key":format!("{prefix}.{index}")
                }}),
            );
        }
    }
    program
        .functions
        .get_mut("main")
        .unwrap()
        .blocks
        .get_mut("intro")
        .unwrap()
        .ops = serde_json::from_value(serde_json::Value::Array(ops)).unwrap();
    let mut p = Player::new(program, "release".into(), "Test".into()).unwrap();
    let commands = p.pump(vec![], 1000);
    ready(&mut p, commands);
    let commands = action(&mut p, UiAction::NewGame);
    let commands = ready(&mut p, commands);
    let meaningful: Vec<_> = commands
        .iter()
        .filter_map(|command| match command {
            AppCommand::PersistProfile { keys } => {
                assert_eq!(keys.len(), 32);
                Some(if keys.iter().all(|key| key.starts_with("before.")) {
                    "before"
                } else if keys.iter().all(|key| key.starts_with("after.")) {
                    "after"
                } else {
                    panic!("progress crossed the audio barrier")
                })
            }
            AppCommand::AudioStop {
                domain: TimeDomain::Story,
                ..
            } => Some("stop"),
            _ => None,
        })
        .collect();
    assert_eq!(meaningful, ["before", "stop", "after"]);
    assert_eq!(p.profile.len(), 64);
    assert!(p.error.is_none());
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
    let mut p: Program = serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    if mode != "plain" {
        p.requires.push("story.typed-result.v1".into());
    }
    p.variables.insert(
        "picked".into(),
        serde_json::from_value(serde_json::json!({"type":"i32","value":0})).unwrap(),
    );
    for (option, value) in p
        .choices
        .get_mut("route")
        .unwrap()
        .options
        .iter_mut()
        .zip([1, 2])
    {
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
    for (block, outcome) in [
        ("after_walk", "walk"),
        ("after_stay", "stay"),
        ("gave_up", "gave_up"),
    ] {
        f.blocks.insert(
            block.into(),
            serde_json::from_value(
                serde_json::json!({"terminator":{"type":"end","outcome":outcome}}),
            )
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
    assert!(
        p.core().state().choice.is_some(),
        "typed interaction pending"
    );
    p
}

#[test]
fn history_choice_view_distinguishes_commit_timeout_cancel_and_never_recommits() {
    use nir_presentation::{HistoryChoiceKind, Messages, ReadingState, Screen};
    for kind in [
        HistoryChoiceKind::Selected,
        HistoryChoiceKind::TimedOut,
        HistoryChoiceKind::Cancelled,
    ] {
        let mut program = typed_program("cancel");
        if kind == HistoryChoiceKind::TimedOut {
            let choice = program.choices.get_mut("route").unwrap();
            choice.timeout_us = Some(Micros(100_000));
            choice.default = Some("stay".into());
        }
        let mut p = Player::new(program, "release".into(), "Test".into()).unwrap();
        let commands = p.pump(vec![], 1000);
        ready(&mut p, commands);
        let commands = action(&mut p, UiAction::NewGame);
        ready(&mut p, commands);
        let commands = match kind {
            HistoryChoiceKind::Selected => action(
                &mut p,
                UiAction::Choose {
                    option: "stay".into(),
                },
            ),
            HistoryChoiceKind::TimedOut => p.pump(vec![AppEvent::Tick { delta_us: 100_000 }], 1000),
            HistoryChoiceKind::Cancelled => action(&mut p, UiAction::CancelChoice),
        };
        ready(&mut p, commands);
        assert_eq!(p.core().state().history.len(), 1);
        let before = serde_json::to_value(p.core().snapshot()).unwrap();
        let commands = action(&mut p, UiAction::History);
        ready(&mut p, commands);
        assert_eq!(p.model().screen, Screen::History);
        let h = p.model().history.remove(0);
        assert_eq!(h.choice, Some(kind));
        assert_eq!(h.voice_count, 0);
        for (width, height) in [(390., 844.), (1280., 720.)] {
            let mut model = p.model();
            // UI language can differ from the frozen option language.
            model.ui_locale = "en".into();
            model.status = "Saved".into();
            let mut text = reading_text();
            let messages = Messages::default();
            let packet = ReadingState::default().project(
                &model,
                (p.generation.session, p.current_interaction()),
                width,
                height,
                &messages,
                &mut text,
            );
            let heading = packet
                .texts
                .iter()
                .find(|t| t.text == messages.text("en", kind.message()))
                .unwrap();
            assert_eq!(heading.locale, "en");
            assert_eq!(heading.font_plan_digest, model.ui_font_plan_digest);
            let body = packet.texts.iter().find(|t| t.text == h.text).unwrap();
            assert_eq!(body.locale, h.locale);
            assert_eq!(body.font_plan_digest, h.font_plan_digest);
            assert!(body.y > heading.y);
            assert!(body.height > 0.);
            let status = packet
                .texts
                .iter()
                .find(|t| t.text == model.status)
                .unwrap();
            let clip = status.clip.unwrap();
            assert!(clip[1] + clip[3] <= heading.y);
            assert!(packet.semantics.iter().all(|s| !matches!(
                s.action,
                UiAction::HistoryVoice { .. } | UiAction::Choose { .. }
            )));
        }
        assert!(action(&mut p, UiAction::HistoryVoice { entry: 0 })
            .iter()
            .all(|c| !matches!(
                c,
                AppCommand::AudioStart { .. }
                    | AppCommand::GetContent { .. }
                    | AppCommand::GetAssets { .. }
            )));
        p.pump(
            vec![AppEvent::Tick {
                delta_us: 1_000_000,
            }],
            1000,
        );
        action(&mut p, UiAction::Close);
        assert_eq!(serde_json::to_value(p.core().snapshot()).unwrap(), before);
    }
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
fn page_preparation_freezes_story_but_preserves_audio_and_device_position() {
    let mut p = playing();
    action(&mut p, UiAction::Advance);
    let music = p.core().state().handles["music"];
    let session = p.generation.session;
    let commands = action(&mut p, UiAction::Advance);
    assert!(p.is_loading());
    assert!(p.paused());
    assert!(!p.domain_paused(TimeDomain::Story));
    assert!(!commands.iter().any(|c| matches!(
        c,
        AppCommand::AudioPause {
            domain: TimeDomain::Story,
            paused: true
        } | AppCommand::AudioReset {
            domain: TimeDomain::Story
        }
    )));
    let tick = p.core().state().tick_us;
    p.pump(
        vec![AppEvent::Tick {
            delta_us: 3_000_000,
        }],
        1000,
    );
    assert_eq!(p.core().state().tick_us, tick);
    p.observe_audio_positions(
        TimeDomain::Story,
        session,
        &[AudioPosition {
            task: music,
            position_us: Micros(3_000_000),
            envelope: None,
        }],
    )
    .unwrap();
    assert_eq!(
        p.core().snapshot().tasks[&music].audio_position_us,
        Some(Micros(3_000_000))
    );
    let saved = action(&mut p, UiAction::Save { slot: 1 });
    let snapshot = saved
        .iter()
        .find_map(|command| match command {
            AppCommand::Save { envelope, .. } => Some(&envelope.snapshot),
            _ => None,
        })
        .unwrap();
    assert_eq!(snapshot.tick_us, tick);
    assert_eq!(
        snapshot.tasks[&music].audio_position_us,
        Some(Micros(3_000_000))
    );
    let commands = ready(&mut p, commands);
    assert!(!p.is_loading());
    assert!(!p.domain_paused(TimeDomain::Story));
    assert_eq!(
        p.core().state().tasks[&music].state,
        nir_core::TaskState::Running
    );
    assert!(!commands
        .iter()
        .any(|c| matches!(c, AppCommand::AudioStart { task, .. } if *task == music)));
}

#[test]
fn loading_does_not_release_explicit_audio_pause_owners() {
    let mut p = playing();
    action(&mut p, UiAction::Advance);
    let loading = action(&mut p, UiAction::Advance);
    assert!(p.is_loading());
    // A caller using the same label still owns an explicit full pause.
    let token = p.acquire_pause("prepare");
    assert!(p.pump(vec![], 1000).iter().any(|c| matches!(
        c,
        AppCommand::AudioPause {
            domain: TimeDomain::Story,
            paused: true
        }
    )));
    p.pump(vec![AppEvent::Hidden(true)], 1000);
    drop(token);
    assert!(p.domain_paused(TimeDomain::Story));
    let commands = p.pump(vec![AppEvent::Hidden(false)], 1000);
    assert!(p.is_loading());
    assert!(p.paused());
    assert!(commands.iter().any(|c| matches!(
        c,
        AppCommand::AudioPause {
            domain: TimeDomain::Story,
            paused: false
        }
    )));
    ready(&mut p, loading);
    assert!(!p.is_loading());
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
fn renderer_recovery_preserves_audio_instances_and_each_existing_pause_owner() {
    for owner in ["none", "menu", "hidden", "plugin", "output", "restored"] {
        let mut p = voice_bound_player(Some((Some("spoken"), VoiceWaitPolicy::Parallel)));
        if owner == "restored" {
            let snapshot = p.core().snapshot();
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
            assert!(p.model().can_continue);
        }
        let external = match owner {
            "plugin" => Some(p.acquire_pause("plugin")),
            "output" => Some(p.acquire_audio_output_wait()),
            _ => None,
        };
        if owner == "menu" {
            action(&mut p, UiAction::Menu);
        }
        if owner == "hidden" {
            p.pump(vec![AppEvent::Hidden(true)], 1000);
        }
        let snapshot = p.core().snapshot();
        let session = p.generation.session;
        let mut completed = p.pump(vec![AppEvent::DeviceLost], 1000);
        assert!(p.paused());
        let commands = p.pump(vec![AppEvent::DeviceReady], 1000);
        completed.extend(ready(&mut p, commands));
        assert!(
            !completed.iter().any(|c| matches!(
                c,
                AppCommand::AudioStart {
                    domain: TimeDomain::Story,
                    ..
                } | AppCommand::AudioStop {
                    domain: TimeDomain::Story,
                    ..
                } | AppCommand::AudioReset {
                    domain: TimeDomain::Story
                }
            )),
            "render recovery must retain the existing audio graph: {owner}"
        );
        assert_eq!(p.core().state().tick_us, snapshot.tick_us);
        assert_eq!(p.core().state().variables, snapshot.variables);
        assert_eq!(
            serde_json::to_value(&p.core().state().history).unwrap(),
            serde_json::to_value(&snapshot.history).unwrap()
        );
        assert_eq!(p.generation.session, session);
        assert_eq!(p.paused(), owner != "none");
        assert_eq!(
            p.model().can_continue,
            owner == "restored",
            "renderer recovery must preserve an existing read confirmation"
        );
        assert_eq!(
            p.bus_paused(TimeDomain::Story, AudioBus::Bgm),
            matches!(owner, "hidden" | "plugin" | "restored")
        );
        assert_eq!(
            p.bus_paused(TimeDomain::Story, AudioBus::Voice),
            matches!(owner, "menu" | "hidden" | "plugin" | "restored")
        );
        drop(external);
    }
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
    assert!(!p.paused());
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
    let c = action(
        &mut p,
        UiAction::SelectChoice {
            option: "stay".into(),
        },
    );
    ready(&mut p, c);
    assert_eq!(
        p.core()
            .state()
            .choice
            .as_ref()
            .unwrap()
            .selected
            .as_deref(),
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
    let c = action(
        &mut p,
        UiAction::Choose {
            option: "stay".into(),
        },
    );
    ready(&mut p, c);
    assert_eq!(p.core().state().variables["picked"], Value::I32(2));
    assert_eq!(p.core().state().outcome.as_deref(), Some("stay"));
}

#[test]
fn rollback_after_a_typed_commit_rewinds_the_write() {
    let mut p = typed_playing("typed");
    let c = action(
        &mut p,
        UiAction::Choose {
            option: "stay".into(),
        },
    );
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
    let choice = p
        .core()
        .state()
        .choice
        .as_ref()
        .expect("interaction re-offered");
    assert_eq!(choice.selected.as_deref(), Some("walk"));
    action(&mut p, UiAction::Continue);
    let c = action(
        &mut p,
        UiAction::Choose {
            option: "stay".into(),
        },
    );
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
    assert!(
        !p.paused(),
        "completed renderer recovery releases only its own pause owner"
    );
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

fn export_command(p: &mut Player) -> (u32, String) {
    action(p, UiAction::Export)
        .into_iter()
        .find_map(|c| {
            if let AppCommand::Export { job, json } = c {
                Some((job, json))
            } else {
                None
            }
        })
        .unwrap()
}

#[test]
fn export_failures_cancel_and_late_replies_keep_story_and_audio_owners() {
    for owner in ["story", "settings", "hidden"] {
        let mut p = playing();
        let (old, _) = export_command(&mut p);
        let (job, json) = export_command(&mut p);
        let envelope: SaveEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(
            envelope.digest,
            nir_content::digest(&serde_json::to_vec(&envelope.snapshot).unwrap())
        );
        if owner == "settings" {
            action(&mut p, UiAction::Settings);
        }
        if owner == "hidden" {
            p.pump(vec![AppEvent::Hidden(true)], 1000);
        }
        let before = serde_json::to_value(p.core().snapshot()).unwrap();
        let pause = p.paused();
        let bgm = p.bus_paused(TimeDomain::Story, AudioBus::Bgm);
        let commands = p.pump(
            vec![AppEvent::ExportFailed {
                job,
                message: "write denied".into(),
            }],
            1000,
        );
        assert_eq!(serde_json::to_value(p.core().snapshot()).unwrap(), before);
        assert_eq!(p.paused(), pause);
        assert_eq!(p.bus_paused(TimeDomain::Story, AudioBus::Bgm), bgm);
        assert!(p.error.is_none());
        assert_eq!(p.diagnostic.as_ref().unwrap().location, "export");
        assert!(!commands.iter().any(|c| matches!(
            c,
            AppCommand::AudioStop { .. }
                | AppCommand::AudioReset { .. }
                | AppCommand::AudioPause { .. }
                | AppCommand::AudioBusPause { .. }
        )));
        let warning = p.diagnostic.as_ref().unwrap().message.clone();
        let status = p.status.clone();
        p.pump(
            vec![
                AppEvent::Exported { job: old },
                AppEvent::ExportFailed {
                    job: old,
                    message: "late".into(),
                },
            ],
            1000,
        );
        assert_eq!(p.diagnostic.as_ref().unwrap().message, warning);
        assert_eq!(p.status, status);
        let (cancelled, _) = export_command(&mut p);
        p.pump(
            vec![
                AppEvent::ExportCancelled { job: cancelled },
                AppEvent::Exported { job: cancelled },
            ],
            1000,
        );
        assert_eq!(p.diagnostic.as_ref().unwrap().message, warning);
        let (retry, _) = export_command(&mut p);
        p.pump(vec![AppEvent::Exported { job: retry }], 1000);
        assert!(p.diagnostic.is_none());
        assert!(p.error.is_none());
        assert!(p.status.is_empty());
        assert_eq!(serde_json::to_value(p.core().snapshot()).unwrap(), before);
        assert_eq!(p.paused(), pause);
    }
}

#[test]
fn successful_export_preserves_other_storage_faults_and_save_feedback() {
    let mut p = playing();
    p.pump(
        vec![AppEvent::PersistenceFailed {
            kind: PersistenceKind::Preferences,
            message: "still unwritten".into(),
        }],
        1000,
    );
    let (job, _) = export_command(&mut p);
    p.pump(
        vec![AppEvent::ExportFailed {
            job,
            message: "file denied".into(),
        }],
        1000,
    );
    let (job, _) = export_command(&mut p);
    p.status = "Saved".into();
    p.pump(vec![AppEvent::Exported { job }], 1000);
    assert_eq!(p.status, "Saved");
    assert_eq!(p.diagnostic.as_ref().unwrap().location, "preferences");
    p.pump(
        vec![AppEvent::PersistenceStored(PersistenceKind::Preferences)],
        1000,
    );
    assert_eq!(p.status, "Saved");
    assert!(p.diagnostic.is_none());
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
        key: 0,
        voice_count: 0,
        choice: None,
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
    let w = m
        .window_transition
        .as_ref()
        .expect("reveal live in the model");
    assert_eq!(w.style, StageTransition::Dissolve);
    assert!(!w.to_visible);
    assert!((w.progress - 0.5).abs() < 0.001);
    assert!(
        !m.hidden_dialogue,
        "the committed flag lands at the deadline"
    );
    assert!(
        m.dialogue.is_some(),
        "the view stays projectable while live"
    );
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
                AppCommand::GetAssets {
                    request, assets, ..
                } => {
                    if assets.iter().any(|a| a == "bg.river") {
                        assert!(
                            mask_request.replace((request, assets)).is_none(),
                            "the mask is fetched by exactly one top-up"
                        );
                    } else {
                        for asset in assets {
                            next.extend(
                                p.pump(vec![AppEvent::AssetReady { request, asset }], 1000),
                            );
                        }
                    }
                }
                AppCommand::PreparePresentation { request } => {
                    next.extend(p.pump(vec![AppEvent::PresentationReady { request }], 1000))
                }
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
    let (request, withheld) = mask_request.expect("the reveal mask is fetched by a top-up");
    assert!(
        p.paused(),
        "the story clock holds while the mask is outstanding"
    );
    let before = p.core().state().tick_us;
    p.pump(
        vec![AppEvent::Tick {
            delta_us: 5_000_000,
        }],
        1000,
    );
    assert_eq!(
        p.core().state().tick_us,
        before,
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
    let w = m
        .window_transition
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
    assert!(
        faded_quads < before.quads.len(),
        "HUD quads keep their color"
    );
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
    assert_eq!(after.quads.len(), before.quads.len() - diverted + 1);
    // Window texts stay in the packet for layout, routed to the window pass.
    assert_eq!(after.texts, before.texts);
    assert!(!layers.texts.is_empty());
    assert!(layers.texts.windows(2).all(|pair| pair[0] + 1 == pair[1]));
    assert!(layers
        .texts
        .iter()
        .any(|&i| before.texts[i].region.is_some()));
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
        AppCommand::AudioBusPause {
            domain: TimeDomain::Story,
            bus: AudioBus::Voice,
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

#[test]
fn continue_control_requires_a_restore_pause_it_can_release() {
    let mut p = playing();
    let has_continue = |p: &Player| {
        nir_presentation::project(
            &p.model(),
            390.,
            844.,
            &nir_presentation::Messages::default(),
        )
        .semantics
        .iter()
        .any(|n| n.enabled && n.action == UiAction::Continue)
    };
    assert!(!has_continue(&p));
    let output = p.acquire_audio_output_wait();
    assert!(p.paused());
    assert!(
        !has_continue(&p),
        "Continue cannot release an output barrier"
    );
    drop(output);
    let snapshot = p.core().snapshot();
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
    assert!(
        has_continue(&p),
        "a prepared restore needs explicit continuation"
    );
    let external = p.acquire_pause("plugin");
    assert!(
        !has_continue(&p),
        "another owner still prevents continuation"
    );
    drop(external);
    assert!(has_continue(&p));
    action(&mut p, UiAction::Continue);
    assert!(!p.paused());
    assert!(!has_continue(&p));
}

#[test]
fn audio_output_wait_preserves_sources_and_never_releases_other_pause_owners() {
    let mut p = voice_bound_player(Some((Some("spoken"), VoiceWaitPolicy::Parallel)));
    let token = p.current_interaction();
    let tick = p.core().state().tick_us;
    action(&mut p, UiAction::ToggleAuto);
    let output = p.acquire_audio_output_wait();
    assert!(p.paused());
    assert!(!p.domain_paused(TimeDomain::Story));
    let commands = p.pump(
        vec![AppEvent::Tick {
            delta_us: 2_000_000,
        }],
        1000,
    );
    assert!(!commands.iter().any(|c| matches!(
        c,
        AppCommand::AudioStop { .. } | AppCommand::AudioStart { .. }
    )));
    assert_eq!(p.core().state().tick_us, tick);
    assert_eq!(p.current_interaction(), token);
    action(&mut p, UiAction::Advance);
    assert_eq!(p.current_interaction(), token);
    action(&mut p, UiAction::Menu);
    p.pump(vec![AppEvent::Hidden(true)], 1000);
    drop(output);
    assert!(p.paused());
    assert!(p.domain_paused(TimeDomain::Story));
    p.pump(vec![AppEvent::Hidden(false)], 1000);
    assert!(p.paused(), "menu still owns Story pause");
    assert!(!p.bus_paused(TimeDomain::Story, AudioBus::Bgm));
    assert!(p.bus_paused(TimeDomain::Story, AudioBus::Voice));
    action(&mut p, UiAction::Close);
    assert!(!p.paused());
    p.pump(vec![AppEvent::Tick { delta_us: 1 }], 1000);
    assert_eq!(p.core().state().tick_us.0, tick.0 + 1);
    assert_eq!(p.current_interaction(), token);
}

fn voice_bound_player(binding: Option<(Option<&str>, VoiceWaitPolicy)>) -> Player {
    voice_bound_player_with(binding, |_| {})
}
fn voice_bound_player_with(
    binding: Option<(Option<&str>, VoiceWaitPolicy)>,
    change: impl FnOnce(&mut Program),
) -> Player {
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
                    loop_region: None,
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
    change(&mut program);
    let mut p = Player::new(program, "release".into(), "Test".into()).unwrap();
    let commands = p.pump(vec![], 1000);
    ready(&mut p, commands);
    let commands = action(&mut p, UiAction::NewGame);
    ready(&mut p, commands);
    action(&mut p, UiAction::Advance);
    assert!(p.core().dialogue().unwrap().1.awaiting_advance);
    p
}

fn history_audio(commands: &[AppCommand]) -> Vec<u32> {
    commands
        .iter()
        .filter_map(|c| match c {
            AppCommand::AudioStart {
                domain: TimeDomain::ForegroundUi,
                bus: AudioBus::Voice,
                task,
                looped: false,
                ..
            } => Some(*task),
            _ => None,
        })
        .collect()
}

fn authored_voice_program(program: &mut Program, flow: bool) {
    program.requires.extend(
        [
            "ui.menu-elements.v1",
            "ui.menu-state.v1",
            "ui.menu-services.v1",
            "ui.menu-history-voice.v1",
            if flow {
                "ui.menu-history-flow.v1"
            } else {
                "ui.menu-history.v1"
            },
        ]
        .map(str::to_owned),
    );
    program.theme.menu_overlay = Some("history".into());
    let content = if flow {
        serde_json::json!({"type":"history_flow","voice_controls":true,"size":16,"line_height":24,"gap":12,"wheel_step":48,"page_step":120,"max_visible":56,"color":[1,1,1,1]})
    } else {
        serde_json::json!({"type":"history_window","voice_controls":true,"offset_local":"offset","limit":16,"row_height":80,"size":16,"color":[1,1,1,1]})
    };
    program.theme.image_menus.insert("history".into(), serde_json::from_value(serde_json::json!({
        "background":"bg.station","buttons":[],
        "locals":{"offset":{"type":"int","initial":0,"min":0,"max":999},"shown":{"type":"bool","initial":true},"enabled":{"type":"bool","initial":true}},
        "elements":[{"id":"records","rect":[20,20,600,360],"content":content,
            "visible_when":[{"type":"local","name":"shown","equals":true}],
            "enabled_when":[{"type":"local","name":"enabled","equals":true}]},
            {"id":"hide","rect":[20,500,200,60],"content":{"type":"hit_region","label":"Hide","action":{"type":"set_local","local":"shown","value":false}}},
            {"id":"disable","rect":[240,500,200,60],"content":{"type":"hit_region","label":"Disable","action":{"type":"set_local","local":"enabled","value":false}}}]
    })).unwrap());
}
fn authored_voice_packet(p: &Player, width: f32, height: f32) -> nir_presentation::DrawPacket {
    authored_voice_packet_with(p, width, height, |_| {})
}
fn authored_voice_packet_with(
    p: &Player,
    width: f32,
    height: f32,
    change: impl FnOnce(&mut nir_presentation::UiModel),
) -> nir_presentation::DrawPacket {
    let mut model = p.model();
    model.ui_locale = "en".into();
    change(&mut model);
    let mut text = nir_presentation::TextEngine::default();
    text.add_font_asset(
        "font.reader",
        include_bytes!("../../../examples/rain-letters/assets/source/reader.otf").to_vec(),
    )
    .unwrap();
    let mut reading = nir_presentation::ReadingState::default();
    let messages = nir_presentation::Messages::default();
    let identity = (p.generation.session, p.current_interaction());
    let mut packet = reading.project(&model, identity, width, height, &messages, &mut text);
    for _ in 0..100 {
        if !reading.history_pending() {
            break;
        }
        packet = reading.project(&model, identity, width, height, &messages, &mut text);
    }
    assert!(
        reading.history_error().is_none(),
        "{:?}",
        reading.history_error()
    );
    assert!(!reading.history_pending());
    packet
}

#[test]
fn authored_voice_controls_follow_page_compositing_and_cannot_start_during_a_spatial_reveal() {
    for flow in [false, true] {
        let mut p = voice_bound_player_with(
            Some((Some("spoken"), VoiceWaitPolicy::Parallel)),
            |program| authored_voice_program(program, flow),
        );
        let commands = action(&mut p, UiAction::Menu);
        ready(&mut p, commands);
        let plain = authored_voice_packet(&p, 1280., 720.);
        assert!(plain.menu_paint.iter().all(|paint| match paint {
            nir_presentation::MenuPaint::Quad(i) => *i < plain.quads.len(),
            nir_presentation::MenuPaint::Text(i) => *i < plain.texts.len(),
        }));
        let packet = authored_voice_packet_with(&p, 1280., 720., |m| {
            m.menu_transition = Some((
                StageTransition::Wipe {
                    direction: WipeDirection::LeftToRight,
                    softness: 0.1,
                },
                true,
                0.5,
            ));
        });
        let control = packet
            .semantics
            .iter()
            .find(|n| matches!(n.action, UiAction::MenuHistoryVoice { .. }))
            .unwrap();
        assert!(!control.enabled);
        let layer = packet.menu_layers.as_ref().unwrap();
        assert!(layer.quads.iter().any(|q| q.rect == control.rect));
        assert!(!packet.quads.iter().any(|q| q.rect == control.rect));
        assert!(layer
            .texts
            .iter()
            .any(|i| packet.texts[*i].text == "Replay voice"));
        assert_eq!(
            layer
                .texts
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            layer.texts.len(),
            "each page text is drawn once"
        );
        assert!(packet.menu_paint.is_empty());
        let ids = packet
            .semantics
            .iter()
            .map(|n| n.id)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(ids.len(), packet.semantics.len());
        assert!(p.history_voice_model().is_none());
    }
}

#[test]
fn authored_history_voice_controls_preserve_later_author_hotspot_priority() {
    for flow in [false, true] {
        let mut p = voice_bound_player_with(
            Some((Some("spoken"), VoiceWaitPolicy::Parallel)),
            |program| {
                authored_voice_program(program, flow);
                program.theme.image_menus.get_mut("history").unwrap().elements.push(serde_json::from_value(serde_json::json!({
                "id":"cover","rect":[502,20,118,44],"content":{"type":"hit_region","label":"Cover","action":{"type":"close"}}
            })).unwrap());
            },
        );
        let commands = action(&mut p, UiAction::Menu);
        ready(&mut p, commands);
        let packet = authored_voice_packet(&p, 1280., 720.);
        let voice = packet
            .semantics
            .iter()
            .find(|n| matches!(n.action, UiAction::MenuHistoryVoice { .. }))
            .unwrap();
        let [x, y, w, h] = voice.rect;
        assert_eq!(
            packet.hit_node(x + w / 2., y + h / 2.).unwrap().label,
            "Cover"
        );
    }
}

#[test]
fn authored_history_audition_requires_scoped_controls_and_retires_on_disable_hide_and_close() {
    for flow in [false, true] {
        for boundary in ["disable", "hide", "close"] {
            let mut p = voice_bound_player_with(
                Some((Some("spoken"), VoiceWaitPolicy::Parallel)),
                |program| authored_voice_program(program, flow),
            );
            let commands = action(&mut p, UiAction::Menu);
            ready(&mut p, commands);
            let before = serde_json::to_value(p.core().snapshot()).unwrap();
            let packet = authored_voice_packet(&p, 1280., 720.);
            let control = packet
                .semantics
                .iter()
                .find(|node| matches!(node.action, UiAction::MenuHistoryVoice { .. }))
                .unwrap();
            assert!(control.enabled);
            assert!(control.rect[2] >= 72. && control.rect[3] >= 44.);
            let request = control.action.clone();
            assert!(
                history_audio(&action(&mut p, request.clone())).is_empty(),
                "raw actor events have no layout authority"
            );
            assert!(p.history_voice_model().is_none());
            let mut stale = request.clone();
            if let UiAction::MenuHistoryVoice { revision, .. } = &mut stale {
                *revision += 1;
            }
            p.audition_menu_history(&stale);
            assert!(p.history_voice_model().is_none());
            p.audition_menu_history(&request);
            let commands = p.pump(vec![], 1000);
            let commands = ready(&mut p, commands);
            let task = history_audio(&commands)[0];
            assert_eq!(
                p.history_voice_model().unwrap().window.as_deref(),
                Some("records")
            );
            let packet = authored_voice_packet(&p, 1280., 720.);
            assert!(!p.validate_history_voice_controls(&packet));
            assert!(packet
                .semantics
                .iter()
                .any(|n| matches!(&n.action, UiAction::MenuHistoryVoice { stop: true, .. })));
            let ui = p.model();
            let commands = if boundary == "close" {
                action(&mut p, UiAction::Close)
            } else {
                action(
                    &mut p,
                    UiAction::MenuControl {
                        control: boundary.into(),
                        instance: ui.menu_instance,
                        revision: ui.menu_revision,
                    },
                )
            };
            assert!(commands.iter().any(|c| matches!(c,AppCommand::AudioStop {domain:TimeDomain::ForegroundUi,task:id,..} if *id==task)));
            assert!(p.history_voice_model().is_none());
            p.audition_menu_history(&request);
            assert!(p.history_voice_model().is_none());
            p.pump(
                vec![AppEvent::AudioEnded {
                    domain: TimeDomain::ForegroundUi,
                    task,
                    session: p.generation.session,
                }],
                1000,
            );
            assert_eq!(serde_json::to_value(p.core().snapshot()).unwrap(), before);
            assert!(p.error.is_none());
        }
    }
}

#[test]
fn authored_history_controls_adapt_to_portrait_and_retire_when_a_control_is_clipped() {
    for flow in [false, true] {
        let mut p = voice_bound_player_with(
            Some((Some("spoken"), VoiceWaitPolicy::Parallel)),
            |program| authored_voice_program(program, flow),
        );
        let commands = action(&mut p, UiAction::Menu);
        ready(&mut p, commands);
        let packet = authored_voice_packet(&p, 390., 844.);
        let control = packet
            .semantics
            .iter()
            .find(|n| matches!(n.action, UiAction::MenuHistoryVoice { .. }))
            .unwrap();
        assert_eq!(control.rect[3], 44.);
        assert_eq!(control.rect[2], 118.);
        let body = packet
            .texts
            .iter()
            .find(|r| r.text == p.core().state().history.last().unwrap().text)
            .unwrap();
        assert!(body.y >= control.rect[1] + 52.);
        let ui = p.model();
        let mut disabled = ui.clone();
        disabled
            .theme
            .image_menus
            .get_mut("history")
            .unwrap()
            .elements[0]
            .clip = Some([20., 20., 600., 40.]);
        let mut reading = nir_presentation::ReadingState::default();
        let mut text = nir_presentation::TextEngine::default();
        text.add_font_asset(
            "font.reader",
            include_bytes!("../../../examples/rain-letters/assets/source/reader.otf").to_vec(),
        )
        .unwrap();
        let clipped = reading.project(
            &disabled,
            (p.generation.session, p.current_interaction()),
            1280.,
            720.,
            &nir_presentation::Messages::default(),
            &mut text,
        );
        assert!(!clipped
            .semantics
            .iter()
            .any(|n| matches!(n.action, UiAction::MenuHistoryVoice { .. })));
        p.audition_menu_history(&control.action);
        let commands = p.pump(vec![], 1000);
        let commands = ready(&mut p, commands);
        let task = history_audio(&commands)[0];
        assert!(p.validate_history_voice_controls(&clipped));
        let commands = p.pump(vec![], 1000);
        assert!(commands.iter().any(|c| matches!(c,AppCommand::AudioStop {domain:TimeDomain::ForegroundUi,task:id,..} if *id==task)));
    }
}

#[test]
fn history_voice_isolated_replacement_close_and_stale_callbacks_preserve_the_story() {
    let mut p = voice_bound_player(Some((Some("spoken"), VoiceWaitPolicy::Parallel)));
    action(&mut p, UiAction::History);
    let snapshot = serde_json::to_value(p.core().snapshot()).unwrap();
    let entry = p.core().state().history.len() - 1;
    let commands = action(&mut p, UiAction::HistoryVoice { entry });
    assert!(
        history_audio(&commands).is_empty(),
        "no playback before media is ready"
    );
    assert!(
        !p.is_loading(),
        "audition must not block closing or replacing it"
    );
    let commands = ready(&mut p, commands);
    let first = history_audio(&commands)[0];
    let commands = action(&mut p, UiAction::HistoryVoice { entry });
    assert!(commands.iter().any(|c| matches!(c, AppCommand::AudioStop { domain: TimeDomain::ForegroundUi, task, .. } if *task == first)));
    let commands = ready(&mut p, commands);
    let second = history_audio(&commands)[0];
    assert_ne!(first, second);
    p.pump(
        vec![AppEvent::AudioEnded {
            domain: TimeDomain::ForegroundUi,
            task: first,
            session: p.generation.session,
        }],
        1000,
    );
    assert!(p.history_voice_model().is_some());
    p.pump(
        vec![AppEvent::Tick {
            delta_us: 2_000_000,
        }],
        1000,
    );
    assert_eq!(serde_json::to_value(p.core().snapshot()).unwrap(), snapshot);
    let commands = action(&mut p, UiAction::Close);
    assert!(commands.iter().any(|c| matches!(c, AppCommand::AudioStop { domain: TimeDomain::ForegroundUi, task, .. } if *task == second)));
    assert!(p.history_voice_model().is_none());
    assert_eq!(serde_json::to_value(p.core().snapshot()).unwrap(), snapshot);
    p.pump(
        vec![AppEvent::AudioFailed {
            domain: TimeDomain::ForegroundUi,
            task: second,
            session: p.generation.session,
            message: "late failure".into(),
        }],
        1000,
    );
    assert!(p.error.is_none());
}

#[test]
fn history_media_cancel_ignores_stale_readiness_and_failure_can_retry() {
    let mut p = voice_bound_player(Some((Some("spoken"), VoiceWaitPolicy::Parallel)));
    action(&mut p, UiAction::History);
    let entry = p.core().state().history.len() - 1;
    let commands = action(&mut p, UiAction::HistoryVoice { entry });
    let request = commands
        .iter()
        .find_map(|c| match c {
            AppCommand::GetAssets { request, .. } => Some(*request),
            _ => None,
        })
        .unwrap();
    let commands = action(&mut p, UiAction::StopHistoryVoice);
    assert!(commands
        .iter()
        .any(|c| matches!(c, AppCommand::CancelAssets { request: id } if *id == request)));
    assert!(!p.accepts_resource(request));
    let stale = p.pump(
        vec![AppEvent::AssetReady {
            request,
            asset: p.core().state().history[entry].voices[0].asset.clone(),
        }],
        1000,
    );
    assert!(history_audio(&stale).is_empty());
    p.pump(vec![AppEvent::AssetsCancelled { request }], 1000);
    let commands = action(&mut p, UiAction::HistoryVoice { entry });
    let request = commands
        .iter()
        .find_map(|c| match c {
            AppCommand::GetAssets { request, .. } => Some(*request),
            _ => None,
        })
        .unwrap();
    p.pump(
        vec![AppEvent::AssetFailed {
            request,
            message: "decode failed".into(),
        }],
        1000,
    );
    assert!(p.history_voice_model().unwrap().failed);
    assert!(p.error.is_none());
    p.pump(vec![AppEvent::AssetsCancelled { request }], 1000);
    let commands = action(&mut p, UiAction::HistoryVoice { entry });
    assert_eq!(history_audio(&ready(&mut p, commands)).len(), 1);
}

#[test]
fn history_audio_preparation_survives_visual_changes_but_device_loss_retires_it() {
    let mut p = voice_bound_player(Some((Some("spoken"), VoiceWaitPolicy::Parallel)));
    action(&mut p, UiAction::History);
    let entry = p.core().state().history.len() - 1;
    let stale = p.pump(
        vec![AppEvent::Action {
            action: UiAction::HistoryVoice { entry },
            interaction: 0,
            sequence: p.core().state().last_input + 1,
            session: p.generation.session,
        }],
        1000,
    );
    assert!(!stale
        .iter()
        .any(|c| matches!(c, AppCommand::GetAssets { .. })));
    assert!(p.history_voice_model().is_none());
    let commands = action(&mut p, UiAction::HistoryVoice { entry });
    p.generation.surface += 1;
    p.generation.typography += 1;
    let commands = ready(&mut p, commands);
    let task = history_audio(&commands)[0];
    let session = p.generation.session;
    let lost = p.pump(vec![AppEvent::DeviceLost], 1000);
    assert!(lost.iter().any(|c| matches!(c, AppCommand::AudioStop { domain: TimeDomain::ForegroundUi, task: id, .. } if *id == task)));
    assert!(p.history_voice_model().is_none());
    let stale = p.pump(
        vec![AppEvent::AudioEnded {
            domain: TimeDomain::ForegroundUi,
            task,
            session,
        }],
        1000,
    );
    assert!(history_audio(&stale).is_empty());
    assert!(p.error.is_none());
}

#[test]
fn a_history_row_replays_multiple_utterances_serially_and_ends_without_vm_work() {
    let mut p = voice_bound_player_with(
        Some((Some("spoken"), VoiceWaitPolicy::Parallel)),
        |program| {
            let ops = &mut program
                .functions
                .get_mut("main")
                .unwrap()
                .blocks
                .get_mut("wait_intro")
                .unwrap()
                .ops;
            ops.push(Op {
                id: "second-history-voice".into(),
                operation: Operation::DialogueVoice {
                    task: "line".into(),
                    voice: Some("unrelated".into()),
                    wait: VoiceWaitPolicy::Parallel,
                },
            });
        },
    );
    action(&mut p, UiAction::History);
    let snapshot = serde_json::to_value(p.core().snapshot()).unwrap();
    let entry = p.core().state().history.len() - 1;
    assert_eq!(p.core().state().history[entry].voices.len(), 2);
    let commands = action(&mut p, UiAction::HistoryVoice { entry });
    let commands = ready(&mut p, commands);
    let first = history_audio(&commands)[0];
    let commands = p.pump(
        vec![AppEvent::AudioEnded {
            domain: TimeDomain::ForegroundUi,
            task: first,
            session: p.generation.session,
        }],
        1000,
    );
    assert!(history_audio(&commands).is_empty());
    let commands = ready(&mut p, commands);
    let second = history_audio(&commands)[0];
    assert_ne!(first, second);
    p.pump(
        vec![AppEvent::AudioEnded {
            domain: TimeDomain::ForegroundUi,
            task: second,
            session: p.generation.session,
        }],
        1000,
    );
    assert!(p.history_voice_model().is_none());
    assert_eq!(serde_json::to_value(p.core().snapshot()).unwrap(), snapshot);
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
fn legacy_auto_waits_for_spoken_voice_but_a_loop_cannot_hold_the_page_forever() {
    for continue_voice in [false, true] {
        let mut p = voice_bound_player_with(None, |program| {
            let unrelated = program
                .cues
                .get_mut("intro")
                .unwrap()
                .effects
                .iter_mut()
                .find(|effect| effect.id == "unrelated")
                .unwrap();
            let Effect::Audio { looped, .. } = &mut unrelated.effect else {
                unreachable!()
            };
            *looped = true;
        });
        let spoken = p.core().state().handles["spoken"];
        let looped = p.core().state().handles["unrelated"];
        let interaction = p.current_interaction();
        action(
            &mut p,
            UiAction::VoiceContinue {
                enabled: continue_voice,
            },
        );
        action(&mut p, UiAction::ToggleAuto);
        for _ in 0..40 {
            p.pump(vec![AppEvent::Tick { delta_us: 250_000 }], 1000);
        }
        assert_eq!(
            p.current_interaction(),
            interaction,
            "finite speech still owns the additional wait"
        );
        p.pump(
            vec![AppEvent::AudioEnded {
                domain: TimeDomain::Story,
                task: spoken,
                session: p.generation.session,
            }],
            1000,
        );
        let commands = p.pump(vec![AppEvent::Tick { delta_us: 1 }], 1000);
        assert_ne!(
            p.current_interaction(),
            interaction,
            "a looping Voice is not a finite line to wait for"
        );
        assert_eq!(
            p.core().state().tasks[&looped].state,
            nir_core::TaskState::Running
        );
        assert!(!commands.iter().any(|command| matches!(command,
            AppCommand::AudioStop { task, .. } if *task == looped)));
        assert!(p.error.is_none());
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
fn auto_voice_preference_can_skip_additional_wait_for_every_authored_voice_policy() {
    for binding in [
        None,
        Some((Some("spoken"), VoiceWaitPolicy::Parallel)),
        Some((Some("spoken"), VoiceWaitPolicy::AfterVoice)),
        Some((Some("spoken"), VoiceWaitPolicy::SampledRemaining)),
    ] {
        let mut p = voice_bound_player(binding);
        assert!(p.preferences.auto_wait_voice);
        let token = p.current_interaction();
        let spoken = p.core().state().handles["spoken"];
        let commands = action(&mut p, UiAction::AutoWaitVoice { enabled: false });
        assert!(commands.iter().any(|c| matches!(c, AppCommand::PersistPreferences { preferences } if !preferences.auto_wait_voice)));
        action(&mut p, UiAction::ToggleAuto);
        for _ in 0..30 {
            let commands = p.pump(vec![AppEvent::Tick { delta_us: 100_000 }], 1000);
            ready(&mut p, commands);
            if p.current_interaction() != token {
                break;
            }
        }
        assert_ne!(p.current_interaction(), token, "{binding:?}");
        assert_eq!(
            p.core().state().tasks[&spoken].state,
            nir_core::TaskState::Running,
            "preference must not stop authored session audio"
        );
    }
}

#[test]
fn auto_voice_preference_is_frozen_during_menus_until_the_next_cycle() {
    for wait in [
        VoiceWaitPolicy::Parallel,
        VoiceWaitPolicy::AfterVoice,
        VoiceWaitPolicy::SampledRemaining,
    ] {
        let mut p = voice_bound_player(Some((Some("spoken"), wait)));
        let token = p.current_interaction();
        action(&mut p, UiAction::ToggleAuto);
        p.pump(vec![AppEvent::Tick { delta_us: 100_000 }], 1000);
        action(&mut p, UiAction::Menu);
        let tick = p.core().state().tick_us;
        action(&mut p, UiAction::AutoWaitVoice { enabled: false });
        p.pump(
            vec![AppEvent::Tick {
                delta_us: 5_000_000,
            }],
            1000,
        );
        assert_eq!(p.core().state().tick_us, tick);
        action(&mut p, UiAction::Close);
        p.pump(vec![AppEvent::Tick { delta_us: 100_000 }], 1000);
        assert_eq!(
            p.current_interaction(),
            token,
            "in-flight {wait:?} cycle keeps voice waiting"
        );
        action(&mut p, UiAction::ToggleAuto);
        action(&mut p, UiAction::ToggleAuto);
        for _ in 0..30 {
            let commands = p.pump(vec![AppEvent::Tick { delta_us: 100_000 }], 1000);
            ready(&mut p, commands);
            if p.current_interaction() != token {
                break;
            }
        }
        assert_ne!(
            p.current_interaction(),
            token,
            "next {wait:?} cycle uses updated preference"
        );
    }
}

#[test]
fn opting_out_of_auto_voice_wait_cannot_bypass_an_authored_task_wait() {
    let mut p = voice_bound_player_with(
        Some((Some("spoken"), VoiceWaitPolicy::Parallel)),
        |program| {
            if let Terminator::Await { conditions, .. } = &mut program
                .functions
                .get_mut("main")
                .unwrap()
                .blocks
                .get_mut("wait_intro")
                .unwrap()
                .terminator
            {
                *conditions = vec![WaitCondition {
                    task: "spoken".into(),
                    milestone: Milestone::Finished,
                }];
            } else {
                panic!("expected fixture wait");
            }
        },
    );
    let spoken = p.core().state().handles["spoken"];
    let line = p.core().state().handles["line"];
    let location = p.core().location();
    action(&mut p, UiAction::AutoWaitVoice { enabled: false });
    action(&mut p, UiAction::ToggleAuto);
    for _ in 0..30 {
        p.pump(vec![AppEvent::Tick { delta_us: 100_000 }], 1000);
    }
    assert_eq!(
        p.core().state().tasks[&line].state,
        nir_core::TaskState::Finished
    );
    assert_eq!(
        p.core().state().tasks[&spoken].state,
        nir_core::TaskState::Running
    );
    assert_eq!(p.core().location(), location);
    let commands = p.pump(
        vec![AppEvent::AudioEnded {
            domain: TimeDomain::Story,
            task: spoken,
            session: p.generation.session,
        }],
        1000,
    );
    ready(&mut p, commands);
    assert_ne!(p.core().location(), location);
}

#[test]
fn auto_reading_wait_starts_after_reveal_even_when_one_tick_is_longer_than_the_wait() {
    let mut program: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    program.requires.push("player.auto-delay-policy.v1".into());
    program.player.auto_delay_policy = AutoDelayPolicy::Fixed;
    program.player.auto_delay_us = Micros(100_000);
    if let Effect::Dialogue { reveal_us, .. } =
        &mut program.cues.get_mut("intro").unwrap().effects[0].effect
    {
        *reveal_us = Micros(500_000);
    } else {
        panic!("expected dialogue");
    }
    let mut p = Player::new(program, "release".into(), "Test".into()).unwrap();
    let commands = p.pump(vec![], 1000);
    ready(&mut p, commands);
    let commands = action(&mut p, UiAction::NewGame);
    ready(&mut p, commands);
    let token = p.current_interaction();
    action(&mut p, UiAction::AutoWaitVoice { enabled: false });
    action(&mut p, UiAction::ToggleAuto);
    for _ in 0..128 {
        p.pump(vec![AppEvent::Tick { delta_us: 500_000 }], 1000);
        assert_eq!(
            p.current_interaction(),
            token,
            "reveal time must not consume reading delay"
        );
        if p.core().dialogue().unwrap().1.awaiting_advance {
            break;
        }
    }
    assert!(p.core().dialogue().unwrap().1.awaiting_advance);
    p.pump(vec![AppEvent::Tick { delta_us: 99_999 }], 1000);
    assert_eq!(p.current_interaction(), token);
    let commands = p.pump(vec![AppEvent::Tick { delta_us: 1 }], 1000);
    ready(&mut p, commands);
    assert_ne!(p.current_interaction(), token);
}

#[test]
fn after_voice_wait_counts_only_time_after_an_authored_fade_finishes() {
    // The 80 ms stop lies inside one 150 ms owner turn; only its last
    // 70 ms belongs to the 100 ms reading wait. Start away from tick zero
    // to check that the boundary uses absolute story time.
    let mut p = voice_bound_player_with(
        Some((Some("spoken"), VoiceWaitPolicy::AfterVoice)),
        |program| {
            program
                .requires
                .extend(["player.auto-delay-policy.v1".into(), "audio.stop.v1".into()]);
            program.player.auto_delay_policy = AutoDelayPolicy::Fixed;
            program
                .cues
                .get_mut("intro")
                .unwrap()
                .effects
                .push(EffectDef {
                    id: "stop-spoken".into(),
                    scope: Scope::Session,
                    effect: Effect::AudioStop {
                        target: "spoken".into(),
                        duration_us: Micros(80_000),
                    },
                });
        },
    );
    let spoken = p.core().state().handles["spoken"];
    let token = p.current_interaction();
    p.pump(vec![AppEvent::Tick { delta_us: 30_000 }], 1000);
    action(&mut p, UiAction::ToggleAuto);
    p.pump(vec![AppEvent::Tick { delta_us: 120_000 }], 1000);
    assert_eq!(
        p.core().state().tasks[&spoken].state,
        nir_core::TaskState::Cancelled
    );
    assert_eq!(
        p.current_interaction(),
        token,
        "fade time is not reading time"
    );
    p.pump(vec![AppEvent::Tick { delta_us: 29_999 }], 1000);
    assert_eq!(p.current_interaction(), token);
    let commands = p.pump(vec![AppEvent::Tick { delta_us: 1 }], 1000);
    ready(&mut p, commands);
    assert_ne!(p.current_interaction(), token);
}

#[test]
fn legacy_reading_preferences_wait_for_voice_and_reject_nonboolean_opt_out() {
    let mut json = serde_json::to_value(Preferences::default()).unwrap();
    json.as_object_mut().unwrap().remove("auto_wait_voice");
    assert!(
        serde_json::from_value::<Preferences>(json.clone())
            .unwrap()
            .auto_wait_voice
    );
    json["auto_wait_voice"] = false.into();
    assert!(
        !serde_json::from_value::<Preferences>(json.clone())
            .unwrap()
            .auto_wait_voice
    );
    json["auto_wait_voice"] = "false".into();
    assert!(serde_json::from_value::<Preferences>(json).is_err());
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
fn voice_continuation_is_global_and_stops_only_associated_story_voices_on_completion() {
    for enabled in [true, false] {
        let mut p = voice_bound_player(Some((Some("spoken"), VoiceWaitPolicy::Parallel)));
        let spoken = p.core().state().handles["spoken"];
        let unrelated = p.core().state().handles["unrelated"];
        let interaction = p.current_interaction();
        let commands = action(&mut p, UiAction::VoiceContinue { enabled });
        assert!(commands.iter().any(|c| matches!(c, AppCommand::PersistPreferences { preferences } if preferences.voice_continue == enabled)));
        assert_eq!(p.current_interaction(), interaction);
        assert_eq!(
            p.core().state().tasks[&spoken].state,
            nir_core::TaskState::Running
        );
        action(&mut p, UiAction::Menu);
        action(&mut p, UiAction::Advance);
        assert_eq!(p.current_interaction(), interaction);
        assert_eq!(
            p.core().state().tasks[&spoken].state,
            nir_core::TaskState::Running
        );
        action(&mut p, UiAction::Close);
        let commands = action(&mut p, UiAction::Advance);
        assert_ne!(p.current_interaction(), interaction);
        assert_eq!(commands.iter().filter(|c| matches!(c, AppCommand::AudioStop { domain: TimeDomain::Story, task, .. } if *task == spoken)).count(), usize::from(!enabled));
        assert_eq!(
            p.core().state().tasks[&spoken].state,
            if enabled {
                nir_core::TaskState::Running
            } else {
                nir_core::TaskState::Cancelled
            }
        );
        assert_eq!(
            p.core().state().tasks[&unrelated].state,
            nir_core::TaskState::Running
        );
        assert!(!commands
            .iter()
            .any(|c| matches!(c, AppCommand::AudioStop { task, .. } if *task == unrelated)));
        assert!(p.error.is_none());
    }
}

#[test]
fn manual_and_automatic_completion_share_voice_stop_policy_with_explicit_none_and_legacy_fallback()
{
    for automatic in [false, true] {
        for binding in [
            None,
            Some((None, VoiceWaitPolicy::Parallel)),
            Some((Some("spoken"), VoiceWaitPolicy::AfterVoice)),
        ] {
            let mut p = voice_bound_player(binding);
            let spoken = p.core().state().handles["spoken"];
            let unrelated = p.core().state().handles["unrelated"];
            let interaction = p.current_interaction();
            action(&mut p, UiAction::VoiceContinue { enabled: false });
            let mut commands = vec![];
            if automatic {
                action(&mut p, UiAction::AutoWaitVoice { enabled: false });
                action(&mut p, UiAction::ToggleAuto);
                for _ in 0..100 {
                    commands.extend(p.pump(vec![AppEvent::Tick { delta_us: 100_000 }], 1000));
                    if p.current_interaction() != interaction {
                        break;
                    }
                }
            } else {
                commands = action(&mut p, UiAction::Advance);
            }
            assert_ne!(p.current_interaction(), interaction);
            let expected_stop = binding.is_none() || binding.unwrap().0.is_some();
            assert_eq!(
                p.core().state().tasks[&spoken].state == nir_core::TaskState::Cancelled,
                expected_stop
            );
            assert_eq!(
                p.core().state().tasks[&unrelated].state == nir_core::TaskState::Cancelled,
                binding.is_none()
            );
            assert_eq!(commands.iter().any(|c| matches!(c, AppCommand::AudioStop { domain: TimeDomain::Story, task, .. } if *task == spoken)), expected_stop);
            assert!(p.error.is_none());
        }
    }
}

#[test]
fn held_and_latched_seen_skip_clean_up_voice_even_with_continuation_enabled() {
    for held in [false, true] {
        for seen in [false, true] {
            let mut p = voice_bound_player(Some((Some("spoken"), VoiceWaitPolicy::Parallel)));
            let spoken = p.core().state().handles["spoken"];
            let unrelated = p.core().state().handles["unrelated"];
            let interaction = p.current_interaction();
            assert!(p.preferences.voice_continue);
            if seen {
                let d = p.core().dialogue().unwrap().1;
                let key = format!("read:{}:{}", d.text_id, d.meaning_revision);
                p.pump(vec![AppEvent::Profile([key].into_iter().collect())], 1000);
            }
            action(
                &mut p,
                if held {
                    UiAction::HoldSkip { pressed: true }
                } else {
                    UiAction::ToggleSkip
                },
            );
            let commands = p.pump(vec![AppEvent::Tick { delta_us: 1000 }], 1000);
            assert_eq!(p.current_interaction() != interaction, seen);
            assert_eq!(commands.iter().any(|c| matches!(c, AppCommand::AudioStop { domain: TimeDomain::Story, task, .. } if *task == spoken)), seen);
            assert_eq!(
                p.core().state().tasks[&unrelated].state,
                nir_core::TaskState::Running
            );
            assert!(p.error.is_none());
        }
    }
}

#[test]
fn seen_skip_is_cleared_in_the_turn_that_publishes_a_choice() {
    for held in [false, true] {
        let mut p = voice_bound_player_with(
            Some((Some("spoken"), VoiceWaitPolicy::Parallel)),
            |program| {
                let Terminator::Await { next, .. } = &mut program
                    .functions
                    .get_mut("main")
                    .unwrap()
                    .blocks
                    .get_mut("wait_intro")
                    .unwrap()
                    .terminator
                else {
                    panic!("fixture must wait for dialogue");
                };
                *next = "choose".into();
            },
        );
        let d = p.core().dialogue().unwrap().1;
        let key = format!("read:{}:{}", d.text_id, d.meaning_revision);
        p.pump(vec![AppEvent::Profile([key].into_iter().collect())], 1000);
        action(
            &mut p,
            if held {
                UiAction::HoldSkip { pressed: true }
            } else {
                UiAction::ToggleSkip
            },
        );
        p.pump(vec![AppEvent::Tick { delta_us: 1000 }], 1000);
        assert!(p.core().state().choice.is_some());
        assert!(!p.model().skip, "publishing a choice must also stop skip");

        let before = serde_json::to_value(p.core().snapshot()).unwrap();
        action(&mut p, UiAction::ToggleSkip);
        assert!(!p.model().skip, "skip cannot be enabled inside a choice");
        action(&mut p, UiAction::HoldSkip { pressed: true });
        assert!(!p.model().skip);
        assert_eq!(serde_json::to_value(p.core().snapshot()).unwrap(), before);
        assert!(p.error.is_none());
    }
}

#[test]
fn legacy_voice_continuation_defaults_true_and_rejects_nonboolean_preferences() {
    let mut json = serde_json::to_value(Preferences::default()).unwrap();
    json.as_object_mut().unwrap().remove("voice_continue");
    assert!(
        serde_json::from_value::<Preferences>(json.clone())
            .unwrap()
            .voice_continue
    );
    json["voice_continue"] = false.into();
    assert!(
        !serde_json::from_value::<Preferences>(json.clone())
            .unwrap()
            .voice_continue
    );
    json["voice_continue"] = "false".into();
    assert!(serde_json::from_value::<Preferences>(json).is_err());
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
        let mut found_voice_wait = false;
        let mut found_voice_continue = false;
        for _ in 0..30 {
            let packet = state.project(&m, (1, 1), width, height, &messages, &mut text);
            assert!(packet.semantics.iter().any(|n| n.action == UiAction::Close));
            for n in &packet.semantics {
                assert!(n.rect[1] >= 0. && n.rect[1] + n.rect[3] <= height);
                found_speed |= matches!(n.action, UiAction::TextSpeed { .. });
                found_wait |= matches!(n.action, UiAction::AutoWait { .. });
                found_voice_wait |= matches!(n.action, UiAction::AutoWaitVoice { .. });
                if matches!(n.action, UiAction::VoiceContinue { .. }) && n.rect[3] >= 44. {
                    found_voice_continue = true;
                }
            }
            if !state.scroll(ScrollRegion::Settings, 1, &packet) {
                break;
            }
        }
        assert!(
            found_speed && found_wait && found_voice_wait && found_voice_continue,
            "{width}x{height}"
        );
    }
}

#[test]
fn small_builtin_panels_keep_every_action_reachable_without_wrong_touch_targets() {
    use nir_presentation::{pointer_action, Messages, ReadingState, Screen};
    let p = playing();
    for (width, height) in [
        (240., 320.),
        (360., 640.),
        (640., 360.),
        (844., 260.),
        (640., 240.),
        (1280., 720.),
    ] {
        for (screen, region) in [
            (Screen::Menu, ScrollRegion::Menu),
            (Screen::Saves, ScrollRegion::Saves),
        ] {
            let mut m = p.model();
            m.screen = screen;
            m.loading = false;
            m.status = "Saved".into();
            for slot in &mut m.slots {
                slot.exists = true;
            }
            let expected = if screen == Screen::Menu {
                vec![
                    UiAction::Close,
                    UiAction::Saves,
                    UiAction::Settings,
                    UiAction::History,
                    UiAction::Rollback,
                    UiAction::Title,
                ]
            } else {
                vec![
                    UiAction::Save { slot: 0 },
                    UiAction::Load { slot: 0 },
                    UiAction::Save { slot: 1 },
                    UiAction::Load { slot: 1 },
                    UiAction::Save { slot: 2 },
                    UiAction::Load { slot: 2 },
                    UiAction::Export,
                    UiAction::Import,
                    UiAction::Close,
                ]
            };
            let mut state = ReadingState::default();
            let mut text = reading_text();
            let mut found = vec![];
            for _ in 0..100 {
                let packet =
                    state.project(&m, (1, 1), width, height, &Messages::default(), &mut text);
                let status = packet.texts.iter().find(|r| r.text == m.status).unwrap();
                assert!(status.y >= 0. && status.y + status.height <= height);
                assert!(packet
                    .semantics
                    .iter()
                    .any(|n| n.enabled && n.action == UiAction::Close));
                for n in packet.semantics.iter().filter(|n| n.rect[3] > 0.) {
                    let [x, y, w, h] = n.rect;
                    assert!(x >= 0. && y >= 0. && x + w <= width && y + h <= height);
                    let tapped = pointer_action(&packet, &m, x + w / 2., y + h / 2., 0);
                    if n.enabled {
                        assert!(
                            status.x + status.width <= x
                                || status.x >= x + w
                                || status.y + status.height <= y
                                || status.y >= y + h,
                            "status hides a touch target at {width}x{height}: {n:?}"
                        );
                        assert!(w >= 44. && h >= 44., "{screen:?} target too small: {n:?}");
                        assert_eq!(tapped, Some(n.action.clone()), "{width}x{height}: {n:?}");
                        if !found.contains(&n.action) {
                            found.push(n.action.clone());
                        }
                    } else {
                        assert_eq!(tapped, None, "cropped target must not activate: {n:?}");
                    }
                }
                let Some(view) = packet.scrolls.iter().find(|v| v.region == region) else {
                    break;
                };
                if view.offset >= view.max {
                    break;
                }
                assert!(state.scroll(region, 1, &packet));
            }
            assert!(
                expected.iter().all(|a| found.contains(a)),
                "{width}x{height} {screen:?}: {found:?}"
            );
        }
    }
}

#[test]
fn builtin_title_and_settings_keep_complete_touch_targets_and_visible_labels() {
    use nir_presentation::{
        pointer_action, CharacterVoiceView, Messages, ReadingState, Screen, TextEngine,
    };
    let p = playing();
    let messages = Messages::default();
    for (width, height) in [
        (240., 240.),
        (240., 320.),
        (360., 640.),
        (640., 240.),
        (640., 360.),
        (844., 260.),
        (1280., 720.),
    ] {
        for locale in ["en", "zh-Hans"] {
            for screen in [Screen::Title, Screen::Settings] {
                for failure in [false, true] {
                    if screen == Screen::Title && failure {
                        continue;
                    }
                    let mut m = p.model();
                    m.screen = screen;
                    m.loading = false;
                    m.ui_locale = locale.into();
                    m.locale_error = failure.then(|| "E_LANGUAGE: preparation failed".into());
                    m.character_voices = vec![CharacterVoiceView {
                        id: "speaker.aki".into(),
                        name: "Aki".into(),
                        locale: locale.into(),
                        fonts: m.ui_fonts.clone(),
                    }];
                    let expected =
                        if screen == Screen::Title {
                            vec![UiAction::NewGame, UiAction::Saves, UiAction::Settings]
                        } else {
                            let mut a = vec![
                                UiAction::Close,
                                UiAction::FontSize { delta: -0.1 },
                                UiAction::FontSize { delta: 0.1 },
                                UiAction::Volume {
                                    bus: AudioBus::Bgm,
                                    delta: -0.1,
                                },
                                UiAction::Volume {
                                    bus: AudioBus::Bgm,
                                    delta: 0.1,
                                },
                                UiAction::Volume {
                                    bus: AudioBus::Voice,
                                    delta: -0.1,
                                },
                                UiAction::Volume {
                                    bus: AudioBus::Voice,
                                    delta: 0.1,
                                },
                                UiAction::Volume {
                                    bus: AudioBus::Sfx,
                                    delta: -0.1,
                                },
                                UiAction::Volume {
                                    bus: AudioBus::Sfx,
                                    delta: 0.1,
                                },
                                UiAction::ReducedMotion,
                                UiAction::TextSpeed { delta: -0.25 },
                                UiAction::TextSpeed { delta: 0.25 },
                                UiAction::AutoWait { delta: -0.25 },
                                UiAction::AutoWait { delta: 0.25 },
                                UiAction::AutoWaitVoice {
                                    enabled: !m.prefs.auto_wait_voice,
                                },
                                UiAction::VoiceContinue {
                                    enabled: !m.prefs.voice_continue,
                                },
                                UiAction::CharacterVolume {
                                    character: "speaker.aki".into(),
                                    delta: -0.1,
                                },
                                UiAction::CharacterVolume {
                                    character: "speaker.aki".into(),
                                    delta: 0.1,
                                },
                                UiAction::CharacterMute {
                                    character: "speaker.aki".into(),
                                    muted: true,
                                },
                            ];
                            a.extend(m.available_ui_locales.iter().map(|locale| {
                                UiAction::UiLocale {
                                    locale: locale.clone(),
                                }
                            }));
                            a.extend(m.available_text_locales.iter().map(|locale| {
                                UiAction::TextLocale {
                                    locale: locale.clone(),
                                }
                            }));
                            if failure {
                                a.extend([UiAction::LocaleRetry, UiAction::LocaleCancel]);
                            }
                            a
                        };
                    let mut state = ReadingState::default();
                    let mut text = reading_text();
                    let mut found = vec![];
                    for _ in 0..600 {
                        let packet = state.project(&m, (1, 1), width, height, &messages, &mut text);
                        text.layout(&packet);
                        assert!(text.missing_font.is_none());
                        let enabled: Vec<_> =
                            packet.semantics.iter().filter(|n| n.enabled).collect();
                        for (i, n) in enabled.iter().enumerate() {
                            let [x, y, w, h] = n.rect;
                            assert!(
                                w >= 44.
                                    && h >= 44.
                                    && x >= 0.
                                    && y >= 0.
                                    && x + w <= width
                                    && y + h <= height,
                                "{screen:?} {locale} {width}x{height}: invalid target {n:?}"
                            );
                            assert_eq!(
                                pointer_action(&packet, &m, x + w / 2., y + h / 2., 0),
                                Some(n.action.clone())
                            );
                            for other in enabled.iter().skip(i + 1) {
                                let [a, b, c, d] = other.rect;
                                assert!(
                                    x + w <= a || a + c <= x || y + h <= b || b + d <= y,
                                    "touch targets overlap: {n:?} / {other:?}"
                                );
                            }
                            let run = packet
                                .texts
                                .iter()
                                .find(|r| r.x >= x && r.x < x + w && r.y >= y && r.y < y + h)
                                .unwrap();
                            let bottom = text.buffers[&TextEngine::key(run)]
                                .layout_runs()
                                .map(|l| l.line_top + l.line_height)
                                .fold(0., f32::max);
                            assert!(
                                bottom <= run.height,
                                "{screen:?} {locale} {width}x{height}: label clipped {run:?}"
                            );
                            if !found.contains(&n.action) {
                                found.push(n.action.clone());
                            }
                        }
                        for n in packet
                            .semantics
                            .iter()
                            .filter(|n| !n.enabled && n.rect[3] > 0.)
                        {
                            let [x, y, w, h] = n.rect;
                            assert_eq!(
                                pointer_action(&packet, &m, x + w / 2., y + h / 2., 0),
                                None,
                                "cropped target activated {n:?}"
                            );
                        }
                        let Some(view) = packet
                            .scrolls
                            .iter()
                            .find(|v| v.region == ScrollRegion::Settings)
                        else {
                            break;
                        };
                        if view.offset >= view.max {
                            break;
                        }
                        assert!(state.scroll(ScrollRegion::Settings, 1, &packet));
                    }
                    assert!(expected.iter().all(|a| found.contains(a)), "{screen:?} {locale} {width}x{height} missing actions: {expected:?}; found: {found:?}");
                }
            }
        }
    }
}

#[test]
fn builtin_settings_clipped_buttons_never_activate() {
    use nir_presentation::{pointer_action, Messages, ReadingState, Screen};
    let p = playing();
    let mut m = p.model();
    m.screen = Screen::Settings;
    m.loading = false;
    let messages = Messages::default();
    let mut state = ReadingState::default();
    let mut text = reading_text();
    let mut observed = false;
    for _ in 0..600 {
        let packet = state.project(&m, (1, 1), 240., 320., &messages, &mut text);
        // Baseline whole language buttons are 34px high; below 30px proves
        // this fixture exposes a cut target, rather than just a small one.
        for n in packet
            .semantics
            .iter()
            .filter(|n| n.rect[3] > 0. && n.rect[3] < 30.)
        {
            observed = true;
            assert!(
                !n.enabled,
                "partially cropped settings button remains actionable: {n:?}"
            );
            let [x, y, w, h] = n.rect;
            assert_eq!(pointer_action(&packet, &m, x + w / 2., y + h / 2., 0), None);
        }
        let Some(view) = packet
            .scrolls
            .iter()
            .find(|v| v.region == ScrollRegion::Settings)
        else {
            break;
        };
        if view.offset >= view.max {
            break;
        }
        assert!(state.scroll(ScrollRegion::Settings, 1, &packet));
    }
    assert!(
        observed,
        "fixture must actually expose a partially clipped target"
    );
}

#[test]
fn builtin_loading_footer_keeps_the_close_control_visible() {
    use nir_presentation::{Messages, ReadingState, Screen};
    let p = playing();
    for screen in [Screen::Menu, Screen::Saves, Screen::Settings] {
        let mut m = p.model();
        m.screen = screen;
        m.loading = true;
        let messages = Messages::default();
        let packet =
            ReadingState::default().project(&m, (1, 1), 240., 320., &messages, &mut reading_text());
        let close = packet
            .semantics
            .iter()
            .find(|n| n.action == UiAction::Close)
            .unwrap();
        let loading = packet
            .texts
            .iter()
            .find(|r| r.text == messages.text(&m.ui_locale, "loading"))
            .unwrap();
        assert!(loading.y >= close.rect[1] + close.rect[3]);
        for q in packet
            .quads
            .iter()
            .filter(|q| q.rect[0] == 0. && q.rect[2] == 240. && q.rect[3] < 40.)
        {
            assert!(
                q.rect[1] >= close.rect[1] + close.rect[3],
                "loading bar hides Close"
            );
        }
    }
}

#[test]
fn builtin_panel_loading_feedback_does_not_overlap_saved_feedback() {
    use nir_presentation::{Messages, ReadingState, Screen};
    let p = playing();
    for screen in [Screen::Menu, Screen::Saves] {
        let mut m = p.model();
        m.screen = screen;
        m.loading = true;
        m.status = "Saved".into();
        let messages = Messages::default();
        let packet =
            ReadingState::default().project(&m, (1, 1), 240., 320., &messages, &mut reading_text());
        assert!(packet
            .texts
            .iter()
            .any(|r| r.text == messages.text(&m.ui_locale, "loading")));
        assert!(
            !packet.texts.iter().any(|r| r.text == m.status),
            "stale saved feedback overlaps loading: {screen:?}"
        );
    }
}

#[test]
fn builtin_panel_scroll_labels_fit_and_keep_full_accessible_names() {
    use nir_presentation::{Messages, ReadingState, Screen, TextEngine};
    let p = playing();
    for locale in ["en", "zh-Hans"] {
        for screen in [Screen::Menu, Screen::Saves] {
            let mut m = p.model();
            m.screen = screen;
            m.loading = false;
            m.ui_locale = locale.into();
            let messages = Messages::default();
            let mut text = reading_text();
            let packet =
                ReadingState::default().project(&m, (1, 1), 240., 320., &messages, &mut text);
            let controls = packet
                .semantics
                .iter()
                .filter(|n| matches!(n.action, UiAction::Scroll { .. }))
                .collect::<Vec<_>>();
            assert_eq!(controls.len(), 2);
            text.layout(&packet);
            for n in controls {
                let UiAction::Scroll { delta, .. } = n.action else {
                    unreachable!()
                };
                let label = if delta < 0 {
                    "scroll-back"
                } else {
                    "scroll-forward"
                };
                assert_eq!(n.label, messages.text(locale, label));
                let run = packet
                    .texts
                    .iter()
                    .find(|r| {
                        r.x >= n.rect[0]
                            && r.x < n.rect[0] + n.rect[2]
                            && r.y >= n.rect[1]
                            && r.y < n.rect[1] + n.rect[3]
                    })
                    .unwrap();
                let bottom = text.buffers[&TextEngine::key(run)]
                    .layout_runs()
                    .map(|l| l.line_top + l.line_height)
                    .fold(0., f32::max);
                assert!(
                    bottom <= run.height,
                    "{screen:?} {locale} scroll label is clipped: {run:?}"
                );
            }
        }
    }
}

#[test]
fn builtin_panel_scrolling_never_changes_story_or_reading_modes() {
    let mut p = playing();
    action(&mut p, UiAction::Menu);
    p.auto = true;
    p.pump(vec![], 1000);
    let before = serde_json::to_value(p.core().snapshot()).unwrap();
    let interaction = p.current_interaction();
    for region in [
        ScrollRegion::Menu,
        ScrollRegion::Saves,
        ScrollRegion::Settings,
    ] {
        let commands = action(&mut p, UiAction::Scroll { region, delta: 1 });
        assert!(commands.iter().all(|c| matches!(c, AppCommand::Observation { stage, .. } if stage == "input_dispatched")), "unexpected scroll side effect: {commands:?}");
        assert_eq!(serde_json::to_value(p.core().snapshot()).unwrap(), before);
        assert_eq!(p.current_interaction(), interaction);
        assert!(p.model().auto);
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
fn prepared_audio_replacement_keeps_old_music_through_delay_failure_and_retry() {
    for asset in ["audio.bgm", "audio.bell"] {
        let mut program: Program =
            serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        program.requires.push("audio.stop.v1".into());
        if let Effect::Dialogue { reveal_us, .. } =
            &mut program.cues.get_mut("intro").unwrap().effects[0].effect
        {
            *reveal_us = Micros(0);
        }
        program.cues.insert(
            "replace_music".into(),
            serde_json::from_value(serde_json::json!({"effects":[
                {"id":"music_stop","scope":"session","effect":{"type":"audio_stop","target":"music","duration_us":"0"}},
                {"id":"music","scope":"session","effect":{"type":"audio","asset":asset,"bus":"bgm","looped":true}},
                {"id":"line","scope":"interaction","effect":{"type":"dialogue","text":"arrival","speaker":"","reveal_us":"0"}}
            ]}))
            .unwrap(),
        );
        let main = program.functions.get_mut("main").unwrap();
        if let Terminator::Await { next, .. } =
            &mut main.blocks.get_mut("wait_intro").unwrap().terminator
        {
            *next = "replace_music".into();
        } else {
            panic!("fixture must await its first dialogue");
        }
        main.blocks.insert(
            "replace_music".into(),
            serde_json::from_value(serde_json::json!({"terminator":{
                "type":"activate","cue":"replace_music","next":"replacement_wait"}}))
            .unwrap(),
        );
        main.blocks.insert(
            "replacement_wait".into(),
            serde_json::from_value(serde_json::json!({"terminator":{
                "type":"await","conditions":[{"task":"line","milestone":{"type":"finished"}}],
                "next":"walk_end","on_cancelled":"walk_end","on_failed":"walk_end"}}))
            .unwrap(),
        );
        let mut p = Player::new(program, "release".into(), "Test".into()).unwrap();
        let commands = p.pump(vec![], 1000);
        ready(&mut p, commands);
        let commands = action(&mut p, UiAction::NewGame);
        ready(&mut p, commands);
        let old = p.core().state().handles["music"];
        // No device tick has elapsed in this unit fixture. Even an authored
        // zero reveal interval advances on a tick; the first input therefore
        // completes the visible line without leaving its interaction.
        let interaction = p.current_interaction();
        let reveal = action(&mut p, UiAction::Advance);
        assert_eq!(p.current_interaction(), interaction);
        assert!(!p.is_loading());
        assert!(!reveal.iter().any(|c| matches!(
            c,
            AppCommand::AudioStop {
                domain: TimeDomain::Story,
                ..
            } | AppCommand::AudioStart {
                domain: TimeDomain::Story,
                ..
            }
        )));
        let commands = action(&mut p, UiAction::Advance);
        let request = commands
            .iter()
            .find_map(|c| match c {
                AppCommand::GetAssets { request, .. } => Some(*request),
                _ => None,
            })
            .unwrap();
        assert!(p.is_loading());
        assert_eq!(
            p.core().state().tasks[&old].state,
            nir_core::TaskState::Running
        );
        let tick = p.core().state().tick_us;
        let mut before_commit = commands;
        before_commit.extend(p.pump(
            vec![AppEvent::Tick {
                delta_us: 1_000_000,
            }],
            1000,
        ));
        assert_eq!(p.core().state().tick_us, tick);
        assert!(!p.bus_paused(TimeDomain::Story, AudioBus::Bgm));
        before_commit.extend(p.pump(
            vec![AppEvent::AssetFailed {
                request,
                message: "replacement media unavailable".into(),
            }],
            1000,
        ));
        assert!(p.error.is_some());
        assert_eq!(
            p.core().state().tasks[&old].state,
            nir_core::TaskState::Running
        );
        assert!(!before_commit.iter().any(|c| matches!(
            c,
            AppCommand::AudioStart {
                domain: TimeDomain::Story,
                ..
            } | AppCommand::AudioStop {
                domain: TimeDomain::Story,
                ..
            } | AppCommand::AudioReset {
                domain: TimeDomain::Story
            }
        )));
        // A failed request's late successes cannot stop the retained source.
        ready(&mut p, before_commit);
        assert_eq!(p.core().state().handles["music"], old);
        let retry = action(&mut p, UiAction::Retry);
        let committed = ready(&mut p, retry);
        assert!(p.error.is_none());
        assert!(!p.is_loading());
        let new = p.core().state().handles["music"];
        assert_ne!(new, old);
        assert_eq!(
            p.core().state().tasks[&new].state,
            nir_core::TaskState::Running
        );
        assert_eq!(
            p.core().state().tasks[&old].state,
            nir_core::TaskState::Cancelled
        );
        assert_eq!(
            committed
                .iter()
                .filter(|c| matches!(c,
                    AppCommand::AudioStop { domain: TimeDomain::Story, task, .. } if *task == old
                ))
                .count(),
            1
        );
        assert_eq!(committed.iter().filter(|c| matches!(c,
            AppCommand::AudioStart { domain: TimeDomain::Story, task, asset: started, position_us: Micros(0), .. }
                if *task == new && started == asset
        )).count(), 1);
    }
}

#[test]
fn loop_region_and_cumulative_playhead_cross_the_player_restore_boundary() {
    let mut program: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    program.requires.push("audio.loop-region.v1".into());
    let expected = AudioLoopRegion {
        start_us: Micros(200_000),
        end_us: Micros(600_000),
    };
    let mut changed = false;
    for cue in program.cues.values_mut() {
        for definition in &mut cue.effects {
            if let Effect::Audio {
                asset,
                looped: true,
                loop_region,
                ..
            } = &mut definition.effect
            {
                *loop_region = Some(expected);
                program.assets.get_mut(asset).unwrap().duration_us = Micros(800_000);
                changed = true;
            }
        }
    }
    assert!(changed);
    let mut p = Player::new(program, "release".into(), "Test".into()).unwrap();
    let commands = p.pump(vec![], 1000);
    ready(&mut p, commands);
    let commands = action(&mut p, UiAction::NewGame);
    let commands = ready(&mut p, commands);
    let music = commands
        .iter()
        .find_map(|c| match c {
            AppCommand::AudioStart {
                task,
                loop_region: Some(region),
                position_us: Micros(0),
                ..
            } if *region == expected => Some(*task),
            _ => None,
        })
        .unwrap();
    p.observe_audio_positions(
        TimeDomain::Story,
        p.generation.session,
        &[AudioPosition {
            task: music,
            position_us: Micros(1_400_000),
            envelope: None,
        }],
    )
    .unwrap();
    action(&mut p, UiAction::Menu);
    let commands = action(&mut p, UiAction::Save { slot: 1 });
    let envelope = commands
        .into_iter()
        .find_map(|c| match c {
            AppCommand::Save { envelope, .. } => Some(envelope),
            _ => None,
        })
        .unwrap();
    let commands = p.pump(vec![AppEvent::Loaded { envelope }], 1000);
    let commands = ready(&mut p, commands);
    assert!(commands.iter().any(|c| matches!(c,
        AppCommand::AudioStart { task, looped: true, loop_region: Some(region), position_us: Micros(1_400_000), .. }
        if *task == music && *region == expected)));
    assert!(p.paused());
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
fn stored_slot_inspection_uses_typed_digest_and_rejects_bad_identity_or_checksum() {
    let p = playing();
    let mut envelope = *slot_envelope(&p, 1);
    envelope.snapshot.scene[0].x = -0.0;
    envelope.digest = nir_content::digest(&serde_json::to_vec(&envelope.snapshot).unwrap());
    let inspect = |envelope: &SaveEnvelope| {
        // Value reorders object keys; the typed snapshot encoding stays stable.
        let json = serde_json::to_string(&serde_json::to_value(envelope).unwrap()).unwrap();
        inspect_save_slot(&json, 1, &p.release, &p.core().snapshot().game_id)
    };
    assert_eq!(inspect(&envelope).unwrap(), 1);
    for case in 0..6 {
        let mut bad = envelope.clone();
        match case {
            0 => bad.format = 2,
            1 => bad.slot = 0,
            2 => bad.snapshot.release = "other".into(),
            3 => bad.snapshot.game_id = "other".into(),
            4 => bad.revision = 0,
            5 => bad.snapshot.tick_us.0 += 1,
            _ => unreachable!(),
        }
        let error = inspect(&bad).unwrap_err();
        assert_eq!(
            error.code,
            if case < 4 {
                "E_SAVE_IDENTITY"
            } else if case == 4 {
                "E_SAVE_REVISION"
            } else {
                "E_DIGEST"
            }
        );
    }
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
    let mut p: Program = serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
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
    assert_eq!(
        state.tasks[&state.handles["ring"]].state,
        nir_core::TaskState::Running
    );
    assert_eq!(
        state.tasks[&state.handles["fade"]].state,
        nir_core::TaskState::Running
    );
    // Local state changed and the chain is mid-flight.
    p.pump(
        vec![AppEvent::Tick {
            delta_us: 1_000_000,
        }],
        1000,
    );
    let state = p.core().state();
    assert_eq!(state.variables["affection"], Value::I32(5));
    assert_eq!(
        state.tasks[&state.handles["fade"]].elapsed_us,
        Micros(1_000_000)
    );

    // The save envelope freezes the mid-flight chain and the changed state.
    let commands = action(&mut p, UiAction::Save { slot: 0 });
    let (job, envelope) = commands
        .into_iter()
        .find_map(|c| {
            if let AppCommand::Save { job, envelope, .. } = c {
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
    assert_eq!(
        (saved_fade.elapsed_us, saved_fade.captured),
        (Micros(1_000_000), 1.)
    );
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
    let commands = p.pump(vec![AppEvent::SlotLoaded { job, envelope }], 1000);
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
    assert_eq!(
        state.tasks[&state.handles["ring"]].state,
        nir_core::TaskState::Running
    );
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
    let commands = p.pump(
        vec![AppEvent::Tick {
            delta_us: 9_000_000,
        }],
        1000,
    );
    ready(&mut p, commands);
    let state = p.core().state();
    assert_eq!(
        state.tasks[&state.handles["fade"]].state,
        nir_core::TaskState::Finished
    );
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
    let commands = p.pump(
        vec![AppEvent::Tick {
            delta_us: 9_000_000,
        }],
        1000,
    );
    ready(&mut p, commands);
    // The beat cue's activation stops the clock mid-tick; the next host frame
    // resumes it, exactly like a render loop would.
    let commands = p.pump(
        vec![AppEvent::Tick {
            delta_us: 1_000_000,
        }],
        1000,
    );
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
    p.pump(
        vec![AppEvent::Tick {
            delta_us: 2_000_000,
        }],
        1000,
    );
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
    let commands = p.pump(
        vec![AppEvent::Tick {
            delta_us: 4_000_000,
        }],
        1000,
    );
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
    assert_eq!(
        state.tasks[&state.handles["ring"]].state,
        nir_core::TaskState::Running
    );
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
    let commands = p.pump(
        vec![AppEvent::Tick {
            delta_us: 8_000_000,
        }],
        1000,
    );
    ready(&mut p, commands);
    let commands = p.pump(
        vec![AppEvent::Tick {
            delta_us: 10_000_000,
        }],
        1000,
    );
    ready(&mut p, commands);
    let state = p.core().state();
    assert_eq!(
        state.tasks[&state.handles["fade"]].state,
        nir_core::TaskState::Finished
    );
    assert!(state.tasks[&state.handles["chain"]]
        .milestones
        .contains(&nir_format::Milestone::Finished));
    assert_eq!(state.outcome.as_deref(), Some("done"));
}

#[test]
fn reading_overlays_keep_music_but_freeze_voice_sfx_and_story() {
    for overlay in [
        UiAction::Menu,
        UiAction::Settings,
        UiAction::History,
        UiAction::Saves,
    ] {
        let mut p = playing();
        let tick = p.core().state().tick_us;
        let commands = action(&mut p, overlay);
        assert!(p.paused());
        assert!(!p.bus_paused(TimeDomain::Story, AudioBus::Bgm));
        assert!(p.bus_paused(TimeDomain::Story, AudioBus::Voice));
        assert!(p.bus_paused(TimeDomain::Story, AudioBus::Sfx));
        assert!(!commands.iter().any(|c| matches!(
            c,
            AppCommand::AudioPause {
                domain: TimeDomain::Story,
                paused: true
            } | AppCommand::AudioReset {
                domain: TimeDomain::Story
            } | AppCommand::AudioStop {
                domain: TimeDomain::Story,
                ..
            }
        )));
        p.pump(vec![AppEvent::Tick { delta_us: 500_000 }], 1000);
        assert_eq!(p.core().state().tick_us, tick);
        // Background owns a stronger suspension; closing cannot release it.
        p.pump(vec![AppEvent::Hidden(true)], 1000);
        action(&mut p, UiAction::Close);
        for bus in [AudioBus::Bgm, AudioBus::Voice, AudioBus::Sfx] {
            assert!(p.bus_paused(TimeDomain::Story, bus));
        }
        p.pump(vec![AppEvent::Hidden(false)], 1000);
        for bus in [AudioBus::Bgm, AudioBus::Voice, AudioBus::Sfx] {
            assert!(!p.bus_paused(TimeDomain::Story, bus));
        }
    }
}

#[test]
fn paused_audio_failure_does_not_execute_story_and_first_terminal_event_wins() {
    for failure_first in [false, true] {
        let mut program: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        for definition in program["cues"]["opening"]["effects"]
            .as_array_mut()
            .unwrap()
        {
            if definition["id"] == "music" {
                definition["effect"]["looped"] = false.into();
            }
        }
        program["functions"]["main"]["blocks"]["wait_intro"]["terminator"]["conditions"] =
            serde_json::json!([{"task":"music","milestone":{"type":"finished"}}]);
        program["functions"]["main"]["blocks"]["failed"]["terminator"] =
            serde_json::json!({"type":"end","outcome":"failed"});
        let mut p = Player::new(
            serde_json::from_value(program).unwrap(),
            "release".into(),
            "Test".into(),
        )
        .unwrap();
        let commands = p.pump(vec![], 1000);
        ready(&mut p, commands);
        let commands = action(&mut p, UiAction::NewGame);
        ready(&mut p, commands);
        let (task, session) = (p.core().state().handles["music"], p.generation.session);
        action(&mut p, UiAction::Menu);
        let before = serde_json::to_value(p.core().snapshot()).unwrap();
        let failed = || AppEvent::AudioFailed {
            domain: TimeDomain::Story,
            task,
            session,
            message: "device failure".repeat(1000),
        };
        let ended = || AppEvent::AudioEnded {
            domain: TimeDomain::Story,
            task,
            session,
        };
        p.pump(vec![if failure_first { failed() } else { ended() }], 1000);
        for _ in 0..3 {
            p.pump(vec![failed(), ended()], 1000);
        }
        assert_eq!(serde_json::to_value(p.core().snapshot()).unwrap(), before);
        assert_eq!(
            p.pending_events(),
            0,
            "held completion must not spin a paused host"
        );
        // Resume and a conflicting callback can be admitted in the same pump.
        // It still cannot supersede the first accepted device event.
        p.pump(
            vec![
                AppEvent::Action {
                    action: UiAction::Close,
                    interaction: p.current_interaction(),
                    sequence: p.core().state().last_input + 1,
                    session,
                },
                if failure_first { ended() } else { failed() },
            ],
            1000,
        );
        assert_eq!(
            p.core().state().tasks[&task].state,
            if failure_first {
                nir_core::TaskState::Failed
            } else {
                nir_core::TaskState::Finished
            }
        );
        if failure_first {
            assert_eq!(p.core().state().outcome.as_deref(), Some("failed"));
            assert!(p.diagnostic.as_ref().unwrap().message.len() <= 4096);
        } else {
            assert_eq!(
                p.core().state().tasks[&task].end_reason,
                Some(nir_core::TaskEndReason::NaturalEnd)
            );
            assert!(p.diagnostic.is_none());
        }
    }
}

#[test]
fn deferred_loop_music_failure_is_cleared_when_the_session_is_replaced() {
    let mut p = playing();
    let (task, session) = (p.core().state().handles["music"], p.generation.session);
    action(&mut p, UiAction::Menu);
    let before = serde_json::to_value(p.core().snapshot()).unwrap();
    let failed = || AppEvent::AudioFailed {
        domain: TimeDomain::Story,
        task,
        session,
        message: "unavailable".into(),
    };
    p.pump(vec![failed()], 1000);
    assert_eq!(serde_json::to_value(p.core().snapshot()).unwrap(), before);
    let commands = action(&mut p, UiAction::Title);
    ready(&mut p, commands);
    let commands = action(&mut p, UiAction::NewGame);
    ready(&mut p, commands);
    p.pump(vec![failed()], 1000);
    assert_eq!(
        p.core().state().tasks[&p.core().state().handles["music"]].state,
        nir_core::TaskState::Running
    );
    assert_eq!(p.pending_events(), 0);
}

#[test]
fn music_natural_end_during_menu_waits_to_execute_until_close() {
    let mut program: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    for definition in program["cues"]["opening"]["effects"]
        .as_array_mut()
        .unwrap()
    {
        if definition["id"] == "music" {
            definition["effect"]["looped"] = false.into();
        }
    }
    program["functions"]["main"]["blocks"]["wait_intro"]["terminator"]["conditions"] =
        serde_json::json!([{"task":"music","milestone":{"type":"finished"}}]);
    let mut p = Player::new(
        serde_json::from_value(program).unwrap(),
        "release".into(),
        "Test".into(),
    )
    .unwrap();
    let commands = p.pump(vec![], 1000);
    ready(&mut p, commands);
    let commands = action(&mut p, UiAction::NewGame);
    ready(&mut p, commands);
    let task = p.core().state().handles["music"];
    let session = p.generation.session;
    action(&mut p, UiAction::Menu);
    let snapshot = p.core().snapshot();
    for _ in 0..3 {
        p.pump(
            vec![AppEvent::AudioEnded {
                domain: TimeDomain::Story,
                task,
                session,
            }],
            1000,
        );
    }
    assert_eq!(
        serde_json::to_value(p.core().snapshot()).unwrap(),
        serde_json::to_value(snapshot).unwrap()
    );
    // Deferred callbacks do not spin the paused host.
    assert_eq!(p.pending_events(), 0);
    let commands = action(&mut p, UiAction::Close);
    assert_eq!(
        p.core().state().tasks[&task].state,
        nir_core::TaskState::Finished
    );
    assert!(commands
        .iter()
        .any(|c| matches!(c, AppCommand::GetAssets { .. })));
    assert!(p.error.is_none(), "{:?}", p.error);
}

#[test]
fn paused_old_audio_completion_does_not_survive_a_new_session() {
    let mut program: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    for definition in program["cues"]["opening"]["effects"]
        .as_array_mut()
        .unwrap()
    {
        if definition["id"] == "music" {
            definition["effect"]["looped"] = false.into();
        }
    }
    let mut p = Player::new(
        serde_json::from_value(program).unwrap(),
        "release".into(),
        "Test".into(),
    )
    .unwrap();
    let commands = p.pump(vec![], 1000);
    ready(&mut p, commands);
    let commands = action(&mut p, UiAction::NewGame);
    ready(&mut p, commands);
    let session = p.generation.session;
    action(&mut p, UiAction::Menu);
    let task = p.core().state().handles["music"];
    p.pump(
        vec![AppEvent::AudioEnded {
            domain: TimeDomain::Story,
            task,
            session,
        }],
        1000,
    );
    let commands = action(&mut p, UiAction::Title);
    ready(&mut p, commands);
    let commands = action(&mut p, UiAction::NewGame);
    ready(&mut p, commands);
    p.pump(
        vec![AppEvent::AudioEnded {
            domain: TimeDomain::Story,
            task,
            session,
        }],
        1000,
    );
    assert_eq!(
        p.core().state().tasks[&p.core().state().handles["music"]].state,
        nir_core::TaskState::Running
    );
    assert!(p.error.is_none());
}

fn character_player(wait: VoiceWaitPolicy) -> Player {
    voice_bound_player_with(Some((Some("spoken"), wait)), |program| {
        let Effect::Dialogue { speaker, .. } =
            &mut program.cues.get_mut("intro").unwrap().effects[0].effect
        else {
            unreachable!()
        };
        *speaker = "speaker.aki".into();
    })
}
#[test]
fn character_preferences_keep_identity_gain_and_history_independent_of_display_names() {
    let mut p = character_player(VoiceWaitPolicy::Parallel);
    let spoken = p.core().state().handles["spoken"];
    let unrelated = p.core().state().handles["unrelated"];
    assert_eq!(
        p.core().state().tasks[&spoken].voice_character,
        "speaker.aki"
    );
    assert!(p.core().state().tasks[&unrelated]
        .voice_character
        .is_empty());
    let frozen = p.core().snapshot();
    let commands = action(
        &mut p,
        UiAction::CharacterVolume {
            character: "speaker.aki".into(),
            delta: -0.6,
        },
    );
    assert!(commands
        .iter()
        .any(|c| matches!(c, AppCommand::PersistPreferences { .. })));
    assert!((p.preferences.character_voice_gain("speaker.aki") - 0.4).abs() < 1e-6);
    action(
        &mut p,
        UiAction::CharacterMute {
            character: "speaker.aki".into(),
            muted: true,
        },
    );
    assert_eq!(p.preferences.character_voice_gain("speaker.aki"), 0.);
    assert_eq!(
        serde_json::to_value(p.core().snapshot()).unwrap(),
        serde_json::to_value(frozen).unwrap()
    );
    action(
        &mut p,
        UiAction::CharacterMute {
            character: "speaker.aki".into(),
            muted: false,
        },
    );
    assert!((p.preferences.character_voice_gain("speaker.aki") - 0.4).abs() < 1e-6);
    action(&mut p, UiAction::History);
    let entry = p.core().state().history.len() - 1;
    let c = action(&mut p, UiAction::HistoryVoice { entry });
    let commands = ready(&mut p, c);
    assert!(commands.iter().any(|c| matches!(c,AppCommand::AudioStart { domain:TimeDomain::ForegroundUi,character,.. } if character=="speaker.aki")));
    assert_eq!(p.core().state().history[entry].speaker_id, "speaker.aki");
    assert!(p.error.is_none());
}
#[test]
fn unknown_and_nonfinite_character_edits_do_not_mutate_preferences() {
    let mut p = character_player(VoiceWaitPolicy::Parallel);
    let before = serde_json::to_value(&p.preferences).unwrap();
    for edit in [
        UiAction::CharacterMute {
            character: "not-a-role".into(),
            muted: true,
        },
        UiAction::CharacterVolume {
            character: "speaker.aki".into(),
            delta: f32::NAN,
        },
        UiAction::CharacterVolume {
            character: "speaker.aki".into(),
            delta: 2.,
        },
    ] {
        let commands = action(&mut p, edit);
        assert!(!commands
            .iter()
            .any(|c| matches!(c, AppCommand::PersistPreferences { .. })));
        assert_eq!(serde_json::to_value(&p.preferences).unwrap(), before);
    }
}
#[test]
fn character_restore_keeps_keys_and_migrates_unambiguous_old_live_bindings() {
    let p = character_player(VoiceWaitPolicy::Parallel);
    let snapshot = p.core().snapshot();
    let spoken = snapshot.handles["spoken"];
    let restored = nir_core::Core::restore(
        p.core().validated_program().clone(),
        snapshot.clone(),
        "release",
    )
    .unwrap();
    assert_eq!(
        restored.state().tasks[&spoken].voice_character,
        "speaker.aki"
    );
    let mut json = serde_json::to_value(&snapshot).unwrap();
    for task in json["tasks"].as_object_mut().unwrap().values_mut() {
        task.as_object_mut().unwrap().remove("voice_character");
        if let Some(dialogue) = task["dialogue"].as_object_mut() {
            dialogue.remove("speaker_id");
        }
    }
    for entry in json["history"].as_array_mut().unwrap() {
        entry.as_object_mut().unwrap().remove("speaker_id");
    }
    let restored = nir_core::Core::restore(
        p.core().validated_program().clone(),
        serde_json::from_value(json).unwrap(),
        "release",
    )
    .unwrap();
    assert_eq!(
        restored.state().tasks[&spoken].voice_character,
        "speaker.aki"
    );
    assert_eq!(restored.dialogue().unwrap().1.speaker_id, "speaker.aki");
    let mut corrupt = snapshot.clone();
    corrupt.tasks.get_mut(&spoken).unwrap().voice_character = "unknown".into();
    assert!(
        nir_core::Core::restore(p.core().validated_program().clone(), corrupt, "release").is_err()
    );
    let mut corrupt = snapshot;
    let line = corrupt.handles["line"];
    corrupt
        .tasks
        .get_mut(&line)
        .unwrap()
        .dialogue
        .as_mut()
        .unwrap()
        .speaker_id = "intro".into();
    assert!(
        nir_core::Core::restore(p.core().validated_program().clone(), corrupt, "release").is_err()
    );
}

#[test]
fn sampled_character_mute_is_frozen_for_the_current_auto_cycle() {
    for muted in [false, true] {
        let mut p = character_player(VoiceWaitPolicy::SampledRemaining);
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
        action(
            &mut p,
            UiAction::CharacterMute {
                character: "speaker.aki".into(),
                muted,
            },
        );
        let interaction = p.current_interaction();
        action(&mut p, UiAction::ToggleAuto);
        // Changing audibility after the sample cannot rewrite its frozen delay.
        action(
            &mut p,
            UiAction::CharacterMute {
                character: "speaker.aki".into(),
                muted: !muted,
            },
        );
        let delay = if muted { 100_000 } else { 850_000 };
        for delta in [delay / 2, delay - delay / 2 - 1] {
            p.pump(vec![AppEvent::Tick { delta_us: delta }], 1000);
        }
        assert_eq!(p.current_interaction(), interaction, "muted={muted}");
        let commands = p.pump(vec![AppEvent::Tick { delta_us: 1 }], 1000);
        ready(&mut p, commands);
        assert_ne!(p.current_interaction(), interaction, "muted={muted}");
        assert_eq!(
            p.core().state().tasks[&task].state,
            nir_core::TaskState::Running
        );
    }
}

#[test]
fn asynchronously_loaded_character_preferences_use_the_same_normalization_as_boot() {
    let mut p = character_player(VoiceWaitPolicy::Parallel);
    let mut saved = p.preferences.clone();
    saved.character_voices.insert(
        "speaker.aki".into(),
        CharacterVoicePreference {
            volume: 2.,
            muted: true,
        },
    );
    saved.character_voices.insert(
        "intro".into(),
        CharacterVoicePreference {
            volume: f32::NAN,
            muted: false,
        },
    );
    saved.character_voices.insert(
        "unknown".into(),
        CharacterVoicePreference {
            volume: 0.4,
            muted: false,
        },
    );
    let commands = p.pump(vec![AppEvent::Preferences(saved)], 1000);
    assert_eq!(p.preferences.character_voices.len(), 1);
    assert_eq!(
        p.preferences.character_voices["speaker.aki"],
        CharacterVoicePreference {
            volume: 1.,
            muted: true
        }
    );
    assert!(commands.iter().any(|c| matches!(c, AppCommand::ApplyPreferences { preferences } if preferences.character_voices.len() == 1)));
}

#[test]
fn repeated_retry_preserves_one_inflight_preparation_and_can_retry_a_second_failure() {
    let mut p = player();
    let commands = p.pump(vec![], 1000);
    let request = commands
        .iter()
        .find_map(|c| match c {
            AppCommand::GetAssets { request, .. } => Some(*request),
            _ => None,
        })
        .unwrap();
    p.pump(
        vec![AppEvent::AssetFailed {
            request,
            message: "offline".into(),
        }],
        1000,
    );
    let failed_packet = nir_presentation::project(
        &p.model(),
        390.,
        844.,
        &nir_presentation::Messages::default(),
    );
    assert_eq!(
        failed_packet
            .texts
            .iter()
            .filter(|t| Some(&t.text) == p.error.as_ref())
            .count(),
        1
    );
    let commands = action(&mut p, UiAction::Retry);
    let next = commands
        .iter()
        .find_map(|c| match c {
            AppCommand::GetAssets { request, .. } => Some(*request),
            _ => None,
        })
        .unwrap();
    assert!(p.status.is_empty());
    let mut all = commands;
    for _ in 0..20 {
        let commands = action(&mut p, UiAction::Retry);
        assert!(!commands.iter().any(|c| matches!(
            c,
            AppCommand::GetAssets { .. } | AppCommand::CancelAssets { .. }
        )));
        all.extend(commands);
    }
    assert!(p.accepts(next));
    let packet = nir_presentation::project(
        &p.model(),
        320.,
        720.,
        &nir_presentation::Messages::default(),
    );
    let retry = packet
        .semantics
        .iter()
        .find(|n| matches!(n.action, UiAction::Retry))
        .unwrap();
    assert!(!retry.enabled);
    assert!(retry.rect[2] >= 44. && retry.rect[3] >= 44.);
    p.pump(
        vec![AppEvent::AssetFailed {
            request: next,
            message: "still offline".into(),
        }],
        1000,
    );
    let packet = nir_presentation::project(
        &p.model(),
        320.,
        720.,
        &nir_presentation::Messages::default(),
    );
    assert!(
        packet
            .semantics
            .iter()
            .find(|n| matches!(n.action, UiAction::Retry))
            .unwrap()
            .enabled
    );
    let commands = action(&mut p, UiAction::Retry);
    ready(&mut p, commands);
    assert!(!p.is_loading());
    assert!(p.error.is_none());
    assert_eq!(p.core().state().tick_us.0, 0);
}

#[test]
fn narrow_story_status_stays_below_the_wrapped_toolbar() {
    let mut p = playing();
    p.pump(
        vec![AppEvent::LoadFailed("storage unavailable".into())],
        1000,
    );
    assert!(p.error.is_none());
    let packet = nir_presentation::project(
        &p.model(),
        390.,
        844.,
        &nir_presentation::Messages::default(),
    );
    let toolbar_bottom = packet
        .semantics
        .iter()
        .filter(|n| {
            matches!(
                n.action,
                UiAction::Menu
                    | UiAction::History
                    | UiAction::ToggleAuto
                    | UiAction::ToggleSkip
                    | UiAction::ToggleInterface
            )
        })
        .map(|n| n.rect[1] + n.rect[3])
        .fold(0., f32::max);
    let status = packet.texts.iter().find(|t| t.text == p.status).unwrap();
    assert!(status.y >= toolbar_bottom + 8.);
}

#[test]
fn unreadable_slot_listing_keeps_story_and_healthy_slots_without_overwrite() {
    use nir_presentation::{Messages, SlotView};
    use std::collections::BTreeMap;
    let mut p = playing();
    action(&mut p, UiAction::Saves);
    let before = serde_json::to_value(p.core().snapshot()).unwrap();
    let epoch = p.generation.session;
    p.status = "已保存".into();
    let commands = p.pump(
        vec![AppEvent::Slots(
            vec![
                SlotView {
                    slot: 0,
                    error: Some("E_SAVE_PARSE: malformed".into()),
                    ..Default::default()
                },
                SlotView {
                    slot: 1,
                    exists: true,
                    label: "#3".into(),
                    ..Default::default()
                },
                SlotView {
                    slot: 2,
                    ..Default::default()
                },
            ],
            BTreeMap::from([(1, 3)]),
        )],
        1000,
    );
    assert_eq!(serde_json::to_value(p.core().snapshot()).unwrap(), before);
    assert_eq!(p.generation.session, epoch);
    assert!(p.error.is_none());
    assert_eq!(
        p.status, "已保存",
        "A list warning cannot hide a successful save"
    );
    assert!(commands
        .iter()
        .any(|c| matches!(c, AppCommand::Diagnostic { diagnostic }
        if diagnostic.details.as_ref().unwrap().operation == "list")));
    let model = p.model();
    assert!(!model.slots[0].exists && model.slots[0].error.is_some());
    assert!(model.slots[1].exists && model.slots[1].error.is_none());
    assert!(!model.slots[2].exists && model.slots[2].error.is_none());
    let packet = nir_presentation::project(&model, 1280., 720., &Messages::default());
    let enabled = |action: &UiAction| {
        packet
            .semantics
            .iter()
            .find(|s| &s.action == action)
            .unwrap()
            .enabled
    };
    assert!(!enabled(&UiAction::Save { slot: 0 }));
    assert!(
        enabled(&UiAction::Load { slot: 0 }),
        "Unreadable slot offers a read retry"
    );
    assert!(enabled(&UiAction::Save { slot: 1 }));
    assert!(enabled(&UiAction::Load { slot: 1 }));
    assert!(!enabled(&UiAction::Load { slot: 2 }));
    assert!(packet.texts.iter().any(|t| t.text.contains("存储操作失败")));
    assert!(!action(&mut p, UiAction::Save { slot: 0 })
        .iter()
        .any(|c| matches!(c, AppCommand::Save { .. })));
    let saved = action(&mut p, UiAction::Save { slot: 1 });
    assert!(saved.iter().any(|c| matches!(
        c,
        AppCommand::Save {
            slot: 1,
            expected_revision: 3,
            ..
        }
    )));
    let job = slot_load_job(&mut p, 0);
    p.pump(
        vec![AppEvent::SlotLoadFailed {
            job,
            message: "Still malformed".into(),
        }],
        1000,
    );
    assert_eq!(serde_json::to_value(p.core().snapshot()).unwrap(), before);
    assert_eq!(p.generation.session, epoch);
    assert!(p.error.is_none());
    assert!(!p.is_loading());
}

#[test]
fn slot_read_failure_preserves_known_revision_until_a_valid_listing_recovers() {
    use nir_presentation::SlotView;
    use std::collections::BTreeMap;
    let mut p = playing();
    action(&mut p, UiAction::Saves);
    let valid = |revision| {
        AppEvent::Slots(
            vec![SlotView {
                slot: 0,
                exists: true,
                label: format!("#{revision}"),
                ..Default::default()
            }],
            BTreeMap::from([(0, revision)]),
        )
    };
    p.pump(vec![valid(5)], 1000);
    p.pump(
        vec![AppEvent::Slots(
            vec![SlotView {
                slot: 0,
                error: Some("E_IO".into()),
                ..Default::default()
            }],
            BTreeMap::new(),
        )],
        1000,
    );
    assert!(p.model().slots[0].error.is_some());
    p.pump(vec![valid(4)], 1000);
    assert!(
        p.model().slots[0].error.is_some(),
        "Older data must not clear the failure or roll the revision back"
    );
    p.pump(vec![valid(5)], 1000);
    assert!(p.model().slots[0].error.is_none());
    assert!(action(&mut p, UiAction::Save { slot: 0 }).iter().any(|c|
        matches!(c, AppCommand::Save { expected_revision: 5, envelope, .. } if envelope.revision == 6)));
}

#[test]
fn startup_metadata_warnings_survive_preparation_and_clear_independently_without_blocking_reading()
{
    for kinds in [
        vec![PersistenceKind::Preferences],
        vec![PersistenceKind::Profile],
        vec![PersistenceKind::Preferences, PersistenceKind::Profile],
    ] {
        let mut p = player();
        let initial = p.pump(
            kinds
                .iter()
                .map(|kind| AppEvent::PersistenceReadFailed {
                    kind: *kind,
                    message: "unreadable original metadata".into(),
                })
                .collect(),
            1000,
        );
        ready(&mut p, initial);
        assert_eq!(p.screen, nir_presentation::Screen::Title);
        assert!(p.error.is_none());
        let d = p.diagnostic.as_ref().unwrap();
        assert_eq!(d.code, "E_STORAGE");
        assert_eq!(d.details.as_ref().unwrap().operation, "load_metadata");
        assert_eq!(d.details.as_ref().unwrap().stage, "read");
        assert!(!p.status.is_empty());
        let commands = action(&mut p, UiAction::NewGame);
        ready(&mut p, commands);
        assert_eq!(p.screen, nir_presentation::Screen::Story);
        assert!(!p.paused());
        assert!(p.error.is_none());
        assert!(p.diagnostic.is_some());
        let snapshot = serde_json::to_vec(&p.core().snapshot()).unwrap();
        for (i, kind) in kinds.iter().enumerate() {
            let recovered = match kind {
                PersistenceKind::Preferences => {
                    AppEvent::PreferencesRecovered(p.preferences.clone())
                }
                PersistenceKind::Profile => AppEvent::ProfileRecovered(Default::default()),
            };
            let commands = p.pump(vec![recovered], 1000);
            assert_eq!(serde_json::to_vec(&p.core().snapshot()).unwrap(), snapshot);
            assert!(!p.paused());
            assert!(!commands.iter().any(|c| matches!(
                c,
                AppCommand::AudioStop { .. }
                    | AppCommand::AudioPause { .. }
                    | AppCommand::AudioBusPause { .. }
            )));
            assert_eq!(p.diagnostic.is_some(), i + 1 < kinds.len());
            assert_eq!(!p.status.is_empty(), i + 1 < kinds.len());
        }
    }
}

#[test]
fn persistence_failure_keeps_story_preferences_and_audio_policy_and_clears_only_its_warning() {
    let mut p = playing();
    let before = serde_json::to_vec(&p.core().snapshot()).unwrap();
    let session = p.generation.session;
    let preferences = serde_json::to_value(&p.preferences).unwrap();
    for kind in [PersistenceKind::Preferences, PersistenceKind::Profile] {
        let commands = p.pump(
            vec![AppEvent::PersistenceFailed {
                kind,
                message: "controlled write failure".into(),
            }],
            1000,
        );
        assert!(p.error.is_none());
        assert!(!p.paused());
        assert_eq!(p.generation.session, session);
        assert_eq!(serde_json::to_vec(&p.core().snapshot()).unwrap(), before);
        assert_eq!(serde_json::to_value(&p.preferences).unwrap(), preferences);
        assert!(commands.iter().any(|c|matches!(c,AppCommand::Diagnostic {diagnostic} if diagnostic.details.as_ref().unwrap().domain==ErrorDomain::Storage)));
        assert!(!commands.iter().any(|c| matches!(
            c,
            AppCommand::AudioStop { .. }
                | AppCommand::AudioPause { paused: true, .. }
                | AppCommand::AudioBusPause { paused: true, .. }
        )));
        let status = p.status.clone();
        let other = if kind == PersistenceKind::Preferences {
            PersistenceKind::Profile
        } else {
            PersistenceKind::Preferences
        };
        p.pump(vec![AppEvent::PersistenceStored(other)], 1000);
        assert_eq!(p.status, status);
        p.pump(vec![AppEvent::PersistenceStored(kind)], 1000);
        assert!(p.status.is_empty());
        assert!(p.diagnostic.is_none());
    }
    action(&mut p, UiAction::Saves);
    let commands = action(&mut p, UiAction::Save { slot: 0 });
    let job = commands
        .iter()
        .find_map(|c| {
            if let AppCommand::Save { job, .. } = c {
                Some(*job)
            } else {
                None
            }
        })
        .unwrap();
    p.pump(
        vec![AppEvent::PersistenceFailed {
            kind: PersistenceKind::Profile,
            message: "profile pending".into(),
        }],
        1000,
    );
    p.pump(
        vec![AppEvent::Saved {
            job,
            slot: 0,
            revision: 1,
        }],
        1000,
    );
    let saved = p.status.clone();
    assert!(!saved.is_empty());
    p.pump(
        vec![AppEvent::PersistenceStored(PersistenceKind::Profile)],
        1000,
    );
    assert_eq!(p.status, saved);
}

#[test]
fn recovering_one_persistence_kind_keeps_other_failure_and_survives_locale_change() {
    let mut p = playing();
    p.pump(
        vec![
            AppEvent::PersistenceFailed {
                kind: PersistenceKind::Preferences,
                message: "preferences blocked".into(),
            },
            AppEvent::PersistenceFailed {
                kind: PersistenceKind::Profile,
                message: "profile blocked".into(),
            },
        ],
        1000,
    );
    p.pump(
        vec![AppEvent::PersistenceStored(PersistenceKind::Profile)],
        1000,
    );
    assert_eq!(p.diagnostic.as_ref().unwrap().location, "preferences");
    assert!(!p.status.is_empty());
    assert!(p.error.is_none());
    // Exercise warning ownership across locale selection without a renderer:
    // the already emitted message belongs to the earlier UI locale.
    p.effective_ui_locale = if p.effective_ui_locale == "en" {
        "zh-Hans".into()
    } else {
        "en".into()
    };
    p.pump(
        vec![AppEvent::PersistenceStored(PersistenceKind::Preferences)],
        1000,
    );
    assert!(p.diagnostic.is_none());
    assert!(p.status.is_empty());
    assert!(!p.paused());
}

#[test]
fn metadata_read_recovery_preserves_new_edits_and_does_not_acknowledge_failed_writes() {
    let mut p = playing();
    let commands = action(&mut p, UiAction::Menu);
    ready(&mut p, commands);
    p.pump(
        vec![
            AppEvent::PersistenceReadFailed {
                kind: PersistenceKind::Preferences,
                message: "read blocked".into(),
            },
            AppEvent::PersistenceFailed {
                kind: PersistenceKind::Preferences,
                message: "write blocked".into(),
            },
            AppEvent::PersistenceReadFailed {
                kind: PersistenceKind::Profile,
                message: "progress blocked".into(),
            },
        ],
        1000,
    );
    let old = p.preferences.bgm_volume;
    action(
        &mut p,
        UiAction::Volume {
            bus: AudioBus::Bgm,
            delta: -0.1,
        },
    );
    action(
        &mut p,
        UiAction::Volume {
            bus: AudioBus::Bgm,
            delta: 0.1,
        },
    );
    let snapshot = serde_json::to_vec(&p.core().snapshot()).unwrap();
    let session = p.generation.session;
    let recovered = Preferences {
        font_scale: 1.4,
        bgm_volume: 0.9,
        ..p.preferences.clone()
    };
    let commands = p.pump(vec![AppEvent::PreferencesRecovered(recovered)], 1000);
    assert!((p.preferences.bgm_volume - old).abs() < 1e-6);
    assert_eq!(p.preferences.font_scale, 1.4);
    assert!(commands.iter().any(|c| matches!(c,AppCommand::PersistPreferences{preferences} if preferences.font_scale==1.4 && (preferences.bgm_volume-old).abs()<1e-6)));
    p.profile.insert("read:new".into());
    p.pump(
        vec![AppEvent::ProfileRecovered(["read:old".into()].into())],
        1000,
    );
    assert!(p.profile.contains("read:new") && p.profile.contains("read:old"));
    assert!(p.diagnostic.is_some());
    assert_eq!(
        p.diagnostic
            .as_ref()
            .unwrap()
            .details
            .as_ref()
            .unwrap()
            .operation,
        "persist"
    );
    p.pump(
        vec![AppEvent::PersistenceStored(PersistenceKind::Preferences)],
        1000,
    );
    assert!(p.diagnostic.is_none());
    assert_eq!(serde_json::to_vec(&p.core().snapshot()).unwrap(), snapshot);
    assert_eq!(p.generation.session, session);
    assert!(p.paused());
}

#[test]
fn a_successful_write_does_not_hide_unrecovered_progress_or_restart_audio() {
    let mut p = playing();
    let before = serde_json::to_vec(&p.core().snapshot()).unwrap();
    p.pump(
        vec![
            AppEvent::PersistenceReadFailed {
                kind: PersistenceKind::Profile,
                message: "old progress unavailable".into(),
            },
            AppEvent::PersistenceFailed {
                kind: PersistenceKind::Profile,
                message: "new progress unsaved".into(),
            },
        ],
        1000,
    );
    let commands = p.pump(
        vec![AppEvent::PersistenceStored(PersistenceKind::Profile)],
        1000,
    );
    assert_eq!(
        p.diagnostic
            .as_ref()
            .unwrap()
            .details
            .as_ref()
            .unwrap()
            .operation,
        "load_metadata"
    );
    assert!(!commands.iter().any(|c| matches!(
        c,
        AppCommand::AudioStop { .. }
            | AppCommand::AudioPause { .. }
            | AppCommand::AudioBusPause { .. }
    )));
    p.pump(
        vec![AppEvent::ProfileRecovered(["old".into()].into())],
        1000,
    );
    assert!(p.diagnostic.is_none());
    assert!(p.profile.contains("old"));
    assert_eq!(serde_json::to_vec(&p.core().snapshot()).unwrap(), before);
}

#[test]
fn retrying_an_unchanged_preference_write_uses_recovered_fields_and_requires_a_commit() {
    let mut p = player();
    let initial = p.pump(vec![], 1000);
    ready(&mut p, initial);
    action(
        &mut p,
        UiAction::Volume {
            bus: AudioBus::Bgm,
            delta: 0.,
        },
    );
    p.pump(
        vec![AppEvent::PersistenceFailed {
            kind: PersistenceKind::Preferences,
            message: "write unavailable".into(),
        }],
        1000,
    );
    let saved = Preferences {
        font_scale: 1.4,
        bgm_volume: 0.2,
        ..p.preferences.clone()
    };
    let commands = p.pump(vec![AppEvent::PreferencesRecovered(saved)], 1000);
    assert!(commands.iter().any(|c|matches!(c,AppCommand::PersistPreferences{preferences} if preferences.font_scale==1.4 && preferences.bgm_volume==0.2)));
    assert_eq!(
        p.diagnostic
            .as_ref()
            .unwrap()
            .details
            .as_ref()
            .unwrap()
            .operation,
        "persist"
    );
    p.pump(
        vec![AppEvent::PersistenceStored(PersistenceKind::Preferences)],
        1000,
    );
    assert!(p.diagnostic.is_none());
}

#[test]
fn unconfirmed_save_retains_busy_slot_until_its_original_acknowledgement() {
    let mut p = playing();
    let job = action(&mut p, UiAction::Save { slot: 1 })
        .into_iter()
        .find_map(|c| match c {
            AppCommand::Save { job, .. } => Some(job),
            _ => None,
        })
        .unwrap();
    let snapshot = serde_json::to_value(p.core().snapshot()).unwrap();
    p.pump(
        vec![AppEvent::SavePending {
            job,
            message: "awaiting native result".into(),
        }],
        1000,
    );
    assert_eq!(p.model().busy_slots, std::collections::BTreeSet::from([1]));
    assert_eq!(p.diagnostic.as_ref().unwrap().code, "E_STORAGE_UNCERTAIN");
    assert!(!p.status.contains("E_STORAGE"));
    assert!(!p.model().fault_recovery.contains(&Recovery::Retry));
    assert_eq!(serde_json::to_value(p.core().snapshot()).unwrap(), snapshot);
    assert!(!action(&mut p, UiAction::Save { slot: 1 })
        .iter()
        .any(|c| matches!(c, AppCommand::Save { .. })));
    // An unrelated slot acknowledgement cannot release the original job.
    p.pump(
        vec![AppEvent::Saved {
            job,
            slot: 0,
            revision: 1,
        }],
        1000,
    );
    assert_eq!(p.model().busy_slots, std::collections::BTreeSet::from([1]));
    p.pump(
        vec![AppEvent::Saved {
            job,
            slot: 1,
            revision: 1,
        }],
        1000,
    );
    assert!(p.model().busy_slots.is_empty());
    assert!(p.diagnostic.is_none());
    let status = p.status.clone();
    p.pump(
        vec![AppEvent::SavePending {
            job,
            message: "stale".into(),
        }],
        1000,
    );
    assert_eq!(p.status, status);
    assert!(p.diagnostic.is_none());
}

#[test]
fn unconfirmed_saves_remain_visible_across_other_completions_and_session_replacement() {
    let mut p = playing();
    let origin = p.generation.session;
    let mut jobs = Vec::new();
    for slot in 0..3 {
        jobs.push(
            action(&mut p, UiAction::Save { slot })
                .into_iter()
                .find_map(|c| match c {
                    AppCommand::Save { job, .. } => Some(job),
                    _ => None,
                })
                .unwrap(),
        );
    }
    for job in &jobs[..2] {
        p.pump(
            vec![AppEvent::SavePending {
                job: *job,
                message: "unknown".into(),
            }],
            1000,
        );
    }
    p.pump(
        vec![AppEvent::Saved {
            job: jobs[2],
            slot: 2,
            revision: 1,
        }],
        1000,
    );
    assert_eq!(p.diagnostic.as_ref().unwrap().code, "E_STORAGE_UNCERTAIN");
    let c = action(&mut p, UiAction::Title);
    ready(&mut p, c);
    assert_ne!(p.generation.session, origin);
    p.pump(
        vec![AppEvent::Saved {
            job: jobs[0],
            slot: 0,
            revision: 1,
        }],
        1000,
    );
    assert_eq!(p.model().busy_slots, std::collections::BTreeSet::from([1]));
    let details = p.diagnostic.as_ref().unwrap().details.as_ref().unwrap();
    assert_eq!(details.request, Some(jobs[1]));
    assert_eq!(details.session, Some(origin));
    p.pump(
        vec![AppEvent::Saved {
            job: jobs[1],
            slot: 1,
            revision: 1,
        }],
        1000,
    );
    assert!(p.model().busy_slots.is_empty());
    assert!(p.diagnostic.is_none());
}

#[test]
fn confirmed_save_failure_releases_unknown_slot_and_preserves_original_revision() {
    let mut p = playing();
    let job = action(&mut p, UiAction::Save { slot: 0 })
        .into_iter()
        .find_map(|c| match c {
            AppCommand::Save { job, .. } => Some(job),
            _ => None,
        })
        .unwrap();
    p.pump(
        vec![AppEvent::SavePending {
            job,
            message: "unknown".into(),
        }],
        1000,
    );
    p.pump(
        vec![AppEvent::SaveFailed {
            job,
            message: "native aborted".into(),
        }],
        1000,
    );
    assert!(p.model().busy_slots.is_empty());
    let commands = action(&mut p, UiAction::Save { slot: 0 });
    assert!(commands.iter().any(|c| matches!(
        c,
        AppCommand::Save {
            expected_revision: 0,
            ..
        }
    )));
    assert!(p.error.is_none());
}

fn save_warning_job(p: &mut Player, slot: u32) -> u32 {
    action(p, UiAction::Save { slot })
        .into_iter()
        .find_map(|c| match c {
            AppCommand::Save { job, .. } => Some(job),
            _ => None,
        })
        .unwrap()
}

#[test]
fn save_warnings_clear_by_classification_only_after_the_same_slot_commits() {
    for code in ["E_STORAGE_QUOTA", "E_SAVE_CONFLICT", "E_STORAGE"] {
        let mut p = playing();
        let before = serde_json::to_value(p.core().snapshot()).unwrap();
        let job = save_warning_job(&mut p, 0);
        p.pump(
            vec![AppEvent::SaveFault {
                job,
                diagnostic: Box::new(Diagnostic::new(code, "save", "controlled write failure")),
            }],
            1000,
        );
        assert_eq!(p.diagnostic.as_ref().unwrap().code, code);
        assert!(p.model().busy_slots.is_empty());
        let retry = action(&mut p, UiAction::Save { slot: 0 })
            .into_iter()
            .find_map(|c| match c {
                AppCommand::Save {
                    job,
                    expected_revision: 0,
                    ..
                } => Some(job),
                _ => None,
            })
            .unwrap();
        for (ack_job, slot) in [(job, 0), (retry, 1)] {
            p.pump(
                vec![AppEvent::Saved {
                    job: ack_job,
                    slot,
                    revision: 1,
                }],
                1000,
            );
            assert_eq!(p.diagnostic.as_ref().unwrap().code, code);
            assert_eq!(p.model().busy_slots, std::collections::BTreeSet::from([0]));
        }
        let commands = p.pump(
            vec![AppEvent::Saved {
                job: retry,
                slot: 0,
                revision: 1,
            }],
            1000,
        );
        assert!(p.diagnostic.is_none(), "committed retry must clear {code}");
        assert!(p.error.is_none());
        assert!(p.model().busy_slots.is_empty());
        assert_eq!(serde_json::to_value(p.core().snapshot()).unwrap(), before);
        assert!(!commands.iter().any(|c| matches!(
            c,
            AppCommand::AudioStop { .. } | AppCommand::AudioPause { .. }
        )));
    }
}

#[test]
fn save_warnings_keep_other_failed_slots_and_unrecovered_metadata() {
    let mut p = playing();
    let before = serde_json::to_value(p.core().snapshot()).unwrap();
    let jobs: Vec<_> = (0..3).map(|slot| save_warning_job(&mut p, slot)).collect();
    for (slot, code) in [(0, "E_STORAGE_QUOTA"), (1, "E_SAVE_CONFLICT")] {
        p.pump(
            vec![AppEvent::SaveFault {
                job: jobs[slot],
                diagnostic: Box::new(Diagnostic::new(code, "save", "controlled failure")),
            }],
            1000,
        );
    }
    p.pump(
        vec![AppEvent::Saved {
            job: jobs[2],
            slot: 2,
            revision: 1,
        }],
        1000,
    );
    let fault = p.diagnostic.as_ref().unwrap();
    assert_eq!(fault.code, "E_STORAGE_QUOTA");
    assert_eq!(fault.details.as_ref().unwrap().request, Some(jobs[0]));
    for slot in [1, 0] {
        let job = save_warning_job(&mut p, slot);
        p.pump(
            vec![AppEvent::PersistenceReadFailed {
                kind: PersistenceKind::Profile,
                message: "old read progress unavailable".into(),
            }],
            1000,
        );
        p.pump(
            vec![AppEvent::Saved {
                job,
                slot,
                revision: 1,
            }],
            1000,
        );
        if slot == 1 {
            // A later metadata warning can be visible. Its recovery must
            // reveal the failed save that still needs an explicit retry.
            p.pump(vec![AppEvent::ProfileRecovered(Default::default())], 1000);
            assert_eq!(p.diagnostic.as_ref().unwrap().code, "E_STORAGE_QUOTA");
            assert_eq!(
                p.diagnostic
                    .as_ref()
                    .unwrap()
                    .details
                    .as_ref()
                    .unwrap()
                    .request,
                Some(jobs[0])
            );
        } else {
            assert_eq!(p.diagnostic.as_ref().unwrap().location, "profile");
            p.pump(
                vec![AppEvent::PersistenceStored(PersistenceKind::Profile)],
                1000,
            );
            assert!(p.diagnostic.is_some());
            p.pump(vec![AppEvent::ProfileRecovered(Default::default())], 1000);
            assert!(p.diagnostic.is_none());
        }
    }
    assert_eq!(serde_json::to_value(p.core().snapshot()).unwrap(), before);
    assert!(p.error.is_none());
}

#[test]
fn save_warnings_survive_another_slots_uncertain_commit_until_their_own_retry() {
    let mut p = playing();
    let jobs: Vec<_> = (0..2).map(|slot| save_warning_job(&mut p, slot)).collect();
    p.pump(
        vec![
            AppEvent::SaveFault {
                job: jobs[1],
                diagnostic: Box::new(Diagnostic::new("E_SAVE_CONFLICT", "save", "slot changed")),
            },
            AppEvent::SavePending {
                job: jobs[0],
                message: "native result pending".into(),
            },
        ],
        1000,
    );
    assert_eq!(p.diagnostic.as_ref().unwrap().code, "E_STORAGE_UNCERTAIN");
    p.pump(
        vec![AppEvent::Saved {
            job: jobs[0],
            slot: 0,
            revision: 1,
        }],
        1000,
    );
    assert_eq!(p.diagnostic.as_ref().unwrap().code, "E_SAVE_CONFLICT");
    assert_eq!(
        p.diagnostic
            .as_ref()
            .unwrap()
            .details
            .as_ref()
            .unwrap()
            .request,
        Some(jobs[1])
    );
    let retry = save_warning_job(&mut p, 1);
    p.pump(
        vec![AppEvent::Saved {
            job: retry,
            slot: 1,
            revision: 1,
        }],
        1000,
    );
    assert!(p.diagnostic.is_none());
}

#[test]
fn save_warnings_do_not_clear_a_matching_code_from_another_error_domain() {
    let mut p = playing();
    let job = save_warning_job(&mut p, 0);
    let unrelated = Diagnostic::new("E_STORAGE", "save", "authored fault with a similar code")
        .classified(
            ErrorDomain::Core,
            "execute",
            "core",
            vec![Recovery::KeepCurrent],
        );
    p.diagnostic = Some(unrelated.clone());
    p.pump(
        vec![AppEvent::Saved {
            job,
            slot: 0,
            revision: 1,
        }],
        1000,
    );
    assert_eq!(p.diagnostic, Some(unrelated));
}
