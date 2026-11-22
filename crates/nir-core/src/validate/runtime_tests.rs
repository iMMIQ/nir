use super::*;

// The previous full-sweep solver is a test oracle for cyclic control flow.
fn reference_assignments(fid: &str, function: &Function, initial: BTreeSet<String>) -> Result<()> {
    let mut incoming = BTreeMap::from([(function.entry.clone(), initial)]);
    let mut changed = true;
    while changed {
        changed = false;
        for (bid, block) in &function.blocks {
            let Some(mut assigned) = incoming.get(bid).cloned() else {
                continue;
            };
            for op in &block.ops {
                if let Operation::Assign { target, .. } | Operation::Random { target, .. } =
                    &op.operation
                {
                    assigned.insert(target.clone());
                }
            }
            if let Terminator::Call {
                result: Some(result),
                ..
            } = &block.terminator
            {
                assigned.insert(result.clone());
            }
            for target in outgoing(&block.terminator) {
                let merged = match incoming.get(target) {
                    Some(old) => old.intersection(&assigned).cloned().collect(),
                    None => assigned.clone(),
                };
                if incoming.get(target) != Some(&merged) {
                    incoming.insert(target.to_owned(), merged);
                    changed = true;
                }
            }
        }
    }
    let check = |expr: &Expr, assigned: &BTreeSet<String>, at: &str| {
        let mut used = BTreeSet::new();
        reads(expr, &mut used);
        used.difference(assigned)
            .next()
            .map_or(Ok(()), |name| Err(err("E_UNINITIALIZED", at, name)))
    };
    for (bid, block) in &function.blocks {
        if let Some(mut assigned) = incoming.get(bid).cloned() {
            for op in &block.ops {
                match &op.operation {
                    Operation::Assign { target, value }
                    | Operation::ProfileValueAssign { target, value, .. } => {
                        check(value, &assigned, &op.id)?;
                        assigned.insert(target.clone());
                    }
                    Operation::Random { target, .. } => {
                        assigned.insert(target.clone());
                    }
                    _ => {}
                }
            }
            for expr in term_exprs(&block.terminator) {
                check(expr, &assigned, &format!("{fid}/{bid}"))?;
            }
        }
    }
    Ok(())
}

fn assignment_result(result: Result<()>) -> Option<(String, String, String)> {
    result
        .err()
        .map(|error| (error.code, error.location, error.message))
}

#[test]
fn assignment_worklist_matches_sweep_on_branches_cycles_and_call_results() {
    use rand_core::{RngCore, SeedableRng};
    let mut random = rand_chacha::ChaCha8Rng::seed_from_u64(17);
    let initialized = BTreeSet::from(["global".to_owned(), "parameter".to_owned()]);
    for _ in 0..256 {
        let mut function = simple_function();
        function.entry = "b0".into();
        function.blocks.clear();
        function.locals =
            BTreeMap::from([("x".into(), ValueType::I32), ("y".into(), ValueType::I32)]);
        for index in 0..12 {
            let name = format!("b{index}");
            let target = if random.next_u32().is_multiple_of(2) {
                "x"
            } else {
                "y"
            };
            let mut ops = vec![];
            if random.next_u32().is_multiple_of(2) {
                let read = ["global", "parameter", "x", "y"][random.next_u32() as usize % 4];
                ops.push(Op {
                    id: format!("op{index}"),
                    operation: Operation::Assign {
                        target: target.into(),
                        value: Expr::Var { name: read.into() },
                    },
                });
            }
            let next = format!("b{}", random.next_u32() % 12);
            let terminator = match random.next_u32() % 4 {
                0 => Terminator::Goto { target: next },
                1 => Terminator::Branch {
                    condition: Expr::Const {
                        value: Value::Bool(true),
                    },
                    yes: next,
                    no: format!("b{}", random.next_u32() % 12),
                },
                2 => Terminator::Call {
                    function: "callee".into(),
                    args: BTreeMap::new(),
                    result: Some(target.into()),
                    next,
                },
                _ => Terminator::End {
                    outcome: "done".into(),
                },
            };
            function.blocks.insert(name, block(terminator, ops));
        }
        assert_eq!(
            assignment_result(validate_assignments("f", &function, initialized.iter())),
            assignment_result(reference_assignments("f", &function, initialized.clone())),
            "control flow: {function:?}",
        );
    }
}

