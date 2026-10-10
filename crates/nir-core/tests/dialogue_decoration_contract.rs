use nir_core::*;
use nir_format::*;
use serde_json::json;

fn program() -> Program {
    let mut p: Program = serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    p.requires
        .extend(["dialogue.decoration.v1".into(), "text.rect.v1".into()]);
    p.theme.dialogue.text_rect = Some([200., 490., 880., 120.]);
    p.theme.dialogue.rect = Some([160., 450., 960., 200.]);
    for (id, size) in [("portrait", [80, 100]), ("name", [120, 30])] {
        let mut image = p.assets["bg.station"].clone();
        image.width = size[0];
        image.height = size[1];
        image.object = id.into();
        p.assets.insert(id.into(), image);
    }
    let contract = p.texts.get_mut("intro").unwrap();
    contract.gates = vec!["portrait".into()];
    contract.contract_digest = text_contract_digest(contract);
    for docs in p.locales.values_mut() {
        let doc = docs.get_mut("intro").unwrap();
        doc.contract_digest = contract.contract_digest.clone();
        doc.spans = serde_json::from_value(json!([
            {"type":"text","id":"before","text":"Before"},
            {"type":"gate","id":"portrait"},
            {"type":"text","id":"after","text":"After"}
        ]))
        .unwrap();
    }
    for (id, effects) in [
        (
            "music",
            json!([{"id":"music","scope":"session","effect":{"type":"audio","asset":"audio.bgm","bus":"bgm","looped":true}}]),
        ),
        (
            "line",
            json!([{"id":"line","scope":"interaction","effect":{"type":"dialogue","text":"intro","reveal_us":"0"}}]),
        ),
        (
            "decorate",
            json!([
                {"id":"portrait","scope":"session","effect":{"type":"dialogue_decoration","slot":"portrait","image":{"asset":"portrait","size":[80,100],"placement":{"type":"absolute","point":[10,350]}}}},
                {"id":"name","scope":"session","effect":{"type":"dialogue_decoration","slot":"name","image":{"asset":"name","size":[120,30],"placement":{"type":"text_origin","offset":[0,-40]}}}},
                {"id":"hold","scope":"session","effect":{"type":"delay","duration_us":"20000"}}
            ]),
        ),
        (
            "remove",
            json!([{"id":"portrait","scope":"session","effect":{"type":"dialogue_decoration","slot":"portrait","image":null}}]),
        ),
    ] {
        p.cues.insert(
            id.into(),
            serde_json::from_value(json!({"effects":effects})).unwrap(),
        );
    }
    p.functions.get_mut("main").unwrap().entry = "music".into();
    p.functions.get_mut("main").unwrap().blocks = serde_json::from_value(json!({
        "music":{"terminator":{"type":"activate","cue":"music","next":"line"}},
        "line":{"terminator":{"type":"activate","cue":"line","next":"gate"}},
        "gate":{"terminator":{"type":"await","conditions":[{"task":"line","milestone":{"type":"marker","id":"portrait"}}],"next":"decorate","on_cancelled":"done","on_failed":"done"}},
        "decorate":{"terminator":{"type":"activate","cue":"decorate","next":"hold"}},
        "hold":{"terminator":{"type":"await","conditions":[{"task":"hold","milestone":{"type":"finished"}}],"next":"remove","on_cancelled":"done","on_failed":"done"}},
        "remove":{"terminator":{"type":"activate","cue":"remove","next":"read"}},
        "read":{"ops":[{"id":"continue","operation":{"type":"dialogue_continue","task":"line"}}],"terminator":{"type":"await","conditions":[{"task":"line","milestone":{"type":"finished"}}],"next":"done","on_cancelled":"done","on_failed":"done"}},
        "done":{"terminator":{"type":"end","outcome":"completed"}}
    })).unwrap();
    p
}

fn prepare(core: &mut Core) {
    while let Some(pending) = core.state().pending.clone() {
        core.step(
            CoreInput::Prepared {
                activation: pending.id,
            },
            1000,
        );
        assert!(core.state().fault.is_none(), "{:?}", core.state().fault);
    }
}

