use nir_core::*;
use nir_format::*;
use serde_json::json;

fn program() -> Program {
    let mut p: Program = serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    p.requires.extend([
        "stage.sprite-timeline.v1".into(),
        "stage.sprite-continuity.v1".into(),
    ]);
    let root: Node =
        serde_json::from_value(json!({"id":"movie","x":10,"y":20,"width":32,"height":24,
        "timeline_binding":"movie.frames","preserve_pose":["x"]}))
        .unwrap();
    let child: Node =
        serde_json::from_value(json!({"id":"picture","parent":"movie","asset":"bg.station",
        "x":0,"y":0,"width":32,"height":24}))
        .unwrap();
    p.scenes
        .insert("movie.first".into(), vec![root.clone(), child.clone()]);
    let mut next = root;
    next.x = 900.;
    next.y = 40.;
    p.scenes.insert("movie.next".into(), vec![next, child]);
    let pose = |time, x| SpriteKeyframe {
        at_us: Micros(time),
        rect: [x, 0., 32., 24.],
        opacity: 1.,
        color: [1.; 4],
        transform: None,
    };
    p.sprite_timelines.insert(
        "movie.frames".into(),
        SpriteTimeline {
            id: "movie.frames".into(),
            duration_us: Micros(1_000_000),
            tracks: vec![SpriteTimelineTrack {
                node: "picture".into(),
                frames: vec![
                    pose(0, 0.),
                    pose(200_000, 2.),
                    pose(500_000, 5.),
                    pose(900_000, 9.),
                ],
            }],
        },
    );
    p.cues.insert("movie".into(),serde_json::from_value(json!({"effects":[
        {"id":"music","scope":"session","effect":{"type":"audio","asset":"audio.bgm","bus":"bgm","looped":true}},
        {"id":"stage","scope":"session","effect":{"type":"stage_present","scene":"movie.first"}},
        {"id":"movie","scope":"session","effect":{"type":"sprite_timeline","timeline":"movie.frames","root":"movie","duration_us":"1000000"}},
        {"id":"move","scope":"session","effect":{"type":"clip","node":"movie","property":"x","to":110,"duration_us":"1000000"}},
        {"id":"park","scope":"session","effect":{"type":"delay","duration_us":"1000000000"}},
        {"id":"delay","scope":"session","effect":{"type":"delay","duration_us":"300000"}}
    ]})).unwrap());
    p.cues.insert("next".into(),serde_json::from_value(json!({"effects":[
        {"id":"stage","scope":"session","effect":{"type":"stage_present","scene":"movie.next","duration_us":"200000"}}
    ]})).unwrap());
    let f = p.functions.get_mut("main").unwrap();
    f.entry = "test".into();
    f.blocks=serde_json::from_value(json!({
        "test":{"terminator":{"type":"activate","cue":"movie","next":"delay"}},
        "delay":{"terminator":{"type":"await","conditions":[{"task":"delay","milestone":{"type":"finished"}}],"next":"next","on_cancelled":"done","on_failed":"done"}},
        "next":{"terminator":{"type":"activate","cue":"next","next":"movie"}},
        "movie":{"terminator":{"type":"await","conditions":[{"task":"movie","milestone":{"type":"finished"}}],"next":"hold","on_cancelled":"hold","on_failed":"done"}},
        "hold":{"terminator":{"type":"await","conditions":[{"task":"park","milestone":{"type":"finished"}}],"next":"done","on_cancelled":"done","on_failed":"done"}},
        "done":{"terminator":{"type":"end","outcome":"done"}}
    })).unwrap();
    p
}
#[test]
fn completed_motion_deletes_its_tree_without_resurrection_after_fade_or_restore() {
    let mut p = program();
    p.requires.push("stage.sprite-lifecycle.v1".into());
    let Effect::SpriteTimeline {
        delete_on_finish, ..
    } = &mut p.cues.get_mut("movie").unwrap().effects[2].effect
    else {
        panic!()
    };
    *delete_on_finish = true;
    p.scenes.get_mut("movie.next").unwrap()[0].inherit_existence = true;
    let Effect::StagePresent { duration_us, .. } =
        &mut p.cues.get_mut("next").unwrap().effects[0].effect
    else {
        panic!()
    };
    *duration_us = Micros(2_000_000);
    let mut after = p.cues["next"].clone();
    let Effect::StagePresent { duration_us, .. } = &mut after.effects[0].effect else {
        panic!()
    };
    *duration_us = Micros(0);
    p.cues.insert("after".into(), after);
    let f = p.functions.get_mut("main").unwrap();
    f.blocks.insert("movie".into(),serde_json::from_value(json!({"terminator":{"type":"await","conditions":[{"task":"stage","milestone":{"type":"finished"}}],"next":"after","on_cancelled":"done","on_failed":"done"}})).unwrap());
    f.blocks.insert(
        "after".into(),
        serde_json::from_value(
            json!({"terminator":{"type":"activate","cue":"after","next":"hold"}}),
        )
        .unwrap(),
    );
    let validated = ValidatedProgram::new(p.clone()).unwrap();
    let mut missing = p.clone();
    missing
        .requires
        .retain(|cap| cap != "stage.sprite-lifecycle.v1");
    assert_eq!(
        ValidatedProgram::new(missing).unwrap_err().code,
        "E_CAPABILITY"
    );
    let mut c = start(p);
    let movie = c.state().handles["movie"];
    let music = c.state().handles["music"];
    tick(&mut c, 300000);
    prepare(&mut c);
    tick(&mut c, 650000);
    let mut restored = Core::restore(validated.clone(), c.snapshot(), "timeline").unwrap();
    tick(&mut c, 50000);
    tick(&mut restored, 50000);
    for core in [&c, &restored] {
        assert!(core.state().fault.is_none());
        assert!(core.state().scene.is_empty());
        assert!(core.sample_scene().is_empty());
        let stage = &core.state().tasks[&core.state().handles["stage"]];
        assert!(stage.target.is_empty());
        assert!(!stage.source.is_empty()); // Frozen pre-fade pixels remain valid.
        assert_eq!(core.state().tasks[&movie].state, TaskState::Finished);
        assert_eq!(core.state().tasks[&music].state, TaskState::Running);
    }
    let mut restored = Core::restore(validated, restored.snapshot(), "timeline").unwrap();
    tick(&mut restored, 1300000);
    prepare(&mut restored);
    assert!(restored.state().fault.is_none());
    assert!(restored.state().scene.is_empty());
    assert_eq!(restored.state().handles["music"], music);
}