#[test]
fn assignment_join_requires_every_path_and_loop_cannot_initialize_entry() {
    let mut function: Function = serde_json::from_value(serde_json::json!({
        "entry":"start", "locals":{"x":"i32"}, "blocks":{
            "start":{"terminator":{"type":"branch","condition":{"type":"const","value":{"type":"bool","value":true}},"yes":"set","no":"detour"}},
            "set":{"ops":[{"id":"write","operation":{"type":"assign","target":"x","value":{"type":"const","value":{"type":"i32","value":1}}}}],"terminator":{"type":"goto","target":"read"}},
            "detour":{"terminator":{"type":"goto","target":"join"}},
            "join":{"terminator":{"type":"goto","target":"read"}},
            "read":{"ops":[{"id":"read-local","operation":{"type":"assign","target":"x","value":{"type":"var","name":"x"}}}],"terminator":{"type":"goto","target":"set"}},
            "unreachable":{"ops":[{"id":"dead-read","operation":{"type":"assign","target":"x","value":{"type":"var","name":"x"}}}],"terminator":{"type":"end","outcome":"done"}}
        }
    })).unwrap();
    let globals = BTreeSet::new();
    assert_eq!(
        validate_assignments("f", &function, globals.iter())
            .unwrap_err()
            .location,
        "read-local"
    );
    function.blocks.get_mut("detour").unwrap().ops = function.blocks["set"].ops.clone();
    validate_assignments("f", &function, globals.iter()).unwrap();
    function.entry = "read".into();
    assert_eq!(
        validate_assignments("f", &function, globals.iter())
            .unwrap_err()
            .location,
        "read-local"
    );
}

fn block(terminator: Terminator, ops: Vec<Op>) -> Block {
    Block { ops, terminator }
}
fn simple_function() -> Function {
    Function {
        params: BTreeMap::new(),
        locals: BTreeMap::new(),
        returns: None,
        entry: "end".into(),
        blocks: BTreeMap::from([(
            "end".into(),
            block(
                Terminator::End {
                    outcome: "done".into(),
                },
                vec![],
            ),
        )]),
    }
}
#[test]
fn runtime_admission_rejects_uninitialized_local_with_same_diagnostic() {
    let mut function = simple_function();
    function.locals.insert("unset".into(), ValueType::I32);
    function.blocks.get_mut("end").unwrap().ops.push(Op {
        id: "read-unset".into(),
        operation: Operation::Assign {
            target: "unset".into(),
            value: Expr::Var {
                name: "unset".into(),
            },
        },
    });
    let (program, objects) = runtime_fixture(
        BTreeMap::from([("m.main".into(), function)]),
        empty_static(),
    );
    let error = program.install_batch(objects).unwrap_err();
    assert_eq!(error.code, "E_UNINITIALIZED");
    assert_eq!(error.location, "read-unset");
}

