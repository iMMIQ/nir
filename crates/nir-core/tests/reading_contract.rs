use nir_core::*;
use nir_format::*;
use serde_json::json;

fn program() -> Program {
    let mut p: Program = serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    p.requires.push("text.voice-binding.v1".into());
    let asset = p
        .assets
        .iter()
        .find(|(_, a)| a.kind == AssetKind::Audio)
        .unwrap()
        .0
        .clone();
    p.cues.insert("test".into(), serde_json::from_value(json!({"effects":[
        {"id":"line","scope":"interaction","effect":{"type":"dialogue","text":"letter","reveal_us":"1000"}},
        {"id":"voice","scope":"session","effect":{"type":"audio","asset":asset,"bus":"voice","looped":false}},
        {"id":"other","scope":"session","effect":{"type":"audio","asset":asset,"bus":"voice","looped":false}}
    ]})).unwrap());
    let gate = p.texts["letter"].gates.first().unwrap().clone();
    let f = p.functions.get_mut("main").unwrap();
    f.entry = "test".into();
    f.blocks.insert(
        "test".into(),
        serde_json::from_value(
            json!({"terminator":{"type":"activate","cue":"test","next":"gate"}}),
        )
        .unwrap(),
    );
    f.blocks.insert("gate".into(), serde_json::from_value(json!({"terminator":{"type":"await","conditions":[{"task":"line","milestone":{"type":"marker","id":gate}}],"next":"bind","on_cancelled":"done","on_failed":"done"}})).unwrap());
    f.blocks.insert("bind".into(), serde_json::from_value(json!({"ops":[
        {"id":"bind-voice","operation":{"type":"dialogue_voice","task":"line","voice":"voice","wait":"after_voice"}}
    ],"terminator":{"type":"await","conditions":[{"task":"voice","milestone":{"type":"finished"}}],"next":"continue","on_cancelled":"continue","on_failed":"continue"}})).unwrap());
    f.blocks.insert("continue".into(), serde_json::from_value(json!({"ops":[{"id":"continue-line","operation":{"type":"dialogue_continue","task":"line"}}],"terminator":{"type":"await","conditions":[{"task":"line","milestone":{"type":"finished"}}],"next":"done","on_cancelled":"done","on_failed":"done"}})).unwrap());
    f.blocks.insert(
        "done".into(),
        serde_json::from_value(json!({"terminator":{"type":"end","outcome":"done"}})).unwrap(),
    );
    p
}
fn start(p: Program) -> Core {
    let mut c = Core::new(
        ValidatedProgram::new(p).unwrap(),
        "reading".into(),
        "en".into(),
    )
    .unwrap();
    c.step(CoreInput::None, 1000);
    let activation = c.state().pending.as_ref().unwrap().id;
    c.step(CoreInput::Prepared { activation }, 1000);
    let interaction = c.dialogue().unwrap().1.interaction;
    c.step(
        CoreInput::Advance {
            interaction,
            sequence: 1,
        },
        1000,
    );
    assert!(c.state().fault.is_none(), "{:?}", c.state().fault);
    c
}

fn plain_reading(mut p: Program) -> Core {
    p.cues.get_mut("test").unwrap().effects.push(EffectDef {
        id: "hold-clock".into(),
        scope: Scope::Session,
        effect: Effect::Delay {
            duration_us: Micros(10_000_000),
        },
    });
    let Effect::Dialogue { text, .. } = &mut p.cues.get_mut("test").unwrap().effects[0].effect
    else {
        unreachable!()
    };
    *text = "intro".into();
    let blocks = &mut p.functions.get_mut("main").unwrap().blocks;
    blocks.get_mut("test").unwrap().terminator =
        serde_json::from_value(json!({"type":"activate","cue":"test","next":"bind"})).unwrap();
    blocks.get_mut("bind").unwrap().terminator = serde_json::from_value(
        json!({"type":"await","conditions":[{"task":"line","milestone":{"type":"finished"}}],"next":"hold","on_cancelled":"done","on_failed":"done"}),
    ).unwrap();
    blocks.insert("hold".into(), serde_json::from_value(
        json!({"terminator":{"type":"await","conditions":[{"task":"hold-clock","milestone":{"type":"finished"}}],"next":"done","on_cancelled":"done","on_failed":"done"}}),
    ).unwrap());
    let mut c = Core::new(
        ValidatedProgram::new(p).unwrap(),
        "reading".into(),
        "en".into(),
    )
    .unwrap();
    c.step(CoreInput::None, 1000);
    let activation = c.state().pending.as_ref().unwrap().id;
    c.step(CoreInput::Prepared { activation }, 1000);
    assert!(c.state().fault.is_none(), "{:?}", c.state().fault);
    c
}

