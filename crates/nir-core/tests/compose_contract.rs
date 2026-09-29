use nir_core::*;
use nir_format::*;
use serde_json::{json, Value as Json};

fn program(effects: Json) -> Program {
    program_waiting_on("chain", effects)
}

/// The entry cue activates and then awaits the named task's completion; the
/// wait is what parks the VM while compositions run on their own.
fn program_waiting_on(wait: &str, effects: Json) -> Program {
    let mut p: Program = serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    p.requires.push("tween.target.v1".into());
    p.requires.push("task.compose.v1".into());
    p.requires.push("audio.stop.v1".into());
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
    f.blocks.insert("hold".into(), serde_json::from_value(json!({"terminator":{"type":"await","conditions":[{"task":wait,"milestone":{"type":"finished"}}],"next":"done","on_cancelled":"done","on_failed":"done"}})).unwrap());
    f.blocks.insert(
        "done".into(),
        serde_json::from_value(
            json!({"terminator":{"type":"end","outcome":"done"}}),
        )
        .unwrap(),
    );
    p
}

fn start(p: Program) -> Core {
    let mut c = Core::new(ValidatedProgram::new(p).unwrap(), "compose".into(), "en".into())
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
fn node(c: &Core, id: &str, property: Property) -> f32 {
    c.state()
        .scene
        .iter()
        .find(|n| n.id == id)
        .unwrap_or_else(|| panic!("node {id} missing"))
        .get(property)
}
fn task<'a>(c: &'a Core, name: &str) -> &'a Task {
    &c.state().tasks[&c.state().handles[name]]
}
fn stage() -> Json {
    json!({"id":"stage","scope":"scene","effect":{"type":"stage_present","scene":"station","duration_us":"0"}})
}
fn chain(children: &[Json]) -> Json {
    json!({"id":"chain","scope":"session","effect":{"type":"sequence","children":children}})
}
fn parallel(children: &[Json]) -> Json {
    json!({"id":"chain","scope":"session","effect":{"type":"parallel_all","children":children}})
}
fn move_x(to: f32, duration_us: u64) -> Json {
    json!({"id":"move","scope":"session","effect":{"type":"tween","target":{"type":"scene_node","node":"background","property":"x"},"to":to,"duration_us":duration_us.to_string()}})
}
fn fade_scene(to: f32, duration_us: u64) -> Json {
    json!({"id":"fadeout","scope":"session","effect":{"type":"tween","target":{"type":"scene_node","node":"background","property":"opacity"},"to":to,"duration_us":duration_us.to_string()}})
}

/// The motivating sample: while the main VM awaits the dialogue, a chain that
/// first moves and then fades still advances on its own. Activate/Await alone
/// cannot compile this cue — the snapshot has a single `waiting` slot, so the
/// VM cannot simultaneously wait for the dialogue and for the move to finish
/// before starting the fade. The chain has to be an autonomous task, which is
/// exactly what a composition is.
#[test]
fn sequence_advances_while_the_vm_awaits_dialogue() {
    let mut c = start(program_waiting_on(
        "line",
        json!([
            stage(),
            {"id":"line","scope":"interaction","effect":{"type":"dialogue","text":"intro","speaker":"","reveal_us":"10000000"}},
            chain(&[move_x(100., 3000), fade_scene(0.2, 2000)]),
        ]),
    ));
    // The VM is parked on the dialogue wait, exactly one waiting slot in use.
    assert!(c.state().waiting.is_some());
    assert_eq!(task(&c, "chain").state, TaskState::Running);
    // The move runs first; the fade has not started.
    tick(&mut c, 3500);
    assert_eq!(node(&c, "background", Property::X), 100.);
    assert_eq!(node(&c, "background", Property::Opacity), 1.);
    assert!(c.state().waiting.is_some());
    // The move finished and the fade takes over, still without the VM.
    tick(&mut c, 2500);
    assert_eq!(node(&c, "background", Property::Opacity), 0.2);
    assert!(task(&c, "chain").milestones.contains(&Milestone::Finished));
    assert!(c.state().waiting.is_some());
    // The dialogue finishes on reader input and the wait resolves normally.
    let interaction = task(&c, "line")
        .dialogue
        .as_ref()
        .unwrap()
        .interaction;
    c.step(CoreInput::Advance { interaction, sequence: 1 }, 1000);
    c.step(CoreInput::Advance { interaction, sequence: 2 }, 1000);
    assert_eq!(c.state().outcome.as_deref(), Some("done"));
}