fn runtime_fixture(
    functions: BTreeMap<String, Function>,
    mut static_package: ModuleStatic,
) -> (ValidatedProgram, Vec<(ContentKey, RuntimeObject, u64)>) {
    static_package.format = RUNTIME_FORMAT_VERSION;
    static_package.module = "m".into();
    let signatures: BTreeMap<_, _> = functions
        .iter()
        .map(|(id, function)| (id.clone(), FunctionSignature::from(function)))
        .collect();
    let code = ModuleCode {
        format: RUNTIME_FORMAT_VERSION,
        module: "m".into(),
        functions,
    };
    let mut assets: BTreeMap<String, AssetIndexEntry> = BTreeMap::new();
    assets.insert(
        "font".into(),
        AssetIndexEntry {
            kind: AssetKind::Font,
            object: "f".repeat(64),
            catalog: "resources".into(),
        },
    );
    assets.insert(
        "image".into(),
        AssetIndexEntry {
            kind: AssetKind::Image,
            object: "1".repeat(64),
            catalog: "resources".into(),
        },
    );
    let objects: BTreeMap<_, _> = assets
        .iter()
        .map(|(id, asset)| (id.clone(), asset.object.clone()))
        .collect();
    let plan = LocaleFontPlan {
        fonts: vec!["font".into()],
        digest: LocaleFontPlan::digest_for(&["font".into()], &objects),
    };
    let locale_config = LocaleConfig {
        default_ui: "en".into(),
        default_text: "en".into(),
        ui: BTreeMap::from([("en".into(), plan.clone())]),
        text: BTreeMap::from([("en".into(), plan.clone()), ("zh-Hans".into(), plan)]),
    };
    let module_texts: BTreeSet<_> = static_package.text_contracts.keys().cloned().collect();
    let locales = if module_texts.is_empty() {
        BTreeMap::new()
    } else {
        BTreeMap::from([
            ("en".into(), "b".repeat(64)),
            ("zh-Hans".into(), "e".repeat(64)),
        ])
    };
    let module = ModuleIndex {
        functions: signatures.clone(),
        texts: module_texts,
        code: "c".repeat(64),
        static_content: "a".repeat(64),
        locales,
    };
    let function_index = signatures
        .iter()
        .map(|(id, signature)| {
            (
                id.clone(),
                RuntimeFunctionIndex {
                    execution_module: None,
                    module: "m".into(),
                    signature: signature.clone(),
                },
            )
        })
        .collect();
    let scene_owners = static_package
        .scenes
        .keys()
        .map(|id| (id.clone(), "m".into()))
        .collect();
    let cue_owners = static_package
        .cues
        .keys()
        .map(|id| (id.clone(), "m".into()))
        .collect();
    let choice_owners = static_package
        .choices
        .keys()
        .map(|id| (id.clone(), "m".into()))
        .collect();
    let text_owners = static_package
        .text_contracts
        .keys()
        .map(|id| (id.clone(), "m".into()))
        .collect();
    let mut task_owners = BTreeMap::new();
    for cue in static_package.cues.values() {
        for effect in &cue.effects {
            task_owners.insert(effect.id.clone(), "m".into());
        }
    }
    let text_contracts = static_package
        .text_contracts
        .iter()
        .map(|(id, contract)| {
            (
                id.clone(),
                RuntimeTextIdentity {
                    module: "m".into(),
                    source_revision: contract.source_revision,
                    contract_revision: contract.contract_revision,
                    meaning_revision: contract.meaning_revision,
                    contract_digest: contract.contract_digest.clone(),
                    images: contract.images.clone(),
                },
            )
        })
        .collect();
    let root = RuntimeProgram {
        timeline_owners: Default::default(),
        format: RUNTIME_FORMAT_VERSION,
        game_id: "runtime-test".into(),
        revision: "r1".into(),
        entry: "m.main".into(),
        requires: vec![],
        stage: Stage {
            width: 640,
            height: 480,
        },
        variables: BTreeMap::new(),
        function_index,
        modules: BTreeMap::from([("m".into(), module)]),
        scene_owners,
        cue_owners,
        choice_owners,
        text_owners,
        task_owners,
        text_contracts,
        locales: BTreeSet::from(["en".into(), "zh-Hans".into()]),
        locale_config,
        assets,
        catalogs: BTreeMap::from([("resources".into(), "d".repeat(64))]),
        default_locale: "en".into(),
        title_scene: None,
        title_nodes: vec![],
        theme: Theme::default(),
        player: PlayerDefaults::default(),
    };
    let module = &root.modules["m"];
    assert!(
        !module.functions.is_empty(),
        "test module function index is empty"
    );
    assert!(
        valid_hash(&module.code),
        "invalid test code hash {:?}",
        module.code
    );
    assert!(
        valid_hash(&module.static_content),
        "invalid test static hash {:?}",
        module.static_content
    );
    assert!(
        module
            .locales
            .keys()
            .all(|locale| root.locales.contains(locale)),
        "invalid locale hashes/index"
    );
    let static_size = serde_json::to_vec(&static_package).unwrap().len() as u64;
    let code_size = serde_json::to_vec(&code).unwrap().len() as u64;
    let view = ValidatedProgram::from_runtime(root).unwrap();
    let batch = vec![
        (
            ContentKey::Static { module: "m".into() },
            RuntimeObject::Static(static_package),
            static_size,
        ),
        (
            ContentKey::Code { module: "m".into() },
            RuntimeObject::Code(code),
            code_size,
        ),
    ];
    (view, batch)
}
fn empty_static() -> ModuleStatic {
    ModuleStatic {
        sprite_timelines: Default::default(),
        format: RUNTIME_FORMAT_VERSION,
        module: "m".into(),
        scenes: BTreeMap::new(),
        cues: BTreeMap::new(),
        choices: BTreeMap::new(),
        text_contracts: BTreeMap::new(),
        activation_recipes: BTreeMap::new(),
    }
}
fn contract() -> TextContract {
    let mut contract = TextContract {
        source_revision: 1,
        contract_revision: 1,
        meaning_revision: 1,
        contract_digest: String::new(),
        gates: vec![],
        pauses: vec![],
        images: vec![],
        params: BTreeMap::new(),
    };
    contract.contract_digest = text_contract_digest(&contract);
    contract
}
#[test]
fn runtime_content_installs_valid_static_and_code() {
    let (view, batch) = runtime_fixture(
        BTreeMap::from([("m.main".into(), simple_function())]),
        empty_static(),
    );
    let next = view.install_batch(batch).unwrap();
    assert!(next.is_resident(&ContentKey::Static { module: "m".into() }));
    assert!(next.is_resident(&ContentKey::Code { module: "m".into() }));
    assert!(view.program().functions.get("m.main").is_none());
    assert!(next.program().functions.get("m.main").is_some());
}

