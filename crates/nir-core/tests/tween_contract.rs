use nir_core::*;
use nir_format::*;
use serde_json::{json, Value as Json};

fn program(effects: Json) -> Program {
    let mut p: Program = serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    p.requires.push("tween.target.v1".into());
    p.cues.insert(
        "test".into(),
        serde_json::from_value(json!({"effects":effects})).unwrap(),
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
    f.blocks.insert("hold".into(), serde_json::from_value(json!({"terminator":{"type":"await","conditions":[{"task":"hold","milestone":{"type":"finished"}}],"next":"done","on_cancelled":"done","on_failed":"done"}})).unwrap());
    f.blocks.insert(
        "done".into(),
        serde_json::from_value(json!({"terminator":{"type":"end","outcome":"done"}})).unwrap(),
    );
    p
}
fn fade(property: &str, to: f32) -> Json {
    json!({"id":"fade","scope":"session","effect":{"type":"tween","target":{"type":"dialogue_root","property":property},"to":to,"duration_us":"1000"}})
}
fn hold() -> Json {
    json!({"id":"hold","scope":"session","effect":{"type":"delay","duration_us":"10000"}})
}
fn start(p: Program) -> Core {
    let mut c = Core::new(
        ValidatedProgram::new(p).unwrap(),
        "tween".into(),
        "en".into(),
    )
    .unwrap();
    c.step(CoreInput::None, 1000);
    prepare(&mut c);
    c
}
fn prepare(c: &mut Core) {
    let activation = c.state().pending.as_ref().unwrap().id;
    c.step(CoreInput::Prepared { activation }, 1000);
    assert!(c.state().fault.is_none(), "{:?}", c.state().fault);
}
fn tick(c: &mut Core, delta_us: u64) {
    c.step(CoreInput::Time { delta_us }, 1000);
    assert!(c.state().fault.is_none(), "{:?}", c.state().fault);
}
#[test]
fn leaf_sprite_transform_is_capability_gated_and_survives_cold_restore() {
    let mut p = program(json!([
        {"id":"stage","scope":"session","effect":{"type":"stage_present","scene":"station","duration_us":"0"}},
        hold()
    ]));
    let node = &mut p.scenes.get_mut("station").unwrap()[0];
    node.width = 32.;
    node.height = 24.;
    node.sprite_transform = Some(SpriteTransform {
        origin: [24., 0.],
        basis_x: [0., 1.],
        basis_y: [-1., 0.],
    });
    let mut missing = p.clone();
    missing
        .requires
        .retain(|cap| cap != "stage.sprite-transform.v1");
    assert_eq!(
        ValidatedProgram::new(missing).unwrap_err().code,
        "E_CAPABILITY"
    );
    p.requires.push("stage.sprite-transform.v1".into());
    let mut invalid = p.clone();
    invalid.scenes.get_mut("station").unwrap()[0].clip = Some([0., 0., 32., 24.]);
    assert_eq!(ValidatedProgram::new(invalid).unwrap_err().code, "E_VISUAL");
    let mut invalid = p.clone();
    let mut child = invalid.scenes["station"][0].clone();
    child.id = "child".into();
    child.parent = Some(invalid.scenes["station"][0].id.clone());
    child.sprite_transform = None;
    invalid.scenes.get_mut("station").unwrap().push(child);
    assert_eq!(ValidatedProgram::new(invalid).unwrap_err().code, "E_VISUAL");
    let c = start(p);
    let cold: Snapshot =
        serde_json::from_slice(&serde_json::to_vec(&c.snapshot()).unwrap()).unwrap();
    let restored = Core::restore(c.validated_program().clone(), cold, "tween").unwrap();
    assert_eq!(c.sample_scene(), restored.sample_scene());
    let mut bad = c.snapshot();
    bad.scene[0].sprite_transform.as_mut().unwrap().basis_x[0] = f32::NAN;
    assert!(Core::restore(c.validated_program().clone(), bad, "tween").is_err());
}
#[test]
fn reserved_window_flip_waits_for_scene_preparation_and_restores_its_fade() {
    let mut p = program(json!([
        {"id":"stage","scope":"session","effect":{"type":"stage_present","scene":"station",
            "duration_us":"1000000","dialogue_visible":false}},
        {"id":"hold","scope":"session","effect":{"type":"delay","duration_us":"2000000"}}
    ]));
    p.requires.extend([
        "stage.window-flip.v1".into(),
        "text.window-transition.v1".into(),
    ]);
    let mut missing = p.clone();
    missing.requires.retain(|cap| cap != "stage.window-flip.v1");
    assert_eq!(
        ValidatedProgram::new(missing).unwrap_err().code,
        "E_CAPABILITY"
    );
    let mut c = Core::new(
        ValidatedProgram::new(p).unwrap(),
        "tween".into(),
        "en".into(),
    )
    .unwrap();
    c.step(CoreInput::None, 1000);
    assert!(c.state().pending.is_some());
    c.step(
        CoreInput::Time {
            delta_us: 1_000_000,
        },
        1000,
    );
    assert!(!c.state().dialogue_hidden);
    assert!(c.window_reveal().is_none());
    prepare(&mut c);
    assert!(!c.state().dialogue_hidden);
    assert_eq!(c.window_reveal().unwrap().2, 0.);
    tick(&mut c, 400_000);
    assert_eq!(c.window_reveal().unwrap().2, 0.4);
    assert_eq!(c.transition().unwrap().1, 0.4);
    let cold: Snapshot =
        serde_json::from_slice(&serde_json::to_vec(&c.snapshot()).unwrap()).unwrap();
    let mut restored = Core::restore(c.validated_program().clone(), cold, "tween").unwrap();
    assert_eq!(restored.window_reveal().unwrap().2, 0.4);
    tick(&mut restored, 600_000);
    assert!(restored.window_reveal().is_none());
    assert!(restored.transition().is_none());
    assert!(restored.state().dialogue_hidden);
}
#[test]
fn sprite_wave_composes_with_motion_restores_and_keeps_music_running() {
    let mut p = program(json!([hold()]));
    p.requires.push("stage.sprite-wave.v1".into());
    let node = p.scenes["station"][0].id.clone();
    p.scenes.get_mut("station").unwrap()[0].y = 20.;
    let mut other = p.scenes["station"][0].clone();
    other.id = "wave.other".into();
    p.scenes.get_mut("station").unwrap().push(other);
    p.cues.get_mut("test").unwrap().effects = serde_json::from_value(json!([
        {"id":"stage","scope":"session","effect":{"type":"stage_present","scene":"station","duration_us":"0"}},
        {"id":"music","scope":"session","effect":{"type":"audio","asset":"audio.bgm","bus":"bgm","looped":true}},
        {"id":"motion","scope":"session","effect":{"type":"clip","node":node,"property":"y","to":120,"duration_us":"1000000"}},
        {"id":"wave","scope":"scene","effect":{"type":"sprite_wave","nodes":[node,"wave.other"],"spec":{
            "amplitude":[0,10],"step_us":"150000","duration_us":"750000"}}},
        {"id":"hold","scope":"session","effect":{"type":"delay","duration_us":"2000000"}}
    ])).unwrap();
    let mut missing = p.clone();
    missing.requires.retain(|cap| cap != "stage.sprite-wave.v1");
    assert_eq!(
        ValidatedProgram::new(missing).unwrap_err().code,
        "E_CAPABILITY"
    );
    let mut c = start(p);
    let music = c.state().handles["music"];
    let wave = c.state().handles["wave"];
    let motion = c.state().handles["motion"];
    tick(&mut c, 75_000);
    let sampled = c.sample_scene();
    let sprite = sampled.iter().find(|n| n.id == node).unwrap();
    assert_eq!(sprite.y, 27.5);
    assert_eq!(sprite.offset, [0., -7.]);
    assert_eq!(
        sampled
            .iter()
            .find(|n| n.id == "wave.other")
            .unwrap()
            .offset,
        sprite.offset
    );
    let snapshot = c.snapshot();
    let before = serde_json::to_value(&snapshot).unwrap();
    for _ in 0..10 {
        c.sample_scene();
    }
    assert_eq!(serde_json::to_value(c.snapshot()).unwrap(), before);
    let cold: Snapshot = serde_json::from_slice(&serde_json::to_vec(&snapshot).unwrap()).unwrap();
    let mut restored = Core::restore(c.validated_program().clone(), cold, "tween").unwrap();
    assert_eq!(restored.sample_scene(), sampled);
    for delta in [75_000, 150_000, 300_000, 149_000, 1_000] {
        let step = restored.step(CoreInput::Time { delta_us: delta }, 1000);
        assert!(!step.intents.iter().any(|intent| matches!(
            intent,
            CoreIntent::AudioStart { .. } | CoreIntent::AudioStop { .. }
        )));
        tick(&mut c, delta);
        assert_eq!(restored.sample_scene(), c.sample_scene());
        assert_eq!(
            serde_json::to_value(restored.snapshot()).unwrap(),
            serde_json::to_value(c.snapshot()).unwrap()
        );
    }
    assert!(restored.sample_scene().iter().all(|n| n.offset == [0.; 2]));
    assert_eq!(restored.state().tasks[&wave].state, TaskState::Finished);
    assert_eq!(restored.state().tasks[&music].state, TaskState::Running);
    assert_eq!(restored.state().tasks[&motion].state, TaskState::Running);
    let mut forged = snapshot;
    forged
        .tasks
        .get_mut(&wave)
        .unwrap()
        .shake
        .as_mut()
        .unwrap()
        .targets[0] = [0, 10];
    assert!(Core::restore(c.validated_program().clone(), forged, "tween").is_err());
}
#[test]
fn dialogue_quake_freezes_choices_restores_trajectory_and_preserves_music() {
    let mut p = program(json!([
        {"id":"music","scope":"session","effect":{"type":"audio","asset":"audio.bgm","bus":"bgm","looped":true}},
        {"id":"quake","scope":"session","effect":{"type":"dialogue_shake","spec":{
            "amplitude":[15,15],"step_us":"15000","duration_us":"500000","randomize":true}}},
        {"id":"hold","scope":"session","effect":{"type":"delay","duration_us":"1000000"}}
    ]));
    p.requires.push("stage.dialogue-shake.v1".into());
    let mut missing = p.clone();
    missing
        .requires
        .retain(|cap| cap != "stage.dialogue-shake.v1");
    assert_eq!(
        ValidatedProgram::new(missing).unwrap_err().code,
        "E_CAPABILITY"
    );
    let mut invalid = p.clone();
    if let Effect::DialogueShake { spec } =
        &mut invalid.cues.get_mut("test").unwrap().effects[1].effect
    {
        spec.duration_us = Micros(0);
    }
    assert_eq!(ValidatedProgram::new(invalid).unwrap_err().code, "E_SHAKE");
    let mut c = start(p);
    let id = c.state().handles["quake"];
    let music = c.state().handles["music"];
    tick(&mut c, 67_000);
    let snapshot = c.snapshot();
    let before = serde_json::to_value(&snapshot).unwrap();
    for _ in 0..10 {
        c.sample_dialogue_appearance();
    }
    assert_eq!(serde_json::to_value(c.snapshot()).unwrap(), before);
    let cold: Snapshot = serde_json::from_slice(&serde_json::to_vec(&snapshot).unwrap()).unwrap();
    let mut restored = Core::restore(c.validated_program().clone(), cold, "tween").unwrap();
    assert_eq!(
        restored.sample_dialogue_appearance(),
        c.sample_dialogue_appearance()
    );
    for delta in [1_000, 7_000, 100_000, 324_000, 1_000] {
        let step = restored.step(CoreInput::Time { delta_us: delta }, 1000);
        assert!(!step.intents.iter().any(|i| matches!(
            i,
            CoreIntent::AudioStart { .. } | CoreIntent::AudioStop { .. }
        )));
        tick(&mut c, delta);
        assert_eq!(
            restored.sample_dialogue_appearance(),
            c.sample_dialogue_appearance()
        );
        assert_eq!(
            serde_json::to_value(restored.snapshot()).unwrap(),
            serde_json::to_value(c.snapshot()).unwrap()
        );
    }
    assert_eq!(restored.sample_dialogue_appearance().text_offset, [0.; 2]);
    assert_eq!(restored.state().tasks[&id].state, TaskState::Finished);
    assert_eq!(restored.state().handles["music"], music);
    assert_eq!(restored.state().tasks[&music].state, TaskState::Running);
    let mut forged = snapshot.clone();
    forged
        .tasks
        .get_mut(&id)
        .unwrap()
        .shake
        .as_mut()
        .unwrap()
        .targets[0] = [1, 0];
    assert!(Core::restore(c.validated_program().clone(), forged, "tween").is_err());
    let mut forged = snapshot.clone();
    forged.tasks.get_mut(&id).unwrap().shake = None;
    assert!(Core::restore(c.validated_program().clone(), forged, "tween").is_err());
    let mut stopped = Core::restore(c.validated_program().clone(), snapshot, "tween").unwrap();
    stopped.step(
        CoreInput::TaskFailed {
            task: id,
            message: "neutral device failure".into(),
        },
        1000,
    );
    assert_eq!(stopped.sample_dialogue_appearance().text_offset, [0.; 2]);
    assert_eq!(stopped.state().tasks[&music].state, TaskState::Running);
}
#[test]
fn preserved_sprite_motion_crosses_scene_changes_and_both_transition_layers_after_restore() {
    let mut p = program(json!([hold()]));
    p.requires.push("stage.sprite-continuity.v1".into());
    let node = p.scenes["station"][0].id.clone();
    p.scenes.get_mut("station").unwrap()[0].y = 0.;
    let mut patched = p.scenes["station"].clone();
    patched[0].y = -999.;
    patched[0].preserve_pose = vec![Property::Y];
    p.scenes.insert("patched".into(), patched);
    p.cues.insert("motion".into(),serde_json::from_value(json!({"effects":[
        {"id":"motion","scope":"session","effect":{"type":"clip","node":node,"property":"y","to":100,"duration_us":"2000"}},
        {"id":"phase","scope":"frame","effect":{"type":"delay","duration_us":"1000"}}
    ]})).unwrap());
    p.cues.insert("patch".into(),serde_json::from_value(json!({"effects":[
        {"id":"patch","scope":"session","effect":{"type":"stage_present","scene":"patched","duration_us":"500"}}
    ]})).unwrap());
    p.functions.get_mut("main").unwrap().entry = "start-continuity".into();
    p.functions.get_mut("main").unwrap().blocks=serde_json::from_value(json!({
        "start-continuity":{"terminator":{"type":"activate","cue":"opening","next":"motion"}},
        "motion":{"terminator":{"type":"activate","cue":"motion","next":"phase"}},
        "phase":{"terminator":{"type":"await","conditions":[{"task":"phase","milestone":{"type":"finished"}}],"next":"patch","on_cancelled":"done","on_failed":"done"}},
        "patch":{"terminator":{"type":"activate","cue":"patch","next":"hold"}},
        "hold":{"terminator":{"type":"activate","cue":"test","next":"wait"}},
        "wait":{"terminator":{"type":"await","conditions":[{"task":"hold","milestone":{"type":"finished"}}],"next":"done","on_cancelled":"done","on_failed":"done"}},
        "done":{"terminator":{"type":"end","outcome":"done"}}
    })).unwrap();
    let mut missing = p.clone();
    missing
        .requires
        .retain(|cap| cap != "stage.sprite-continuity.v1");
    assert_eq!(
        ValidatedProgram::new(missing).unwrap_err().code,
        "E_CAPABILITY"
    );
    let mut malformed = p.clone();
    malformed.scenes.get_mut("patched").unwrap()[0]
        .preserve_pose
        .push(Property::Y);
    assert!(ValidatedProgram::new(malformed).is_err());
    let mut c = start(p);
    prepare(&mut c); // motion
    let music = c.state().handles["music"];
    let motion = c.state().handles["motion"];
    tick(&mut c, 1000);
    assert_eq!(
        c.sample_scene().iter().find(|n| n.id == node).unwrap().y,
        50.
    );
    prepare(&mut c); // patch
    prepare(&mut c); // hold
    tick(&mut c, 200);
    let y = c.sample_scene().iter().find(|n| n.id == node).unwrap().y;
    assert!((y - 60.).abs() < 0.00001);
    assert_eq!(
        c.transition()
            .unwrap()
            .0
            .iter()
            .find(|n| n.id == node)
            .unwrap()
            .y,
        y
    );
    assert_eq!(c.transition_progress(), Some(0.4));
    assert_eq!(c.state().handles["motion"], motion);
    assert_eq!(
        c.state().tasks[&motion].scene_generation,
        c.state().scene_generation
    );
    let mut restored = Core::restore(c.validated_program().clone(), c.snapshot(), "tween").unwrap();
    assert_eq!(restored.sample_scene(), c.sample_scene());
    assert_eq!(restored.transition(), c.transition());
    tick(&mut restored, 800);
    assert_eq!(
        restored
            .sample_scene()
            .iter()
            .find(|n| n.id == node)
            .unwrap()
            .y,
        100.
    );
    assert!(restored.transition().is_none());
    assert_eq!(restored.state().handles["music"], music);
    assert_eq!(restored.state().tasks[&music].state, TaskState::Running);
    assert_eq!(restored.state().tasks[&motion].state, TaskState::Finished);
}
#[test]
fn dialogue_channels_are_independent_and_restore_mid_animation() {
    for (name, property) in [
        ("opacity", DialogueProperty::Opacity),
        ("background_opacity", DialogueProperty::BackgroundOpacity),
        ("text_opacity", DialogueProperty::TextOpacity),
    ] {
        let mut c = start(program(json!([fade(name, 0.), hold()])));
        tick(&mut c, 400);
        let appearance = c.sample_dialogue_appearance();
        assert!((appearance.get(property) - 0.6).abs() < 0.00001);
        for other in [
            DialogueProperty::Opacity,
            DialogueProperty::BackgroundOpacity,
            DialogueProperty::TextOpacity,
        ] {
            if other != property {
                assert_eq!(appearance.get(other), 1.);
            }
        }
        let mut restored =
            Core::restore(c.validated_program().clone(), c.snapshot(), "tween").unwrap();
        assert_eq!(appearance, restored.sample_dialogue_appearance());
        tick(&mut c, 600);
        tick(&mut restored, 600);
        assert_eq!(
            c.sample_dialogue_appearance(),
            restored.sample_dialogue_appearance()
        );
        assert_eq!(restored.state().dialogue_appearance.get(property), 0.);
        assert_eq!(
            restored.state().tasks[&restored.state().handles["fade"]].end_reason,
            Some(TaskEndReason::Completed)
        );
    }
}
#[test]
fn zero_duration_and_explicit_settlement_policies() {
    let mut zero = fade("opacity", 0.);
    zero["effect"]["duration_us"] = json!("0");
    let c = start(program(json!([zero, hold()])));
    assert_eq!(c.sample_dialogue_appearance().opacity, 0.);
    for (action, policy, expected) in [
        ("cancel", "commit_current", 0.6),
        ("cancel", "restore_base", 1.),
        ("cancel", "settle_end", 0.),
        ("finish", "commit_current", 0.),
    ] {
        let mut effect = fade("opacity", 0.);
        effect["effect"]["cancel"] = json!(policy);
        let mut p = program(json!([effect, hold()]));
        let f = p.functions.get_mut("main").unwrap();
        f.blocks.get_mut("hold").unwrap().terminator = serde_json::from_value(json!({"type":"await","conditions":[{"task":"pause","milestone":{"type":"finished"}}],"next":"control","on_cancelled":"done","on_failed":"done"})).unwrap();
        f.blocks.insert("control".into(), serde_json::from_value(json!({"ops":[{"id":"control","operation":{"type":"task_control","task":"fade","action":action}}],"terminator":{"type":"await","conditions":[{"task":"hold","milestone":{"type":"finished"}}],"next":"done","on_cancelled":"done","on_failed":"done"}})).unwrap());
        p.cues.get_mut("test").unwrap().effects.push(serde_json::from_value(json!({"id":"pause","scope":"session","effect":{"type":"delay","duration_us":"400"}})).unwrap());
        let mut c = start(p);
        tick(&mut c, 400);
        assert!((c.sample_dialogue_appearance().opacity - expected).abs() < 0.00001);
    }
}
#[test]
fn replacement_captures_visible_value_and_not_previous_base() {
    let mut p = program(
        json!([fade("opacity", 0.), hold(), {"id":"pause","scope":"session","effect":{"type":"delay","duration_us":"400"}}]),
    );
    let mut replacement = fade("opacity", 1.);
    replacement["id"] = json!("return-fade");
    replacement["effect"]["replace"] = json!(true);
    p.cues.insert(
        "replacement".into(),
        serde_json::from_value(json!({"effects":[replacement]})).unwrap(),
    );
    let f = p.functions.get_mut("main").unwrap();
    f.blocks.get_mut("hold").unwrap().terminator = serde_json::from_value(json!({"type":"await","conditions":[{"task":"pause","milestone":{"type":"finished"}}],"next":"replace","on_cancelled":"done","on_failed":"done"})).unwrap();
    f.blocks.insert(
        "replace".into(),
        serde_json::from_value(
            json!({"terminator":{"type":"activate","cue":"replacement","next":"wait-end"}}),
        )
        .unwrap(),
    );
    f.blocks.insert("wait-end".into(), serde_json::from_value(json!({"terminator":{"type":"await","conditions":[{"task":"hold","milestone":{"type":"finished"}}],"next":"done","on_cancelled":"done","on_failed":"done"}})).unwrap());
    let mut c = start(p);
    let old = c.state().handles["fade"];
    tick(&mut c, 400);
    prepare(&mut c);
    assert_eq!(
        c.state().tasks[&old].end_reason,
        Some(TaskEndReason::Replaced)
    );
    assert!((c.sample_dialogue_appearance().opacity - 0.6).abs() < 0.00001);
    tick(&mut c, 500);
    assert!((c.sample_dialogue_appearance().opacity - 0.8).abs() < 0.00001);
}
#[test]
fn invalid_values_missing_capability_and_cross_syntax_writers_are_rejected() {
    let mut p = program(json!([fade("opacity", 0.), hold()]));
    p.requires.retain(|cap| cap != "tween.target.v1");
    assert_eq!(ValidatedProgram::new(p).unwrap_err().code, "E_CAPABILITY");
    for value in [-1., 1.01] {
        assert_eq!(
            ValidatedProgram::new(program(json!([fade("opacity", value), hold()])))
                .unwrap_err()
                .code,
            "E_VISUAL"
        );
    }
    let p = program(json!([
        {"id":"a","scope":"scene","effect":{"type":"clip","node":"art","property":"x","to":10.,"duration_us":"1000"}},
        {"id":"b","scope":"scene","effect":{"type":"tween","target":{"type":"scene_node","node":"art","property":"x"},"to":20.,"duration_us":"1000"}},hold()
    ]));
    assert_eq!(ValidatedProgram::new(p).unwrap_err().code, "E_OWNERSHIP");
    assert!(serde_json::from_value::<Effect>(json!({"type":"tween","target":{"type":"dialogue_root","property":"x"},"to":0.,"duration_us":"1000"})).is_err());
}
#[test]
fn corrupt_snapshot_cannot_inject_nonfinite_or_expired_property_state() {
    let mut c = start(program(json!([fade("opacity", 0.), hold()])));
    tick(&mut c, 400);
    for corruption in 0..4 {
        let mut snapshot = c.snapshot();
        let id = snapshot.handles["fade"];
        match corruption {
            0 => snapshot.dialogue_appearance.text_opacity = -1.,
            1 => snapshot.tasks.get_mut(&id).unwrap().captured = f32::NAN,
            2 => snapshot.tasks.get_mut(&id).unwrap().elapsed_us = Micros(1000),
            _ => snapshot.tasks.get_mut(&id).unwrap().scene_generation = u32::MAX,
        }
        assert!(Core::restore(c.validated_program().clone(), snapshot, "tween").is_err());
    }
}

#[test]
fn dialogue_track_uses_its_scope_instead_of_scene_generation() {
    for scope in ["session", "scene"] {
        let mut effect = fade("opacity", 0.);
        effect["scope"] = json!(scope);
        effect["effect"]["cancel"] = json!("restore_base");
        let mut p = program(
            json!([effect, hold(), {"id":"pause","scope":"session","effect":{"type":"delay","duration_us":"400"}}]),
        );
        let scene = p.scenes.keys().next().unwrap().clone();
        p.cues.insert("change-scene".into(), serde_json::from_value(json!({"effects":[{"id":"stage-test","scope":"scene","effect":{"type":"stage_present","scene":scene,"duration_us":"0"}}]})).unwrap());
        let f = p.functions.get_mut("main").unwrap();
        f.blocks.get_mut("hold").unwrap().terminator = serde_json::from_value(json!({"type":"await","conditions":[{"task":"pause","milestone":{"type":"finished"}}],"next":"change-scene","on_cancelled":"done","on_failed":"done"})).unwrap();
        f.blocks.insert(
            "change-scene".into(),
            serde_json::from_value(
                json!({"terminator":{"type":"activate","cue":"change-scene","next":"wait-end"}}),
            )
            .unwrap(),
        );
        f.blocks.insert("wait-end".into(), serde_json::from_value(json!({"terminator":{"type":"await","conditions":[{"task":"hold","milestone":{"type":"finished"}}],"next":"done","on_cancelled":"done","on_failed":"done"}})).unwrap());
        let mut c = start(p);
        tick(&mut c, 400);
        prepare(&mut c);
        tick(&mut c, 200);
        if scope == "scene" {
            assert_eq!(c.sample_dialogue_appearance().opacity, 1.);
            assert_eq!(
                c.state().tasks[&c.state().handles["fade"]].end_reason,
                Some(TaskEndReason::ScopeExited)
            );
        } else {
            assert!((c.sample_dialogue_appearance().opacity - 0.4).abs() < 0.00001);
            let restored =
                Core::restore(c.validated_program().clone(), c.snapshot(), "tween").unwrap();
            assert_eq!(
                restored.sample_dialogue_appearance(),
                c.sample_dialogue_appearance()
            );
        }
    }
}

#[test]
fn text_shadow_requires_capability_and_bounded_finite_values() {
    let mut p: Program = serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    p.theme.dialogue.shadow = Some(TextShadow {
        offset: [2., 2.],
        color: [0., 0., 0., 0.8],
    });
    assert_eq!(
        ValidatedProgram::new(p.clone()).unwrap_err().code,
        "E_CAPABILITY"
    );
    p.requires.push("text.shadow.v1".into());
    assert!(ValidatedProgram::new(p.clone()).is_ok());
    for offset in [17., f32::NAN, f32::INFINITY] {
        let mut bad = p.clone();
        bad.theme.dialogue.shadow.as_mut().unwrap().offset[0] = offset;
        assert_eq!(
            ValidatedProgram::new(bad).unwrap_err().code,
            "E_THEME_PROPS"
        );
    }
    p.theme.dialogue.shadow.as_mut().unwrap().color[3] = 1.1;
    assert_eq!(ValidatedProgram::new(p).unwrap_err().code, "E_THEME_PROPS");
}

#[test]
fn directional_wipe_restores_frozen_style_and_progress() {
    let mut p = program(json!([hold()]));
    let scene = p.scenes.keys().next().unwrap().clone();
    p.requires.push("stage.wipe.v1".into());
    p.cues.get_mut("test").unwrap().effects.push(EffectDef {
        id: "wipe".into(),
        scope: Scope::Session,
        effect: Effect::StagePresent {
            scene,
            dialogue_visible: None,
            inherit_images: Vec::new(),
            inherit_image_geometry: Vec::new(),
            duration_us: Micros(1000),
            transition: StageTransition::Wipe {
                direction: WipeDirection::RightToLeft,
                softness: 0.2,
            },
        },
    });
    let mut missing = p.clone();
    missing.requires.retain(|c| c != "stage.wipe.v1");
    assert_eq!(
        ValidatedProgram::new(missing).unwrap_err().code,
        "E_CAPABILITY"
    );
    let mut invalid = p.clone();
    if let Effect::StagePresent { transition, .. } =
        &mut invalid.cues.get_mut("test").unwrap().effects[1].effect
    {
        *transition = StageTransition::Wipe {
            direction: WipeDirection::RightToLeft,
            softness: f32::NAN,
        };
    }
    assert_eq!(
        ValidatedProgram::new(invalid).unwrap_err().code,
        "E_TRANSITION"
    );
    let mut c = start(p);
    tick(&mut c, 400);
    let mut restored = Core::restore(c.validated_program().clone(), c.snapshot(), "tween").unwrap();
    assert_eq!(restored.transition_style(), c.transition_style());
    assert_eq!(restored.transition().unwrap().1, 0.4);
    let mut forged = c.snapshot();
    let id = forged.handles["wipe"];
    if let Effect::StagePresent { transition, .. } = &mut forged.tasks.get_mut(&id).unwrap().effect
    {
        *transition = StageTransition::Dissolve;
    }
    assert!(Core::restore(c.validated_program().clone(), forged, "tween").is_err());
    tick(&mut restored, 600);
    assert!(restored.transition().is_none());
}

#[test]
fn mask_asset_is_validated_prepared_and_restored_as_part_of_the_transition() {
    let mut p = program(json!([hold()]));
    let descriptor = p
        .assets
        .values()
        .find(|a| a.kind == AssetKind::Image)
        .unwrap()
        .clone();
    p.assets.insert("mask.pattern".into(), descriptor);
    let scene = p.scenes.keys().next().unwrap().clone();
    p.cues.get_mut("test").unwrap().effects.push(EffectDef {
        id: "mask".into(),
        scope: Scope::Session,
        effect: Effect::StagePresent {
            scene,
            dialogue_visible: None,
            inherit_images: Vec::new(),
            inherit_image_geometry: Vec::new(),
            duration_us: Micros(1000),
            transition: StageTransition::Mask {
                asset: "mask.pattern".into(),
                channel: MaskChannel::Alpha,
                invert: true,
                softness: 0.1,
            },
        },
    });
    assert_eq!(
        ValidatedProgram::new(p.clone()).unwrap_err().code,
        "E_CAPABILITY"
    );
    p.requires.push("stage.mask.v1".into());
    let v = ValidatedProgram::new(p.clone()).unwrap();
    assert!(v.cue_assets("test").contains("mask.pattern"));
    let mut wrong = p.clone();
    wrong.assets.get_mut("mask.pattern").unwrap().kind = AssetKind::Audio;
    assert!(ValidatedProgram::new(wrong).is_err());
    let mut c = start(p);
    tick(&mut c, 400);
    let restored = Core::restore(c.validated_program().clone(), c.snapshot(), "tween").unwrap();
    assert_eq!(restored.transition_style().asset(), Some("mask.pattern"));
    assert_eq!(restored.transition_style(), c.transition_style());
    assert_eq!(restored.transition().unwrap().1, 0.4);
}