fn stop_on_advance(c: &mut Core, interaction: u32, sequence: u32) -> CoreStep {
    c.step(
        CoreInput::AdvanceReading {
            interaction,
            sequence,
            stop_voice: true,
        },
        1000,
    )
}

#[test]
fn voice_stop_is_atomic_with_real_completion_and_ignores_reveal_gate_and_stale_inputs() {
    let mut c = plain_reading(program());
    let interaction = c.dialogue().unwrap().1.interaction;
    let voice = c.state().handles["voice"];
    let other = c.state().handles["other"];
    let rejected = stop_on_advance(&mut c, interaction + 1, 1);
    assert!(rejected.intents.is_empty());
    assert_eq!(c.state().last_input, 0);
    let reveal = stop_on_advance(&mut c, interaction, 1);
    assert!(c.dialogue().unwrap().1.awaiting_advance);
    assert!(!reveal
        .intents
        .iter()
        .any(|i| matches!(i, CoreIntent::AudioStop { .. })));
    assert!(stop_on_advance(&mut c, interaction, 1).intents.is_empty());
    let complete = stop_on_advance(&mut c, interaction, 2);
    assert_eq!(
        c.state().tasks[&voice].end_reason,
        Some(TaskEndReason::CancelledByControl)
    );
    assert_eq!(c.state().tasks[&other].state, TaskState::Running);
    assert_eq!(
        complete
            .intents
            .iter()
            .filter(|i| matches!(i, CoreIntent::AudioStop { task } if *task == voice))
            .count(),
        1
    );
    assert!(!c.state().tasks[&voice]
        .milestones
        .contains(&Milestone::Finished));
    let late = c.step(CoreInput::AudioEnded { task: voice }, 1000);
    assert!(!late
        .intents
        .iter()
        .any(|i| matches!(i, CoreIntent::AudioStop { .. })));
    assert_eq!(
        c.state().tasks[&voice].end_reason,
        Some(TaskEndReason::CancelledByControl)
    );

    let mut gated = start(program());
    let interaction = gated.dialogue().unwrap().1.interaction;
    assert!(gated.dialogue().unwrap().1.at_gate);
    assert!(!stop_on_advance(&mut gated, interaction, 2)
        .intents
        .iter()
        .any(|i| matches!(i, CoreIntent::AudioStop { .. })));
    assert!(gated.dialogue().unwrap().1.at_gate);
    assert_eq!(
        gated.state().tasks[&gated.state().handles["voice"]].state,
        TaskState::Running
    );
}