#[test]
fn immutable_timeline_bodies_obey_typed_ownership_and_eviction() {
    let (base, mut batch) = runtime_fixture(
        BTreeMap::from([("m.main".into(), simple_function())]),
        empty_static(),
    );
    let mut root = base.runtime_root().unwrap().clone();
    root.requires.push("stage.sprite-timeline.v1".into());
    root.timeline_owners.insert("m.movie".into(), "m".into());
    let timeline = SpriteTimeline {
        id: "m.movie".into(),
        duration_us: Micros(1000),
        tracks: vec![SpriteTimelineTrack {
            node: "picture".into(),
            frames: vec![SpriteKeyframe {
                at_us: Micros(0),
                rect: [0., 0., 32., 24.],
                opacity: 1.,
                color: [1.; 4],
                transform: None,
            }],
        }],
    };
    if let RuntimeObject::Static(package) = &mut batch[0].1 {
        package
            .sprite_timelines
            .insert(timeline.id.clone(), timeline.clone());
    }
    let view = ValidatedProgram::from_runtime(root.clone()).unwrap();
    assert!(view.program().sprite_timelines.contains_key("m.movie"));
    assert!(view.program().sprite_timelines.get("m.movie").is_none());
    let mut bad = batch.clone();
    if let RuntimeObject::Static(package) = &mut bad[0].1 {
        package.sprite_timelines.clear();
    }
    assert_eq!(view.install_batch(bad).unwrap_err().code, "E_CONTENT_KEY");
    let mut bad = batch.clone();
    if let RuntimeObject::Static(package) = &mut bad[0].1 {
        package.sprite_timelines.get_mut("m.movie").unwrap().tracks[0].frames[0].opacity = -1.;
    }
    assert_eq!(view.install_batch(bad).unwrap_err().code, "E_TIMELINE");
    let loaded = view.install_batch(batch).unwrap();
    let body = loaded.program().sprite_timelines.shared("m.movie").unwrap();
    assert!(Arc::ptr_eq(
        &body,
        &loaded
            .clone()
            .program()
            .sprite_timelines
            .shared("m.movie")
            .unwrap()
    ));
    let key = ContentKey::Static { module: "m".into() };
    let lease = loaded
        .lease(BTreeSet::from([key.clone()]), "animation".into())
        .unwrap();
    assert_eq!(
        loaded
            .evict(BTreeSet::from([key.clone()]))
            .unwrap_err()
            .code,
        "E_CONTENT_PINNED"
    );
    drop(lease);
    let evicted = loaded.evict(BTreeSet::from([key])).unwrap();
    assert!(evicted.program().sprite_timelines.contains_key("m.movie"));
    assert!(evicted.program().sprite_timelines.get("m.movie").is_none());
    assert_eq!(body.as_ref(), &timeline);
    root.timeline_owners
        .insert("m.movie".into(), "missing".into());
    assert!(ValidatedProgram::from_runtime(root).is_err());
}

fn text_block(contract: &TextContract) -> RuntimeObject {
    text_block_locale("en", contract)
}

fn text_block_locale(locale: &str, contract: &TextContract) -> RuntimeObject {
    RuntimeObject::Text(ModuleTexts {
        format: RUNTIME_FORMAT_VERSION,
        module: "m".into(),
        locale: locale.into(),
        texts: BTreeMap::from([(
            "m.label".into(),
            TextDoc {
                source_revision: contract.source_revision,
                contract_revision: contract.contract_revision,
                contract_digest: contract.contract_digest.clone(),
                spans: vec![],
            },
        )]),
    })
}

fn catalog_block() -> RuntimeObject {
    let font = Asset {
        kind: AssetKind::Font,
        object: "f".repeat(64),
        bytes: 1,
        width: 0,
        height: 0,
        duration_us: Micros(0),
        decoded_bytes: 1,
    };
    let image = Asset {
        kind: AssetKind::Image,
        object: "1".repeat(64),
        bytes: 4,
        width: 1,
        height: 1,
        duration_us: Micros(0),
        decoded_bytes: 4,
    };
    RuntimeObject::Catalog(AssetCatalog {
        format: RUNTIME_FORMAT_VERSION,
        catalog: "resources".into(),
        assets: BTreeMap::from([("font".into(), font), ("image".into(), image)]),
    })
}

