//! P0 acceptance fixtures (docs/NIR-NEXT-P0-BASELINE.md): original neutral
//! examples that must check, run their authored scenarios, derive exactly the
//! capabilities their content uses, and drive their menu entry functions
//! through the runtime to the replay outcome.
use nir_compiler::*;
use std::path::{Path, PathBuf};
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples").join(name)
}
fn requires(root: &Path) -> Vec<String> {
    load_project(root).unwrap().program.requires.clone()
}
fn assert_requires(root: &Path, caps: &[&str]) {
    let got = requires(root);
    for cap in caps {
        assert!(got.iter().any(|c| c == cap), "{cap} missing from {got:?}");
    }
}

#[test]
fn reading_lamp_checks_runs_scenarios_and_derives_dialogue_capabilities() {
    let root = fixture("reading-lamp");
    let p = load_project(&root).unwrap();
    let e = compile(&p.program).unwrap();
    validate_executable(&e).unwrap();
    assert_eq!(test_project(&root).unwrap(), ["sunrise", "rest"]);
    assert_requires(
        &root,
        &[
            "text.voice-binding.v1",
            "text.voice-timer.v1",
            "text.window-transition.v1",
            "audio.gain.v1",
            "audio.stop.v1",
            "story.typed-result.v1",
            "player.auto-delay-policy.v1",
        ],
    );
    // The authored fixed Auto policy survives into the program defaults.
    assert_eq!(p.program.player.auto_delay_policy, nir_format::AutoDelayPolicy::Fixed);
    assert_eq!(p.program.player.auto_delay_us.0, 2_500_000);
    // One explicit voice binding on the spoken page, sampled-remaining.
    let bindings: Vec<_> = p
        .program
        .functions
        .values()
        .flat_map(|f| f.blocks.values())
        .flat_map(|b| &b.ops)
        .filter_map(|op| {
            if let nir_format::Operation::DialogueVoice { wait, .. } = &op.operation {
                Some(*wait)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(bindings, [nir_format::VoiceWaitPolicy::SampledRemaining]);
}

#[test]
fn replay_atlas_checks_runs_tour_and_derives_menu_capabilities() {
    let root = fixture("replay-atlas");
    let p = load_project(&root).unwrap();
    let e = compile(&p.program).unwrap();
    validate_executable(&e).unwrap();
    assert_eq!(test_project(&root).unwrap(), ["tour"]);
    assert_requires(
        &root,
        &[
            "ui.replay.v1",
            "ui.menu-effects.v1",
            "ui.menu-transition.v1",
            "ui.menu-element-tween.v1",
        ],
    );
    // The story grants exactly atlas.north; atlas.south stays a locked entry.
    let unlocks: std::collections::BTreeSet<_> = p
        .program
        .functions
        .values()
        .flat_map(|f| f.blocks.values())
        .flat_map(|b| &b.ops)
        .filter_map(|op| {
            if let nir_format::Operation::ProfileMerge { key } = &op.operation {
                Some(key.clone())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(unlocks, ["atlas.north".to_string()].into_iter().collect::<std::collections::BTreeSet<_>>());
    let gallery = &p.program.theme.image_menus["gallery"];
    let guard = |id: &str| {
        gallery
            .elements
            .iter()
            .find(|el| el.id == id)
            .and_then(|el| match &el.content {
                nir_format::MenuContent::Button { requires, .. } => requires.clone(),
                _ => None,
            })
            .unwrap()
    };
    assert_eq!(guard("north"), "atlas.north");
    assert_eq!(guard("south"), "atlas.south");
    assert!(unlocks.contains(&guard("north")));
    assert!(!unlocks.contains(&guard("south")));
    assert!(gallery.effects.as_ref().unwrap().music.is_some());
}

#[test]
fn replay_atlas_entry_functions_finish_replay_completed() {
    let p = load_project(&fixture("replay-atlas")).unwrap();
    let validated = nir_core::ValidatedProgram::new(p.program.clone()).unwrap();
    for entry in ["replay_north", "replay_south"] {
        assert_eq!(&drive(&validated, entry), "replay_completed");
    }
    assert_eq!(&drive(&validated, &p.program.entry), "completed");
}

/// Advance/feed loop shared with the corpus harness: prepared activations,
/// gate ticks, dialogue advances, and time for everything else.
fn drive(validated: &nir_core::ValidatedProgram, entry: &str) -> String {
    let mut core = nir_core::Core::new_at(
        validated.clone(),
        "p0".into(),
        "zh-Hans".into(),
        entry,
    )
    .unwrap();
    for sequence in 1..100_000 {
        let input = if let Some(pending) = &core.state().pending {
            nir_core::CoreInput::Prepared {
                activation: pending.id,
            }
        } else if let Some((_, d)) = core.dialogue() {
            if d.at_gate {
                nir_core::CoreInput::Time {
                    delta_us: 1_000_000,
                }
            } else {
                nir_core::CoreInput::Advance {
                    interaction: d.interaction,
                    sequence,
                }
            }
        } else {
            nir_core::CoreInput::Time {
                delta_us: 1_000_000,
            }
        };
        core.step(input, 10_000);
        assert!(core.state().fault.is_none(), "{entry}: {:?}", core.state().fault);
        if let Some(outcome) = core.state().outcome.clone() {
            return outcome;
        }
    }
    panic!("{entry} did not finish");
}