#[test]
fn stopping_multiple_bound_phrases_survives_restore_and_history_eviction() {
    let mut p = program();
    let asset = match &p.cues["test"].effects[1].effect {
        Effect::Audio { asset, .. } => asset.clone(),
        _ => unreachable!(),
    };
    for i in 0..MAX_HISTORY_VOICES {
        let name = format!("phrase-{i}");
        p.cues.get_mut("test").unwrap().effects.push(EffectDef {
            id: name.clone(),
            scope: Scope::Session,
            effect: Effect::Audio {
                asset: asset.clone(),
                bus: AudioBus::Voice,
                looped: false,
                loop_region: None,
                gain: 1.,
            },
        });
        p.functions
            .get_mut("main")
            .unwrap()
            .blocks
            .get_mut("bind")
            .unwrap()
            .ops
            .push(Op {
                id: format!("bind-{i}"),
                operation: Operation::DialogueVoice {
                    task: "line".into(),
                    voice: Some(name),
                    wait: VoiceWaitPolicy::Parallel,
                },
            });
    }
    let original = plain_reading(p);
    assert!(original.state().history.is_empty());
    assert_eq!(
        original
            .dialogue()
            .unwrap()
            .1
            .reading
            .as_ref()
            .unwrap()
            .active_voices
            .len(),
        MAX_HISTORY_VOICES + 1
    );
    let mut c = Core::restore(
        original.validated_program().clone(),
        original.snapshot(),
        "reading",
    )
    .unwrap();
    let interaction = c.dialogue().unwrap().1.interaction;
    stop_on_advance(&mut c, interaction, 1);
    let complete = stop_on_advance(&mut c, interaction, 2);
    assert_eq!(
        complete
            .intents
            .iter()
            .filter(|i| matches!(i, CoreIntent::AudioStop { .. }))
            .count(),
        MAX_HISTORY_VOICES + 1
    );
    assert_eq!(
        c.state().tasks[&c.state().handles["other"]].state,
        TaskState::Running
    );
    assert!(c.state().fault.is_none());
    Core::restore(c.validated_program().clone(), c.snapshot(), "reading").unwrap();
}

#[test]
fn voice_associations_validate_identity_and_live_state_and_migrate_old_snapshots() {
    let mut c = plain_reading(program());
    let line = c.state().handles["line"];
    let voice = c.state().handles["voice"];
    for invalid in [vec![line], vec![u32::MAX], vec![voice, voice]] {
        let mut snapshot = c.snapshot();
        snapshot
            .tasks
            .get_mut(&line)
            .unwrap()
            .dialogue
            .as_mut()
            .unwrap()
            .reading
            .as_mut()
            .unwrap()
            .active_voices = invalid;
        assert!(Core::restore(c.validated_program().clone(), snapshot, "reading").is_err());
    }
    let mut old = serde_json::to_value(c.snapshot()).unwrap();
    old["tasks"][line.to_string()]["dialogue"]["reading"]
        .as_object_mut()
        .unwrap()
        .remove("active_voices");
    let mut restored = Core::restore(
        c.validated_program().clone(),
        serde_json::from_value(old).unwrap(),
        "reading",
    )
    .unwrap();
    let interaction = restored.dialogue().unwrap().1.interaction;
    stop_on_advance(&mut restored, interaction, 1);
    stop_on_advance(&mut restored, interaction, 2);
    assert_eq!(
        restored.state().tasks[&voice].end_reason,
        Some(TaskEndReason::CancelledByControl)
    );

    c.step(CoreInput::AudioEnded { task: voice }, 1000);
    assert!(c
        .dialogue()
        .unwrap()
        .1
        .reading
        .as_ref()
        .unwrap()
        .active_voices
        .is_empty());
    let mut bad = c.snapshot();
    bad.tasks
        .get_mut(&line)
        .unwrap()
        .dialogue
        .as_mut()
        .unwrap()
        .reading
        .as_mut()
        .unwrap()
        .active_voices = vec![voice];
    assert!(Core::restore(c.validated_program().clone(), bad, "reading").is_err());
    Core::restore(c.validated_program().clone(), c.snapshot(), "reading").unwrap();
}

