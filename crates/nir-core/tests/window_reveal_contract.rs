//! P2.4 text.window-transition.v1: a styled message-window flip defers its
//! committed value to the reveal deadline, captures interrupted coverage on
//! reversal, keeps demanding ticks while parked at an input barrier, and
//! restores only structurally valid in-flight reveals.
use nir_core::*;
use nir_format::*;
use serde_json::json;

/// The entry block commits `ops` and then parks on the intro dialogue twice so
/// story time can pass between the committed operations.
fn program(ops: serde_json::Value, capability: bool) -> Program {
    let mut p: Program = serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    if capability {
        p.requires.push("text.window-transition.v1".into());
    }
    let f = p.functions.get_mut("main").unwrap();
    f.entry = "test".into();
    f.blocks.insert(
        "test".into(),
        serde_json::from_value(json!({
            "ops":[{"id":"reveal","operation":ops}],
            "terminator":{"type":"activate","cue":"intro","next":"hold1"}
        }))
        .unwrap(),
    );
    for block in ["hold1", "hold2"] {
        f.blocks.insert(
            block.into(),
            serde_json::from_value(json!({
                "terminator":{"type":"await","conditions":[{"task":"line","milestone":{"type":"finished"}}],
                    "next":if block=="hold1" {"reverse"} else {"done"},
                    "on_cancelled":"done","on_failed":"done"}
            }))
            .unwrap(),
        );
    }
    f.blocks.insert(
        "reverse".into(),
        serde_json::from_value(json!({
            "ops":[{"id":"reverse","operation":{
                "type":"dialogue_visibility","visible":true,
                "transition":{"type":"dissolve"},"duration_us":"1000000"}}],
            "terminator":{"type":"activate","cue":"intro","next":"hold2"}
        }))
        .unwrap(),
    );
    f.blocks.insert(
        "done".into(),
        serde_json::from_value(json!({"terminator":{"type":"end","outcome":"done"}})).unwrap(),
    );
    p
}

fn hide_op() -> serde_json::Value {
    json!({
        "type":"dialogue_visibility","visible":false,
        "transition":{"type":"dissolve"},"duration_us":"1000000"
    })
}

fn start(p: Program) -> Core {
    let c = Core::new(
        ValidatedProgram::new(p).unwrap(),
        "reveal".into(),
        "zh-Hans".into(),
    )
    .unwrap();
    assert!(c.state().fault.is_none());
    c
}

/// Drive until the VM parks on the awaited dialogue: activation prepared and
/// the dialogue interaction offered.
fn park(c: &mut Core) {
    let activation = c.state().pending.as_ref().unwrap().id;
    c.step(CoreInput::Prepared { activation }, 1000);
    assert!(c.state().fault.is_none(), "{:?}", c.state().fault);
    assert!(
        c.dialogue().is_some(),
        "dialogue must be offered to park on"
    );
}

/// One press completes a still-running text reveal, the second finishes the
/// dialogue task and releases the await.
fn advance_dialogue(c: &mut Core, sequence: u32) {
    for seq in [sequence, sequence + 1] {
        let Some((_, d)) = c.dialogue() else { return };
        c.step(
            CoreInput::Advance {
                interaction: d.interaction,
                sequence: seq,
            },
            1000,
        );
        assert!(c.state().fault.is_none(), "{:?}", c.state().fault);
    }
}

/// The committed flag lands only at the deadline; coverage progress is linear
/// and the reveal keeps the clock demand alive while the reader holds input.
#[test]
fn styled_flip_defers_commit_and_completes_at_the_deadline() {
    let mut c = start(program(hide_op(), true));
    c.step(CoreInput::None, 1000);
    assert!(!c.state().dialogue_hidden, "pre-op window visible");
    park(&mut c);
    let (style, to_visible, progress) = c.window_reveal().expect("reveal in flight");
    assert_eq!(*style, StageTransition::Dissolve);
    assert!(!to_visible);
    assert_eq!(progress, 0.);
    assert!(!c.state().dialogue_hidden, "hide commits at the deadline");
    assert!(c.needs_clock(), "reveal demands ticks while parked");
    for (delta, expected) in [(500_000, 0.5), (400_000, 0.9)] {
        c.step(CoreInput::Time { delta_us: delta }, 1000);
        let (_, _, progress) = c.window_reveal().expect("reveal still in flight");
        assert!((progress - expected as f32).abs() < 0.0001, "{progress}");
    }
    // The remaining slice lands exactly on the deadline: the reveal joins the
    // story clock's next-wakeup computation even though the VM is parked.
    c.step(CoreInput::Time { delta_us: 100_000 }, 1000);
    assert!(c.window_reveal().is_none(), "reveal completed");
    assert!(
        c.state().dialogue_hidden,
        "hidden flag commits at the deadline"
    );
    assert!(c.state().tick_us <= Micros(1_000_000));
}