#[test]
fn delayed_catalog_completes_loop_validation_in_either_install_order() {
    let mut body = empty_static();
    body.cues.insert(
        "m.music".into(),
        serde_json::from_value(serde_json::json!({
            "effects":[{"id":"m.loop","scope":"session","effect":{
            "type":"audio","asset":"audio","bus":"bgm","looped":true,
                "loop_region":{"start_us":"200000","end_us":"600000"}
            }}]
        }))
        .unwrap(),
    );
    body.activation_recipes
        .insert("m.music".into(), BTreeSet::from(["audio".into()]));
    let (fixture, batch) =
        runtime_fixture(BTreeMap::from([("m.main".into(), simple_function())]), body);
    let mut root = fixture.runtime_root().unwrap().clone();
    root.requires
        .extend(["audio.buffer.v1".into(), "audio.loop-region.v1".into()]);
    root.assets.insert(
        "audio".into(),
        AssetIndexEntry {
            kind: AssetKind::Audio,
            object: "2".repeat(64),
            catalog: "resources".into(),
        },
    );
    let base = ValidatedProgram::from_runtime(root.clone()).unwrap();
    for catalog_first in [false, true] {
        for duration in [500_000, 800_000] {
            let RuntimeObject::Catalog(mut catalog) = catalog_block() else {
                unreachable!()
            };
            catalog.assets.insert(
                "audio".into(),
                Asset {
                    kind: AssetKind::Audio,
                    object: "2".repeat(64),
                    bytes: 1,
                    width: 0,
                    height: 0,
                    duration_us: Micros(duration),
                    decoded_bytes: 4,
                },
            );
            let catalog_batch = vec![(
                ContentKey::Catalog {
                    catalog: "resources".into(),
                },
                RuntimeObject::Catalog(catalog),
                100,
            )];
            let (first, second) = if catalog_first {
                (catalog_batch, batch.clone())
            } else {
                (batch.clone(), catalog_batch)
            };
            // Static declarations may arrive before duration metadata.
            let staged = base.install_batch(first).unwrap();
            let result = staged.install_batch(second);
            if duration == 800_000 {
                assert!(result.is_ok(), "{:?}", result.unwrap_err());
            } else {
                assert_eq!(result.unwrap_err().code, "E_AUDIO_LOOP");
                // A rejected candidate cannot add its objects to live state.
                assert_eq!(
                    staged.is_resident(&ContentKey::Static { module: "m".into() }),
                    !catalog_first
                );
                assert_eq!(
                    staged.is_resident(&ContentKey::Catalog {
                        catalog: "resources".into()
                    }),
                    catalog_first
                );
            }
        }
    }
    root.requires.retain(|cap| cap != "audio.loop-region.v1");
    assert_eq!(
        ValidatedProgram::from_runtime(root)
            .unwrap()
            .install_batch(batch)
            .unwrap_err()
            .code,
        "E_CAPABILITY"
    );
}

#[test]
fn runtime_content_eviction_is_independent_and_reloadable() {
    let mut static_package = empty_static();
    let contract = contract();
    static_package
        .text_contracts
        .insert("m.label".into(), contract.clone());
    static_package.cues.insert(
        "m.cue".into(),
        Cue {
            effects: vec![EffectDef {
                id: "m.wait".into(),
                scope: Scope::Session,
                effect: Effect::Delay {
                    duration_us: Micros(1),
                },
            }],
        },
    );
    static_package
        .activation_recipes
        .insert("m.cue".into(), BTreeSet::new());
    let (base, batch) = runtime_fixture(
        BTreeMap::from([("m.main".into(), simple_function())]),
        static_package,
    );
    let static_object = batch
        .iter()
        .find_map(|(key, object, _)| {
            matches!(key, ContentKey::Static { .. }).then(|| object.clone())
        })
        .unwrap();
    let static_bytes = batch
        .iter()
        .find_map(|(key, _, bytes)| matches!(key, ContentKey::Static { .. }).then_some(*bytes))
        .unwrap();
    let mut resident = base.install_batch(batch).unwrap();
    resident = resident
        .install_batch(vec![
            (
                ContentKey::Text {
                    module: "m".into(),
                    locale: "en".into(),
                },
                text_block(&contract),
                100,
            ),
            (
                ContentKey::Text {
                    module: "m".into(),
                    locale: "zh-Hans".into(),
                },
                text_block_locale("zh-Hans", &contract),
                100,
            ),
            (
                ContentKey::Catalog {
                    catalog: "resources".into(),
                },
                catalog_block(),
                100,
            ),
        ])
        .unwrap();
    assert!(resident.program().cues.get("m.cue").is_some());
    assert!(resident.program().functions.get("m.main").is_some());
    assert!(resident.program().locales["en"].get("m.label").is_some());
    assert!(resident.program().locales["zh-Hans"]
        .get("m.label")
        .is_some());
    assert!(resident.asset("image").is_some());
    let cue = resident.program.cues.values.get("m.cue").unwrap();
    let weak_cue = Arc::downgrade(cue);

    let without_static = resident
        .evict(BTreeSet::from([ContentKey::Static { module: "m".into() }]))
        .unwrap();
    assert!(!without_static.is_resident(&ContentKey::Static { module: "m".into() }));
    assert!(without_static.is_resident(&ContentKey::Code { module: "m".into() }));
    assert!(without_static.is_resident(&ContentKey::Text {
        module: "m".into(),
        locale: "en".into(),
    }));
    assert!(without_static.is_resident(&ContentKey::Catalog {
        catalog: "resources".into(),
    }));
    assert!(without_static.program().functions.get("m.main").is_some());
    assert!(without_static.program().locales["en"]
        .get("m.label")
        .is_some());
    assert!(without_static.program().locales["zh-Hans"]
        .get("m.label")
        .is_some());
    assert!(without_static.program().texts.get("m.label").is_none());
    assert!(weak_cue.upgrade().is_some());
    drop(resident);
    assert!(weak_cue.upgrade().is_none());

    let restored_static = without_static
        .install_batch(vec![(
            ContentKey::Static { module: "m".into() },
            static_object,
            static_bytes,
        )])
        .unwrap();
    assert!(restored_static.program().cues.get("m.cue").is_some());
    assert!(restored_static.program().texts.get("m.label").is_some());
    assert!(restored_static.program().locales["en"]
        .get("m.label")
        .is_some());
    assert!(restored_static.program().locales["zh-Hans"]
        .get("m.label")
        .is_some());

    let without_english = restored_static
        .evict(BTreeSet::from([ContentKey::Text {
            module: "m".into(),
            locale: "en".into(),
        }]))
        .unwrap();
    assert!(without_english.program().texts.get("m.label").is_some());
    assert!(without_english.program().locales["en"]
        .get("m.label")
        .is_none());
    assert!(without_english.program().locales["zh-Hans"]
        .get("m.label")
        .is_some());
    assert!(without_english.program().functions.get("m.main").is_some());
    let without_chinese = without_english
        .evict(BTreeSet::from([ContentKey::Text {
            module: "m".into(),
            locale: "zh-Hans".into(),
        }]))
        .unwrap();
    assert!(without_chinese.program().locales["zh-Hans"]
        .get("m.label")
        .is_none());
    let without_catalog = without_chinese
        .evict(BTreeSet::from([ContentKey::Catalog {
            catalog: "resources".into(),
        }]))
        .unwrap();
    assert!(without_catalog.asset("image").is_none());
    assert!(without_catalog.program().functions.get("m.main").is_some());
    assert!(without_catalog.program().cues.get("m.cue").is_some());
}