#[test]
fn explicit_unvoiced_reading_and_looped_voice_are_not_stopped_by_legacy_policy() {
    for explicit_none in [false, true] {
        let mut p = program();
        let Effect::Audio { looped, .. } = &mut p.cues.get_mut("test").unwrap().effects[2].effect
        else {
            unreachable!()
        };
        *looped = true;
        let ops = &mut p
            .functions
            .get_mut("main")
            .unwrap()
            .blocks
            .get_mut("bind")
            .unwrap()
            .ops;
        if explicit_none {
            let Operation::DialogueVoice { voice, .. } = &mut ops[0].operation else {
                unreachable!()
            };
            *voice = None;
        } else {
            ops.clear();
        }
        let mut c = plain_reading(p);
        let interaction = c.dialogue().unwrap().1.interaction;
        stop_on_advance(&mut c, interaction, 1);
        stop_on_advance(&mut c, interaction, 2);
        assert_eq!(
            c.state().tasks[&c.state().handles["voice"]].state,
            if explicit_none {
                TaskState::Running
            } else {
                TaskState::Cancelled
            }
        );
        assert_eq!(
            c.state().tasks[&c.state().handles["other"]].state,
            TaskState::Running
        );
        assert!(c.state().fault.is_none());
    }
}
#[test]
fn history_records_only_bound_instances_in_order_and_deduplicates_rebinding() {
    let mut p = program();
    let ops = &mut p
        .functions
        .get_mut("main")
        .unwrap()
        .blocks
        .get_mut("bind")
        .unwrap()
        .ops;
    for (id, voice) in [
        ("same-again", "voice"),
        ("second-phrase", "other"),
        ("same-second", "other"),
    ] {
        ops.push(Op {
            id: id.into(),
            operation: Operation::DialogueVoice {
                task: "line".into(),
                voice: Some(voice.into()),
                wait: VoiceWaitPolicy::Parallel,
            },
        });
    }
    let c = start(p);
    let record = c.state().history.last().unwrap();
    assert_eq!(record.interaction, c.dialogue().unwrap().1.interaction);
    assert_eq!(
        record.voices.iter().map(|v| v.instance).collect::<Vec<_>>(),
        [c.state().handles["voice"], c.state().handles["other"]]
    );
    assert!(record
        .voices
        .iter()
        .all(|v| v.gain == 1. && c.program().asset_kind(&v.asset) == Some(AssetKind::Audio)));
    Core::restore(c.validated_program().clone(), c.snapshot(), "reading").unwrap();
}

#[test]
fn restored_dialogue_keeps_its_history_voice_association_at_the_next_gate_binding() {
    let mut p = program();
    p.functions
        .get_mut("main")
        .unwrap()
        .blocks
        .get_mut("continue")
        .unwrap()
        .ops
        .insert(
            0,
            Op {
                id: "bind-second-phrase".into(),
                operation: Operation::DialogueVoice {
                    task: "line".into(),
                    voice: Some("other".into()),
                    wait: VoiceWaitPolicy::Parallel,
                },
            },
        );
    let original = start(p);
    let interaction = original.dialogue().unwrap().1.interaction;
    let voice = original.state().handles["voice"];
    let other = original.state().handles["other"];
    let mut restored = Core::restore(
        original.validated_program().clone(),
        original.snapshot(),
        "reading",
    )
    .unwrap();
    let fresh = restored.dialogue().unwrap().1.interaction;
    assert_ne!(
        fresh, interaction,
        "old host input must remain stale after restore"
    );
    assert_eq!(restored.state().history.last().unwrap().interaction, fresh);
    restored.step(CoreInput::AudioEnded { task: voice }, 1000);
    assert!(restored.state().fault.is_none());
    assert_eq!(
        restored
            .state()
            .history
            .last()
            .unwrap()
            .voices
            .iter()
            .map(|v| v.instance)
            .collect::<Vec<_>>(),
        [voice, other]
    );
}