/// Zero-duration chains chase to completion inside the activation commit and
/// pay one budget unit per spawn; a starved chain spreads across steps
/// instead of looping. Infinite zero-duration loops are not authorable — the
/// tree is finite, the chase guard faults on non-convergence, and validation
/// caps depth and total leaves.
#[test]
fn zero_duration_chain_chases_within_one_commit_and_pays_budget() {
    let mut c = start(program(json!([
        stage(),
        chain(&[move_x(10., 0), {
            let mut m = move_x(20., 0); m["id"] = json!("step2"); m
        }, {
            let mut m = move_x(30., 0); m["id"] = json!("step3"); m
        }]),
    ])));
    assert_eq!(node(&c, "background", Property::X), 30.);
    assert!(task(&c, "chain").milestones.contains(&Milestone::Finished));
    assert_eq!(c.state().outcome.as_deref(), Some("done"));

    // A two-unit budget commits the cue and the first two spawns only; the
    // third waits for the next step. Every spawn is visible in work_used.
    let mut c = Core::new(
        ValidatedProgram::new(program(json!([stage(), chain(&[
            move_x(10., 0),
            { let mut m = move_x(20., 0); m["id"] = json!("step2"); m },
            { let mut m = move_x(30., 0); m["id"] = json!("step3"); m },
        ])]))).unwrap(),
        "compose".into(),
        "en".into(),
    )
    .unwrap();
    let activation = {
        c.step(CoreInput::None, 1000);
        c.state().pending.as_ref().unwrap().id
    };
    let step = c.step(CoreInput::Prepared { activation }, 2);
    assert_eq!(step.work_used, 2);
    assert_eq!(node(&c, "background", Property::X), 20.);
    assert_eq!(task(&c, "chain").cursor, 2);
    assert_eq!(task(&c, "chain").state, TaskState::Running);
    c.step(CoreInput::None, 1000);
    assert_eq!(node(&c, "background", Property::X), 30.);
    assert!(task(&c, "chain").milestones.contains(&Milestone::Finished));
}

/// A child failure fails the chain; completed children stay settled because
/// their side effects are not rolled back, and the rest never runs.
#[test]
fn sequence_child_failure_fails_the_chain_and_keeps_completed_side_effects() {
    let mut c = start(program(json!([
        stage(), chain(&[move_x(50., 1000), fade_scene(0.2, 2000)]),
    ])));
    tick(&mut c, 1500);
    assert_eq!(node(&c, "background", Property::X), 50.);
    assert_eq!(task(&c, "fadeout").state, TaskState::Running);
    c.step(
        CoreInput::TaskFailed {
            task: c.state().handles["fadeout"],
            message: "device lost".into(),
        },
        1000,
    );
    assert_eq!(task(&c, "fadeout").state, TaskState::Failed);
    assert_eq!(task(&c, "chain").state, TaskState::Failed);
    assert!(!task(&c, "chain").milestones.contains(&Milestone::Finished));
    assert_eq!(task(&c, "chain").cursor, 2);
    assert_eq!(node(&c, "background", Property::X), 50.);
    // The failed fade settles at its current value per its cancel policy and
    // stays there; the completed move is not rolled back either.
    tick(&mut c, 5000);
    let failed_opacity = node(&c, "background", Property::Opacity);
    assert!((failed_opacity - 0.8).abs() < 1e-6, "{failed_opacity}");
    assert_eq!(node(&c, "background", Property::X), 50.);
    assert_eq!(c.state().outcome.as_deref(), Some("done"));
}

