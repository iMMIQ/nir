use nir_core::*;
use nir_format::*;
fn program() -> Program {
    serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap()
}
fn core() -> Core {
    Core::new(
        ValidatedProgram::new(program()).unwrap(),
        "test-release".into(),
        "zh-Hans".into(),
    )
    .unwrap()
}
#[test]
fn bitmap_digits_capture_explicit_updates_and_restore_frozen_geometry() {
    let mut p = program();
    p.requires.push("stage.bitmap-text.v1".into());
    p.variables.insert("number".into(), Value::I32(12));
    p.variables.insert("captured".into(), Value::I32(0));
    let node: Node = serde_json::from_value(serde_json::json!({
        "id":"digits","asset":"bg.station","x":101,"y":51,"width":0,"height":0,
        "bitmap_text":{"slot":"captured","alphabet":"0123456789","cell":[10,20],"line_spacing":2,
            "align":"start","x_anchor":"end","y_anchor":"center"}
    }))
    .unwrap();
    p.scenes.get_mut("station").unwrap().push(node);
    p.cues.insert("redraw".into(), serde_json::from_value(serde_json::json!({"effects":[
        {"id":"redraw","scope":"session","effect":{"type":"stage_present","scene":"station","duration_us":"0"}}
    ]})).unwrap());
    p.cues.insert(
        "hold".into(),
        serde_json::from_value(serde_json::json!({"effects":[
            {"id":"hold","scope":"session","effect":{"type":"delay","duration_us":"1000000000"}}
        ]}))
        .unwrap(),
    );
    p.functions.get_mut("main").unwrap().blocks = serde_json::from_value(serde_json::json!({
        "start":{"ops":[{"id":"capture","operation":{"type":"assign","target":"captured","value":{"type":"var","name":"number"}}}],"terminator":{"type":"activate","cue":"opening","next":"changed"}},
        "changed":{"ops":[{"id":"source-changed","operation":{"type":"assign","target":"number","value":{"type":"const","value":{"type":"i32","value":34}}}}],"terminator":{"type":"activate","cue":"intro","next":"wait"}},
        "wait":{"ops":[],"terminator":{"type":"await","conditions":[{"task":"line","milestone":{"type":"finished"}}],"next":"redraw","on_cancelled":"cancelled","on_failed":"failed"}},
        "redraw":{"ops":[],"terminator":{"type":"activate","cue":"redraw","next":"updated"}},
        "updated":{"ops":[{"id":"recapture","operation":{"type":"assign","target":"captured","value":{"type":"var","name":"number"}}}],"terminator":{"type":"activate","cue":"redraw","next":"hold"}},
        "hold":{"ops":[],"terminator":{"type":"activate","cue":"hold","next":"wait-hold"}},
        "wait-hold":{"ops":[],"terminator":{"type":"await","conditions":[{"task":"hold","milestone":{"type":"finished"}}],"next":"cancelled","on_cancelled":"cancelled","on_failed":"failed"}},
        "cancelled":{"ops":[],"terminator":{"type":"end","outcome":"done"}},
        "failed":{"ops":[],"terminator":{"type":"fault","code":"E_TEST","message":"failed"}}
    })).unwrap();
    let mut bad = p.clone();
    bad.requires.retain(|cap| cap != "stage.bitmap-text.v1");
    assert_eq!(ValidatedProgram::new(bad).unwrap_err().code, "E_CAPABILITY");
    let mut bad = p.clone();
    bad.variables.insert("captured".into(), Value::Bool(false));
    assert_eq!(
        ValidatedProgram::new(bad).unwrap_err().code,
        "E_BITMAP_TEXT"
    );
    let validated = ValidatedProgram::new(p).unwrap();
    let mut c = Core::new(validated.clone(), "bitmap".into(), "en".into()).unwrap();
    c.step(CoreInput::None, 1000);
    for _ in 0..2 {
        let id = c.state().pending.as_ref().unwrap().id;
        c.step(CoreInput::Prepared { activation: id }, 1000);
    }
    assert_eq!(c.state().variables["number"], Value::I32(34));
    let scene = c.state().scene.clone();
    assert!(scene.iter().all(|node| node.bitmap_text.is_none()));
    let group = scene.iter().find(|n| n.id == "digits").unwrap();
    assert_eq!(
        [group.x, group.y, group.width, group.height],
        [81., 15., 20., 20.]
    );
    let clips: Vec<_> = scene
        .iter()
        .filter(|n| n.parent.as_deref() == Some("digits"))
        .map(|n| n.clip.unwrap())
        .collect();
    assert_eq!(clips, [[10., 0., 10., 20.], [20., 0., 10., 20.]]);
    let saved = c.snapshot();
    let music = saved.handles["music"];
    let mut forged = saved.clone();
    forged.scene[0].bitmap_text = Some(BitmapText {
        slot: "captured".into(),
        alphabet: "01".into(),
        cell: [10, 20],
        line_spacing: 0,
        align: BitmapAnchor::Start,
        x_anchor: BitmapAnchor::Start,
        y_anchor: BitmapAnchor::Start,
    });
    assert!(Core::restore(validated.clone(), forged, "bitmap").is_err());
    let mut restored = Core::restore(validated, saved, "bitmap").unwrap();
    assert_eq!(restored.state().scene, scene);
    restored.step(
        CoreInput::Time {
            delta_us: 10_000_000,
        },
        1000,
    );
    let interaction = restored.dialogue().unwrap().1.interaction;
    restored.step(
        CoreInput::Advance {
            interaction,
            sequence: 1,
        },
        1000,
    );
    restored.step(
        CoreInput::Advance {
            interaction,
            sequence: 2,
        },
        1000,
    );
    let id = restored.state().pending.as_ref().unwrap().id;
    restored.step(CoreInput::Prepared { activation: id }, 1000);
    assert_eq!(restored.state().scene, scene); // An unrelated redraw keeps the capture.
    for _ in 0..2 {
        let id = restored.state().pending.as_ref().unwrap().id;
        restored.step(CoreInput::Prepared { activation: id }, 1000);
    }
    let clips: Vec<_> = restored
        .state()
        .scene
        .iter()
        .filter(|n| n.parent.as_deref() == Some("digits"))
        .map(|n| n.clip.unwrap())
        .collect();
    assert_eq!(clips, [[30., 0., 10., 20.], [40., 0., 10., 20.]]);
    assert_eq!(restored.state().handles["music"], music);
    assert_eq!(restored.state().tasks[&music].state, TaskState::Running);
}
#[test]
fn mutable_profile_values_keep_frozen_slots_and_allow_zero_after_one() {
    let mut p = program();
    p.requires.push("story.profile-value.v1".into());
    p.variables.insert("value".into(), Value::I32(7));
    p.functions.get_mut("main").unwrap().entry = "read".into();
    p.functions.get_mut("main").unwrap().blocks=serde_json::from_value(serde_json::json!({
        "read":{"ops":[{"id":"read","operation":{"type":"profile_value_read","target":"value","key":"mutable"}}],"terminator":{"type":"activate","cue":"opening","next":"write"}},
        "write":{"ops":[{"id":"clear","operation":{"type":"profile_value_assign","target":"value","key":"mutable","value":{"type":"const","value":{"type":"i32","value":0}}}}],"terminator":{"type":"end","outcome":"done"}}
    })).unwrap();
    let mut bad = p.clone();
    bad.requires.retain(|c| c != "story.profile-value.v1");
    assert_eq!(ValidatedProgram::new(bad).unwrap_err().code, "E_CAPABILITY");
    let validated = ValidatedProgram::new(p).unwrap();
    let mut c = Core::new(validated.clone(), "values".into(), "en".into()).unwrap();
    c.step(CoreInput::None, 1000);
    assert_eq!(c.state().variables["value"], Value::I32(7));
    let frozen = c.snapshot();
    let pending = frozen.pending.as_ref().unwrap().id;
    let mut restored = Core::restore(validated.clone(), frozen, "values").unwrap();
    restored
        .set_profile_values(&std::collections::BTreeMap::from([(
            "mutable".into(),
            Value::I32(1),
        )]))
        .unwrap();
    assert_eq!(restored.state().variables["value"], Value::I32(7));
    let output = restored.step(
        CoreInput::Prepared {
            activation: pending,
        },
        1000,
    );
    assert_eq!(restored.state().variables["value"], Value::I32(0));
    assert!(output.intents.iter().any(
        |i| matches!(i,CoreIntent::ProfileValueAssign{key,value:Value::I32(0)}if key=="mutable")
    ));
    let mut fresh = Core::new(validated, "values".into(), "en".into()).unwrap();
    fresh
        .set_profile_values(&std::collections::BTreeMap::from([(
            "mutable".into(),
            Value::String("wrong type".into()),
        )]))
        .unwrap();
    fresh.step(CoreInput::None, 1000);
    assert_eq!(fresh.state().fault.as_ref().unwrap().code, "E_PROFILE");
}
#[test]
fn profile_flag_reads_use_current_facts_after_restore_without_rewriting_frozen_slots() {
    let mut p = program();
    p.requires.push("story.profile-read.v1".into());
    p.variables.insert("flag".into(), Value::I32(0));
    p.functions.get_mut("main").unwrap().blocks = serde_json::from_value(serde_json::json!({
        "start":{"ops":[{"id":"read-before","operation":{"type":"profile_read","target":"flag","key":"seen"}}],
            "terminator":{"type":"activate","cue":"opening","next":"after"}},
        "after":{"ops":[{"id":"read-after","operation":{"type":"profile_read","target":"flag","key":"seen"}}],
            "terminator":{"type":"end","outcome":"done"}}
    })).unwrap();
    p.functions.get_mut("main").unwrap().entry = "start".into();
    let mut bad = p.clone();
    bad.requires.retain(|c| c != "story.profile-read.v1");
    assert_eq!(ValidatedProgram::new(bad).unwrap_err().code, "E_CAPABILITY");
    let mut bad = p.clone();
    bad.variables
        .insert("flag".into(), Value::String(String::new()));
    assert_eq!(ValidatedProgram::new(bad).unwrap_err().code, "E_PROFILE");
    let validated = ValidatedProgram::new(p).unwrap();
    let mut c = Core::new(validated.clone(), "profile".into(), "en".into()).unwrap();
    c.step(CoreInput::None, 1000);
    assert_eq!(c.state().variables["flag"], Value::I32(0));
    let saved = c.snapshot();
    let mut restored = Core::restore(validated.clone(), saved.clone(), "profile").unwrap();
    restored
        .merge_profile_facts(&std::collections::BTreeSet::from(["seen".into()]))
        .unwrap();
    assert_eq!(restored.state().variables["flag"], Value::I32(0));
    restored.step(
        CoreInput::Prepared {
            activation: saved.pending.unwrap().id,
        },
        1000,
    );
    assert_eq!(restored.state().variables["flag"], Value::I32(1));
    assert_eq!(restored.state().outcome.as_deref(), Some("done"));
    let mut fresh = Core::new(validated, "profile".into(), "en".into()).unwrap();
    fresh
        .merge_profile_facts(&std::collections::BTreeSet::from(["seen".into()]))
        .unwrap();
    fresh.step(CoreInput::None, 1000);
    assert_eq!(fresh.state().variables["flag"], Value::I32(1));
    assert!(fresh
        .merge_profile_facts(&std::collections::BTreeSet::from([String::new()]))
        .is_err());
}
#[test]
fn inline_images_prepare_reveal_restore_and_reject_forged_bindings() {
    let mut p = program();
    let asset = p
        .assets
        .iter()
        .find(|(_, a)| a.kind == AssetKind::Image)
        .unwrap()
        .0
        .clone();
    let image = InlineImage {
        asset: asset.clone(),
        width: 21,
        height: 19,
        align: InlineImageAlign::Center,
        margins: [0; 4],
    };
    p.requires.push("text.inline-image.v1".into());
    let contract = p.texts.get_mut("intro").unwrap();
    contract.images = vec![TextImageContract {
        id: "icon".into(),
        asset: asset.clone(),
    }];
    contract.contract_digest = text_contract_digest(contract);
    let digest = contract.contract_digest.clone();
    for docs in p.locales.values_mut() {
        let doc = docs.get_mut("intro").unwrap();
        doc.contract_digest = digest.clone();
        doc.spans = vec![
            Span::Text {
                id: "before".into(),
                text: "Before".into(),
                emphasis: false,
            },
            Span::Image {
                id: "icon".into(),
                image: image.clone(),
            },
            Span::Text {
                id: "after".into(),
                text: "After".into(),
                emphasis: false,
            },
        ];
    }
    let mut missing = p.clone();
    missing.requires.retain(|c| c != "text.inline-image.v1");
    assert!(ValidatedProgram::new(missing).is_err());
    let mut malformed = p.clone();
    if let Span::Image { image, .. } = &mut malformed
        .locales
        .get_mut("en")
        .unwrap()
        .get_mut("intro")
        .unwrap()
        .spans[1]
    {
        image.width = 0;
    }
    assert!(ValidatedProgram::new(malformed).is_err());
    let validated = ValidatedProgram::new(p).unwrap();
    let mut c = Core::new(validated.clone(), "images".into(), "en".into()).unwrap();
    c.step(CoreInput::None, 1000);
    for _ in 0..10 {
        if c.dialogue().is_some() {
            break;
        }
        let pending = c.state().pending.as_ref().unwrap();
        assert!(
            validated.cue_assets(&pending.cue).contains(&asset)
                || !pending.dialogues.contains_key("line")
        );
        c.step(
            CoreInput::Prepared {
                activation: pending.id,
            },
            1000,
        );
    }
    let (task, d) = c.dialogue().unwrap();
    assert_eq!(
        d.inline_images(),
        vec![InlineImagePlacement {
            offset: 6,
            image: image.clone()
        }]
    );
    assert_eq!(c.state().history.last().unwrap().images, d.inline_images());
    let interaction = d.interaction;
    c.step(
        CoreInput::Advance {
            interaction,
            sequence: 1,
        },
        1000,
    );
    assert_eq!(
        c.dialogue().unwrap().1.visible_text(),
        "Before\u{fffc}After"
    );
    let restored = Core::restore(validated.clone(), c.snapshot(), "images").unwrap();
    assert_eq!(
        restored.dialogue().unwrap().1.inline_images(),
        c.dialogue().unwrap().1.inline_images()
    );
    let mut forged = c.snapshot();
    forged
        .tasks
        .get_mut(&task)
        .unwrap()
        .dialogue
        .as_mut()
        .unwrap()
        .spans[1]
        .image
        .as_mut()
        .unwrap()
        .asset = "forged".into();
    assert!(Core::restore(validated.clone(), forged, "images").is_err());
    let mut forged = c.snapshot();
    forged.history.last_mut().unwrap().images[0].offset = 1;
    assert!(Core::restore(validated, forged, "images").is_err());
}
#[test]
fn ruby_freezes_reading_and_rejects_forged_snapshot_annotation() {
    let mut p = program();
    p.requires.push("text.ruby.v1".into());
    for texts in p.locales.values_mut() {
        texts.get_mut("intro").unwrap().spans = vec![Span::Ruby {
            id: "ruby".into(),
            text: "漢字".into(),
            reading: "かんじ".into(),
        }];
    }
    let mut invalid = p.clone();
    invalid.requires.retain(|c| c != "text.ruby.v1");
    assert_eq!(
        ValidatedProgram::new(invalid).unwrap_err().code,
        "E_CAPABILITY"
    );
    let mut c = Core::new(
        ValidatedProgram::new(p).unwrap(),
        "ruby".into(),
        "en".into(),
    )
    .unwrap();
    c.step(CoreInput::None, 1000);
    for _ in 0..10 {
        if c.dialogue().is_some() {
            break;
        }
        let activation = c.state().pending.as_ref().unwrap().id;
        c.step(CoreInput::Prepared { activation }, 1000);
    }
    let (task, d) = c.dialogue().unwrap();
    assert_eq!(d.spans[0].ruby.as_deref(), Some("かんじ"));
    let interaction = d.interaction;
    c.step(
        CoreInput::Advance {
            interaction,
            sequence: 1,
        },
        1000,
    );
    assert_eq!(c.dialogue().unwrap().1.visible_text(), "漢字");
    let restored = Core::restore(c.validated_program().clone(), c.snapshot(), "ruby").unwrap();
    assert_eq!(
        restored.dialogue().unwrap().1.spans[0].ruby.as_deref(),
        Some("かんじ")
    );
    let mut forged = c.snapshot();
    forged
        .tasks
        .get_mut(&task)
        .unwrap()
        .dialogue
        .as_mut()
        .unwrap()
        .spans[0]
        .ruby = Some("改変".into());
    assert!(Core::restore(c.validated_program().clone(), forged, "ruby").is_err());
}
#[test]
fn paragraph_pause_preserves_text_voice_and_snapshot_until_click_or_timeout() {
    for timeout in [None, Some(Micros(250_000))] {
        let mut p = program();
        p.requires.push("text.pause.v1".into());
        let contract = p.texts.get_mut("intro").unwrap();
        contract.pauses = vec![TextPauseContract {
            id: "pause".into(),
            timeout_us: timeout,
        }];
        contract.contract_digest = text_contract_digest(contract);
        let digest = contract.contract_digest.clone();
        for texts in p.locales.values_mut() {
            texts.get_mut("intro").unwrap().contract_digest = digest.clone();
            texts.get_mut("intro").unwrap().spans = vec![
                Span::Text {
                    id: "before".into(),
                    text: "Before".into(),
                    emphasis: false,
                },
                Span::Pause {
                    id: "pause".into(),
                    timeout_us: timeout,
                },
                Span::Text {
                    id: "after".into(),
                    text: "After".into(),
                    emphasis: false,
                },
            ];
        }
        let mut no_cap = p.clone();
        no_cap.requires.retain(|c| c != "text.pause.v1");
        assert_eq!(
            ValidatedProgram::new(no_cap).unwrap_err().code,
            "E_CAPABILITY"
        );
        let mut c = Core::new(
            ValidatedProgram::new(p).unwrap(),
            "pause".into(),
            "en".into(),
        )
        .unwrap();
        c.step(CoreInput::None, 1000);
        for _ in 0..10 {
            if c.dialogue().is_some() {
                break;
            }
            let activation = c.state().pending.as_ref().unwrap().id;
            c.step(CoreInput::Prepared { activation }, 1000);
        }
        let task = c.dialogue().unwrap().0;
        let interaction = c.dialogue().unwrap().1.interaction;
        c.step(
            CoreInput::Advance {
                interaction,
                sequence: 1,
            },
            1000,
        );
        assert_eq!(c.dialogue().unwrap().1.visible_text(), "Before");
        assert!(c.dialogue().unwrap().1.awaiting_advance);
        assert!(!c.dialogue().unwrap().1.at_gate);
        let music = c.state().handles["music"];
        let snapshot: Snapshot =
            serde_json::from_slice(&serde_json::to_vec(&c.snapshot()).unwrap()).unwrap();
        let mut c = Core::restore(c.validated_program().clone(), snapshot, "pause").unwrap();
        let interaction = c.dialogue().unwrap().1.interaction;
        let mut forged = c.snapshot();
        forged
            .tasks
            .get_mut(&task)
            .unwrap()
            .dialogue
            .as_mut()
            .unwrap()
            .spans[1]
            .pause_timeout_us = Some(Micros(123));
        assert!(Core::restore(c.validated_program().clone(), forged, "pause").is_err());
        let step = if timeout.is_some() {
            c.step(CoreInput::Time { delta_us: 250_000 }, 1000)
        } else {
            c.step(
                CoreInput::Time {
                    delta_us: 1_000_000,
                },
                1000,
            );
            assert_eq!(c.dialogue().unwrap().1.visible_text(), "Before");
            c.step(
                CoreInput::Advance {
                    interaction,
                    sequence: 2,
                },
                1000,
            )
        };
        assert!(!step.intents.iter().any(|i| matches!(
            i,
            CoreIntent::AudioStart { .. } | CoreIntent::AudioStop { .. }
        )));
        assert_eq!(c.state().tasks[&music].state, TaskState::Running);
        assert_eq!(c.dialogue().unwrap().0, task);
        c.step(
            CoreInput::Advance {
                interaction,
                sequence: 3,
            },
            1000,
        );
        assert_eq!(
            c.dialogue().unwrap().1.visible_text(),
            "BeforeAfter",
            "timeout={timeout:?}, dialogue={:?}, fault={:?}",
            c.dialogue(),
            c.state().fault
        );
        assert!(c.state().fault.is_none(), "{:?}", c.state().fault);
    }
}
fn drive(c: &mut Core, route: Option<&str>) -> Vec<String> {
    let mut trace = vec![];
    for seq in 1..1000 {
        let input = if let Some(p) = &c.state().pending {
            CoreInput::Prepared { activation: p.id }
        } else if let Some(ch) = &c.state().choice {
            if let Some(route) = route {
                CoreInput::Choose {
                    interaction: ch.interaction,
                    option: route.into(),
                    sequence: seq,
                }
            } else {
                break;
            }
        } else if let Some(t) = c.state().tasks.values().find(|t| {
            t.state == TaskState::Running && matches!(t.effect, Effect::Audio { looped: false, .. })
        }) {
            CoreInput::AudioEnded { task: t.id }
        } else if let Some((_, d)) = c.dialogue() {
            if !d.at_gate {
                CoreInput::Advance {
                    interaction: d.interaction,
                    sequence: seq,
                }
            } else {
                CoreInput::Time {
                    delta_us: 1_000_000,
                }
            }
        } else {
            CoreInput::Time {
                delta_us: 1_000_000,
            }
        };
        let r = c.step(input, 10_000);
        trace.extend(r.intents.iter().filter_map(|i| {
            if let CoreIntent::Trace { event, at } = i {
                Some(format!("{event}@{at}"))
            } else {
                None
            }
        }));
        assert!(c.state().fault.is_none(), "{:?}", c.state().fault);
        if c.state().outcome.is_some() {
            break;
        }
    }
    trace
}
#[test]
fn two_routes() {
    for (route, end, n) in [("walk", "walk_home", 1), ("stay", "read_letter", 0)] {
        let mut c = core();
        drive(&mut c, Some(route));
        assert_eq!(c.state().outcome.as_deref(), Some(end));
        assert_eq!(c.state().variables["affection"], Value::I32(n));
    }
}
#[test]
fn random_stream_restores_after_budget_boundary() {
    let mut p = program();
    let f = p.functions.get_mut("main").unwrap();
    let b = f.blocks.get_mut(&f.entry).unwrap();
    b.ops = (0..8)
        .map(|i| Op {
            id: format!("random-{i}"),
            operation: Operation::Random {
                target: "affection".into(),
                min: i32::MIN,
                max: i32::MAX,
            },
        })
        .collect();
    b.terminator = Terminator::Return { value: None };
    let validated = ValidatedProgram::new(p).unwrap();
    let mut a = Core::new(validated.clone(), "rng".into(), "en".into()).unwrap();
    a.step(CoreInput::None, 3);
    let mut b = Core::restore(validated, a.snapshot(), "rng").unwrap();
    a.step(CoreInput::None, 100);
    b.step(CoreInput::None, 100);
    assert_eq!(
        serde_json::to_string(a.state()).unwrap(),
        serde_json::to_string(b.state()).unwrap()
    );
    assert_eq!(a.state().outcome.as_deref(), Some("returned"));
    Core::restore(a.validated_program().clone(), a.snapshot(), "rng").unwrap();
}
#[test]
fn non_suspending_loop_yields_then_faults_with_location() {
    let mut p = program();
    let f = p.functions.get_mut("main").unwrap();
    let b = f.blocks.get_mut(&f.entry).unwrap();
    b.ops.clear();
    b.terminator = Terminator::Goto {
        target: f.entry.clone(),
    };
    let mut c = Core::new(
        ValidatedProgram::new(p).unwrap(),
        "loop".into(),
        "en".into(),
    )
    .unwrap();
    c.step(CoreInput::None, 1);
    assert!(c.needs_clock());
    for _ in 0..11 {
        c.step(CoreInput::None, 100_000);
    }
    let fault = c.state().fault.as_ref().unwrap();
    assert_eq!(fault.code, "E_FUEL");
    assert!(fault.location.starts_with("main/"));
    assert!(!c.needs_clock());
}