#[test]
fn leased_content_cannot_be_evicted_and_stale_views_cannot_repin_it() {
    let (base, batch) = runtime_fixture(
        BTreeMap::from([("m.main".into(), simple_function())]),
        empty_static(),
    );
    let resident = base.install_batch(batch).unwrap();
    let code = ContentKey::Code { module: "m".into() };
    let lease = resident
        .lease(BTreeSet::from([code.clone()]), "active frame".into())
        .unwrap();
    assert_eq!(
        resident
            .evict(BTreeSet::from([code.clone()]))
            .unwrap_err()
            .code,
        "E_CONTENT_PINNED"
    );
    assert!(resident.is_resident(&code));
    drop(lease);
    let retired = resident.evict(BTreeSet::from([code.clone()])).unwrap();
    assert!(!retired.is_resident(&code));
    assert_eq!(
        resident
            .lease(BTreeSet::from([code]), "stale view".into())
            .unwrap_err()
            .code,
        "E_CONTENT_RETIRED"
    );
}

#[test]
fn speculative_blocks_are_first_lru_victims_and_admission_is_atomic() {
    let mut static_package = empty_static();
    let contract = contract();
    static_package
        .text_contracts
        .insert("m.label".into(), contract.clone());
    let (base, batch) = runtime_fixture(
        BTreeMap::from([("m.main".into(), simple_function())]),
        static_package,
    );
    let mut resident = base.install_batch(batch).unwrap();
    let static_key = ContentKey::Static { module: "m".into() };
    let code_key = ContentKey::Code { module: "m".into() };
    let catalog_key = ContentKey::Catalog {
        catalog: "resources".into(),
    };
    let prefetch = resident
        .prepare_prefetch_batch(vec![(catalog_key.clone(), catalog_block(), 100)])
        .unwrap();
    assert_eq!(resident.residency().resident_blocks, 2);
    resident = prefetch.commit().unwrap();
    let budget = resident.residency().resident_bytes;
    resident = resident
        .set_residency_budget(ResidencyBudget {
            resident_bytes: budget,
        })
        .unwrap();
    let transaction = resident
        .prepare_install_batch(vec![(
            ContentKey::Text {
                module: "m".into(),
                locale: "en".into(),
            },
            text_block(&contract),
            100,
        )])
        .unwrap();
    assert_eq!(
        transaction.evicted_keys(),
        &BTreeSet::from([catalog_key.clone()])
    );
    assert!(resident.is_resident(&catalog_key));
    assert!(transaction.view().is_resident(&ContentKey::Text {
        module: "m".into(),
        locale: "en".into(),
    }));
    resident = transaction.commit().unwrap();
    assert!(!resident.is_resident(&catalog_key));
    assert!(resident.is_resident(&static_key));
    assert!(resident.is_resident(&code_key));
    assert_eq!(resident.residency().resident_bytes, budget);
}