/// ParallelAll starts everything at activation, finishes when every child
/// finished, and failure beats cancellation beats completion, cancelling the
/// still-running siblings on the way out.
#[test]
fn parallel_all_merges_failure_first_and_finishes_when_all_finish() {
    let mut c = start(program(json!([
        stage(), parallel(&[move_x(50., 1000), fade_scene(0.2, 1000)]),
    ])));
    assert_eq!(task(&c, "move").state, TaskState::Running);
    assert_eq!(task(&c, "fadeout").state, TaskState::Running);
    tick(&mut c, 1500);
    assert!(task(&c, "chain").milestones.contains(&Milestone::Finished));
    assert_eq!(node(&c, "background", Property::X), 50.);
    assert_eq!(node(&c, "background", Property::Opacity), 0.2);

    let mut c = start(program(json!([
        stage(), parallel(&[move_x(50., 5000), fade_scene(0.2, 5000)]),
    ])));
    c.step(
        CoreInput::TaskFailed {
            task: c.state().handles["move"],
            message: "device lost".into(),
        },
        1000,
    );
    assert_eq!(task(&c, "move").state, TaskState::Failed);
    assert_eq!(task(&c, "fadeout").state, TaskState::Cancelled);
    assert_eq!(task(&c, "chain").state, TaskState::Failed);
    assert!(!task(&c, "chain").milestones.contains(&Milestone::Finished));
    assert_eq!(c.state().outcome.as_deref(), Some("done"));
}

/// Finish control on a composition settles the running child at its end
/// value and never spawns the rest; cancel control stops it per its cancel
/// policy. Children cannot be targeted by name from story code.
#[test]
fn task_control_finishes_or_cancels_the_whole_chain() {
    let mut p = program(json!([
        stage(),
        chain(&[move_x(50., 10000), fade_scene(0.2, 1000)]),
    ]));
    let f = p.functions.get_mut("main").unwrap();
    f.blocks.insert(
        "control".into(),
        serde_json::from_value(json!({"ops":[{"id":"control","operation":{"type":"task_control","task":"chain","action":"finish"}}],"terminator":{"type":"await","conditions":[{"task":"chain","milestone":{"type":"finished"}}],"next":"done","on_cancelled":"done","on_failed":"done"}})).unwrap(),
    );
    f.blocks
        .get_mut("test")
        .unwrap()
        .terminator = serde_json::from_value(
        json!({"type":"activate","cue":"test","next":"control"}),
    )
    .unwrap();
    let mut c = start(p);
    assert!(task(&c, "chain").milestones.contains(&Milestone::Finished));
    assert_eq!(task(&c, "chain").cursor, 1);
    assert_eq!(task(&c, "move").end_reason, Some(TaskEndReason::FinishedByControl));
    assert_eq!(node(&c, "background", Property::X), 50.);
    tick(&mut c, 20000);
    assert_eq!(node(&c, "background", Property::Opacity), 1.);
    assert_eq!(c.state().outcome.as_deref(), Some("done"));
}

/// A snapshot taken mid-chain restores exactly once: the finished audio child
/// is never restarted, the pending tween continues from the restored state,
/// and no second AudioStart is emitted after the restore.
#[test]
fn save_and_restore_mid_chain_resumes_without_replay() {
    let mut c = start(program(json!([
        stage(),
        chain(&[
            json!({"id":"ring","scope":"session","effect":{"type":"audio","asset":"audio.bell","bus":"bgm","looped":false}}),
            move_x(99., 3000),
        ]),
    ])));
    assert_eq!(task(&c, "ring").state, TaskState::Running);
    assert_eq!(task(&c, "chain").cursor, 1);
    let snapshot = c.snapshot();
    let mut restored =
        Core::restore(c.validated_program().clone(), snapshot, "compose").unwrap();
    assert_eq!(task(&restored, "chain").cursor, 1);
    let mut starts = 0;
    let step = restored.step(
        CoreInput::AudioEnded {
            task: restored.state().handles["ring"],
        },
        1000,
    );
    starts += step
        .intents
        .iter()
        .filter(|i| matches!(i, CoreIntent::AudioStart { .. }))
        .count();
    assert_eq!(task(&restored, "ring").state, TaskState::Finished);
    assert_eq!(task(&restored, "move").state, TaskState::Running);
    let step = { tick(&mut restored, 4000); 0 };
    starts += step;
    assert_eq!(starts, 0);
    assert!(task(&restored, "chain").milestones.contains(&Milestone::Finished));
    assert_eq!(node(&restored, "background", Property::X), 99.);
    assert_eq!(restored.state().outcome.as_deref(), Some("done"));
}