#[test]
fn await_all_failure_takes_precedence_over_cancellation() {
    use std::collections::BTreeMap;
    let mut p = program();
    p.cues.insert(
        "pair".into(),
        Cue {
            effects: ["a", "b"]
                .into_iter()
                .map(|id| EffectDef {
                    id: id.into(),
                    scope: Scope::Frame,
                    effect: Effect::Delay {
                        duration_us: Micros(1_000_000),
                    },
                })
                .collect(),
        },
    );
    let mut blocks = BTreeMap::new();
    blocks.insert(
        "start".into(),
        Block {
            ops: vec![],
            terminator: Terminator::Activate {
                cue: "pair".into(),
                next: "wait".into(),
            },
        },
    );
    blocks.insert(
        "wait".into(),
        Block {
            ops: vec![Op {
                id: "cancel-b".into(),
                operation: Operation::TaskControl {
                    task: "b".into(),
                    action: TaskAction::Cancel,
                },
            }],
            terminator: Terminator::Await {
                on_advance: None,
                conditions: ["a", "b"]
                    .into_iter()
                    .map(|id| WaitCondition {
                        task: id.into(),
                        milestone: Milestone::Finished,
                    })
                    .collect(),
                next: "success".into(),
                on_cancelled: "cancel".into(),
                on_failed: "failure".into(),
            },
        },
    );
    for id in ["success", "cancel", "failure"] {
        blocks.insert(
            id.into(),
            Block {
                ops: vec![],
                terminator: Terminator::End { outcome: id.into() },
            },
        );
    }
    p.functions.get_mut("main").unwrap().entry = "start".into();
    p.functions.get_mut("main").unwrap().blocks = blocks;
    let mut c = Core::new(ValidatedProgram::new(p).unwrap(), "all".into(), "en".into()).unwrap();
    c.step(CoreInput::None, 100);
    c.step(
        CoreInput::Prepared {
            activation: c.state().pending.as_ref().unwrap().id,
        },
        0,
    );
    c.step(
        CoreInput::TaskFailed {
            task: c.state().handles["a"],
            message: "decode".into(),
        },
        0,
    );
    c.step(CoreInput::None, 100);
    assert_eq!(c.state().outcome.as_deref(), Some("failure"));
}
#[test]
fn deterministic_replay() {
    let mut a = core();
    let mut b = core();
    assert_eq!(drive(&mut a, Some("walk")), drive(&mut b, Some("walk")));
    assert_eq!(
        serde_json::to_string(a.state()).unwrap(),
        serde_json::to_string(b.state()).unwrap()
    );
}
#[test]
fn stale_and_duplicate_choice() {
    let mut c = core();
    drive(&mut c, None);
    let token = c.state().choice.as_ref().unwrap().interaction;
    c.step(
        CoreInput::Choose {
            interaction: token - 1,
            option: "walk".into(),
            sequence: 1000,
        },
        1000,
    );
    assert!(c.state().choice.is_some());
    c.step(
        CoreInput::Choose {
            interaction: token,
            option: "walk".into(),
            sequence: 1001,
        },
        1000,
    );
    let state = serde_json::to_string(c.state()).unwrap();
    c.step(
        CoreInput::Choose {
            interaction: token,
            option: "stay".into(),
            sequence: 1001,
        },
        1000,
    );
    assert_eq!(state, serde_json::to_string(c.state()).unwrap());
}
#[test]
fn pending_restore_does_not_repeat_assign() {
    let mut c = core();
    drive(&mut c, None);
    let token = c.state().choice.as_ref().unwrap().interaction;
    c.step(
        CoreInput::Choose {
            interaction: token,
            option: "walk".into(),
            sequence: 1000,
        },
        1000,
    );
    assert_eq!(c.state().variables["affection"], Value::I32(1));
    assert!(c.state().pending.is_some());
    let mut r = Core::restore(
        ValidatedProgram::new(program()).unwrap(),
        c.snapshot(),
        "test-release",
    )
    .unwrap();
    drive(&mut r, Some("walk"));
    assert_eq!(r.state().variables["affection"], Value::I32(1));
    assert_eq!(r.state().outcome.as_deref(), Some("walk_home"));
}
#[test]
fn restore_rejects_mismatched_release() {
    let c = core();
    assert!(Core::restore(
        ValidatedProgram::new(program()).unwrap(),
        c.snapshot(),
        "another"
    )
    .is_err());
}
#[test]
fn language_does_not_change_route() {
    let mut a = core();
    let mut b = core();
    b.set_locale("en").unwrap();
    drive(&mut a, Some("stay"));
    drive(&mut b, Some("stay"));
    assert_eq!(a.state().variables, b.state().variables);
    assert_eq!(a.state().outcome, b.state().outcome);
}
#[test]
fn snapshots_keep_current_text() {
    let mut c = core();
    c.step(CoreInput::None, 1000);
    let p = c.state().pending.as_ref().unwrap().id;
    c.step(CoreInput::Prepared { activation: p }, 1000);
    let p = c.state().pending.as_ref().unwrap().id;
    c.step(CoreInput::Prepared { activation: p }, 1000);
    let text = c.dialogue().unwrap().1.full_text();
    c.set_locale("en").unwrap();
    assert_eq!(c.dialogue().unwrap().1.full_text(), text);
    assert_eq!(c.dialogue().unwrap().1.locale, "zh-Hans");
}
#[test]
fn missing_gate_is_validation_error() {
    let mut p = program();
    p.locales
        .get_mut("en")
        .unwrap()
        .get_mut("letter")
        .unwrap()
        .spans
        .retain(|s| !matches!(s, Span::Gate { .. }));
    assert_eq!(ValidatedProgram::new(p).unwrap_err().code, "E_GATE");
}
#[test]
fn infinite_bgm_wait_rejected() {
    let mut p = program();
    p.functions
        .get_mut("main")
        .unwrap()
        .blocks
        .get_mut("wait_intro")
        .unwrap()
        .terminator = Terminator::Await {
        on_advance: None,
        conditions: vec![WaitCondition {
            task: "music".into(),
            milestone: Milestone::Finished,
        }],
        next: "enter".into(),
        on_cancelled: "cancelled".into(),
        on_failed: "failed".into(),
    };
    assert_eq!(
        ValidatedProgram::new(p).unwrap_err().code,
        "E_INFINITE_WAIT"
    );
}
#[test]
fn checked_fault_is_not_hoisted() {
    let mut p = program();
    p.functions
        .get_mut("main")
        .unwrap()
        .blocks
        .get_mut("end_walk")
        .unwrap()
        .ops
        .insert(
            0,
            Op {
                id: "divide".into(),
                operation: Operation::Assign {
                    target: "affection".into(),
                    value: Expr::Binary {
                        op: BinaryOp::Div,
                        left: Box::new(Expr::Const {
                            value: Value::I32(1),
                        }),
                        right: Box::new(Expr::Const {
                            value: Value::I32(0),
                        }),
                    },
                },
            },
        );
    let mut c = Core::new(
        ValidatedProgram::new(p).unwrap(),
        "test-release".into(),
        "en".into(),
    )
    .unwrap();
    drive(&mut c, Some("stay"));
    assert_eq!(c.state().outcome.as_deref(), Some("read_letter"));
}
#[test]
fn corrupt_choice_continuation_rejected() {
    let mut c = core();
    drive(&mut c, None);
    let mut s = c.snapshot();
    s.choice
        .as_mut()
        .unwrap()
        .branches
        .insert("walk".into(), "missing".into());
    assert!(Core::restore(ValidatedProgram::new(program()).unwrap(), s, "test-release").is_err());
}
#[test]
fn restore_animation_at_37_percent() {
    let mut c = core();
    drive(&mut c, None);
    let token = c.state().choice.as_ref().unwrap().interaction;
    c.step(
        CoreInput::Choose {
            interaction: token,
            option: "stay".into(),
            sequence: 1000,
        },
        1000,
    );
    let activation = c.state().pending.as_ref().unwrap().id;
    c.step(CoreInput::Prepared { activation }, 1000);
    c.step(CoreInput::Time { delta_us: 129_500 }, 1000);
    let before = c.sample_scene();
    let mut r = Core::restore(
        ValidatedProgram::new(program()).unwrap(),
        c.snapshot(),
        "test-release",
    )
    .unwrap();
    assert_eq!(before, r.sample_scene());
    r.step(CoreInput::Time { delta_us: 220_500 }, 1000);
    assert!((r.sample_scene().iter().find(|n| n.id == "aki").unwrap().y - 112.).abs() < 0.001);
}
#[test]
fn cancelled_wait_is_not_success() {
    let mut c = core();
    c.step(CoreInput::None, 1000);
    let id = c.state().pending.as_ref().unwrap().id;
    c.step(CoreInput::Prepared { activation: id }, 1000);
    let id = c.state().pending.as_ref().unwrap().id;
    c.step(CoreInput::Prepared { activation: id }, 1000);
    let task = c.dialogue().unwrap().0;
    c.step(
        CoreInput::TaskFailed {
            task,
            message: "device".into(),
        },
        1000,
    );
    assert_eq!(c.state().fault.as_ref().unwrap().code, "E_PERFORMANCE");
}
#[test]
fn uninitialized_local_rejected() {
    let mut p = program();
    let f = p.functions.get_mut("main").unwrap();
    f.locals.insert("unset".into(), ValueType::I32);
    f.blocks.get_mut("start").unwrap().ops.push(Op {
        id: "bad".into(),
        operation: Operation::Assign {
            target: "affection".into(),
            value: Expr::Var {
                name: "unset".into(),
            },
        },
    });
    assert_eq!(
        ValidatedProgram::new(p).unwrap_err().code,
        "E_UNINITIALIZED"
    );
}
#[test]
fn clock_frozen_during_prepare() {
    let mut c = core();
    c.step(CoreInput::None, 1000);
    c.step(
        CoreInput::Time {
            delta_us: 9_000_000,
        },
        1000,
    );
    assert_eq!(c.state().tick_us, Micros(0));
}
#[test]
fn checked_integer_overflow_faults() {
    let mut p = program();
    p.functions
        .get_mut("main")
        .unwrap()
        .blocks
        .get_mut("start")
        .unwrap()
        .ops
        .push(Op {
            id: "overflow".into(),
            operation: Operation::Assign {
                target: "affection".into(),
                value: Expr::Binary {
                    op: BinaryOp::Add,
                    left: Box::new(Expr::Const {
                        value: Value::I32(i32::MAX),
                    }),
                    right: Box::new(Expr::Const {
                        value: Value::I32(1),
                    }),
                },
            },
        });
    let mut c = Core::new(
        ValidatedProgram::new(p).unwrap(),
        "test-release".into(),
        "en".into(),
    )
    .unwrap();
    c.step(CoreInput::None, 10);
    assert_eq!(c.state().fault.as_ref().unwrap().code, "E_ARITHMETIC");
    assert_eq!(c.state().variables["affection"], Value::I32(0));
}

