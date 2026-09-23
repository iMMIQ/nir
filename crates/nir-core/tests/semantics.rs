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
    assert_eq!(snapshot.format, 1);
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