#[test]
fn op_id_owner_tombstones_survive_code_eviction_and_stale_views() {
    let with_ops = |op_ids: &[&str]| Function {
        params: BTreeMap::new(),
        locals: BTreeMap::new(),
        returns: None,
        entry: "end".into(),
        blocks: BTreeMap::from([(
            "end".into(),
            block(
                Terminator::End {
                    outcome: "done".into(),
                },
                op_ids
                    .iter()
                    .map(|id| Op {
                        id: (*id).into(),
                        operation: Operation::ProfileMerge { key: "seen".into() },
                    })
                    .collect(),
            ),
        )]),
    };
    let functions = BTreeMap::from([
        ("m.main".into(), with_ops(&["m.main-entry", "m.shared"])),
        ("m.other".into(), with_ops(&["m.other-entry"])),
    ]);
    let (base, batch) = runtime_fixture(functions.clone(), empty_static());
    let static_object = batch
        .iter()
        .find_map(|(key, object, _)| {
            matches!(key, ContentKey::Static { .. }).then(|| object.clone())
        })
        .unwrap();
    let static_bytes = batch
        .iter()
        .find_map(|(key, _, bytes)| matches!(key, ContentKey::Static { .. }).then_some(*bytes))
        .unwrap();
    let stale_root = base.clone();
    let resident = base.install_batch(batch).unwrap();
    let code_key = ContentKey::Code { module: "m".into() };
    let without_code = resident.evict(BTreeSet::from([code_key.clone()])).unwrap();
    assert!(!without_code.program().functions.values().any(|_| true));
    let valid_reload = functions.clone();
    let reloaded = without_code
        .install_batch(vec![(
            code_key.clone(),
            RuntimeObject::Code(ModuleCode {
                format: RUNTIME_FORMAT_VERSION,
                module: "m".into(),
                functions: valid_reload,
            }),
            100,
        )])
        .unwrap();
    assert!(reloaded.program().functions.get("m.main").is_some());
    let without_code = reloaded.evict(BTreeSet::from([code_key])).unwrap();
    let conflicting = BTreeMap::from([
        ("m.main".into(), with_ops(&["m.main-entry"])),
        ("m.other".into(), with_ops(&["m.other-entry", "m.shared"])),
    ]);
    let bad = vec![
        (
            ContentKey::Static { module: "m".into() },
            static_object,
            static_bytes,
        ),
        (
            ContentKey::Code { module: "m".into() },
            RuntimeObject::Code(ModuleCode {
                format: RUNTIME_FORMAT_VERSION,
                module: "m".into(),
                functions: conflicting,
            }),
            100,
        ),
    ];
    assert_eq!(
        stale_root.install_batch(bad).unwrap_err().code,
        "E_DUPLICATE"
    );
    assert!(!without_code.program().functions.values().any(|_| true));
}

#[test]
fn runtime_code_rejects_unknown_task_and_invalid_dialogue_gate() {
    let mut static_package = empty_static();
    static_package.cues.insert(
        "m.cue".into(),
        Cue {
            effects: vec![EffectDef {
                id: "m.work".into(),
                scope: Scope::Session,
                effect: Effect::Delay {
                    duration_us: Micros(1),
                },
            }],
        },
    );
    static_package
        .activation_recipes
        .insert("m.cue".into(), BTreeSet::new());
    let function = Function {
        params: BTreeMap::new(),
        locals: BTreeMap::new(),
        returns: None,
        entry: "wait".into(),
        blocks: BTreeMap::from([
            (
                "wait".into(),
                block(
                    Terminator::Await {
                        on_advance: None,
                        conditions: vec![WaitCondition {
                            task: "m.work".into(),
                            milestone: Milestone::Marker("missing".into()),
                        }],
                        next: "done".into(),
                        on_cancelled: "done".into(),
                        on_failed: "done".into(),
                    },
                    vec![],
                ),
            ),
            (
                "done".into(),
                block(
                    Terminator::End {
                        outcome: "done".into(),
                    },
                    vec![],
                ),
            ),
        ]),
    };
    let (view, batch) = runtime_fixture(
        BTreeMap::from([("m.main".into(), function)]),
        static_package,
    );
    assert_eq!(view.install_batch(batch).unwrap_err().code, "E_MILESTONE");
}

#[test]
fn runtime_code_rejects_interaction_branch_mismatch() {
    let mut static_package = empty_static();
    static_package
        .text_contracts
        .insert("m.label".into(), contract());
    static_package.choices.insert(
        "m.choice".into(),
        Choice {
            options: vec![ChoiceOption {
                image: None,
                id: "yes".into(),
                text: "m.label".into(),
                visible: None,
                enabled: None,
                value: None,
            }],
            timeout_us: None,
            default: None,
        },
    );
    let function = Function {
        params: BTreeMap::new(),
        locals: BTreeMap::new(),
        returns: None,
        entry: "choice".into(),
        blocks: BTreeMap::from([
            (
                "choice".into(),
                block(
                    Terminator::Interact {
                        choice: "m.choice".into(),
                        branches: BTreeMap::from([("no".into(), "done".into())]),
                        on_empty: "done".into(),
                        result: None,
                        on_cancel: None,
                    },
                    vec![],
                ),
            ),
            (
                "done".into(),
                block(
                    Terminator::End {
                        outcome: "done".into(),
                    },
                    vec![],
                ),
            ),
        ]),
    };
    let (view, batch) = runtime_fixture(
        BTreeMap::from([("m.main".into(), function)]),
        static_package,
    );
    assert_eq!(view.install_batch(batch).unwrap_err().code, "E_CHOICE");
}

