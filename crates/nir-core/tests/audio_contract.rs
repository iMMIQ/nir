use nir_core::*;
use nir_format::*;
use serde_json::json;

fn audio_program(gain: f32) -> Program {
    let mut p: Program = serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    let asset = p
        .assets
        .iter()
        .find(|(_, a)| a.kind == AssetKind::Audio)
        .unwrap()
        .0
        .clone();
    p.requires.push("audio.gain.v1".into());
    p.cues.insert("audio-test".into(), serde_json::from_value(json!({"effects":[
        {"id":"sample","scope":"session","effect":{"type":"audio","asset":asset,"bus":"voice","gain":gain}},
        {"id":"hold","scope":"session","effect":{"type":"delay","duration_us":"1000000"}}
    ]})).unwrap());
    let f = p.functions.get_mut("main").unwrap();
    f.entry = "audio-test".into();
    f.blocks.insert(
        "audio-test".into(),
        serde_json::from_value(
            json!({"terminator":{"type":"activate","cue":"audio-test","next":"hold"}}),
        )
        .unwrap(),
    );
    f.blocks.insert("hold".into(), serde_json::from_value(json!({"terminator":{"type":"await","conditions":[{"task":"hold","milestone":{"type":"finished"}}],"next":"done","on_cancelled":"done","on_failed":"done"}})).unwrap());
    f.blocks.insert(
        "done".into(),
        serde_json::from_value(json!({"terminator":{"type":"end","outcome":"done"}})).unwrap(),
    );
    p
}
fn start(p: Program) -> (Core, Vec<CoreIntent>) {
    let mut c = Core::new(
        ValidatedProgram::new(p).unwrap(),
        "audio-test".into(),
        "en".into(),
    )
    .unwrap();
    c.step(CoreInput::None, 1000);
    let activation = c.state().pending.as_ref().unwrap().id;
    let step = c.step(CoreInput::Prepared { activation }, 1000);
    assert!(c.state().fault.is_none(), "{:?}", c.state().fault);
    (c, step.intents)
}
#[test]
fn gain_is_playback_metadata_and_natural_end_is_not_rewritten_by_late_events() {
    let (mut c, intents) = start(audio_program(1.5));
    assert!(intents
        .iter()
        .any(|i| matches!(i, CoreIntent::AudioStart { gain, .. } if *gain == 1.5)));
    let id = c.state().handles["sample"];
    c.step(CoreInput::AudioEnded { task: id }, 100);
    assert_eq!(
        c.state().tasks[&id].end_reason,
        Some(TaskEndReason::NaturalEnd)
    );
    c.step(
        CoreInput::TaskFailed {
            task: id,
            message: "late".into(),
        },
        100,
    );
    c.step(CoreInput::AudioEnded { task: id }, 100);
    assert_eq!(
        c.state().tasks[&id].end_reason,
        Some(TaskEndReason::NaturalEnd)
    );
    let restored =
        Core::restore(c.validated_program().clone(), c.snapshot(), "audio-test").unwrap();
    assert_eq!(
        restored.state().tasks[&id].end_reason,
        Some(TaskEndReason::NaturalEnd)
    );
}
#[test]
fn explicit_cancel_and_finish_have_distinct_reasons() {
    for (action, reason, state) in [
        (
            "cancel",
            TaskEndReason::CancelledByControl,
            TaskState::Cancelled,
        ),
        (
            "finish",
            TaskEndReason::FinishedByControl,
            TaskState::Finished,
        ),
    ] {
        let mut p = audio_program(0.8);
        p.functions.get_mut("main").unwrap().blocks.get_mut("hold").unwrap().ops.push(
            serde_json::from_value(json!({"id":"control-sample","operation":{"type":"task_control","task":"sample","action":action}})).unwrap());
        let (mut c, _) = start(p);
        let id = c.state().handles["sample"];
        assert_eq!(c.state().tasks[&id].end_reason, Some(reason));
        assert_eq!(c.state().tasks[&id].state, state);
        c.step(CoreInput::AudioEnded { task: id }, 100);
        assert_eq!(c.state().tasks[&id].end_reason, Some(reason));
    }
}
#[test]
fn gain_requires_capability_and_valid_range() {
    for gain in [-0.1, 4.1, f32::INFINITY, f32::NAN] {
        let mut p = audio_program(1.);
        if let Effect::Audio { gain: g, .. } =
            &mut p.cues.get_mut("audio-test").unwrap().effects[0].effect
        {
            *g = gain;
        }
        assert_eq!(ValidatedProgram::new(p).unwrap_err().code, "E_AUDIO_GAIN");
    }
    let mut p = audio_program(1.5);
    p.requires.retain(|c| c != "audio.gain.v1");
    assert_eq!(ValidatedProgram::new(p).unwrap_err().code, "E_CAPABILITY");
}
#[test]
fn restore_rejects_forged_reason_and_older_snapshot_version() {
    let (c, _) = start(audio_program(1.));
    let id = c.state().handles["sample"];
    let mut s = c.snapshot();
    s.tasks.get_mut(&id).unwrap().end_reason = Some(TaskEndReason::NaturalEnd);
    assert!(Core::restore(c.validated_program().clone(), s, "audio-test").is_err());
    let mut s = c.snapshot();
    s.format = 1;
    assert!(Core::restore(c.validated_program().clone(), s, "audio-test").is_err());
}