#[test]
fn timed_choice_keeps_visual_clock_demand() {
    let mut p = program();
    for choice in p.choices.values_mut() {
        choice.timeout_us = Some(Micros(1_000_000));
        choice.default = Some(choice.options[0].id.clone());
    }
    let mut c = Core::new(
        ValidatedProgram::new(p).unwrap(),
        "test-release".into(),
        "en".into(),
    )
    .unwrap();
    drive(&mut c, None);
    assert!(c.state().choice.as_ref().unwrap().deadline_us.is_some());
    assert!(c.needs_visual_clock());
}

#[test]
fn elapsed_time_and_logic_share_one_budget() {
    let mut p = program();
    let f = p.functions.get_mut("main").unwrap();
    let b = f.blocks.get_mut(&f.entry).unwrap();
    b.ops.clear();
    b.terminator = Terminator::Goto {
        target: f.entry.clone(),
    };
    let mut c = Core::new(
        ValidatedProgram::new(p).unwrap(),
        "budget".into(),
        "en".into(),
    )
    .unwrap();
    let output = c.step(CoreInput::Time { delta_us: 250_000 }, 7);
    assert_eq!(output.work_used, 7);
    assert_eq!(c.state().unsuspended_ops, 7);
    assert_eq!(c.state().tick_us.0, 0);
    assert_eq!(output.remaining_time_us, 250_000);
    assert!(c.state().fault.is_none());
    assert!(
        c.needs_visual_clock(),
        "retained logic must refresh its picture"
    );
}

