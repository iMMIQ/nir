//! P5.2 story.typed-result.v1: the VM owns typed writes for interactions,
//! a semantic selection cursor survives snapshots while hover/focus stay
//! presentation transients, and an explicit cancel path exists.
use nir_core::*;
use nir_format::*;
use serde_json::json;

/// A program whose entry reaches a typed interaction directly. `mode` picks
/// between plain, typed, and typed-plus-cancel; the destination blocks end in
/// distinct outcomes so the committed branch is observable.
fn program(mode: &str, values: Option<&[i64]>, timeout_us: Option<u64>) -> Program {
    let mut p: Program = serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    if mode != "plain" {
        p.requires.push("story.typed-result.v1".into());
    }
    p.variables.insert(
        "picked".into(),
        serde_json::from_value(json!({"type":"i32","value":0})).unwrap(),
    );
    let route = p.choices.get_mut("route").unwrap();
    if let Some(timeout_us) = timeout_us {
        route.timeout_us = Some(Micros(timeout_us));
        route.default = Some("stay".into());
    }
    for (option, value) in route.options.iter_mut().zip(values.into_iter().flatten()) {
        option.value = Some(serde_json::from_value(json!({"type":"i32","value":value})).unwrap());
    }
    let terminator = |result: bool, cancel: bool| {
        json!({
            "type":"interact",
            "choice":"route",
            "branches":{"walk":"after_walk","stay":"after_stay"},
            "on_empty":"failed",
            "result": result.then_some("picked"),
            "on_cancel": cancel.then_some("gave_up"),
        })
    };
    let f = p.functions.get_mut("main").unwrap();
    f.entry = "test".into();
    f.blocks.insert(
        "test".into(),
        serde_json::from_value(
            json!({"terminator": terminator(mode != "plain", mode == "cancel")}),
        )
        .unwrap(),
    );
    for (block, outcome) in [
        ("after_walk", "walk"),
        ("after_stay", "stay"),
        ("gave_up", "gave_up"),
    ] {
        f.blocks.insert(
            block.into(),
            serde_json::from_value(json!({"terminator":{"type":"end","outcome":outcome}})).unwrap(),
        );
    }
    p
}

fn start(p: Program) -> Core {
    let mut c = Core::new(
        ValidatedProgram::new(p).unwrap(),
        "typed".into(),
        "en".into(),
    )
    .unwrap();
    c.step(CoreInput::None, 1000);
    assert!(c.state().fault.is_none(), "{:?}", c.state().fault);
    c
}
fn offered(c: &Core) -> &OfferedChoice {
    c.state().choice.as_ref().expect("interaction pending")
}

#[test]
fn choice_history_records_only_committed_choices_and_freezes_offered_labels() {
    for mode in ["plain", "typed"] {
        let mut c = start(program(mode, Some(&[1, 2]), None));
        let choice = offered(&c).clone();
        assert!(c.state().history.is_empty());
        for (interaction, option, sequence) in [
            (choice.interaction + 99, "stay", 1),
            (choice.interaction, "unknown", 2),
            (choice.interaction, "stay", 0),
        ] {
            c.step(
                CoreInput::Choose {
                    interaction,
                    option: option.into(),
                    sequence,
                },
                1000,
            );
            assert!(c.state().history.is_empty());
        }
        c.step(
            CoreInput::SelectChoice {
                interaction: choice.interaction,
                option: "stay".into(),
                sequence: 3,
            },
            1000,
        );
        assert!(
            c.state().history.is_empty(),
            "cursor movement is not a committed choice"
        );
        c.step(
            CoreInput::Choose {
                interaction: choice.interaction,
                option: "stay".into(),
                sequence: 4,
            },
            1000,
        );
        let h = &c.state().history[0];
        assert_eq!(h.interaction, choice.interaction);
        assert!(h.voices.is_empty());
        assert_eq!(h.locale, choice.locale);
        assert_eq!(h.font_plan_digest, choice.font_plan_digest);
        assert_eq!(h.text, choice.options[1].label);
        let record = h.choice.as_ref().unwrap();
        assert_eq!(record.id, choice.id);
        assert_eq!(
            record.resolution,
            HistoryChoiceResolution::Selected {
                option: "stay".into()
            }
        );
        assert_eq!(record.options.len(), 2);
        for (frozen, offered) in record.options.iter().zip(&choice.options) {
            assert_eq!(frozen.id, offered.id);
            assert_eq!(frozen.label, offered.label);
            assert_eq!(frozen.enabled, offered.enabled);
        }
        c.step(
            CoreInput::Choose {
                interaction: choice.interaction,
                option: "walk".into(),
                sequence: 5,
            },
            1000,
        );
        assert_eq!(
            c.state().history.len(),
            1,
            "duplicate callback cannot append"
        );
        let snapshot = c.snapshot();
        let restored =
            Core::restore(c.validated_program().clone(), snapshot.clone(), "typed").unwrap();
        assert_eq!(
            serde_json::to_value(&restored.state().history).unwrap(),
            serde_json::to_value(snapshot.history).unwrap()
        );
        assert_eq!(restored.state().variables, c.state().variables);
        assert_eq!(restored.state().outcome, c.state().outcome);
    }
}