#[test]
fn prepared_images_preserve_live_reading_music_and_cold_restore() {
    let validated = ValidatedProgram::new(program()).unwrap();
    assert_eq!(
        validated.cue_assets("decorate"),
        ["name".into(), "portrait".into()].into_iter().collect()
    );
    let mut core = Core::new(validated.clone(), "decoration".into(), "en".into()).unwrap();
    core.step(CoreInput::None, 1000);
    prepare(&mut core);
    core.step(CoreInput::Time { delta_us: 100 }, 1000);
    prepare(&mut core);
    let music = core.state().handles["music"];
    let line = core.state().handles["line"];
    assert_eq!(core.dialogue().unwrap().1.visible_text(), "Before");
    assert_eq!(
        core.state().dialogue_decorations[&DialogueDecorationSlot::Portrait].rect,
        [10., 350., 80., 100.]
    );
    assert_eq!(
        core.state().dialogue_decorations[&DialogueDecorationSlot::Name].rect,
        [200., 450., 120., 30.]
    );
    assert_eq!(
        core.state().tasks[&core.state().handles["portrait"]].state,
        TaskState::Finished
    );
    assert_eq!(core.state().tasks[&line].state, TaskState::Running);
    let saved = core.snapshot();
    let mut restored = Core::restore(validated.clone(), saved.clone(), "decoration").unwrap();
    for current in [&mut core, &mut restored] {
        let step = current.step(CoreInput::Time { delta_us: 20000 }, 1000);
        assert!(!step.intents.iter().any(|i| matches!(
            i,
            CoreIntent::AudioStart { .. } | CoreIntent::AudioStop { .. }
        )));
        assert_eq!(current.state().handles["music"], music);
        assert_eq!(current.state().handles["line"], line);
        prepare(current);
        current.step(CoreInput::Time { delta_us: 100 }, 1000);
        assert!(!current
            .state()
            .dialogue_decorations
            .contains_key(&DialogueDecorationSlot::Portrait));
        assert!(current
            .state()
            .dialogue_decorations
            .contains_key(&DialogueDecorationSlot::Name));
        assert_eq!(current.state().tasks[&music].state, TaskState::Running);
        assert_eq!(current.dialogue().unwrap().1.visible_text(), "BeforeAfter");
    }
    // Restore intentionally refreshes input interaction identities; compare
    // live media, reading, geometry and clocks rather than those new tokens.
    assert_ne!(
        core.dialogue().unwrap().1.interaction,
        restored.dialogue().unwrap().1.interaction
    );
    assert_eq!(core.state().variables, restored.state().variables);
    assert_eq!(core.state().tick_us, restored.state().tick_us);
    assert_eq!(
        core.state().tasks[&music].elapsed_us,
        restored.state().tasks[&music].elapsed_us
    );
    assert_eq!(
        serde_json::to_value(&core.state().dialogue_decorations).unwrap(),
        serde_json::to_value(&restored.state().dialogue_decorations).unwrap()
    );
    let mut forged = saved.clone();
    forged
        .dialogue_decorations
        .get_mut(&DialogueDecorationSlot::Name)
        .unwrap()
        .asset = "audio.bgm".into();
    assert!(Core::restore(validated.clone(), forged, "decoration").is_err());
    let mut forged = saved;
    forged
        .dialogue_decorations
        .get_mut(&DialogueDecorationSlot::Portrait)
        .unwrap()
        .rect[2] += 1.;
    assert!(Core::restore(validated, forged, "decoration").is_err());
}

#[test]
fn decorations_reject_missing_capability_wrong_media_geometry_and_competing_writers() {
    let mut p = program();
    p.requires.retain(|c| c != "dialogue.decoration.v1");
    assert!(ValidatedProgram::new(p).is_err());
    let mut p = program();
    if let Effect::DialogueDecoration {
        image: Some(image), ..
    } = &mut p.cues.get_mut("decorate").unwrap().effects[0].effect
    {
        image.size[0] += 1;
    }
    assert!(ValidatedProgram::new(p).is_err());
    let mut p = program();
    if let Effect::DialogueDecoration {
        image: Some(image), ..
    } = &mut p.cues.get_mut("decorate").unwrap().effects[0].effect
    {
        image.asset = "audio.bgm".into();
    }
    assert!(ValidatedProgram::new(p).is_err());
    let mut p = program();
    let mut duplicate = p.cues["decorate"].effects[0].clone();
    duplicate.id = "another".into();
    p.cues.get_mut("decorate").unwrap().effects.push(duplicate);
    assert!(ValidatedProgram::new(p).is_err());
    let mut p = program();
    p.theme.dialogue.text_rect = None;
    assert!(ValidatedProgram::new(p).is_err());
}