#[test]
fn clock_slices_preserve_reveal_and_complete_snapshot() {
    let mut whole = core();
    for _ in 0..8 {
        let input = whole
            .state()
            .pending
            .as_ref()
            .map(|p| CoreInput::Prepared { activation: p.id })
            .unwrap_or(CoreInput::None);
        whole.step(input, 1000);
        if whole.dialogue().is_some() {
            break;
        }
    }
    assert!(whole.dialogue().is_some());
    let mut sliced = whole.clone();
    whole.step(CoreInput::Time { delta_us: 250_000 }, 1000);
    let mut remaining = 250_000;
    let mut turns = 0;
    while remaining > 0 {
        let out = sliced.step(
            CoreInput::Time {
                delta_us: remaining,
            },
            1,
        );
        assert!(out.work_used <= 1);
        remaining = out.remaining_time_us;
        turns += 1;
        assert!(turns < 1000);
    }
    // If the last clock boundary used the final unit, runnable logic is next-turn work.
    sliced.step(CoreInput::None, 1000);
    assert_eq!(
        serde_json::to_value(whole.snapshot()).unwrap(),
        serde_json::to_value(sliced.snapshot()).unwrap()
    );
    assert!(turns > 1);
}

#[test]
fn revision_identity_is_frozen_during_preparation_and_validated_on_restore() {
    let mut c = core();
    c.step(CoreInput::None, 1000);
    let activation = c.state().pending.as_ref().unwrap().id;
    c.step(CoreInput::Prepared { activation }, 1000);
    let pending = c.state().pending.as_ref().unwrap();
    let frozen = pending.dialogues["line"].clone();
    assert_eq!(frozen.source_revision, 1);
    assert_eq!(frozen.meaning_revision, 1);
    assert_eq!(
        frozen.contract_digest,
        c.program().texts["intro"].contract_digest
    );
    c.set_locale("en").unwrap();
    let snapshot = c.snapshot();
    assert_eq!(snapshot.format, SNAPSHOT_VERSION);
    let mut legacy = serde_json::to_value(&snapshot).unwrap();
    let dialogue = legacy["pending"]["dialogues"]["line"]
        .as_object_mut()
        .unwrap();
    for field in ["source_revision", "meaning_revision", "contract_digest"] {
        dialogue.remove(field);
    }
    dialogue.insert("revision".into(), serde_json::json!(1));
    assert!(serde_json::from_value::<nir_core::Snapshot>(legacy).is_err());
    let validated = c.validated_program().clone();
    let mut restored = Core::restore(validated.clone(), snapshot.clone(), "test-release").unwrap();
    let activation = restored.state().pending.as_ref().unwrap().id;
    restored.step(CoreInput::Prepared { activation }, 1000);
    let d = restored.dialogue().unwrap().1;
    assert_eq!(d.locale, "zh-Hans");
    assert_eq!(d.contract_digest, frozen.contract_digest);
    assert_eq!(restored.state().history[0].source_revision, 1);
    let mut corrupt = snapshot.clone();
    corrupt
        .pending
        .as_mut()
        .unwrap()
        .dialogues
        .get_mut("line")
        .unwrap()
        .source_revision += 1;
    assert!(Core::restore(validated.clone(), corrupt, "test-release").is_err());
    let mut corrupt = restored.snapshot();
    corrupt.history[0].contract_digest = "bad".into();
    assert!(Core::restore(validated.clone(), corrupt, "test-release").is_err());
    assert!(Core::restore(validated, snapshot, "another-release").is_err());
}