#[test]
fn a_history_voice_budget_evicts_the_record_without_faulting_authored_reading() {
    let mut p = program();
    let Effect::Audio { asset, .. } = &p.cues["test"].effects[1].effect else {
        unreachable!()
    };
    let asset = asset.clone();
    for i in 0..MAX_HISTORY_VOICES {
        let name = format!("voice-{i}");
        p.cues.get_mut("test").unwrap().effects.push(EffectDef {
            id: name.clone(),
            scope: Scope::Session,
            effect: Effect::Audio {
                asset: asset.clone(),
                bus: AudioBus::Voice,
                looped: false,
                loop_region: None,
                gain: 1.,
            },
        });
        p.functions
            .get_mut("main")
            .unwrap()
            .blocks
            .get_mut("bind")
            .unwrap()
            .ops
            .push(Op {
                id: format!("bind-{i}"),
                operation: Operation::DialogueVoice {
                    task: "line".into(),
                    voice: Some(name),
                    wait: VoiceWaitPolicy::Parallel,
                },
            });
    }
    let core = start(p);
    assert!(core.state().fault.is_none());
    assert!(
        core.state().history.is_empty(),
        "oversized rows are evicted whole"
    );
    assert_eq!(
        core.dialogue().unwrap().1.reading.as_ref().unwrap().voice,
        Some(core.state().handles[&format!("voice-{}", MAX_HISTORY_VOICES - 1)])
    );
}

#[test]
fn history_voice_restore_rejects_forged_resources_identities_gains_and_limits() {
    let c = start(program());
    for bad in 0..7 {
        let mut snapshot = c.snapshot();
        let next_id = snapshot.next_id;
        let entry = snapshot.history.last_mut().unwrap();
        match bad {
            0 => entry.voices[0].asset = "font.reader".into(),
            1 => entry.voices[0].asset = "missing".into(),
            2 => entry.voices[0].gain = 4.1,
            3 => entry.voices[0].instance = next_id,
            4 => entry.interaction = 0,
            5 => entry.voices.push(entry.voices[0].clone()),
            _ => entry.voices = vec![entry.voices[0].clone(); MAX_HISTORY_VOICES + 1],
        }
        assert!(
            Core::restore(c.validated_program().clone(), snapshot, "reading").is_err(),
            "{bad}"
        );
    }
    let mut old = serde_json::to_value(c.snapshot()).unwrap();
    for entry in old["history"].as_array_mut().unwrap() {
        entry.as_object_mut().unwrap().remove("voices");
        entry.as_object_mut().unwrap().remove("interaction");
        entry.as_object_mut().unwrap().remove("choice");
    }
    let restored = Core::restore(
        c.validated_program().clone(),
        serde_json::from_value(old).unwrap(),
        "reading",
    )
    .unwrap();
    assert!(restored
        .state()
        .history
        .iter()
        .all(|record| record.voices.is_empty()));
}
#[test]
fn voice_can_bind_at_gate_without_consuming_it_and_restore_pins_the_instance() {
    let mut c = start(program());
    let voice = c.state().handles["voice"];
    let d = c.dialogue().unwrap().1;
    assert!(d.at_gate);
    assert_eq!(d.reading.as_ref().unwrap().voice, Some(voice));
    c.step(
        CoreInput::Advance {
            interaction: d.interaction,
            sequence: 2,
        },
        1000,
    );
    assert!(c.dialogue().unwrap().1.at_gate);
    let mut restored =
        Core::restore(c.validated_program().clone(), c.snapshot(), "reading").unwrap();
    assert_eq!(
        restored
            .dialogue()
            .unwrap()
            .1
            .reading
            .as_ref()
            .unwrap()
            .voice,
        Some(voice)
    );
    let other = restored.state().handles["other"];
    restored.step(CoreInput::AudioEnded { task: other }, 1000);
    assert!(restored.dialogue().unwrap().1.at_gate);
    restored.step(CoreInput::AudioEnded { task: voice }, 1000);
    assert!(!restored.dialogue().unwrap().1.at_gate);
    assert_eq!(
        restored
            .dialogue()
            .unwrap()
            .1
            .reading
            .as_ref()
            .unwrap()
            .voice,
        Some(voice)
    );
}
#[test]
fn invalid_bindings_and_missing_capability_fail_closed() {
    let mut p = program();
    p.requires.retain(|c| c != "text.voice-binding.v1");
    assert_eq!(ValidatedProgram::new(p).unwrap_err().code, "E_CAPABILITY");
    for voice in ["missing", "line"] {
        let mut p = program();
        if let Operation::DialogueVoice { voice: v, .. } = &mut p
            .functions
            .get_mut("main")
            .unwrap()
            .blocks
            .get_mut("bind")
            .unwrap()
            .ops[0]
            .operation
        {
            *v = Some(voice.into());
        }
        assert_eq!(
            ValidatedProgram::new(p).unwrap_err().code,
            "E_VOICE_BINDING"
        );
    }
    let mut p = program();
    if let Effect::Audio { bus, .. } = &mut p.cues.get_mut("test").unwrap().effects[1].effect {
        *bus = AudioBus::Bgm;
    }
    assert_eq!(
        ValidatedProgram::new(p).unwrap_err().code,
        "E_VOICE_BINDING"
    );
}
#[test]
fn corrupted_binding_cannot_reference_a_dialogue_missing_task_or_zero_revision() {
    let c = start(program());
    let line = c.state().handles["line"];
    for bad in 0..3 {
        let mut snapshot = c.snapshot();
        let binding = snapshot
            .tasks
            .get_mut(&line)
            .unwrap()
            .dialogue
            .as_mut()
            .unwrap()
            .reading
            .as_mut()
            .unwrap();
        match bad {
            0 => binding.voice = Some(line),
            1 => binding.voice = Some(u32::MAX),
            _ => binding.revision = 0,
        }
        assert!(Core::restore(c.validated_program().clone(), snapshot, "reading").is_err());
    }
}
#[test]
fn repeated_symbolic_voice_handle_does_not_retarget_an_existing_binding() {
    let mut p = program();
    let audio = p.cues["test"].effects[1].clone();
    p.cues.insert(
        "replacement".into(),
        Cue {
            effects: vec![audio],
        },
    );
    let f = p.functions.get_mut("main").unwrap();
    f.blocks.get_mut("bind").unwrap().terminator =
        serde_json::from_value(json!({"type":"activate","cue":"replacement","next":"held"}))
            .unwrap();
    f.blocks.insert("held".into(), serde_json::from_value(json!({"terminator":{"type":"await","conditions":[{"task":"line","milestone":{"type":"finished"}}],"next":"done","on_cancelled":"done","on_failed":"done"}})).unwrap());
    let mut c = start(p);
    let old = c.state().handles["voice"];
    c.step(CoreInput::AudioEnded { task: old }, 1000);
    let activation = c.state().pending.as_ref().unwrap().id;
    c.step(CoreInput::Prepared { activation }, 1000);
    let new = c.state().handles["voice"];
    assert_ne!(old, new);
    assert!(c.state().tasks.contains_key(&old));
    assert_eq!(
        c.dialogue().unwrap().1.reading.as_ref().unwrap().voice,
        Some(old)
    );
    c.step(CoreInput::AudioEnded { task: old }, 1000);
    assert_eq!(c.state().tasks[&new].state, TaskState::Running);
    assert!(Core::restore(c.validated_program().clone(), c.snapshot(), "reading").is_ok());
}