#[test]
fn history_distinguishes_default_timeout_and_explicit_cancel_without_result_writes() {
    let mut timeout = start(program("typed", Some(&[1, 2]), Some(1_000_000)));
    timeout.step(
        CoreInput::Time {
            delta_us: 1_100_000,
        },
        1000,
    );
    let record = timeout.state().history[0].choice.as_ref().unwrap();
    assert_eq!(
        record.resolution,
        HistoryChoiceResolution::TimedOut {
            option: "stay".into()
        }
    );
    assert_eq!(timeout.state().variables["picked"], Value::I32(2));
    let mut cancel = start(program("cancel", Some(&[1, 2]), None));
    cancel.step(
        CoreInput::CancelChoice {
            interaction: offered(&cancel).interaction,
            sequence: 1,
        },
        1000,
    );
    let h = &cancel.state().history[0];
    let record = h.choice.as_ref().unwrap();
    assert_eq!(record.resolution, HistoryChoiceResolution::Cancelled);
    assert_eq!(
        h.text,
        record
            .options
            .iter()
            .map(|o| o.label.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert_eq!(cancel.state().variables["picked"], Value::I32(0));
    for c in [&timeout, &cancel] {
        let restored = Core::restore(c.validated_program().clone(), c.snapshot(), "typed").unwrap();
        assert_eq!(
            serde_json::to_value(&restored.state().history).unwrap(),
            serde_json::to_value(&c.state().history).unwrap()
        );
    }
}

#[test]
fn choice_history_excludes_hidden_rows_and_preserves_disabled_rows() {
    let mut p = program("typed", Some(&[1, 2]), None);
    let disabled =
        serde_json::from_value(json!({"type":"const","value":{"type":"bool","value":false}}))
            .unwrap();
    p.choices.get_mut("route").unwrap().options[0].enabled = Some(disabled);
    let mut c = start(p.clone());
    let interaction = offered(&c).interaction;
    c.step(
        CoreInput::Choose {
            interaction,
            option: "walk".into(),
            sequence: 1,
        },
        1000,
    );
    assert!(c.state().history.is_empty());
    c.step(
        CoreInput::Choose {
            interaction,
            option: "stay".into(),
            sequence: 2,
        },
        1000,
    );
    let rows = &c.state().history[0].choice.as_ref().unwrap().options;
    assert_eq!(rows.len(), 2);
    assert!(!rows[0].enabled);
    let option = &mut p.choices.get_mut("route").unwrap().options[0];
    option.visible = option.enabled.take();
    let mut c = start(p);
    c.step(
        CoreInput::Choose {
            interaction: offered(&c).interaction,
            option: "stay".into(),
            sequence: 1,
        },
        1000,
    );
    let rows = &c.state().history[0].choice.as_ref().unwrap().options;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "stay");
}

#[test]
fn history_restore_rejects_invalid_resolution_option_identity_and_mixed_voice_metadata() {
    let mut c = start(program("typed", Some(&[1, 2]), None));
    c.step(
        CoreInput::Choose {
            interaction: offered(&c).interaction,
            option: "stay".into(),
            sequence: 1,
        },
        1000,
    );
    let snapshot = c.snapshot();
    let rejects = |bad| {
        assert_eq!(
            Core::restore(c.validated_program().clone(), bad, "typed")
                .err()
                .unwrap()
                .code,
            "E_SNAPSHOT"
        )
    };
    let mut bad = snapshot.clone();
    bad.history[0].choice.as_mut().unwrap().resolution = HistoryChoiceResolution::Selected {
        option: "unknown".into(),
    };
    rejects(bad);
    let mut bad = snapshot.clone();
    bad.history[0].choice.as_mut().unwrap().options[1].enabled = false;
    rejects(bad);
    let mut bad = snapshot.clone();
    bad.history[0].choice.as_mut().unwrap().id = "unknown".into();
    rejects(bad);
    let mut bad = snapshot.clone();
    bad.history[0].choice.as_mut().unwrap().options[0].source_revision += 1;
    rejects(bad);
    let mut bad = snapshot.clone();
    let record = bad.history[0].choice.as_mut().unwrap();
    record.options.push(record.options[0].clone());
    rejects(bad);
    let mut bad = snapshot.clone();
    bad.history[0].text.push_str("tampered");
    rejects(bad);
    let mut bad = snapshot.clone();
    bad.history[0].interaction = 0;
    rejects(bad);
    let mut bad = snapshot;
    bad.history[0].voices.push(HistoryVoice {
        instance: 1,
        asset: "voice.arrival".into(),
        gain: 1.,
    });
    rejects(bad);
}

#[test]
fn oversized_choice_history_does_not_fault_or_change_the_committed_result() {
    let mut p = program("typed", Some(&[1, 2]), None);
    p.locales
        .get_mut("en")
        .unwrap()
        .get_mut("walk")
        .unwrap()
        .spans = vec![Span::Text {
        id: "body".into(),
        text: "a".repeat(128 * 1024),
        emphasis: false,
    }];
    let option = p.choices["route"].options[0].clone();
    p.choices.get_mut("route").unwrap().options = (0..40)
        .map(|i| ChoiceOption {
            id: format!("row{i}"),
            ..option.clone()
        })
        .collect();
    for block in p.functions.get_mut("main").unwrap().blocks.values_mut() {
        if let Terminator::Interact {
            choice, branches, ..
        } = &mut block.terminator
        {
            if choice == "route" {
                *branches = (0..40)
                    .map(|i| (format!("row{i}"), "after_walk".into()))
                    .collect();
            }
        }
    }
    let mut c = start(p);
    c.step(
        CoreInput::Choose {
            interaction: offered(&c).interaction,
            option: "row39".into(),
            sequence: 1,
        },
        1000,
    );
    assert!(c.state().fault.is_none());
    assert_eq!(c.state().outcome.as_deref(), Some("walk"));
    assert_eq!(c.state().variables["picked"], Value::I32(1));
    assert!(
        c.state().history.is_empty(),
        "derived history must fit whole or be omitted"
    );
}

/// Choosing commits the option's declared value before the branch; the host
/// named an id and never supplied the value.
#[test]
fn typed_choose_writes_the_declared_value_before_the_branch() {
    let mut c = start(program("typed", Some(&[1, 2]), None));
    let interaction = offered(&c).interaction;
    assert_eq!(offered(&c).result.as_deref(), Some("picked"));
    assert_eq!(
        offered(&c).selected.as_deref(),
        Some("walk"),
        "cursor starts at the first enabled row without a default"
    );
    assert_eq!(c.state().variables["picked"], Value::I32(0));
    c.step(
        CoreInput::Choose {
            interaction,
            option: "stay".into(),
            sequence: 1,
        },
        1000,
    );
    assert_eq!(c.state().variables["picked"], Value::I32(2));
    assert_eq!(c.state().outcome.as_deref(), Some("stay"));
}

/// A plain interaction is unchanged: no values, no cursor, no cancel.
#[test]
fn plain_interaction_carries_no_result_state() {
    let mut c = start(program("plain", Some(&[1, 2]), None));
    let interaction = offered(&c).interaction;
    assert_eq!(offered(&c).result, None);
    assert_eq!(offered(&c).on_cancel, None);
    assert!(offered(&c).values.is_empty());
    assert_eq!(offered(&c).selected, None);
    // Cursor moves and cancels are observations on typed interactions only.
    let before = serde_json::to_string(c.state()).unwrap();
    c.step(
        CoreInput::SelectChoice {
            interaction,
            option: "stay".into(),
            sequence: 1,
        },
        1000,
    );
    c.step(
        CoreInput::CancelChoice {
            interaction,
            sequence: 2,
        },
        1000,
    );
    assert_eq!(c.state().outcome, None);
    assert_eq!(
        serde_json::to_string(c.state()).unwrap(),
        before,
        "plain interactions ignore cursor and cancel inputs"
    );
    c.step(
        CoreInput::Choose {
            interaction,
            option: "walk".into(),
            sequence: 3,
        },
        1000,
    );
    assert_eq!(c.state().variables["picked"], Value::I32(0));
    assert_eq!(c.state().outcome.as_deref(), Some("walk"));
}

/// Cancel jumps through the declared target without writing any value and
/// counts as an input with its own checkpoint.
#[test]
fn cancel_jumps_without_a_typed_write() {
    let mut c = start(program("cancel", Some(&[1, 2]), None));
    let interaction = offered(&c).interaction;
    assert_eq!(offered(&c).on_cancel.as_deref(), Some("gave_up"));
    c.step(
        CoreInput::CancelChoice {
            interaction,
            sequence: 1,
        },
        1000,
    );
    assert_eq!(c.state().variables["picked"], Value::I32(0));
    assert_eq!(c.state().outcome.as_deref(), Some("gave_up"));
    assert_eq!(c.state().last_input, 1);
    // A stale cancel cannot fire twice.
    c.step(
        CoreInput::CancelChoice {
            interaction,
            sequence: 1,
        },
        1000,
    );
}

/// The cursor moves on observation inputs: no input identity, no progress, no
/// checkpoint — but it is part of the snapshot and restores with it.
#[test]
fn selection_moves_snapshot_and_restores_without_progress() {
    let mut c = start(program("typed", Some(&[1, 2]), None));
    let interaction = offered(&c).interaction;
    let before = c.state().last_input;
    let position = c.location().clone();
    c.step(
        CoreInput::SelectChoice {
            interaction,
            option: "stay".into(),
            sequence: 0,
        },
        1000,
    );
    let choice = offered(&c);
    assert_eq!(choice.selected.as_deref(), Some("stay"));
    assert_eq!(c.state().last_input, before);
    assert_eq!(&c.location().clone(), &position);
    // Unknown options, stale interactions and disabled rows never move it.
    c.step(
        CoreInput::SelectChoice {
            interaction: interaction + 7,
            option: "walk".into(),
            sequence: 0,
        },
        1000,
    );
    c.step(
        CoreInput::SelectChoice {
            interaction,
            option: "nope".into(),
            sequence: 0,
        },
        1000,
    );
    assert_eq!(offered(&c).selected.as_deref(), Some("stay"));
    let snapshot = c.snapshot();
    let restored = Core::restore(c.validated_program().clone(), snapshot, "typed").unwrap();
    assert_eq!(
        offered(&restored).selected.as_deref(),
        Some("stay"),
        "the semantic cursor restores with the interaction"
    );
    // The restored cursor still commits the chosen row's value.
    let mut restored = restored;
    restored.step(
        CoreInput::Choose {
            interaction: offered(&restored).interaction,
            option: "stay".into(),
            sequence: 1,
        },
        1000,
    );
    assert_eq!(restored.state().variables["picked"], Value::I32(2));
}

/// A timeout commits the default option with its typed value, exactly like an
/// explicit choice of that row.
#[test]
fn timeout_commits_the_default_typed_value() {
    let mut c = start(program("typed", Some(&[1, 2]), Some(1_000_000)));
    assert_eq!(offered(&c).selected.as_deref(), Some("stay"));
    c.step(
        CoreInput::Time {
            delta_us: 1_100_000,
        },
        1000,
    );
    assert!(c.state().choice.is_none());
    assert_eq!(c.state().variables["picked"], Value::I32(2));
    assert_eq!(c.state().outcome.as_deref(), Some("stay"));
}

/// Restore rejects tampered typed-result state: values that differ from the
/// declarations, a cursor that names no live enabled row, result/cancel fields
/// that disagree with the canonical terminator, and plain interactions that
/// carry result state at all.
#[test]
fn restore_rejects_tampered_typed_interactions() {
    let ok = |snapshot: Snapshot, p: &Program| {
        Core::restore(
            ValidatedProgram::new(p.clone()).unwrap(),
            snapshot.clone(),
            "typed",
        )
        .is_ok()
    };
    let c = start(program("typed", Some(&[1, 2]), None));
    let snapshot = c.snapshot();
    assert!(Core::restore(c.validated_program().clone(), snapshot.clone(), "typed").is_ok());
    // Tampered value.
    let mut bad = snapshot.clone();
    let choice = bad.choice.as_mut().unwrap();
    choice.values.insert("walk".into(), Value::I32(9));
    assert!(!ok(bad, &program("typed", Some(&[1, 2]), None)));
    // Dropped value.
    let mut bad = snapshot.clone();
    let choice = bad.choice.as_mut().unwrap();
    choice.values.remove("stay");
    assert!(!ok(bad, &program("typed", Some(&[1, 2]), None)));
    // Cursor naming an unknown row.
    let mut bad = snapshot.clone();
    let choice = bad.choice.as_mut().unwrap();
    choice.selected = Some("nope".into());
    assert!(!ok(bad, &program("typed", Some(&[1, 2]), None)));
    // Missing cursor in typed mode.
    let mut bad = snapshot.clone();
    let choice = bad.choice.as_mut().unwrap();
    choice.selected = None;
    assert!(!ok(bad, &program("typed", Some(&[1, 2]), None)));
    // Canonical terminator mismatch: the snapshot claims a cancel target the
    // program does not declare.
    let mut bad = snapshot.clone();
    let choice = bad.choice.as_mut().unwrap();
    choice.on_cancel = Some("gave_up".into());
    assert!(!ok(bad, &program("typed", Some(&[1, 2]), None)));
    // A plain interaction carrying result state.
    let plain = start(program("plain", Some(&[1, 2]), None));
    let mut bad = plain.snapshot();
    let choice = bad.choice.as_mut().unwrap();
    choice.values.insert("walk".into(), Value::I32(1));
    assert!(!ok(bad, &program("plain", Some(&[1, 2]), None)));
    let mut bad = plain.snapshot();
    let choice = bad.choice.as_mut().unwrap();
    choice.selected = Some("walk".into());
    assert!(!ok(bad, &program("plain", Some(&[1, 2]), None)));
}

/// Source validation: capability gating, the target variable must exist, every
/// option must carry a value of the target type, and the cancel target must
/// name a block.
#[test]
fn typed_validation_rejects_invalid_interactions() {
    let rejects = |p: Program| ValidatedProgram::new(p).unwrap_err().code;
    // Capability.
    let mut p = program("typed", Some(&[1, 2]), None);
    p.requires.retain(|cap| cap != "story.typed-result.v1");
    assert_eq!(ValidatedProgram::new(p).unwrap_err().code, "E_CAPABILITY");
    // Cancel alone also needs the capability.
    let mut p = program("cancel", Some(&[1, 2]), None);
    p.requires.retain(|cap| cap != "story.typed-result.v1");
    assert_eq!(ValidatedProgram::new(p).unwrap_err().code, "E_CAPABILITY");
    // Unknown target variable.
    let mut p = program("typed", Some(&[1, 2]), None);
    p.functions
        .get_mut("main")
        .unwrap()
        .blocks
        .get_mut("test")
        .unwrap()
        .terminator = serde_json::from_value(json!({
            "type":"interact","choice":"route",
            "branches":{"walk":"after_walk","stay":"after_stay"},"on_empty":"failed",
            "result":"missing"}))
    .unwrap();
    assert_eq!(rejects(p), "E_VARIABLE");
    // A missing option value.
    assert_eq!(rejects(program("typed", Some(&[1]), None)), "E_TYPE");
    // A wrong-typed option value.
    let mut p: Program = serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    p.requires.push("story.typed-result.v1".into());
    p.variables.insert(
        "picked".into(),
        serde_json::from_value(json!({"type":"string","value":""})).unwrap(),
    );
    for option in p.choices.get_mut("route").unwrap().options.iter_mut() {
        option.value = Some(serde_json::from_value(json!({"type":"i32","value":1})).unwrap());
    }
    let f = p.functions.get_mut("main").unwrap();
    f.entry = "test".into();
    f.blocks.insert(
        "test".into(),
        serde_json::from_value(json!({"terminator":{
            "type":"interact","choice":"route",
            "branches":{"walk":"done","stay":"done"},"on_empty":"done","result":"picked"}}))
        .unwrap(),
    );
    f.blocks.insert(
        "done".into(),
        serde_json::from_value(json!({"terminator":{"type":"end","outcome":"done"}})).unwrap(),
    );
    assert_eq!(rejects(p), "E_TYPE");
    // Cancel target must name a block.
    let mut p = program("cancel", Some(&[1, 2]), None);
    p.functions
        .get_mut("main")
        .unwrap()
        .blocks
        .get_mut("test")
        .unwrap()
        .terminator = serde_json::from_value(json!({
            "type":"interact","choice":"route",
            "branches":{"walk":"after_walk","stay":"after_stay"},"on_empty":"failed",
            "on_cancel":"nowhere"}))
    .unwrap();
    assert_eq!(rejects(p), "E_BLOCK");
}

/// E_INFINITE_WAIT generalization: a definition whose effect tree contains a
/// looped-audio leaf anywhere can never reach a natural Finished milestone —
/// sequences stall at the leaf and parallels never see all children finish.
#[test]
fn infinite_wait_diagnoses_looped_audio_inside_compositions() {
    let mut p: Program = serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    p.requires.push("task.compose.v1".into());
    let looped = |id: &str| {
        json!({"id":id,"scope":"session","effect":{
            "type":"audio","asset":"audio.bell","bus":"bgm","looped":true}})
    };
    let wrapped = |kind: &str| {
        json!({
            "id": "amb", "scope": "session", "effect": {
                "type": kind,
                "children": [
                    {"id":"step","scope":"session","effect":{"type":"delay","duration_us":"1000"}},
                    looped("hum"),
                ],
            },
        })
    };
    for (name, effect) in [
        ("direct", looped("amb")),
        ("in_chain", wrapped("sequence")),
        ("in_parallel", wrapped("parallel_all")),
    ] {
        p.cues.insert(
            "test".into(),
            serde_json::from_value(json!({"effects":[effect]})).unwrap(),
        );
        let f = p.functions.get_mut("main").unwrap();
        f.entry = "test".into();
        f.blocks.insert(
            "test".into(),
            serde_json::from_value(
                json!({"terminator":{"type":"activate","cue":"test","next":"hold"}}),
            )
            .unwrap(),
        );
        f.blocks.insert("hold".into(), serde_json::from_value(json!({"terminator":{"type":"await","conditions":[{"task":"amb","milestone":{"type":"finished"}}],"next":"done","on_cancelled":"done","on_failed":"done"}})).unwrap());
        f.blocks.insert(
            "done".into(),
            serde_json::from_value(json!({"terminator":{"type":"end","outcome":"done"}})).unwrap(),
        );
        assert_eq!(
            ValidatedProgram::new(p.clone()).unwrap_err().code,
            "E_INFINITE_WAIT",
            "{name} wait on looped audio must be diagnosed"
        );
    }
    // A non-looping audio leaf in the same shapes stays waitable.
    let mut p: Program = serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    p.requires.push("task.compose.v1".into());
    p.cues.insert(
        "test".into(),
        serde_json::from_value(json!({"effects":[
            {"id":"amb","scope":"session","effect":{"type":"sequence","children":[
                {"id":"step","scope":"session","effect":{"type":"delay","duration_us":"1000"}},
                {"id":"hum","scope":"session","effect":{"type":"audio","asset":"audio.bell","bus":"bgm","looped":false}}]}}]}))
            .unwrap(),
    );
    let f = p.functions.get_mut("main").unwrap();
    f.entry = "test".into();
    f.blocks.insert(
        "test".into(),
        serde_json::from_value(
            json!({"terminator":{"type":"activate","cue":"test","next":"hold"}}),
        )
        .unwrap(),
    );
    f.blocks.insert("hold".into(), serde_json::from_value(json!({"terminator":{"type":"await","conditions":[{"task":"amb","milestone":{"type":"finished"}}],"next":"done","on_cancelled":"done","on_failed":"done"}})).unwrap());
    f.blocks.insert(
        "done".into(),
        serde_json::from_value(json!({"terminator":{"type":"end","outcome":"done"}})).unwrap(),
    );
    assert!(ValidatedProgram::new(p).is_ok());
}