#[test]
fn cancelled_or_replaced_motion_keeps_the_scene_and_explicit_creation_reappears() {
    let mut p = program();
    p.requires.push("stage.sprite-lifecycle.v1".into());
    let Effect::SpriteTimeline {
        delete_on_finish, ..
    } = &mut p.cues.get_mut("movie").unwrap().effects[2].effect
    else {
        panic!()
    };
    *delete_on_finish = true;
    for action in ["cancel", "finish"] {
        let mut changed = p.clone();
        changed.functions.get_mut("main").unwrap().blocks.get_mut("delay").unwrap().ops =
            serde_json::from_value(json!([{"id":"stop","operation":{"type":"task_control","task":"movie","action":action}}])).unwrap();
        let core = start(changed);
        assert!(core.state().fault.is_none());
        assert_eq!(core.state().scene.len(), 2);
    }
    let mut c = start(p);
    tick(&mut c, 1000000); // Next authored creation has inherit_existence=false.
    prepare(&mut c);
    assert!(c.state().fault.is_none());
    assert_eq!(c.state().scene.len(), 2);
}
#[test]
fn stock_motion_continues_its_saved_integer_curve_across_scene_fade() {
    for curve in ["inc", "dec"] {
        let mut p = program();
        p.requires.push("stage.source-motion.v1".into());
        p.cues.get_mut("movie").unwrap().effects[3].effect=serde_json::from_value(json!({
            "type":"source_motion","node":"movie","property":"x","to":110,"duration_us":"1000000","curve":curve})).unwrap();
        let validated = ValidatedProgram::new(p.clone()).unwrap();
        let mut c = start(p);
        tick(&mut c, 300000);
        prepare(&mut c);
        tick(&mut c, 50000);
        let movement = c.state().handles["move"];
        let music = c.state().handles["music"];
        let saved = c.snapshot();
        let mut restored = Core::restore(validated, saved, "timeline").unwrap();
        assert!(restored.transition().is_some());
        for delta in [39000, 81000, 201000] {
            tick(&mut c, delta);
            for _ in 0..delta / 1000 {
                tick(&mut restored, 1000);
            }
            let pose = |c: &Core| c.sample_scene().iter().find(|n| n.id == "movie").unwrap().x;
            assert_eq!(pose(&c), pose(&restored));
            assert_eq!(restored.state().handles["move"], movement);
            assert_eq!(restored.state().handles["music"], music);
            assert_eq!(restored.state().tasks[&music].state, TaskState::Running);
        }
        tick(&mut restored, 1000000);
        assert_eq!(
            restored
                .sample_scene()
                .iter()
                .find(|n| n.id == "movie")
                .unwrap()
                .x,
            110.
        );
        assert_eq!(restored.state().tasks[&movement].state, TaskState::Finished);
    }
}
#[test]
fn source_opacity_has_its_own_capability_and_preserves_byte_samples_across_fade_restore() {
    let mut p = program();
    p.requires.extend([
        "stage.source-motion.v1".into(),
        "stage.source-opacity.v1".into(),
    ]);
    p.cues.get_mut("movie").unwrap().effects[3].effect=serde_json::from_value(json!({
        "type":"source_motion","node":"movie","property":"opacity","to":64,"duration_us":"1000000","curve":"opacity_linear"})).unwrap();
    for scene in ["movie.first", "movie.next"] {
        p.scenes.get_mut(scene).unwrap()[0]
            .preserve_pose
            .push(Property::Opacity);
    }
    let mut missing = p.clone();
    missing
        .requires
        .retain(|cap| cap != "stage.source-opacity.v1");
    assert_eq!(
        ValidatedProgram::new(missing).unwrap_err().code,
        "E_CAPABILITY"
    );
    for patch in [
        json!({"to":256}),
        json!({"curve":"inc"}),
        json!({"property":"x"}),
    ] {
        let mut bad = p.clone();
        let mut effect = serde_json::to_value(&bad.cues["movie"].effects[3].effect).unwrap();
        for (key, value) in patch.as_object().unwrap() {
            effect[key] = value.clone();
        }
        bad.cues.get_mut("movie").unwrap().effects[3].effect =
            serde_json::from_value(effect).unwrap();
        assert_eq!(ValidatedProgram::new(bad).unwrap_err().code, "E_MOTION");
    }
    let validated = ValidatedProgram::new(p.clone()).unwrap();
    let mut c = start(p);
    tick(&mut c, 300000);
    prepare(&mut c);
    tick(&mut c, 50000);
    let music = c.state().handles["music"];
    let saved = c.snapshot();
    let mut restored = Core::restore(validated, saved, "timeline").unwrap();
    assert!(restored.transition().is_some());
    for delta in [39000, 81000, 201000] {
        tick(&mut c, delta);
        for _ in 0..delta / 1000 {
            tick(&mut restored, 1000);
        }
        let alpha = |c: &Core| {
            c.sample_scene()
                .iter()
                .find(|n| n.id == "movie")
                .unwrap()
                .opacity
        };
        assert_eq!(alpha(&c), alpha(&restored));
        assert_eq!(restored.state().handles["music"], music);
        assert_eq!(restored.state().tasks[&music].state, TaskState::Running);
    }
    tick(&mut restored, 1000000);
    assert_eq!(
        restored
            .sample_scene()
            .iter()
            .find(|n| n.id == "movie")
            .unwrap()
            .opacity,
        64. / 255.
    );
}
fn start(p: Program) -> Core {
    let mut c = Core::new(
        ValidatedProgram::new(p).unwrap(),
        "timeline".into(),
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
fn tick(c: &mut Core, dt: u64) {
    c.step(CoreInput::Time { delta_us: dt }, 1000);
    assert!(c.state().fault.is_none(), "{:?}", c.state().fault);
}
fn x(c: &Core, id: &str) -> f32 {
    c.sample_scene().iter().find(|n| n.id == id).unwrap().x
}
#[test]
fn advance_wait_uses_fresh_identity_and_leaves_timeline_and_music_running() {
    let mut p = program();
    p.requires.push("control.advance-wait.v1".into());
    p.functions
        .get_mut("main")
        .unwrap()
        .blocks
        .get_mut("test")
        .unwrap()
        .terminator = Terminator::Activate {
        cue: "movie".into(),
        next: "movie".into(),
    };
    let Terminator::Await { on_advance, .. } = &mut p
        .functions
        .get_mut("main")
        .unwrap()
        .blocks
        .get_mut("movie")
        .unwrap()
        .terminator
    else {
        unreachable!()
    };
    *on_advance = Some("hold".into());
    let validated = ValidatedProgram::new(p.clone()).unwrap();
    let mut c = start(p.clone());
    let movie = c.state().handles["movie"];
    let music = c.state().handles["music"];
    tick(&mut c, 200_000);
    let old = c.advance_wait().unwrap();
    let saved = c.snapshot();
    let mut forged = saved.clone();
    forged
        .waiting
        .as_mut()
        .unwrap()
        .advance
        .as_mut()
        .unwrap()
        .next = "done".into();
    assert!(Core::restore(validated.clone(), forged, "timeline").is_err());
    let mut forged = saved.clone();
    forged
        .waiting
        .as_mut()
        .unwrap()
        .advance
        .as_mut()
        .unwrap()
        .interaction = movie;
    assert!(Core::restore(validated.clone(), forged, "timeline").is_err());
    let mut restored = Core::restore(validated, saved, "timeline").unwrap();
    let current = restored.advance_wait().unwrap();
    assert_ne!(old, current);
    restored.step(
        CoreInput::Advance {
            interaction: old,
            sequence: 1,
        },
        1000,
    );
    assert_eq!(restored.advance_wait(), Some(current));
    assert_eq!(restored.state().last_input, 0);
    restored.step(
        CoreInput::AdvanceReading {
            interaction: current,
            sequence: 1,
            stop_voice: true,
        },
        1000,
    );
    assert!(restored.advance_wait().is_none());
    assert_eq!(restored.state().frames.last().unwrap().block, "hold");
    assert_eq!(restored.state().tick_us, Micros(200_000));
    assert_eq!(restored.state().tasks[&movie].state, TaskState::Running);
    assert_eq!(restored.state().tasks[&music].state, TaskState::Running);
    tick(&mut restored, 300_000);
    assert_eq!(x(&restored, "picture"), 5.);
    tick(&mut restored, 500_000);
    assert_eq!(x(&restored, "picture"), 9.);
    assert_eq!(restored.state().tasks[&movie].state, TaskState::Finished);
    assert_eq!(restored.state().handles["music"], music);
    let mut natural = start(p.clone());
    tick(&mut natural, 1_000_000);
    assert_eq!(natural.state().frames.last().unwrap().block, "hold");
    assert!(natural.advance_wait().is_none());
    p.requires.retain(|c| c != "control.advance-wait.v1");
    assert_eq!(ValidatedProgram::new(p).err().unwrap().code, "E_CAPABILITY");
}
#[test]
fn ordinary_wait_does_not_accept_user_advance() {
    let mut c = start(program());
    let before = c.snapshot();
    assert!(c.advance_wait().is_none());
    c.step(
        CoreInput::Advance {
            interaction: 0,
            sequence: 1,
        },
        1000,
    );
    assert_eq!(
        serde_json::to_value(c.snapshot()).unwrap(),
        serde_json::to_value(before).unwrap()
    );
}
#[test]
fn timeline_prepared_barrier_exact_frames_graft_and_cold_restore_share_one_clock() {
    let p = program();
    let validated = ValidatedProgram::new(p.clone()).unwrap();
    let mut c = start(p);
    let music = c.state().handles["music"];
    let movie = c.state().handles["movie"];
    assert_eq!(x(&c, "picture"), 0.);
    tick(&mut c, 199_999);
    assert_eq!(x(&c, "picture"), 0.);
    tick(&mut c, 1);
    assert_eq!(x(&c, "picture"), 2.);
    tick(&mut c, 100_000);
    assert!(c.state().pending.is_some());
    assert_eq!(c.state().tick_us, Micros(300_000));
    tick(&mut c, 10_000_000);
    assert_eq!(c.state().tick_us, Micros(300_000));
    prepare(&mut c);
    assert_eq!(c.state().handles["movie"], movie);
    assert_eq!(x(&c, "picture"), 2.);
    assert_eq!(x(&c, "movie"), 40.);
    tick(&mut c, 150_000);
    assert_eq!(x(&c, "movie"), 55.);
    let source = c.transition().unwrap().0;
    assert_eq!(source.iter().find(|n| n.id == "movie").unwrap().x, 55.);
    assert_eq!(source.iter().find(|n| n.id == "picture").unwrap().x, 2.);
    let saved = c.snapshot();
    let wire = serde_json::to_string(&saved).unwrap();
    assert!(!wire.contains("keyframes"));
    assert!(!wire.contains("tracks"));
    let mut restored = Core::restore(validated, saved, "timeline").unwrap();
    assert_eq!(restored.sample_scene(), c.sample_scene());
    assert_eq!(restored.transition(), c.transition());
    restored.step(CoreInput::None, 1000);
    assert_eq!(restored.state().tick_us, Micros(450_000));
    tick(&mut restored, 50_000);
    assert_eq!(x(&restored, "picture"), 5.);
    assert!(restored.transition().is_none());
    tick(&mut restored, 500_000);
    assert_eq!(x(&restored, "picture"), 9.);
    assert_eq!(x(&restored, "movie"), 110.);
    assert_eq!(restored.state().tasks[&movie].state, TaskState::Finished);
    assert_eq!(restored.state().tasks[&music].state, TaskState::Running);
    assert_eq!(restored.state().frames.last().unwrap().block, "hold");
    let done = restored.snapshot();
    let stable = restored.sample_scene();
    let cold = Core::restore(ValidatedProgram::new(program()).unwrap(), done, "timeline").unwrap();
    assert_eq!(cold.sample_scene(), stable);
}
#[test]
fn removing_root_cancels_timeline_and_settles_wait_without_stopping_music() {
    let mut p = program();
    p.scenes.get_mut("movie.next").unwrap().clear();
    let mut c = start(p);
    tick(&mut c, 300_000);
    prepare(&mut c);
    let t = &c.state().tasks[&c.state().handles["movie"]];
    assert_eq!(t.end_reason, Some(TaskEndReason::ScopeExited));
    assert_eq!(c.state().frames.last().unwrap().block, "hold");
    assert_eq!(
        c.state().tasks[&c.state().handles["music"]].state,
        TaskState::Running
    );
}
#[test]
fn invalid_resources_binding_ownership_and_forged_restore_are_rejected() {
    let original = program();
    let mut bad = original.clone();
    bad.requires.retain(|c| c != "stage.sprite-timeline.v1");
    assert_eq!(ValidatedProgram::new(bad).unwrap_err().code, "E_CAPABILITY");
    let mut bad = original.clone();
    bad.sprite_timelines.get_mut("movie.frames").unwrap().tracks[0].frames[1].at_us = Micros(0);
    assert_eq!(ValidatedProgram::new(bad).unwrap_err().code, "E_TIMELINE");
    let mut bad = original.clone();
    bad.scenes.get_mut("movie.first").unwrap()[1].parent = None;
    assert_eq!(ValidatedProgram::new(bad).unwrap_err().code, "E_TIMELINE");
    let mut bad = original.clone();
    if let Effect::SpriteTimeline { duration_us, .. } =
        &mut bad.cues.get_mut("movie").unwrap().effects[2].effect
    {
        *duration_us = Micros(2_000_000);
    }
    assert_eq!(ValidatedProgram::new(bad).unwrap_err().code, "E_TIMELINE");
    let c = start(original.clone());
    let validated = ValidatedProgram::new(original).unwrap();
    let mut bad = c.snapshot();
    bad.scene[0].timeline_binding = Some("missing".into());
    assert!(Core::restore(validated.clone(), bad, "timeline").is_err());
    let mut bad = c.snapshot();
    let id = bad.handles["movie"];
    bad.tasks.get_mut(&id).unwrap().scene_generation = 0;
    assert!(Core::restore(validated, bad, "timeline").is_err());
}
#[test]
fn timeline_leaf_pose_rejects_scalar_writer_but_root_motion_remains_independent() {
    let mut p = program();
    if let Effect::Clip { node, .. } = &mut p.cues.get_mut("movie").unwrap().effects[3].effect {
        *node = "picture".into();
    }
    let mut c = Core::new(
        ValidatedProgram::new(p).unwrap(),
        "timeline".into(),
        "en".into(),
    )
    .unwrap();
    c.step(CoreInput::None, 1000);
    let activation = c.state().pending.as_ref().unwrap().id;
    c.step(CoreInput::Prepared { activation }, 1000);
    assert_eq!(c.state().fault.as_ref().unwrap().code, "E_OWNERSHIP");
}

fn inherited_image_program(image: &str) -> Program {
    let mut p: Program = serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    p.requires.push("stage.inherit-image.v1".into());
    let photo = |asset: &str, x: f32| {
        serde_json::from_value(
            json!({"id":"photo","asset":asset,"x":x,"y":40,"width":32,"height":24}),
        )
        .unwrap()
    };
    p.scenes.insert("incoming".into(), vec![photo(image, 10.)]);
    p.scenes.insert(
        "common".into(),
        vec![
            photo("bg.station", 20.),
            serde_json::from_value(
                json!({"id":"new","asset":"bg.river","x":300,"y":40,"width":32,"height":24}),
            )
            .unwrap(),
        ],
    );
    p.cues.insert("incoming".into(),serde_json::from_value(json!({"effects":[
        {"id":"music","scope":"session","effect":{"type":"audio","asset":"audio.bgm","bus":"bgm","looped":true}},
        {"id":"stage","scope":"session","effect":{"type":"stage_present","scene":"incoming"}},
        {"id":"delay","scope":"session","effect":{"type":"delay","duration_us":"100000"}},
        {"id":"park","scope":"session","effect":{"type":"delay","duration_us":"1000000000"}}
    ]})).unwrap());
    p.cues.insert("common".into(),serde_json::from_value(json!({"effects":[
        {"id":"stage","scope":"session","effect":{"type":"stage_present","scene":"common","inherit_images":["photo"],"duration_us":"200000"}}
    ]})).unwrap());
    let f = p.functions.get_mut("main").unwrap();
    f.entry = "start".into();
    f.blocks=serde_json::from_value(json!({
        "start":{"terminator":{"type":"activate","cue":"incoming","next":"delay"}},
        "delay":{"terminator":{"type":"await","conditions":[{"task":"delay","milestone":{"type":"finished"}}],"next":"common","on_cancelled":"done","on_failed":"done"}},
        "common":{"terminator":{"type":"activate","cue":"common","next":"hold"}},
        "hold":{"terminator":{"type":"await","conditions":[{"task":"park","milestone":{"type":"finished"}}],"next":"done","on_cancelled":"done","on_failed":"done"}},
        "done":{"terminator":{"type":"end","outcome":"done"}}
    })).unwrap();
    p
}
#[test]
fn inherited_images_capture_each_incoming_route_across_fade_and_cold_restore() {
    for image in ["actor.aki", "bg.station"] {
        let p = inherited_image_program(image);
        let validated = ValidatedProgram::new(p.clone()).unwrap();
        assert_eq!(
            validated.cue_assets("common"),
            std::collections::BTreeSet::from(["bg.river".into()])
        );
        let mut c = start(p);
        let music = c.state().handles["music"];
        tick(&mut c, 100000);
        assert_eq!(c.state().pending.as_ref().unwrap().cue, "common");
        prepare(&mut c);
        let photo = c
            .sample_scene()
            .into_iter()
            .find(|n| n.id == "photo")
            .unwrap();
        assert_eq!(photo.asset.as_deref(), Some(image));
        assert_eq!(photo.x, 20.);
        assert_eq!(
            c.sample_scene()
                .iter()
                .find(|n| n.id == "new")
                .unwrap()
                .asset
                .as_deref(),
            Some("bg.river")
        );
        tick(&mut c, 100000);
        let saved = c.snapshot();
        assert_eq!(saved.handles["music"], music);
        let mut restored = Core::restore(validated, saved, "timeline").unwrap();
        let (source, progress) = restored.transition().unwrap();
        assert_eq!(progress, 0.5);
        assert_eq!(
            source
                .iter()
                .find(|n| n.id == "photo")
                .unwrap()
                .asset
                .as_deref(),
            Some(image)
        );
        assert_eq!(
            restored
                .sample_scene()
                .iter()
                .find(|n| n.id == "photo")
                .unwrap()
                .asset
                .as_deref(),
            Some(image)
        );
        assert_eq!(restored.state().handles["music"], music);
        tick(&mut restored, 100000);
        assert!(restored.transition().is_none());
        assert_eq!(restored.state().tasks[&music].state, TaskState::Running);
    }
}
#[test]
fn inherited_image_rectangles_follow_each_live_route_through_fade_and_restore() {
    for rectangle in [[10., 40., 32., 24.], [120., 80., 64., 48.]] {
        let mut p = inherited_image_program("actor.aki");
        p.requires.push("stage.inherit-image-geometry.v1".into());
        let photo = &mut p.scenes.get_mut("incoming").unwrap()[0];
        [photo.x, photo.y, photo.width, photo.height] = rectangle;
        let Effect::StagePresent {
            inherit_image_geometry,
            ..
        } = &mut p.cues.get_mut("common").unwrap().effects[0].effect
        else {
            panic!()
        };
        *inherit_image_geometry = vec!["photo".into()];
        let validated = ValidatedProgram::new(p.clone()).unwrap();
        let mut c = start(p);
        let music = c.state().handles["music"];
        tick(&mut c, 100_000);
        prepare(&mut c);
        let rect = |nodes: Vec<Node>| {
            let n = nodes.into_iter().find(|n| n.id == "photo").unwrap();
            [n.x, n.y, n.width, n.height]
        };
        assert_eq!(rect(c.sample_scene()), rectangle);
        tick(&mut c, 100_000);
        let mut restored = Core::restore(validated, c.snapshot(), "timeline").unwrap();
        assert_eq!(rect(restored.sample_scene()), rectangle);
        assert_eq!(rect(restored.transition().unwrap().0), rectangle);
        assert_eq!(restored.state().handles["music"], music);
        tick(&mut restored, 100_000);
        assert!(restored.transition().is_none());
        assert_eq!(rect(restored.sample_scene()), rectangle);
        assert_eq!(restored.state().tasks[&music].state, TaskState::Running);
    }
}
#[test]
fn image_rectangle_inheritance_requires_capability_and_a_unique_image_subset() {
    let mut p = inherited_image_program("actor.aki");
    let Effect::StagePresent {
        inherit_image_geometry,
        ..
    } = &mut p.cues.get_mut("common").unwrap().effects[0].effect
    else {
        panic!()
    };
    *inherit_image_geometry = vec!["photo".into()];
    assert_eq!(
        ValidatedProgram::new(p.clone()).unwrap_err().code,
        "E_CAPABILITY"
    );
    p.requires.push("stage.inherit-image-geometry.v1".into());
    assert!(ValidatedProgram::new(p.clone()).is_ok());
    for ids in [
        vec!["photo".into(), "photo".into()],
        vec!["new".into()],
        vec!["missing".into()],
    ] {
        let mut bad = p.clone();
        let Effect::StagePresent {
            inherit_image_geometry,
            ..
        } = &mut bad.cues.get_mut("common").unwrap().effects[0].effect
        else {
            panic!()
        };
        *inherit_image_geometry = ids;
        assert_eq!(
            ValidatedProgram::new(bad).unwrap_err().code,
            "E_STAGE_INHERIT"
        );
    }
}
#[test]
fn image_inheritance_requires_capability_and_unique_ordinary_image_roots() {
    let p = inherited_image_program("actor.aki");
    let mut missing = p.clone();
    missing
        .requires
        .retain(|cap| cap != "stage.inherit-image.v1");
    assert_eq!(
        ValidatedProgram::new(missing).unwrap_err().code,
        "E_CAPABILITY"
    );
    for inherited in [vec!["missing".into()], vec!["photo".into(), "photo".into()]] {
        let mut invalid = p.clone();
        let Effect::StagePresent { inherit_images, .. } =
            &mut invalid.cues.get_mut("common").unwrap().effects[0].effect
        else {
            panic!()
        };
        *inherit_images = inherited;
        assert_eq!(
            ValidatedProgram::new(invalid).unwrap_err().code,
            "E_STAGE_INHERIT"
        );
    }
    let mut absent = p;
    absent.scenes.get_mut("incoming").unwrap().clear();
    let mut c = start(absent);
    tick(&mut c, 100000);
    let activation = c.state().pending.as_ref().unwrap().id;
    c.step(CoreInput::Prepared { activation }, 1000);
    assert_eq!(c.state().fault.as_ref().unwrap().code, "E_STAGE_INHERIT");
}

fn sprite_shake_program() -> Program {
    let mut p: Program = serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    p.requires.push("stage.sprite-shake.v1".into());
    p.scenes.insert("shake".into(),["left","right"].into_iter().enumerate().map(|(index,id)|
        serde_json::from_value(json!({"id":id,"asset":"bg.station","x":10+index*50,"y":20,"width":32,"height":24})).unwrap()).collect());
    p.cues.insert("shake".into(),serde_json::from_value(json!({"effects":[
        {"id":"stage","scope":"session","effect":{"type":"stage_present","scene":"shake"}},
        {"id":"music","scope":"session","effect":{"type":"audio","asset":"audio.bgm","bus":"bgm","looped":true}},
        {"id":"shake","scope":"scene","effect":{"type":"sprite_shake","nodes":["left","right"],"mode":"bound","spec":{"amplitude":[20,30],"step_us":"12000","duration_us":"500000","randomize":true}}},
        {"id":"park","scope":"session","effect":{"type":"delay","duration_us":"1000000000"}}
    ]})).unwrap());
    let f = p.functions.get_mut("main").unwrap();
    f.entry = "start".into();
    f.blocks=serde_json::from_value(json!({
        "start":{"terminator":{"type":"activate","cue":"shake","next":"hold"}},
        "hold":{"terminator":{"type":"await","conditions":[{"task":"park","milestone":{"type":"finished"}}],"next":"done","on_cancelled":"done","on_failed":"done"}},
        "done":{"terminator":{"type":"end","outcome":"done"}}
    })).unwrap();
    p
}
#[test]
fn random_sprite_paths_are_independent_saved_and_frame_partition_invariant() {
    let p = sprite_shake_program();
    let validated = ValidatedProgram::new(p.clone()).unwrap();
    let mut c = start(p);
    let music = c.state().handles["music"];
    let shake = c.state().handles["shake"];
    let captures = &c.state().tasks[&shake].sprite_shakes;
    assert_ne!(captures["left"].targets, captures["right"].targets);
    tick(&mut c, 65000);
    let saved = c.snapshot();
    let mut forged = saved.clone();
    forged
        .tasks
        .get_mut(&shake)
        .unwrap()
        .sprite_shakes
        .remove("left");
    assert!(Core::restore(validated.clone(), forged, "timeline").is_err());
    let mut forged = saved.clone();
    forged
        .tasks
        .get_mut(&shake)
        .unwrap()
        .sprite_shakes
        .get_mut("left")
        .unwrap()
        .targets[0][0] = 9999;
    assert!(Core::restore(validated.clone(), forged, "timeline").is_err());
    let mut restored = Core::restore(validated, saved, "timeline").unwrap();
    for delta in [17000, 39000, 81000, 201000] {
        tick(&mut c, delta);
        for _ in 0..delta / 1000 {
            tick(&mut restored, 1000);
        }
        let offsets = |c: &Core| {
            c.sample_scene()
                .into_iter()
                .map(|n| (n.id, n.offset))
                .collect::<Vec<_>>()
        };
        assert_eq!(offsets(&c), offsets(&restored));
        assert_eq!(
            serde_json::to_value(&c.state().rng).unwrap(),
            serde_json::to_value(&restored.state().rng).unwrap()
        );
    }
    tick(&mut c, 500000);
    assert!(c.sample_scene().iter().all(|n| n.offset == [0.; 2]));
    assert_eq!(c.state().tasks[&shake].state, TaskState::Finished);
    assert_eq!(c.state().tasks[&music].state, TaskState::Running);
}
#[test]
fn shared_direction_sprite_quake_survives_restore_and_rejects_forged_direction() {
    let mut p = sprite_shake_program();
    p.requires.push("stage.sprite-quake.v1".into());
    let Effect::SpriteShake { mode, .. } = &mut p.cues.get_mut("shake").unwrap().effects[2].effect
    else {
        panic!()
    };
    *mode = SpriteShakeMode::Quake;
    let mut missing = p.clone();
    missing
        .requires
        .retain(|cap| cap != "stage.sprite-quake.v1");
    assert_eq!(
        ValidatedProgram::new(missing).unwrap_err().code,
        "E_CAPABILITY"
    );
    let validated = ValidatedProgram::new(p.clone()).unwrap();
    let mut c = start(p);
    let music = c.state().handles["music"];
    let shake = c.state().handles["shake"];
    let captures = &c.state().tasks[&shake].sprite_shakes;
    assert_ne!(captures["left"].targets, captures["right"].targets);
    tick(&mut c, 65000);
    let saved = c.snapshot();
    let mut forged = saved.clone();
    forged
        .tasks
        .get_mut(&shake)
        .unwrap()
        .sprite_shakes
        .remove("left");
    assert!(Core::restore(validated.clone(), forged, "timeline").is_err());
    let mut forged = saved.clone();
    forged
        .tasks
        .get_mut(&shake)
        .unwrap()
        .sprite_shakes
        .get_mut("left")
        .unwrap()
        .targets[0][0] = 9999;
    assert!(Core::restore(validated.clone(), forged, "timeline").is_err());
    let mut forged = saved.clone();
    let capture = forged
        .tasks
        .get_mut(&shake)
        .unwrap()
        .sprite_shakes
        .get_mut("left")
        .unwrap();
    let (index, axis) = capture
        .targets
        .iter()
        .enumerate()
        .find_map(|(index, values)| {
            (0..2)
                .find(|axis| {
                    values[*axis] != 0
                        && saved.tasks[&shake].sprite_shakes["right"].targets[index][*axis] != 0
                })
                .map(|axis| (index, axis))
        })
        .unwrap();
    capture.targets[index][axis] *= -1;
    assert!(Core::restore(validated.clone(), forged, "timeline").is_err());
    let mut restored = Core::restore(validated, saved, "timeline").unwrap();
    for delta in [17000, 39000, 81000, 201000] {
        tick(&mut c, delta);
        for _ in 0..delta / 1000 {
            tick(&mut restored, 1000);
        }
        let offsets = |c: &Core| {
            c.sample_scene()
                .into_iter()
                .map(|n| (n.id, n.offset))
                .collect::<Vec<_>>()
        };
        assert_eq!(offsets(&c), offsets(&restored));
        assert_eq!(
            serde_json::to_value(&c.state().rng).unwrap(),
            serde_json::to_value(&restored.state().rng).unwrap()
        );
    }
    tick(&mut c, 500000);
    assert!(c.sample_scene().iter().all(|n| n.offset == [0.; 2]));
    assert_eq!(c.state().tasks[&shake].state, TaskState::Finished);
    assert_eq!(c.state().tasks[&music].state, TaskState::Running);
}
#[test]
fn independent_sprite_shake_rejects_missing_capability_duplicate_targets_and_overlap() {
    let p = sprite_shake_program();
    let mut missing = p.clone();
    missing.requires.retain(|c| c != "stage.sprite-shake.v1");
    assert_eq!(
        ValidatedProgram::new(missing).unwrap_err().code,
        "E_CAPABILITY"
    );
    let mut bad = p.clone();
    let Effect::SpriteShake { nodes, .. } =
        &mut bad.cues.get_mut("shake").unwrap().effects[2].effect
    else {
        panic!()
    };
    nodes[1] = nodes[0].clone();
    assert_eq!(ValidatedProgram::new(bad).unwrap_err().code, "E_SHAKE");
    let mut bad = p;
    let mut duplicate = bad.cues["shake"].effects[2].clone();
    duplicate.id = "overlap".into();
    bad.cues.get_mut("shake").unwrap().effects.push(duplicate);
    let mut c = Core::new(
        ValidatedProgram::new(bad).unwrap(),
        "timeline".into(),
        "en".into(),
    )
    .unwrap();
    c.step(CoreInput::None, 1000);
    let activation = c.state().pending.as_ref().unwrap().id;
    c.step(CoreInput::Prepared { activation }, 1000);
    assert_eq!(c.state().fault.as_ref().unwrap().code, "E_OWNERSHIP");
}