#[test]
fn uncommitted_dialogue_cannot_restore_a_forged_reading_binding() {
    let mut c = Core::new(
        ValidatedProgram::new(program()).unwrap(),
        "reading".into(),
        "en".into(),
    )
    .unwrap();
    c.step(CoreInput::None, 1000);
    let mut snapshot = c.snapshot();
    snapshot
        .pending
        .as_mut()
        .unwrap()
        .dialogues
        .get_mut("line")
        .unwrap()
        .reading = Some(DialogueReading {
        voice: None,
        active_voices: vec![],
        wait: VoiceWaitPolicy::Parallel,
        revision: 1,
    });
    assert!(Core::restore(c.validated_program().clone(), snapshot, "reading").is_err());
}

#[test]
fn reading_speed_is_frozen_per_dialogue_and_survives_restore() {
    let validated = ValidatedProgram::new(program()).unwrap();
    let mut c = Core::new(validated.clone(), "reading".into(), "en".into()).unwrap();
    assert!(c.set_text_speed(f32::NAN).is_err());
    assert!(c.set_text_speed(0.).is_err());
    c.set_text_speed(2.).unwrap();
    c.step(CoreInput::None, 1000);
    let activation = c.state().pending.as_ref().unwrap().id;
    assert_eq!(
        c.state().pending.as_ref().unwrap().dialogues["line"].reveal_interval_us,
        Some(Micros(500))
    );
    c.set_text_speed(0.25).unwrap();
    c.step(CoreInput::Prepared { activation }, 1000);
    c.step(CoreInput::Time { delta_us: 499 }, 1000);
    assert_eq!(c.dialogue().unwrap().1.visible_text(), "");
    let mut restored = Core::restore(validated.clone(), c.snapshot(), "reading").unwrap();
    restored.set_text_speed(4.).unwrap();
    restored.step(CoreInput::Time { delta_us: 1 }, 1000);
    assert!(!restored.dialogue().unwrap().1.visible_text().is_empty());
    assert_eq!(
        restored.dialogue().unwrap().1.reveal_interval_us,
        Some(Micros(500))
    );
    let line = restored.state().handles["line"];
    let mut invalid = restored.snapshot();
    invalid
        .tasks
        .get_mut(&line)
        .unwrap()
        .dialogue
        .as_mut()
        .unwrap()
        .reveal_interval_us = Some(Micros(9000));
    assert!(Core::restore(validated.clone(), invalid, "reading").is_err());
    let mut legacy = c.snapshot();
    legacy
        .tasks
        .get_mut(&line)
        .unwrap()
        .dialogue
        .as_mut()
        .unwrap()
        .reveal_interval_us = None;
    assert!(Core::restore(validated, legacy, "reading").is_ok());
}