/// A second styled op mid-flight reverses from the interrupted visual state:
/// from_coverage captures 0.5 so the show continues instead of restarting.
#[test]
fn reversal_captures_interrupted_coverage() {
    let mut c = start(program(hide_op(), true));
    c.step(CoreInput::None, 1000);
    park(&mut c);
    c.step(CoreInput::Time { delta_us: 500_000 }, 1000);
    let (_, _, progress) = c.window_reveal().unwrap();
    assert!((progress - 0.5).abs() < 0.0001);
    advance_dialogue(&mut c, 1);
    // The reverse block ran at the 0.5s mark; its activation parks again.
    park(&mut c);
    let reveal = c.state().window_reveal.as_ref().expect("reversal reveal");
    assert!(reveal.to_visible);
    assert!(
        (reveal.from_coverage - 0.5).abs() < 0.0001,
        "reversal continues from the interrupted coverage"
    );
    assert_eq!(reveal.started_us, c.state().tick_us);
    c.step(
        CoreInput::Time {
            delta_us: 1_100_000,
        },
        1000,
    );
    assert!(c.window_reveal().is_none());
    assert!(!c.state().dialogue_hidden, "show completed");
}

/// Without an actual flip there is nothing to animate: a styled op that
/// matches the committed state (with no reveal in flight) commits instantly.
#[test]
fn redundant_styled_op_commits_immediately() {
    let mut c = start(program(
        json!({"type":"dialogue_visibility","visible":true,
            "transition":{"type":"dissolve"},"duration_us":"1000000"}),
        true,
    ));
    c.step(CoreInput::None, 1000);
    assert!(
        c.window_reveal().is_none(),
        "already-visible window has no reveal"
    );
    assert!(!c.state().dialogue_hidden);
}

#[test]
fn instant_flip_interrupts_an_in_flight_reveal() {
    let mut p = program(hide_op(), true);
    let f = p.functions.get_mut("main").unwrap();
    f.blocks.insert(
        "reverse".into(),
        serde_json::from_value(json!({
            "ops":[{"id":"reverse","operation":{"type":"dialogue_visibility","visible":false}}],
            "terminator":{"type":"activate","cue":"intro","next":"hold2"}
        }))
        .unwrap(),
    );
    let mut c = Core::new(
        ValidatedProgram::new(p).unwrap(),
        "reveal".into(),
        "zh-Hans".into(),
    )
    .unwrap();
    c.step(CoreInput::None, 1000);
    park(&mut c);
    c.step(CoreInput::Time { delta_us: 500_000 }, 1000);
    assert!(c.window_reveal().is_some());
    advance_dialogue(&mut c, 1);
    assert!(
        c.window_reveal().is_none(),
        "bare op interrupts the in-flight reveal"
    );
    assert!(c.state().dialogue_hidden);
}