fn stop_program(duration: u64) -> Program {
    let mut p = audio_program(1.5);
    p.requires.push("audio.stop.v1".into());
    p.cues.get_mut("audio-test").unwrap().effects.push(serde_json::from_value(json!({
        "id":"fade","scope":"session","effect":{"type":"audio_stop","target":"sample","duration_us":duration.to_string()}
    })).unwrap());
    p
}
#[test]
fn fade_stop_keeps_playback_running_then_cancels_it_and_restores_remaining_segment() {
    let (mut c, commands) = start(stop_program(500_000));
    let sound = c.state().handles["sample"];
    let stop = c.state().handles["fade"];
    assert!(commands.iter().any(|i| matches!(i, CoreIntent::AudioEnvelope { task, from, to, duration_us, .. } if *task == sound && *from == 1. && *to == 0. && duration_us.0 == 500_000)));
    c.step(CoreInput::Time { delta_us: 250_000 }, 1000);
    assert_eq!(c.state().tasks[&sound].state, TaskState::Running);
    assert_eq!(c.audio_envelope(sound), (0.5, 0., Micros(250_000)));
    let mut restored =
        Core::restore(c.validated_program().clone(), c.snapshot(), "audio-test").unwrap();
    assert_eq!(restored.audio_envelope(sound), c.audio_envelope(sound));
    let step = restored.step(CoreInput::Time { delta_us: 250_000 }, 1000);
    assert!(step
        .intents
        .iter()
        .any(|i| matches!(i, CoreIntent::AudioStop { task } if *task == sound)));
    assert_eq!(restored.state().tasks[&stop].state, TaskState::Finished);
    assert_eq!(restored.state().tasks[&sound].state, TaskState::Cancelled);
    restored.step(CoreInput::AudioEnded { task: sound }, 100);
    assert_ne!(
        restored.state().tasks[&sound].end_reason,
        Some(TaskEndReason::NaturalEnd)
    );
}
#[test]
fn early_natural_end_finishes_stop_and_zero_duration_is_atomic() {
    let (mut c, _) = start(stop_program(500_000));
    let sound = c.state().handles["sample"];
    let stop = c.state().handles["fade"];
    c.step(CoreInput::AudioEnded { task: sound }, 1000);
    assert_eq!(
        c.state().tasks[&sound].end_reason,
        Some(TaskEndReason::NaturalEnd)
    );
    assert_eq!(c.state().tasks[&stop].state, TaskState::Finished);
    Core::restore(c.validated_program().clone(), c.snapshot(), "audio-test").unwrap();
    let (c, _) = start(stop_program(0));
    assert_eq!(
        c.state().tasks[&c.state().handles["fade"]].state,
        TaskState::Finished
    );
    assert_eq!(
        c.state().tasks[&c.state().handles["sample"]].state,
        TaskState::Cancelled
    );
}
#[test]
fn cancelling_fade_commits_current_envelope_and_does_not_stop_audio() {
    let mut p = stop_program(500_000);
    p.cues.get_mut("audio-test").unwrap().effects.push(serde_json::from_value(json!({"id":"timer","scope":"session","effect":{"type":"delay","duration_us":"250000"}})).unwrap());
    let f = p.functions.get_mut("main").unwrap();
    let mut wait = f.blocks["hold"].clone();
    if let Terminator::Await {
        conditions, next, ..
    } = &mut wait.terminator
    {
        conditions[0].task = "timer".into();
        *next = "cancel-fade".into();
    }
    f.blocks
        .insert("wait-rest".into(), f.blocks["hold"].clone());
    f.blocks.insert("hold".into(), wait);
    f.blocks.insert("cancel-fade".into(), serde_json::from_value(json!({"ops":[{"id":"cancel-fade","operation":{"type":"task_control","task":"fade","action":"cancel"}}],"terminator":{"type":"goto","target":"wait-rest"}})).unwrap());
    let (mut c, _) = start(p);
    let sound = c.state().handles["sample"];
    let step = c.step(CoreInput::Time { delta_us: 250_000 }, 1000);
    assert_eq!(c.state().tasks[&sound].state, TaskState::Running);
    assert_eq!(c.audio_envelope(sound), (0.5, 0.5, Micros(0)));
    assert!(step.intents.iter().any(|i| matches!(i, CoreIntent::AudioEnvelope { from, to, duration_us, .. } if *from == 0.5 && *to == 0.5 && duration_us.0 == 0)));
    Core::restore(c.validated_program().clone(), c.snapshot(), "audio-test").unwrap();
}
#[test]
fn stop_rejects_invalid_targets_and_corrupt_saved_ownership() {
    let mut p = stop_program(500_000);
    if let Effect::AudioStop { target, .. } =
        &mut p.cues.get_mut("audio-test").unwrap().effects[2].effect
    {
        *target = "hold".into();
    }
    assert_eq!(ValidatedProgram::new(p).unwrap_err().code, "E_AUDIO_STOP");
    let (c, _) = start(stop_program(500_000));
    let mut s = c.snapshot();
    let id = s.handles["fade"];
    s.tasks.get_mut(&id).unwrap().target_task = Some(id);
    assert!(Core::restore(c.validated_program().clone(), s, "audio-test").is_err());
}