#[test]
fn sampled_voice_timer_requires_known_duration_and_preserves_policy_on_restore() {
    let mut p = program();
    let Operation::DialogueVoice { wait, .. } = &mut p
        .functions
        .get_mut("main")
        .unwrap()
        .blocks
        .get_mut("bind")
        .unwrap()
        .ops[0]
        .operation
    else {
        panic!()
    };
    *wait = VoiceWaitPolicy::SampledRemaining;
    assert_eq!(
        ValidatedProgram::new(p.clone()).unwrap_err().code,
        "E_CAPABILITY"
    );
    p.requires.push("text.voice-timer.v1".into());
    assert_eq!(
        ValidatedProgram::new(p.clone()).unwrap_err().code,
        "E_VOICE_DURATION"
    );
    for asset in p.assets.values_mut().filter(|a| a.kind == AssetKind::Audio) {
        asset.duration_us = Micros(1_000_000);
    }
    let validated = ValidatedProgram::new(p).unwrap();
    let mut core = Core::new(validated.clone(), "reading".into(), "en".into()).unwrap();
    core.step(CoreInput::None, 1000);
    let id = core.state().pending.as_ref().unwrap().id;
    core.step(CoreInput::Prepared { activation: id }, 1000);
    let interaction = core.dialogue().unwrap().1.interaction;
    core.step(
        CoreInput::Advance {
            interaction,
            sequence: 1,
        },
        1000,
    );
    assert_eq!(
        core.dialogue().unwrap().1.reading.as_ref().unwrap().wait,
        VoiceWaitPolicy::SampledRemaining
    );
    let restored = Core::restore(validated, core.snapshot(), "reading").unwrap();
    assert_eq!(
        restored
            .dialogue()
            .unwrap()
            .1
            .reading
            .as_ref()
            .unwrap()
            .wait,
        VoiceWaitPolicy::SampledRemaining
    );
}

#[test]
fn restored_sampled_timer_cannot_bypass_capability_checks() {
    let core = start(program());
    let mut snapshot = core.snapshot();
    let task = snapshot.handles["line"];
    snapshot
        .tasks
        .get_mut(&task)
        .unwrap()
        .dialogue
        .as_mut()
        .unwrap()
        .reading
        .as_mut()
        .unwrap()
        .wait = VoiceWaitPolicy::SampledRemaining;
    assert!(Core::restore(core.validated_program().clone(), snapshot, "reading").is_err());
}