/// Validation gates the capability, duration range, and mask asset kind at
/// program level; restore re-checks the in-flight reveal structurally.
#[test]
fn validation_and_restore_gate_the_reveal() {
    // Capability is required for any styled op.
    assert_eq!(
        ValidatedProgram::new(program(hide_op(), false))
            .unwrap_err()
            .code,
        "E_CAPABILITY"
    );
    for duration_us in ["0", "60000001"] {
        let mut p = program(hide_op(), true);
        let f = p.functions.get_mut("main").unwrap();
        let block = f.blocks.get_mut("test").unwrap();
        block.ops[0].operation = serde_json::from_value(json!({
            "type":"dialogue_visibility","visible":false,
            "transition":{"type":"dissolve"},"duration_us":duration_us
        }))
        .unwrap();
        assert_eq!(
            ValidatedProgram::new(p).unwrap_err().code,
            "E_TIME",
            "{duration_us}"
        );
    }
    // A mask reveal must name an image asset.
    let mut p = program(hide_op(), true);
    let f = p.functions.get_mut("main").unwrap();
    let block = f.blocks.get_mut("test").unwrap();
    block.ops[0].operation = serde_json::from_value(json!({
        "type":"dialogue_visibility","visible":false,
        "transition":{"type":"mask","asset":"audio.bgm","channel":"alpha"},
        "duration_us":"1000000"
    }))
    .unwrap();
    assert_eq!(ValidatedProgram::new(p).unwrap_err().code, "E_ASSET");

    // A mid-flight mask reveal round trips through the snapshot; tampered
    // structure is refused at restore.
    let p = program(
        json!({
            "type":"dialogue_visibility","visible":false,
            "transition":{"type":"mask","asset":"bg.station","channel":"alpha"},
            "duration_us":"1000000"
        }),
        true,
    );
    let mut c = Core::new(
        ValidatedProgram::new(p).unwrap(),
        "reveal".into(),
        "zh-Hans".into(),
    )
    .unwrap();
    c.step(CoreInput::None, 1000);
    park(&mut c);
    c.step(CoreInput::Time { delta_us: 500_000 }, 1000);
    let snapshot = c.snapshot();
    let restored = Core::restore(c.validated_program().clone(), snapshot.clone(), "reveal")
        .expect("valid mid-flight reveal restores");
    let (_, to_visible, progress) = restored.window_reveal().unwrap();
    assert!(!to_visible);
    assert!((progress - 0.5).abs() < 0.0001);
    let tamper = |edit: &dyn Fn(&mut Snapshot)| {
        let mut bad = snapshot.clone();
        edit(&mut bad);
        match Core::restore(restored.validated_program().clone(), bad, "reveal") {
            Err(e) => e.to_string(),
            Ok(_) => panic!("tampered reveal must not restore"),
        }
    };
    let reveal_err = |edit: &dyn Fn(&mut Snapshot)| {
        let err = tamper(edit);
        assert!(err.contains("invalid window reveal"), "{err}");
    };
    reveal_err(&|s: &mut Snapshot| {
        s.window_reveal.as_mut().unwrap().duration_us = Micros(0);
    });
    reveal_err(&|s: &mut Snapshot| {
        s.window_reveal.as_mut().unwrap().from_coverage = 1.5;
    });
    reveal_err(&|s: &mut Snapshot| {
        let reveal = s.window_reveal.as_mut().unwrap();
        reveal.started_us = Micros(0);
        reveal.duration_us = Micros(100_000);
    });
    let mask_err = tamper(&|s: &mut Snapshot| {
        s.window_reveal.as_mut().unwrap().style = StageTransition::Mask {
            asset: "audio.bgm".into(),
            channel: MaskChannel::Alpha,
            invert: false,
            softness: 0.,
        };
    });
    assert!(
        mask_err.contains("invalid window reveal mask"),
        "{mask_err}"
    );
    reveal_err(&|s: &mut Snapshot| {
        s.window_reveal.as_mut().unwrap().duration_us = Micros(60_000_001);
    });
    reveal_err(&|s: &mut Snapshot| {
        let reveal = s.window_reveal.as_mut().unwrap();
        reveal.started_us = Micros(s.tick_us.0 + 1);
    });
    // The capability must still be declared by the restoring program even
    // when its own ops never used a transition.
    let mut bare = program(json!({"type":"dialogue_visibility","visible":false}), false);
    let f = bare.functions.get_mut("main").unwrap();
    let block = f.blocks.get_mut("reverse").unwrap();
    block.ops[0].operation =
        serde_json::from_value(json!({"type":"dialogue_visibility","visible":false})).unwrap();
    let restored_vp = ValidatedProgram::new(bare).unwrap();
    let err = match Core::restore(restored_vp, snapshot, "reveal") {
        Err(e) => e.to_string(),
        Ok(_) => panic!("capability-less program must not adopt a reveal"),
    };
    assert!(err.contains("invalid window reveal"), "{err}");
}

