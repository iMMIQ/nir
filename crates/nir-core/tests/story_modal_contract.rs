use nir_core::*;
use nir_format::*;
use serde_json::json;

fn program() -> Program {
    let mut p: Program = serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    p.requires = CAPABILITIES.iter().map(|s| (*s).into()).collect();
    p.cues.insert("owner".into(), serde_json::from_value(json!({"effects":[
        {"id":"music","scope":"session","effect":{"type":"audio","asset":"audio.bgm","bus":"bgm","looped":true}},
        {"id":"modal","scope":"session","effect":{"type":"story_modal","target":{"type":"load_saves"}}}
    ]})).unwrap());
    p.cues.insert("continuation".into(), serde_json::from_value(json!({"effects":[
        {"id":"continuation","scope":"session","effect":{"type":"delay","duration_us":"1000000"}}
    ]})).unwrap());
    p.functions.get_mut("main").unwrap().blocks = serde_json::from_value(json!({
        "start":{"terminator":{"type":"activate","cue":"owner","next":"wait"}},
        "wait":{"terminator":{"type":"await","conditions":[{"task":"modal","milestone":{"type":"finished"}}],"next":"done","on_cancelled":"done","on_failed":"done"}},
        "done":{"terminator":{"type":"activate","cue":"continuation","next":"hold"}},
        "hold":{"terminator":{"type":"await","conditions":[{"task":"continuation","milestone":{"type":"finished"}}],"next":"end","on_cancelled":"end","on_failed":"end"}},
        "end":{"terminator":{"type":"end","outcome":"done"}}
    })).unwrap();
    p
}

#[test]
fn modal_wait_and_cold_restore_reject_stale_close_without_restarting_music() {
    let p = ValidatedProgram::new(program()).unwrap();
    let mut core = Core::new(p.clone(), "modal".into(), "en".into()).unwrap();
    core.step(CoreInput::None, 1000);
    let activation = core.state().pending.as_ref().unwrap().id;
    core.step(CoreInput::Prepared { activation }, 1000);
    let task = core.state().handles["modal"];
    let music = core.state().handles["music"];
    let old = core.state().tasks[&task].modal_interaction.unwrap();
    core.step(
        CoreInput::Time {
            delta_us: 1_000_000,
        },
        1000,
    );
    assert_eq!(core.state().tasks[&task].state, TaskState::Running);
    assert!(core.state().outcome.is_none());
    let saved = core.snapshot();
    let mut forged = saved.clone();
    forged.tasks.get_mut(&task).unwrap().modal_interaction = None;
    assert!(Core::restore(p.clone(), forged, "modal").is_err());
    let mut restored = Core::restore(p, saved, "modal").unwrap();
    restored.step(
        CoreInput::ModalClosed {
            task,
            interaction: old,
        },
        1000,
    );
    assert_eq!(restored.state().tasks[&task].state, TaskState::Running);
    let interaction = restored.state().tasks[&task].modal_interaction.unwrap();
    let output = restored.step(CoreInput::ModalClosed { task, interaction }, 1000);
    assert_eq!(
        restored.state().pending.as_ref().unwrap().cue,
        "continuation"
    );
    assert_eq!(restored.state().handles["music"], music);
    assert_eq!(restored.state().tasks[&music].state, TaskState::Running);
    assert!(!output.intents.iter().any(|intent| matches!(
        intent,
        CoreIntent::AudioStart { .. } | CoreIntent::AudioStop { .. }
    )));
}

#[test]
fn modal_requires_capability_known_menu_and_exclusive_cue() {
    let mut p = program();
    p.requires.retain(|c| c != "ui.story-modal.v1");
    assert!(ValidatedProgram::new(p).is_err());
    let mut p = program();
    p.cues.get_mut("owner").unwrap().effects[1].effect = Effect::StoryModal {
        target: StoryModalTarget::ImageMenu {
            menu: "missing".into(),
        },
    };
    assert!(ValidatedProgram::new(p).is_err());
    let mut p = program();
    let mut duplicate = p.cues["owner"].effects[1].clone();
    duplicate.id = "other".into();
    p.cues.get_mut("owner").unwrap().effects.push(duplicate);
    assert!(ValidatedProgram::new(p).is_err());
}