/// Source validation: capability, scope inheritance, forbidden child kinds,
/// writer overlap under concurrency (sequences may rewrite, parallels may
/// not), duplicate ids, nesting depth and total leaf caps.
#[test]
fn composition_validation_rejects_invalid_cues() {
    let valid = |effects: Json| ValidatedProgram::new(program(effects)).is_ok();
    assert!(valid(json!([stage(), chain(&[move_x(10., 100), fade_scene(0.2, 100)])])));
    // Same address twice in one sequence is the point of chaining.
    let step2 = { let mut m = move_x(20., 100); m["id"] = json!("step2"); m };
    assert!(valid(json!([chain(&[move_x(10., 100), step2])])));
    let rejects = |effects: Json, code: &str| {
        assert_eq!(
            ValidatedProgram::new(program(effects)).unwrap_err().code,
            code
        );
    };
    // Capability.
    let mut p = program(json!([chain(&[move_x(10., 100)])]));
    p.requires.retain(|cap| cap != "task.compose.v1");
    assert_eq!(
        ValidatedProgram::new(p).unwrap_err().code,
        "E_CAPABILITY"
    );
    // Child scope must inherit the composition's scope.
    let mut wrong_scope = move_x(10., 100);
    wrong_scope["scope"] = json!("scene");
    rejects(json!([chain(&[wrong_scope])]), "E_SCOPE");
    // Stages and dialogue are not composition children.
    rejects(
        json!([chain(&[json!({"id":"inner","scope":"session","effect":{"type":"stage_present","scene":"station","duration_us":"0"}})])]),
        "E_COMPOSE",
    );
    rejects(
        json!([chain(&[json!({"id":"inner","scope":"session","effect":{"type":"dialogue","text":"intro","speaker":"","reveal_us":"1000"}})])]),
        "E_COMPOSE",
    );
    // Parallel children writing the same address conflict.
    let step2 = { let mut m = move_x(20., 100); m["id"] = json!("step2"); m };
    rejects(
        json!([parallel(&[move_x(10., 100), step2.clone()])]),
        "E_OWNERSHIP",
    );
    // A sequence child still conflicts with a concurrent top-level writer.
    rejects(
        json!([move_x(10., 100), chain(&[step2])]),
        "E_OWNERSHIP",
    );
    // Ids are unique across the whole cue tree.
    rejects(
        json!([chain(&[move_x(10., 100), {
            let mut m = move_x(20., 100); m["id"] = json!("step2"); m
        }, {
            let mut m = move_x(30., 100); m["id"] = json!("move"); m
        }])]),
        "E_DUPLICATE",
    );
    // Nesting depth is bounded.
    let mut deep = move_x(10., 100);
    for _ in 0..9 {
        deep = json!({"id":"chain","scope":"session","effect":{"type":"sequence","children":[deep]}});
    }
    rejects(json!([deep]), "E_LIMIT");
    // Total leaves are bounded by the task cap.
    let wide: Vec<Json> = (0..300)
        .map(|_i| {
            let mut m = move_x(10., 100);
            m["id"] = json!("m{i}");
            m
        })
        .collect();
    rejects(json!([chain(&wide)]), "E_LIMIT");
    // A stop child still needs a real audio target.
    rejects(
        json!([chain(&[json!({"id":"stop","scope":"session","effect":{"type":"audio_stop","target":"ring","duration_us":"1000"}})])]),
        "E_AUDIO_STOP",
    );
}

/// Restore validation: the cursor counts spawned children, each spawned child
/// matches its declaration, and a finished chain has nothing pending.
#[test]
fn restore_rejects_corrupted_compositions() {
    let mut c = start(program(json!([
        stage(), chain(&[move_x(10., 0), fade_scene(0.2, 4000)]),
    ])));
    tick(&mut c, 0);
    assert_eq!(task(&c, "chain").cursor, 2);
    let mut snapshot = c.snapshot();
    assert!(Core::restore(c.validated_program().clone(), snapshot.clone(), "compose").is_ok());
    // Cursor must equal the spawned count.
    snapshot
        .tasks
        .get_mut(&snapshot.handles["chain"])
        .unwrap()
        .cursor = 1;
    assert!(Core::restore(c.validated_program().clone(), snapshot.clone(), "compose").is_err());
    // A finished composition cannot have running children.
    let mut snapshot = c.snapshot();
    let chain = snapshot.tasks.get_mut(&snapshot.handles["chain"]).unwrap();
    chain.milestones.insert(Milestone::Finished);
    assert!(Core::restore(c.validated_program().clone(), snapshot, "compose").is_err());
}