#[test]
fn v2_runtime_code_admission_validates_window_reveal() {
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;

    let build_runtime = |ops: serde_json::Value, capability: bool| {
        let mut p = program(ops, capability);
        for (id, asset) in &mut p.assets {
            asset.object = format!("{:064x}", id.bytes().map(u64::from).sum::<u64>() + 1000);
            asset.decoded_bytes = if asset.kind == AssetKind::Image {
                asset.width as u64 * asset.height as u64 * 4
            } else {
                1
            };
        }
        let media: BTreeMap<_, _> = p
            .assets
            .iter()
            .map(|(id, a)| (id.clone(), a.object.clone()))
            .collect();
        for plan in p
            .locale_config
            .ui
            .values_mut()
            .chain(p.locale_config.text.values_mut())
        {
            plan.digest = LocaleFontPlan::digest_for(&plan.fonts, &media);
        }
        let owner = "story".to_owned();
        let owners = |ids: Vec<String>| ids.into_iter().map(|id| (id, owner.clone())).collect();
        let code_pkg = ModuleCode {
            format: 2,
            module: owner.clone(),
            functions: p.functions.clone(),
        };
        let code_bytes = serde_json::to_vec(&code_pkg).unwrap();
        let code_hash = format!("{:x}", Sha256::digest(&code_bytes));
        let static_pkg = ModuleStatic {
            format: 2,
            module: owner.clone(),
            scenes: p.scenes.clone(),
            cues: p.cues.clone(),
            choices: p.choices.clone(),
            text_contracts: p.texts.clone(),
            activation_recipes: p
                .cues
                .keys()
                .map(|cue| (cue.clone(), p.cue_media_assets(cue)))
                .collect(),
            sprite_timelines: BTreeMap::new(),
        };
        let static_bytes = serde_json::to_vec(&static_pkg).unwrap();
        let static_hash = format!("{:x}", Sha256::digest(&static_bytes));
        let assets = p
            .assets
            .iter()
            .map(|(id, a)| {
                (
                    id.clone(),
                    AssetIndexEntry {
                        kind: a.kind,
                        object: a.object.clone(),
                        catalog: "main".into(),
                    },
                )
            })
            .collect();
        let root = RuntimeProgram {
            timeline_owners: BTreeMap::new(),
            format: 2,
            game_id: p.game_id,
            revision: p.revision,
            entry: p.entry,
            requires: p.requires,
            stage: p.stage,
            variables: p.variables,
            function_index: p
                .functions
                .iter()
                .map(|(id, f)| {
                    (
                        id.clone(),
                        RuntimeFunctionIndex {
                            execution_module: None,
                            module: owner.clone(),
                            signature: FunctionSignature::from(f),
                        },
                    )
                })
                .collect(),
            modules: BTreeMap::from([(
                owner.clone(),
                ModuleIndex {
                    functions: p
                        .functions
                        .iter()
                        .map(|(id, f)| (id.clone(), FunctionSignature::from(f)))
                        .collect(),
                    texts: p.texts.keys().cloned().collect(),
                    code: code_hash,
                    static_content: static_hash,
                    locales: p
                        .locales
                        .keys()
                        .enumerate()
                        .map(|(i, locale)| (locale.clone(), format!("{:064x}", i + 10)))
                        .collect(),
                },
            )]),
            scene_owners: owners(p.scenes.keys().cloned().collect()),
            cue_owners: owners(p.cues.keys().cloned().collect()),
            choice_owners: owners(p.choices.keys().cloned().collect()),
            text_owners: owners(p.texts.keys().cloned().collect()),
            task_owners: owners(
                p.cues
                    .values()
                    .flat_map(|c| c.effects.iter().map(|e| e.id.clone()))
                    .collect(),
            ),
            text_contracts: p
                .texts
                .iter()
                .map(|(id, t)| {
                    (
                        id.clone(),
                        RuntimeTextIdentity {
                            images: vec![],
                            module: owner.clone(),
                            source_revision: t.source_revision,
                            contract_revision: t.contract_revision,
                            meaning_revision: t.meaning_revision,
                            contract_digest: t.contract_digest.clone(),
                        },
                    )
                })
                .collect(),
            locales: p.locales.keys().cloned().collect(),
            locale_config: p.locale_config,
            assets,
            catalogs: BTreeMap::from([("main".into(), format!("{:064x}", 2))]),
            default_locale: p.default_locale,
            title_scene: p
                .title_scene
                .clone()
                .or_else(|| p.scenes.keys().next().cloned()),
            title_nodes: p
                .title_scene
                .as_ref()
                .and_then(|id| p.scenes.get(id))
                .or_else(|| p.scenes.values().next())
                .cloned()
                .unwrap_or_default(),
            theme: p.theme,
            player: p.player,
        };
        let vp = ValidatedProgram::from_runtime(root).unwrap();
        vp.install_batch(vec![
            (
                ContentKey::Static {
                    module: owner.clone(),
                },
                RuntimeObject::Static(static_pkg),
                static_bytes.len() as u64,
            ),
            (
                ContentKey::Code { module: owner },
                RuntimeObject::Code(code_pkg),
                code_bytes.len() as u64,
            ),
        ])
    };

    assert_eq!(
        build_runtime(hide_op(), false).unwrap_err().code,
        "E_CAPABILITY"
    );
    assert_eq!(
        build_runtime(
            json!({
                "type":"dialogue_visibility","visible":false,
                "transition":{"type":"dissolve"},"duration_us":"0"
            }),
            true
        )
        .unwrap_err()
        .code,
        "E_TIME"
    );
    assert_eq!(
        build_runtime(
            json!({
                "type":"dialogue_visibility","visible":false,
                "transition":{"type":"dissolve"},"duration_us":"60000001"
            }),
            true
        )
        .unwrap_err()
        .code,
        "E_TIME"
    );
    assert_eq!(
        build_runtime(
            json!({
                "type":"dialogue_visibility","visible":false,
                "transition":{"type":"mask","asset":"audio.bgm","channel":"alpha"},
                "duration_us":"1000000"
            }),
            true
        )
        .unwrap_err()
        .code,
        "E_ASSET"
    );
    assert!(build_runtime(hide_op(), true).is_ok());
}
