use nir_core::*;
use nir_format::*;
use serde_json::json;

fn program() -> Program {
    let mut p: Program = serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    p.requires
        .extend(["dialogue.style.v1".into(), "text.rect.v1".into()]);
    p.theme.dialogue_styles.insert("alternate".into(), serde_json::from_value(json!({
        "dialogue":{"background":"bg.station","rect":[20,20,700,200],"text_rect":[30,30,650,150],"height":200,"padding":0,"font_size":28,"line_height":1.2,"opacity":0.7},
        "text":[0.1,0.2,0.3,1]
    })).unwrap());
    p.cues.insert("music".into(), serde_json::from_value(json!({"effects":[{"id":"music","scope":"session","effect":{"type":"audio","asset":"audio.bgm","bus":"bgm","looped":true}}]})).unwrap());
    p.cues.insert("switch".into(), serde_json::from_value(json!({"effects":[{"id":"switch","scope":"session","effect":{"type":"dialogue_style","style":"alternate"}}]})).unwrap());
    for (cue, text) in [("first", "intro"), ("second", "arrival")] {
        p.cues.insert(cue.into(), serde_json::from_value(json!({"effects":[{"id":"line","scope":"interaction","effect":{"type":"dialogue","text":text,"reveal_us":"0"}}]})).unwrap());
    }
    p.functions.get_mut("main").unwrap().entry = "boot".into();
    p.functions.get_mut("main").unwrap().blocks = serde_json::from_value(json!({
        "boot":{"terminator":{"type":"activate","cue":"music","next":"first"}},
        "first":{"terminator":{"type":"activate","cue":"first","next":"wait"}},
        "wait":{"terminator":{"type":"await","conditions":[{"task":"line","milestone":{"type":"finished"}}],"next":"switch","on_cancelled":"done","on_failed":"done"}},
        "switch":{"terminator":{"type":"activate","cue":"switch","next":"second"}},
        "second":{"terminator":{"type":"activate","cue":"second","next":"hold"}},
        "hold":{"terminator":{"type":"await","conditions":[{"task":"line","milestone":{"type":"finished"}}],"next":"done","on_cancelled":"done","on_failed":"done"}},
        "done":{"terminator":{"type":"end","outcome":"done"}}
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
    }
}

#[test]
fn prepared_style_switch_keeps_music_history_and_cold_restore() {
    let p = ValidatedProgram::new(program()).unwrap();
    assert!(p.cue_assets("switch").contains("bg.station"));
    let mut core = Core::new(p.clone(), "style".into(), "en".into()).unwrap();
    core.step(CoreInput::None, 1000);
    prepare(&mut core);
    let music = core.state().handles["music"];
    for sequence in 1..=4 {
        let interaction = core.dialogue().unwrap().1.interaction;
        core.step(
            CoreInput::Advance {
                interaction,
                sequence,
            },
            1000,
        );
        assert!(core.state().fault.is_none(), "{:?}", core.state().fault);
        if core.state().pending.is_some() {
            break;
        }
    }
    assert_eq!(core.state().pending.as_ref().unwrap().cue, "switch");
    assert_eq!(core.state().dialogue_style, None);
    let activation = core.state().pending.as_ref().unwrap().id;
    core.step(CoreInput::Prepared { activation }, 1000);
    assert_eq!(core.state().dialogue_style.as_deref(), Some("alternate"));
    assert!(core.dialogue().is_none());
    assert_eq!(
        core.state().tasks[&core.state().handles["switch"]].state,
        TaskState::Finished
    );
    prepare(&mut core);
    assert_eq!(core.state().handles["music"], music);
    assert_eq!(core.state().tasks[&music].state, TaskState::Running);
    assert_eq!(core.state().history.len(), 2);
    let saved = core.snapshot();
    let restored = Core::restore(p.clone(), saved.clone(), "style").unwrap();
    assert_eq!(
        restored.state().dialogue_style.as_deref(),
        Some("alternate")
    );
    assert_eq!(restored.dialogue().unwrap().1.text_id, "arrival");
    let mut forged = saved;
    forged.dialogue_style = Some("unknown".into());
    assert!(Core::restore(p, forged, "style").is_err());
}

#[test]
fn styles_require_capability_known_definition_and_exclusive_window() {
    let mut p = program();
    p.requires.retain(|cap| cap != "dialogue.style.v1");
    assert!(ValidatedProgram::new(p).is_err());
    let mut p = program();
    p.cues.get_mut("switch").unwrap().effects[0].effect = Effect::DialogueStyle {
        style: "unknown".into(),
    };
    assert!(ValidatedProgram::new(p).is_err());
    let mut p = program();
    let mut duplicate = p.cues["switch"].effects[0].clone();
    duplicate.id = "duplicate".into();
    p.cues.get_mut("switch").unwrap().effects.push(duplicate);
    assert!(ValidatedProgram::new(p).is_err());
    let mut p = program();
    let line = p.cues["first"].effects[0].clone();
    p.cues.get_mut("switch").unwrap().effects.push(line);
    assert!(ValidatedProgram::new(p).is_err());
}