#[test]
fn runtime_code_rejects_duplicate_op_ids_across_functions() {
    let op = Op {
        id: "m.shared".into(),
        operation: Operation::ProfileMerge { key: "seen".into() },
    };
    let with_op = || Function {
        params: BTreeMap::new(),
        locals: BTreeMap::new(),
        returns: None,
        entry: "end".into(),
        blocks: BTreeMap::from([(
            "end".into(),
            block(
                Terminator::End {
                    outcome: "done".into(),
                },
                vec![op.clone()],
            ),
        )]),
    };
    let functions = BTreeMap::from([("m.main".into(), with_op()), ("m.other".into(), with_op())]);
    let (view, batch) = runtime_fixture(functions, empty_static());
    assert_eq!(view.install_batch(batch).unwrap_err().code, "E_DUPLICATE");
}

#[test]
fn runtime_code_rejects_undeclared_voice_binding_capability() {
    let mut function = simple_function();
    function
        .blocks
        .get_mut(&function.entry.clone())
        .unwrap()
        .ops
        .push(Op {
            id: "bind".into(),
            operation: Operation::DialogueVoice {
                task: "m.line".into(),
                voice: None,
                wait: VoiceWaitPolicy::Parallel,
            },
        });
    let (view, batch) = runtime_fixture(
        BTreeMap::from([("m.main".into(), function)]),
        empty_static(),
    );
    assert_eq!(view.install_batch(batch).unwrap_err().code, "E_CAPABILITY");
}

#[test]
fn runtime_static_rejects_undeclared_typed_target_capability() {
    let mut package = empty_static();
    package.cues.insert(
        "m.cue".into(),
        Cue {
            effects: vec![EffectDef {
                id: "m.fade".into(),
                scope: Scope::Session,
                effect: Effect::Tween {
                    target: TweenTarget::DialogueRoot {
                        property: DialogueProperty::Opacity,
                    },
                    to: 0.,
                    duration_us: Micros(1000),
                    replace: false,
                    easing: Easing::Linear,
                    finish: FinishPolicy::CommitEnd,
                    cancel: CancelPolicy::CommitCurrent,
                },
            }],
        },
    );
    package
        .activation_recipes
        .insert("m.cue".into(), BTreeSet::new());
    let (view, batch) = runtime_fixture(
        BTreeMap::from([("m.main".into(), simple_function())]),
        package,
    );
    assert_eq!(view.install_batch(batch).unwrap_err().code, "E_CAPABILITY");
}

#[test]
fn runtime_static_rejects_invalid_clip_values_and_incomplete_recipes() {
    let clip = EffectDef {
        id: "m.clip".into(),
        scope: Scope::Scene,
        effect: Effect::Clip {
            node: "art".into(),
            property: Property::X,
            to: f32::NAN,
            duration_us: Micros(1),
            replace: false,
            easing: Easing::Linear,
            finish: FinishPolicy::CommitEnd,
            cancel: CancelPolicy::CommitCurrent,
        },
    };
    let mut clip_package = empty_static();
    clip_package.cues.insert(
        "m.cue".into(),
        Cue {
            effects: vec![clip],
        },
    );
    clip_package
        .activation_recipes
        .insert("m.cue".into(), BTreeSet::new());
    let (view, batch) = runtime_fixture(
        BTreeMap::from([("m.main".into(), simple_function())]),
        clip_package,
    );
    assert_eq!(view.install_batch(batch).unwrap_err().code, "E_VISUAL");

    let mut recipe_package = empty_static();
    recipe_package.scenes.insert(
        "m.scene".into(),
        vec![Node {
            id: "art".into(),
            parent: None,
            asset: Some("image".into()),
            x: 0.,
            y: 0.,
            width: 1.,
            height: 1.,
            scale: 1.,
            opacity: 1.,
            color: [1.; 4],
            order: 0,
            clip: None,
            timeline_binding: None,
            inherit_existence: false,
            sprite_transform: None,
            bitmap_text: None,
            preserve_pose: vec![],
            offset: [0.; 2],
        }],
    );
    recipe_package.cues.insert(
        "m.cue".into(),
        Cue {
            effects: vec![EffectDef {
                id: "m.present".into(),
                scope: Scope::Scene,
                effect: Effect::StagePresent {
                    scene: "m.scene".into(),
                    dialogue_visible: None,
                    inherit_images: Vec::new(),
                    inherit_image_geometry: Vec::new(),
                    transition: StageTransition::default(),
                    duration_us: Micros(0),
                },
            }],
        },
    );
    recipe_package
        .activation_recipes
        .insert("m.cue".into(), BTreeSet::new());
    let (view, batch) = runtime_fixture(
        BTreeMap::from([("m.main".into(), simple_function())]),
        recipe_package,
    );
    assert_eq!(view.install_batch(batch).unwrap_err().code, "E_RECIPE");
}