#[test]
fn typed_scene_tracks_preserve_legacy_clip_route_traces() {
    for route in ["walk", "stay"] {
        let mut p = program();
        p.requires.push("tween.target.v1".into());
        for definition in p.cues.values_mut().flat_map(|cue| &mut cue.effects) {
            if let Effect::Clip {
                node,
                property,
                to,
                duration_us,
                replace,
                easing,
                finish,
                cancel,
            } = &definition.effect
            {
                definition.effect = Effect::Tween {
                    target: TweenTarget::SceneNode {
                        node: node.clone(),
                        property: *property,
                    },
                    to: *to,
                    duration_us: *duration_us,
                    replace: *replace,
                    easing: *easing,
                    finish: *finish,
                    cancel: *cancel,
                };
            }
        }
        let mut legacy = core();
        let mut typed = Core::new(
            ValidatedProgram::new(p).unwrap(),
            "test-release".into(),
            "zh-Hans".into(),
        )
        .unwrap();
        assert_eq!(
            drive(&mut legacy, Some(route)),
            drive(&mut typed, Some(route))
        );
        assert_eq!(legacy.sample_scene(), typed.sample_scene());
        assert_eq!(legacy.state().outcome, typed.state().outcome);
        assert_eq!(legacy.state().variables, typed.state().variables);
    }
}