#[test]
fn audio_only_wait_keeps_offset_clock_alive_for_restore() {
    let mut p = audio_program(1.);
    p.cues
        .get_mut("audio-test")
        .unwrap()
        .effects
        .retain(|e| e.id == "sample");
    p.functions.get_mut("main").unwrap().blocks.get_mut("hold").unwrap().terminator = serde_json::from_value(json!({"type":"await","conditions":[{"task":"sample","milestone":{"type":"finished"}}],"next":"done","on_cancelled":"done","on_failed":"done"})).unwrap();
    let (mut c, _) = start(p);
    assert!(
        c.needs_clock(),
        "active audio must accumulate resumable offset even without animation or text reveal"
    );
    c.step(
        CoreInput::Time {
            delta_us: 2_000_000,
        },
        1000,
    );
    let id = c.state().handles["sample"];
    assert_eq!(c.state().tasks[&id].elapsed_us, Micros(2_000_000));
    let restored =
        Core::restore(c.validated_program().clone(), c.snapshot(), "audio-test").unwrap();
    assert_eq!(restored.state().tasks[&id].elapsed_us, Micros(2_000_000));
    c.step(CoreInput::AudioEnded { task: id }, 1000);
    assert!(!c.needs_clock());
}

#[test]
fn device_positions_are_snapshot_metadata_without_story_execution() {
    let (mut c, _) = start(audio_program(1.));
    let id = c.state().handles["sample"];
    let before = c.snapshot();
    c.observe_audio_positions(&[AudioPosition {
        envelope: None,
        task: id,
        position_us: Micros(800_000),
    }])
    .unwrap();
    let mut expected = before.clone();
    expected.tasks.get_mut(&id).unwrap().audio_position_us = Some(Micros(800_000));
    assert_eq!(
        serde_json::to_value(c.snapshot()).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    let restored =
        Core::restore(c.validated_program().clone(), c.snapshot(), "audio-test").unwrap();
    assert_eq!(
        restored.state().tasks[&id].audio_position_us,
        Some(Micros(800_000))
    );
    assert!(Core::restore(c.validated_program().clone(), before, "audio-test").is_ok());
    let duplicate = [
        AudioPosition {
            envelope: None,
            task: id,
            position_us: Micros(1),
        },
        AudioPosition {
            envelope: None,
            task: id,
            position_us: Micros(2),
        },
    ];
    assert!(c.observe_audio_positions(&duplicate).is_err());
    assert_eq!(
        c.state().tasks[&id].audio_position_us,
        Some(Micros(800_000))
    );
    let hold = c.state().handles["hold"];
    c.observe_audio_positions(&[
        AudioPosition {
            envelope: None,
            task: hold,
            position_us: Micros(5),
        },
        AudioPosition {
            envelope: None,
            task: u32::MAX,
            position_us: Micros(5),
        },
    ])
    .unwrap();
    assert_eq!(c.state().tasks[&hold].audio_position_us, None);
    let mut bad = c.snapshot();
    bad.tasks.get_mut(&hold).unwrap().audio_position_us = Some(Micros(5));
    assert!(Core::restore(c.validated_program().clone(), bad, "audio-test").is_err());
    c.step(CoreInput::AudioEnded { task: id }, 100);
    c.observe_audio_positions(&[AudioPosition {
        envelope: None,
        task: id,
        position_us: Micros(9),
    }])
    .unwrap();
    assert_eq!(
        c.state().tasks[&id].audio_position_us,
        Some(Micros(800_000))
    );
}

#[test]
fn device_envelope_checkpoint_restores_without_advancing_story_and_rejects_forgery() {
    let (mut c, _) = start(stop_program(1_000_000));
    let sound = c.state().handles["sample"];
    let owner = c.state().handles["fade"];
    c.step(CoreInput::Time { delta_us: 100_000 }, 1000);
    let before = c.snapshot();
    let observation = AudioPosition {
        task: sound,
        position_us: Micros(800_000),
        envelope: Some(AudioEnvelopePosition {
            owner,
            elapsed_us: Micros(800_000),
        }),
    };
    c.observe_audio_positions(std::slice::from_ref(&observation))
        .unwrap();
    let mut expected = before.clone();
    expected.tasks.get_mut(&sound).unwrap().audio_position_us = Some(Micros(800_000));
    expected
        .tasks
        .get_mut(&owner)
        .unwrap()
        .audio_device_elapsed_us = Some(Micros(800_000));
    assert_eq!(
        serde_json::to_value(c.snapshot()).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
    let (gain, to, remaining) = c.audio_envelope(sound);
    assert!((gain - 0.2).abs() < 0.00001);
    assert_eq!(to, 0.);
    assert_eq!(remaining, Micros(200_000));
    let restored =
        Core::restore(c.validated_program().clone(), c.snapshot(), "audio-test").unwrap();
    assert_eq!(restored.audio_envelope(sound), c.audio_envelope(sound));
    assert_eq!(
        restored.audio_envelope_checkpoint(sound),
        (Some(owner), Micros(800_000))
    );
    // An out-of-order observation cannot rewind the active device ramp.
    let mut older = observation.clone();
    older.envelope.as_mut().unwrap().elapsed_us = Micros(400_000);
    c.observe_audio_positions(&[older]).unwrap();
    assert_eq!(c.audio_envelope(sound), restored.audio_envelope(sound));
    // An invalid batch is rejected before even the audio position is changed.
    let mut invalid = observation;
    invalid.position_us = Micros(123);
    invalid.envelope.as_mut().unwrap().elapsed_us = Micros(1_000_001);
    assert!(c.observe_audio_positions(&[invalid]).is_err());
    assert_eq!(
        c.state().tasks[&sound].audio_position_us,
        Some(Micros(800_000))
    );
    let mut bad = expected.clone();
    bad.tasks.get_mut(&owner).unwrap().audio_device_elapsed_us = Some(Micros(1_000_001));
    assert!(Core::restore(c.validated_program().clone(), bad, "audio-test").is_err());
    let mut bad = expected;
    bad.tasks.get_mut(&sound).unwrap().audio_device_elapsed_us = Some(Micros(1));
    assert!(Core::restore(c.validated_program().clone(), bad, "audio-test").is_err());
    // Older snapshots with no device checkpoint keep their original logical envelope.
    let older = Core::restore(c.validated_program().clone(), before, "audio-test").unwrap();
    assert!((older.audio_envelope(sound).0 - 0.9).abs() < 0.00001);
}

#[test]
fn cancelling_observed_fade_commits_device_value_and_late_owner_cannot_change_it() {
    let mut p = stop_program(1_000_000);
    p.functions.get_mut("main").unwrap().blocks.insert("cancel-observed".into(),serde_json::from_value(json!({"ops":[{"id":"cancel","operation":{"type":"task_control","task":"fade","action":"cancel"}}],"terminator":{"type":"goto","target":"hold"}})).unwrap());
    p.cues.get_mut("audio-test").unwrap().effects.push(serde_json::from_value(json!({"id":"timer","scope":"session","effect":{"type":"delay","duration_us":"250000"}})).unwrap());
    let f = p.functions.get_mut("main").unwrap();
    f.blocks
        .insert("wait-rest".into(), f.blocks["hold"].clone());
    if let Terminator::Await {
        conditions, next, ..
    } = &mut f.blocks.get_mut("hold").unwrap().terminator
    {
        conditions[0].task = "timer".into();
        *next = "cancel-observed".into();
    }
    f.blocks.get_mut("cancel-observed").unwrap().terminator = Terminator::Goto {
        target: "wait-rest".into(),
    };
    let (mut c, _) = start(p);
    let sound = c.state().handles["sample"];
    let owner = c.state().handles["fade"];
    let observation = AudioPosition {
        task: sound,
        position_us: Micros(750_000),
        envelope: Some(AudioEnvelopePosition {
            owner,
            elapsed_us: Micros(750_000),
        }),
    };
    c.observe_audio_positions(std::slice::from_ref(&observation))
        .unwrap();
    let result = c.step(CoreInput::Time { delta_us: 250_000 }, 1000);
    assert!(result.intents.iter().any(|i|matches!(i,CoreIntent::AudioEnvelope {owner:None,from,to,duration_us,..} if *from==0.25 && *to==0.25 && duration_us.0==0)));
    assert_eq!(c.audio_envelope(sound), (0.25, 0.25, Micros(0)));
    let mut late = observation;
    late.envelope.as_mut().unwrap().elapsed_us = Micros(900_000);
    c.observe_audio_positions(&[late]).unwrap();
    assert_eq!(c.audio_envelope(sound), (0.25, 0.25, Micros(0)));
}