#[test]
fn extended_numeric_branch_and_save_keep_bits_beyond_double_precision() {
    let mut p = program();
    p.requires.push("story.float80.v1".into());
    let next = Float80::from_bits((0x3fffu128 << 64) | (1u128 << 63) | 1).unwrap();
    p.variables.insert("extended".into(), Value::F80(next));
    let mut missing = p.clone();
    missing.requires.retain(|c| c != "story.float80.v1");
    assert_eq!(
        ValidatedProgram::new(missing).unwrap_err().code,
        "E_CAPABILITY"
    );
    let validated = ValidatedProgram::new(p).unwrap();
    let mut c = Core::new(validated.clone(), "extended".into(), "en".into()).unwrap();
    c.step(CoreInput::None, 1000);
    let bytes = serde_json::to_vec(&c.snapshot()).unwrap();
    let snapshot = serde_json::from_slice(&bytes).unwrap();
    let restored = Core::restore(validated, snapshot, "extended").unwrap();
    let difference = Expr::Binary {
        op: BinaryOp::Sub,
        left: Box::new(Expr::Var {
            name: "extended".into(),
        }),
        right: Box::new(Expr::ToF80 {
            value: Box::new(Expr::Const {
                value: Value::I32(1),
            }),
        }),
    };
    let greater = Expr::Binary {
        op: BinaryOp::Gt,
        left: Box::new(difference.clone()),
        right: Box::new(Expr::Const {
            value: Value::F80(Float80::from_i32(0)),
        }),
    };
    assert_eq!(restored.eval(&greater).unwrap(), Value::Bool(true));
    let Value::F80(value) = restored.eval(&difference).unwrap() else {
        panic!("typed extended result")
    };
    assert_eq!(value.bits(), (0x3fc0u128 << 64) | (1u128 << 63));
    let divide_zero = Expr::Binary {
        op: BinaryOp::Div,
        left: Box::new(difference),
        right: Box::new(Expr::Const {
            value: Value::F80(Float80::from_i32(0)),
        }),
    };
    assert_eq!(
        restored.eval(&divide_zero).unwrap_err().code,
        "E_ARITHMETIC"
    );
    let overflow = Expr::ToI32 {
        value: Box::new(Expr::Const {
            value: Value::F80(Float80::from_bits((0x401eu128 << 64) | (1u128 << 63)).unwrap()),
        }),
    };
    assert_eq!(restored.eval(&overflow).unwrap_err().code, "E_ARITHMETIC");
}
