//! Integration checks for runtime views and staged snapshot verification.
use nir_core::*;
use nir_format::*;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

fn object<T: serde::Serialize>(objects: &mut BTreeMap<String, Vec<u8>>, value: &T) -> String {
    let bytes = serde_json::to_vec(value).unwrap();
    let hash = format!("{:x}", Sha256::digest(&bytes));
    objects.insert(hash.clone(), bytes);
    hash
}

fn bundled() -> (RuntimeProgram, BTreeMap<String, Vec<u8>>) {
    let mut p: Program = serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    for (id, asset) in &mut p.assets {
        asset.object = format!("{:064x}", id.bytes().map(u64::from).sum::<u64>() + 1000);
        asset.decoded_bytes = if asset.kind == AssetKind::Image {
            asset.width as u64 * asset.height as u64 * 4
        } else {
            1
        };
        if asset.kind == AssetKind::Audio {
            asset.duration_us = Micros(1_000_000);
        }
    }
    let media = p
        .assets
        .iter()
        .map(|(id, asset)| (id.clone(), asset.object.clone()))
        .collect();
    for plan in p
        .locale_config
        .ui
        .values_mut()
        .chain(p.locale_config.text.values_mut())
    {
        plan.digest = LocaleFontPlan::digest_for(&plan.fonts, &media);
    }
    let mut objects = BTreeMap::new();
    let owner = "story".to_owned();
    let owners = |ids: Vec<String>| ids.into_iter().map(|id| (id, owner.clone())).collect();
    let code = object(
        &mut objects,
        &ModuleCode {
            format: 2,
            module: owner.clone(),
            functions: p.functions.clone(),
        },
    );
    let mut locales = BTreeMap::new();
    for (locale, texts) in &p.locales {
        let hash = object(
            &mut objects,
            &ModuleTexts {
                format: 2,
                module: owner.clone(),
                locale: locale.clone(),
                texts: texts.clone(),
            },
        );
        locales.insert(locale.clone(), hash);
    }
    let source = ValidatedProgram::new(p.clone()).unwrap();
    let recipes = p
        .cues
        .keys()
        .map(|cue| {
            let mut assets = source.cue_assets(cue);
            assets.retain(|id| p.assets[id].kind != AssetKind::Font);
            (cue.clone(), assets)
        })
        .collect();
    let static_content = object(
        &mut objects,
        &ModuleStatic {
            format: 2,
            module: owner.clone(),
            scenes: p.scenes.clone(),
            cues: p.cues.clone(),
            choices: p.choices.clone(),
            text_contracts: p.texts.clone(),
            activation_recipes: recipes,
        },
    );
    let mut catalogs = BTreeMap::new();
    let mut assets = BTreeMap::new();
    for (id, asset) in &p.assets {
        let catalog = id.clone();
        let hash = object(
            &mut objects,
            &AssetCatalog {
                format: 2,
                catalog: catalog.clone(),
                assets: BTreeMap::from([(id.clone(), asset.clone())]),
            },
        );
        catalogs.insert(catalog.clone(), hash);
        assets.insert(
            id.clone(),
            AssetIndexEntry {
                kind: asset.kind,
                object: asset.object.clone(),
                catalog,
            },
        );
    }
    let title_nodes = p
        .title_scene
        .as_ref()
        .and_then(|id| p.scenes.get(id))
        .or_else(|| p.scenes.values().next())
        .cloned()
        .unwrap_or_default();
    let root = RuntimeProgram {
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
                code,
                static_content,
                locales,
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
        catalogs,
        default_locale: p.default_locale,
        title_scene: p.title_scene.or_else(|| p.scenes.keys().next().cloned()),
        title_nodes,
        theme: p.theme,
        player: p.player,
    };
    (root, objects)
}

fn package(
    root: &RuntimeProgram,
    objects: &BTreeMap<String, Vec<u8>>,
    key: ContentKey,
) -> (ContentKey, RuntimeObject, u64) {
    let digest = root.content_requirement(&key).unwrap().digest;
    let bytes = &objects[&digest];
    assert_eq!(format!("{:x}", Sha256::digest(bytes)), digest);
    let object = match &key {
        ContentKey::Static { .. } => RuntimeObject::Static(serde_json::from_slice(bytes).unwrap()),
        ContentKey::Code { .. } => RuntimeObject::Code(serde_json::from_slice(bytes).unwrap()),
        ContentKey::Text { .. } => RuntimeObject::Text(serde_json::from_slice(bytes).unwrap()),
        ContentKey::Catalog { .. } => {
            RuntimeObject::Catalog(serde_json::from_slice(bytes).unwrap())
        }
    };
    (key, object, bytes.len() as u64)
}

fn installed() -> (ValidatedProgram, BTreeMap<String, Vec<u8>>) {
    let (root, objects) = bundled();
    let mut keys = vec![
        ContentKey::Static {
            module: "story".into(),
        },
        ContentKey::Code {
            module: "story".into(),
        },
    ];
    keys.extend(root.locales.iter().map(|locale| ContentKey::Text {
        module: "story".into(),
        locale: locale.clone(),
    }));
    keys.extend(root.catalogs.keys().map(|catalog| ContentKey::Catalog {
        catalog: catalog.clone(),
    }));
    let batch = keys
        .into_iter()
        .map(|key| package(&root, &objects, key))
        .collect();
    (
        ValidatedProgram::from_runtime(root)
            .unwrap()
            .install_batch(batch)
            .unwrap(),
        objects,
    )
}

fn at_dialogue(view: ValidatedProgram) -> Core {
    at_dialogue_in(view, "zh-Hans")
}

fn at_dialogue_in(view: ValidatedProgram, locale: &str) -> Core {
    let mut core = Core::new(view, "release".into(), locale.into()).unwrap();
    for _ in 0..100 {
        let input = core
            .state()
            .pending
            .as_ref()
            .map(|p| CoreInput::Prepared { activation: p.id })
            .unwrap_or(CoreInput::None);
        core.step(input, 10000);
        assert!(core.state().fault.is_none(), "{:?}", core.state().fault);
        if core.dialogue().is_some() {
            return core;
        }
    }
    panic!("fixture never reached a dialogue");
}

#[test]
fn replacement_preserves_loading_barriers_but_rejects_missing_active_bodies() {
    let (view, objects) = installed();
    let empty = view.empty_content_view().unwrap();
    let mut loading = Core::new(empty.clone(), "release".into(), "zh-Hans".into()).unwrap();
    let catalog = empty
        .runtime_root()
        .unwrap()
        .catalogs
        .keys()
        .next()
        .unwrap()
        .clone();
    let batch = vec![package(
        empty.runtime_root().unwrap(),
        &objects,
        ContentKey::Catalog { catalog },
    )];
    loading
        .replace_program(empty.install_batch(batch).unwrap())
        .unwrap();
    // A cached Code object may outlive its independently evicted Static
    // object before a new Core begins executing. Catalog admission must not
    // turn this ordinary content barrier into a replacement error.
    let scratch = view.empty_content_view().unwrap();
    let root = scratch.runtime_root().unwrap();
    let static_key = ContentKey::Static {
        module: "story".into(),
    };
    let code_only = scratch
        .install_batch(vec![
            package(root, &objects, static_key.clone()),
            package(
                root,
                &objects,
                ContentKey::Code {
                    module: "story".into(),
                },
            ),
        ])
        .unwrap()
        .evict(BTreeSet::from([static_key]))
        .unwrap();
    let mut cached = Core::new(code_only.clone(), "release".into(), "zh-Hans".into()).unwrap();
    let before = serde_json::to_value(cached.state()).unwrap();
    let key = ContentKey::Catalog {
        catalog: root.catalogs.keys().next().unwrap().clone(),
    };
    cached
        .replace_program(
            code_only
                .install_batch(vec![package(root, &objects, key)])
                .unwrap(),
        )
        .unwrap();
    assert_eq!(serde_json::to_value(cached.state()).unwrap(), before);
    let mut active = at_dialogue(view);
    let before = serde_json::to_value(active.state()).unwrap();
    assert_eq!(
        active.replace_program(empty).unwrap_err().code,
        "E_CONTENT_MISSING"
    );
    assert_eq!(serde_json::to_value(active.state()).unwrap(), before);
    let retired = active
        .validated_program()
        .evict(BTreeSet::from([ContentKey::Static {
            module: "story".into(),
        }]))
        .unwrap();
    assert_eq!(
        active.replace_program(retired).unwrap_err().code,
        "E_CONTENT_MISSING"
    );
    assert_eq!(serde_json::to_value(active.state()).unwrap(), before);
}

#[test]
fn instantiated_dialogue_reveals_without_its_original_text_package() {
    let (view, objects) = installed();
    let mut core = at_dialogue(view.clone());
    let scratch = view.empty_content_view().unwrap();
    let root = scratch.runtime_root().unwrap();
    let mut keys = vec![
        ContentKey::Static {
            module: "story".into(),
        },
        ContentKey::Code {
            module: "story".into(),
        },
    ];
    keys.extend(root.catalogs.keys().map(|catalog| ContentKey::Catalog {
        catalog: catalog.clone(),
    }));
    let replacement = scratch
        .install_batch(
            keys.into_iter()
                .map(|key| package(root, &objects, key))
                .collect(),
        )
        .unwrap();
    let dialogue_text = core.dialogue().unwrap().1.text_id.clone();
    assert!(replacement.program().locales["zh-Hans"]
        .get(&dialogue_text)
        .is_none());
    core.replace_program(replacement).unwrap();
    let interaction = core.dialogue().unwrap().1.interaction;
    core.step(
        CoreInput::Advance {
            interaction,
            sequence: 1,
        },
        10000,
    );
    assert!(core.state().fault.is_none(), "{:?}", core.state().fault);
    assert!(core.dialogue().is_some());
}

fn verify_snapshot(
    view: &ValidatedProgram,
    objects: &BTreeMap<String, Vec<u8>>,
    snapshot: Snapshot,
) -> Result<VerifiedSnapshot> {
    let mut session = RestoreSession::new(view, snapshot, "release")?;
    while let Some(requirements) = session.next_requirements() {
        let scratch = view.empty_content_view()?;
        let batch = requirements
            .into_iter()
            .map(|r| package(scratch.runtime_root().unwrap(), objects, r.key))
            .collect();
        let scratch = scratch.install_batch(batch)?;
        session.verify_next_unit(&scratch)?;
    }
    session.finish()
}

#[test]
fn staged_and_eager_restore_preserve_the_same_semantics() {
    let (view, objects) = installed();
    let core = at_dialogue(view.clone());
    let snapshot = core.snapshot();
    let eager = Core::restore(view.clone(), snapshot.clone(), "release").unwrap();
    let proof = verify_snapshot(&view, &objects, snapshot).unwrap();
    let staged = Core::restore_verified(view, proof, "release").unwrap();
    assert_eq!(
        serde_json::to_value(staged.state()).unwrap(),
        serde_json::to_value(eager.state()).unwrap()
    );
}

#[test]
fn staged_proof_rejects_incomplete_coverage_and_an_independent_root() {
    let (view, objects) = installed();
    let snapshot = at_dialogue(view.clone()).snapshot();
    let session = RestoreSession::new(&view, snapshot.clone(), "release").unwrap();
    assert_eq!(session.finish().unwrap_err().code, "E_SNAPSHOT");
    let mut session = RestoreSession::new(&view, snapshot.clone(), "release").unwrap();
    let (foreign, _) = installed();
    assert_eq!(
        session.verify_next_unit(&foreign).unwrap_err().code,
        "E_SNAPSHOT"
    );
    let proof = verify_snapshot(&view, &objects, snapshot).unwrap();
    assert_eq!(
        Core::restore_verified(foreign, proof, "release")
            .err()
            .unwrap()
            .code,
        "E_SNAPSHOT"
    );
}

#[test]
fn staged_verification_rejects_tampered_frozen_text_and_task_effect() {
    let (view, objects) = installed();
    let snapshot = at_dialogue(view.clone()).snapshot();
    for mutate_text in [false, true] {
        let mut changed = snapshot.clone();
        let task = changed
            .tasks
            .values_mut()
            .find(|t| t.dialogue.is_some())
            .unwrap();
        if mutate_text {
            task.dialogue.as_mut().unwrap().spans[0]
                .text
                .push_str("tampered");
        } else {
            let Effect::Dialogue { speaker, .. } = &mut task.effect else {
                panic!("dialogue effect");
            };
            speaker.push_str("tampered");
        }
        assert_eq!(
            Core::restore(view.clone(), changed.clone(), "release")
                .err()
                .unwrap()
                .code,
            "E_SNAPSHOT"
        );
        assert_eq!(
            verify_snapshot(&view, &objects, changed).unwrap_err().code,
            "E_SNAPSHOT"
        );
    }
}

#[test]
fn staged_proof_does_not_replace_structural_snapshot_validation() {
    let (view, objects) = installed();
    let mut snapshot = at_dialogue(view.clone()).snapshot();
    snapshot.handles.insert("dangling".into(), u32::MAX);
    let proof = verify_snapshot(&view, &objects, snapshot).unwrap();
    assert_eq!(
        Core::restore_verified(view, proof, "release")
            .err()
            .unwrap()
            .code,
        "E_SNAPSHOT"
    );
}

#[test]
fn verified_restore_needs_no_original_dialogue_text_in_the_active_view() {
    let (view, objects) = installed();
    let snapshot = at_dialogue(view.clone()).snapshot();
    let proof = verify_snapshot(&view, &objects, snapshot.clone()).unwrap();
    let scratch = view.empty_content_view().unwrap();
    let root = scratch.runtime_root().unwrap();
    let keys = [
        ContentKey::Static {
            module: "story".into(),
        },
        ContentKey::Code {
            module: "story".into(),
        },
    ];
    let active = scratch
        .install_batch(
            keys.into_iter()
                .map(|key| package(root, &objects, key))
                .collect(),
        )
        .unwrap();
    assert!(Core::restore(active.clone(), snapshot, "release").is_err());
    let restored = Core::restore_verified(active, proof, "release").unwrap();
    assert!(restored.dialogue().is_some());
}

#[test]
fn a_corrupt_later_locale_unit_cannot_finish_a_partial_proof() {
    let (view, objects) = installed();
    let mut snapshot = at_dialogue(view.clone()).snapshot();
    let english = at_dialogue_in(view.clone(), "en");
    let mut task = english
        .state()
        .tasks
        .values()
        .find(|t| t.dialogue.is_some())
        .unwrap()
        .clone();
    task.id = snapshot.next_id;
    snapshot.next_id += 1;
    snapshot.tasks.insert(task.id, task);
    let task = snapshot
        .tasks
        .values_mut()
        .find(|t| t.dialogue.as_ref().is_some_and(|d| d.locale == "zh-Hans"))
        .unwrap();
    task.dialogue.as_mut().unwrap().spans[0]
        .text
        .push_str("changed");
    let mut session = RestoreSession::new(&view, snapshot, "release").unwrap();
    let mut verified = 0;
    while let Some(requirements) = session.next_requirements() {
        let scratch = view.empty_content_view().unwrap();
        let batch = requirements
            .into_iter()
            .map(|r| package(scratch.runtime_root().unwrap(), &objects, r.key))
            .collect();
        let scratch = scratch.install_batch(batch).unwrap();
        if session.verify_next_unit(&scratch).is_err() {
            break;
        }
        verified += 1;
    }
    assert_eq!(
        verified, 1,
        "English verifies before the corrupt original locale"
    );
    assert_eq!(session.finish().unwrap_err().code, "E_SNAPSHOT");
}

fn long_fixture() -> (RuntimeProgram, BTreeMap<String, Vec<u8>>) {
    let (mut root, mut objects) = bundled();
    let effect = Effect::Delay {
        duration_us: Micros(0),
    };
    for index in 0..101 {
        let module = format!("z{index:03}");
        let function_id = format!("{module}.main");
        let task_id = format!("{module}.delay");
        let cue_id = format!("{module}.cue");
        let function = Function {
            params: BTreeMap::new(),
            locals: BTreeMap::new(),
            returns: None,
            entry: "activate".into(),
            blocks: BTreeMap::from([
                (
                    "activate".into(),
                    Block {
                        ops: vec![],
                        terminator: Terminator::Activate {
                            cue: cue_id.clone(),
                            next: "wait".into(),
                        },
                    },
                ),
                (
                    "wait".into(),
                    Block {
                        ops: vec![],
                        terminator: Terminator::Await {
                            conditions: vec![WaitCondition {
                                task: task_id.clone(),
                                milestone: Milestone::Finished,
                            }],
                            next: "end".into(),
                            on_cancelled: "end".into(),
                            on_failed: "end".into(),
                        },
                    },
                ),
                (
                    "end".into(),
                    Block {
                        ops: vec![],
                        terminator: Terminator::Return { value: None },
                    },
                ),
            ]),
        };
        let signature = FunctionSignature::from(&function);
        let code = object(
            &mut objects,
            &ModuleCode {
                format: 2,
                module: module.clone(),
                functions: BTreeMap::from([(function_id.clone(), function)]),
            },
        );
        let static_content = object(
            &mut objects,
            &ModuleStatic {
                format: 2,
                module: module.clone(),
                scenes: BTreeMap::new(),
                choices: BTreeMap::new(),
                text_contracts: BTreeMap::new(),
                cues: BTreeMap::from([(
                    cue_id.clone(),
                    Cue {
                        effects: vec![EffectDef {
                            id: task_id.clone(),
                            scope: Scope::Frame,
                            effect: effect.clone(),
                        }],
                    },
                )]),
                activation_recipes: BTreeMap::from([(cue_id.clone(), Default::default())]),
            },
        );
        // Valid trailing JSON whitespace makes this an actual encoded-byte
        // workload, without huge dialogue payloads or fabricated size metadata.
        let mut padded = objects.remove(&static_content).unwrap();
        padded.extend(std::iter::repeat_n(b' ', 180 * 1024));
        let static_content = format!("{:x}", Sha256::digest(&padded));
        objects.insert(static_content.clone(), padded);
        root.modules.insert(
            module.clone(),
            ModuleIndex {
                functions: BTreeMap::from([(function_id.clone(), signature.clone())]),
                texts: Default::default(),
                code,
                static_content,
                locales: BTreeMap::new(),
            },
        );
        root.function_index.insert(
            function_id,
            RuntimeFunctionIndex {
                module: module.clone(),
                signature,
            },
        );
        root.cue_owners.insert(cue_id, module.clone());
        root.task_owners.insert(task_id, module);
    }
    (root, objects)
}

#[test]
fn canonical_restore_closure_can_exceed_the_resident_budget() {
    let (root, objects) = long_fixture();
    let effect = Effect::Delay {
        duration_us: Micros(0),
    };
    let mut keys = vec![
        ContentKey::Static {
            module: "story".into(),
        },
        ContentKey::Code {
            module: "story".into(),
        },
    ];
    keys.extend(root.locales.iter().map(|locale| ContentKey::Text {
        module: "story".into(),
        locale: locale.clone(),
    }));
    keys.extend(root.catalogs.keys().map(|catalog| ContentKey::Catalog {
        catalog: catalog.clone(),
    }));
    let batch = keys
        .into_iter()
        .map(|key| package(&root, &objects, key))
        .collect();
    let view = ValidatedProgram::from_runtime(root)
        .unwrap()
        .install_batch(batch)
        .unwrap();
    let mut snapshot = at_dialogue(view.clone()).snapshot();
    let template = snapshot.tasks.values().next().unwrap().clone();
    for index in 0..101 {
        let mut task = template.clone();
        task.id = snapshot.next_id;
        snapshot.next_id += 1;
        task.name = format!("z{index:03}.delay");
        task.scope = Scope::Frame;
        task.effect = effect.clone();
        task.state = TaskState::Finished;
        task.milestones = std::collections::BTreeSet::from([Milestone::Finished]);
        task.dialogue = None;
        task.source.clear();
        task.target.clear();
        snapshot.handles.insert(task.name.clone(), task.id);
        snapshot.tasks.insert(task.id, task);
    }
    assert!(Core::restore(view.clone(), snapshot.clone(), "release").is_err());
    let resident_before = view.residency().resident_bytes;
    let mut session = RestoreSession::new(&view, snapshot, "release").unwrap();
    let mut verified_bytes = 0;
    let mut units = 0;
    while let Some(requirements) = session.next_requirements() {
        let scratch = view.empty_content_view().unwrap();
        let batch = requirements
            .into_iter()
            .map(|r| package(scratch.runtime_root().unwrap(), &objects, r.key))
            .collect();
        let scratch = scratch.install_batch(batch).unwrap();
        let bytes = scratch.residency().resident_bytes;
        assert!(bytes <= MAX_INPUT_BYTES as u64);
        verified_bytes += bytes;
        units += 1;
        session.verify_next_unit(&scratch).unwrap();
    }
    assert!(verified_bytes > MAX_INPUT_BYTES as u64);
    assert_eq!(units, 102);
    let restored =
        Core::restore_verified(view.clone(), session.finish().unwrap(), "release").unwrap();
    assert!(restored.state().handles.contains_key("z100.delay"));
    assert_eq!(view.residency().resident_bytes, resident_before);
    assert!(!view.is_resident(&ContentKey::Static {
        module: "z100".into()
    }));
}

#[test]
fn sequential_calls_across_101_modules_evict_reload_and_keep_old_handles() {
    let (mut root, mut objects) = long_fixture();
    let mut blocks = BTreeMap::new();
    for index in 0..101 {
        blocks.insert(
            format!("call{index}"),
            Block {
                ops: vec![],
                terminator: Terminator::Call {
                    function: format!("z{index:03}.main"),
                    args: BTreeMap::new(),
                    next: format!("call{}", index + 1),
                    result: None,
                },
            },
        );
    }
    // Re-enter the first module after eviction, awaiting its old terminal
    // handle in a distinct function so the original task is not overwritten.
    let probe = Function {
        params: BTreeMap::new(),
        locals: BTreeMap::new(),
        returns: None,
        entry: "wait".into(),
        blocks: BTreeMap::from([
            (
                "wait".into(),
                Block {
                    ops: vec![],
                    terminator: Terminator::Await {
                        conditions: vec![WaitCondition {
                            task: "z000.delay".into(),
                            milestone: Milestone::Finished,
                        }],
                        next: "end".into(),
                        on_cancelled: "bad".into(),
                        on_failed: "bad".into(),
                    },
                },
            ),
            (
                "end".into(),
                Block {
                    ops: vec![],
                    terminator: Terminator::Return { value: None },
                },
            ),
            (
                "bad".into(),
                Block {
                    ops: vec![],
                    terminator: Terminator::End {
                        outcome: "bad-handle".into(),
                    },
                },
            ),
        ]),
    };
    let signature = FunctionSignature::from(&probe);
    let module = root.modules.get_mut("z000").unwrap();
    let mut code: ModuleCode = serde_json::from_slice(&objects[&module.code]).unwrap();
    code.functions.insert("z000.probe".into(), probe);
    module.code = object(&mut objects, &code);
    module
        .functions
        .insert("z000.probe".into(), signature.clone());
    root.function_index.insert(
        "z000.probe".into(),
        RuntimeFunctionIndex {
            module: "z000".into(),
            signature,
        },
    );
    blocks.insert(
        "call101".into(),
        Block {
            ops: vec![],
            terminator: Terminator::Call {
                function: "z000.probe".into(),
                args: BTreeMap::new(),
                next: "done".into(),
                result: None,
            },
        },
    );
    blocks.insert(
        "done".into(),
        Block {
            ops: vec![],
            terminator: Terminator::End {
                outcome: "long-done".into(),
            },
        },
    );
    blocks.insert(
        "bad".into(),
        Block {
            ops: vec![],
            terminator: Terminator::End {
                outcome: "bad-handle".into(),
            },
        },
    );
    let function = Function {
        params: BTreeMap::new(),
        locals: BTreeMap::new(),
        returns: None,
        entry: "call0".into(),
        blocks,
    };
    let signature = FunctionSignature::from(&function);
    root.function_index
        .retain(|_, index| index.module != "story");
    root.function_index.insert(
        root.entry.clone(),
        RuntimeFunctionIndex {
            module: "story".into(),
            signature: signature.clone(),
        },
    );
    let story = root.modules.get_mut("story").unwrap();
    story.functions = BTreeMap::from([(root.entry.clone(), signature)]);
    story.code = object(
        &mut objects,
        &ModuleCode {
            format: 2,
            module: "story".into(),
            functions: BTreeMap::from([(root.entry.clone(), function)]),
        },
    );
    let mut view = ValidatedProgram::from_runtime(root)
        .unwrap()
        .set_residency_budget(ResidencyBudget {
            resident_bytes: 256 * 1024,
        })
        .unwrap();
    let mut core = Core::new(view.clone(), "release".into(), "zh-Hans".into()).unwrap();
    let mut pinned = None;
    let mut fetched_bytes = 0;
    let mut first_module_loads = 0;
    let mut evicted = 0;
    for _ in 0..1000 {
        let input = core
            .state()
            .pending
            .as_ref()
            .map(|p| CoreInput::Prepared { activation: p.id })
            .unwrap_or(CoreInput::Time { delta_us: 1 });
        let step = core.step(input, 10000);
        assert!(core.state().fault.is_none(), "{:?}", core.state().fault);
        for intent in step.intents {
            let CoreIntent::PrepareContent { module, locale } = intent else {
                continue;
            };
            let root = view.runtime_root().unwrap();
            let active_keys = core
                .state()
                .frames
                .iter()
                .flat_map(|frame| {
                    let module = root.function_index[&frame.function].module.clone();
                    [
                        ContentKey::Static {
                            module: module.clone(),
                        },
                        ContentKey::Code { module },
                    ]
                })
                .filter(|key| view.is_resident(key))
                .collect();
            let next_lease = view.lease(active_keys, "live".into()).unwrap();
            drop(pinned.replace(next_lease));
            let mut keys = vec![
                ContentKey::Static {
                    module: module.clone(),
                },
                ContentKey::Code {
                    module: module.clone(),
                },
            ];
            if !root.modules[&module].texts.is_empty() {
                keys.push(ContentKey::Text {
                    module: module.clone(),
                    locale,
                });
            }
            let batch: Vec<_> = keys
                .into_iter()
                .filter(|key| !view.is_resident(key))
                .map(|key| package(root, &objects, key))
                .collect();
            fetched_bytes += batch.iter().map(|(_, _, bytes)| bytes).sum::<u64>();
            first_module_loads += usize::from(module == "z000");
            let admission = view.prepare_install_batch(batch).unwrap();
            evicted += admission.evicted_keys().len();
            let mut checked = core.clone();
            checked.replace_program(admission.view().clone()).unwrap();
            view = admission.commit().unwrap();
            core.replace_program(view.clone()).unwrap();
            assert!(view.residency().resident_bytes <= 256 * 1024);
        }
        if core.state().outcome.is_some() {
            break;
        }
    }
    assert_eq!(core.state().outcome.as_deref(), Some("long-done"));
    assert_eq!(first_module_loads, 2);
    assert!(fetched_bytes > MAX_INPUT_BYTES as u64);
    assert!(evicted >= 100);
    assert!(core.state().handles.contains_key("z100.delay"));
}

fn prediction_fixture(hops: usize, stop: Option<Terminator>) -> Core {
    let (mut root, mut objects) = bundled();
    let future_function = Function {
        params: BTreeMap::new(),
        locals: BTreeMap::new(),
        returns: None,
        entry: "end".into(),
        blocks: BTreeMap::from([(
            "end".into(),
            Block {
                ops: vec![],
                terminator: Terminator::Return { value: None },
            },
        )]),
    };
    let signature = FunctionSignature::from(&future_function);
    let future_code = object(
        &mut objects,
        &ModuleCode {
            format: 2,
            module: "future".into(),
            functions: BTreeMap::from([("future.main".into(), future_function)]),
        },
    );
    let future_static = object(
        &mut objects,
        &ModuleStatic {
            format: 2,
            module: "future".into(),
            scenes: BTreeMap::new(),
            cues: BTreeMap::new(),
            choices: BTreeMap::new(),
            text_contracts: BTreeMap::new(),
            activation_recipes: BTreeMap::new(),
        },
    );
    root.modules.insert(
        "future".into(),
        ModuleIndex {
            functions: BTreeMap::from([("future.main".into(), signature.clone())]),
            texts: Default::default(),
            code: future_code,
            static_content: future_static,
            locales: BTreeMap::new(),
        },
    );
    root.function_index.insert(
        "future.main".into(),
        RuntimeFunctionIndex {
            module: "future".into(),
            signature,
        },
    );
    let mut code: ModuleCode =
        serde_json::from_slice(&objects[&root.modules["story"].code]).unwrap();
    let function = code.functions.get_mut(&root.entry).unwrap();
    let Terminator::Await { next, .. } =
        &mut function.blocks.get_mut("wait_intro").unwrap().terminator
    else {
        panic!("wait");
    };
    *next = "probe0".into();
    for index in 0..=hops {
        let terminator = if index < hops {
            Terminator::Goto {
                target: format!("probe{}", index + 1),
            }
        } else {
            Terminator::Call {
                function: "future.main".into(),
                args: BTreeMap::new(),
                next: "end_stay".into(),
                result: None,
            }
        };
        function.blocks.insert(
            format!("probe{index}"),
            Block {
                ops: vec![],
                terminator,
            },
        );
    }
    let probe = function.blocks.get_mut("probe0").unwrap();
    probe.ops.push(Op {
        id: "prediction.random".into(),
        operation: Operation::Random {
            target: "affection".into(),
            min: 0,
            max: 100,
        },
    });
    if let Some(stop) = stop {
        probe.terminator = stop;
    }
    root.modules.get_mut("story").unwrap().code = object(&mut objects, &code);
    let mut keys = vec![
        ContentKey::Static {
            module: "story".into(),
        },
        ContentKey::Code {
            module: "story".into(),
        },
    ];
    keys.extend(root.locales.iter().map(|locale| ContentKey::Text {
        module: "story".into(),
        locale: locale.clone(),
    }));
    let batch = keys
        .into_iter()
        .map(|key| package(&root, &objects, key))
        .collect();
    at_dialogue(
        ValidatedProgram::from_runtime(root)
            .unwrap()
            .install_batch(batch)
            .unwrap(),
    )
}

#[test]
fn prefetch_prediction_is_read_only_and_bounded_to_64_blocks() {
    for (hops, expected) in [(0, Some("future")), (62, Some("future")), (63, None)] {
        let core = prediction_fixture(hops, None);
        let before = serde_json::to_value(core.state()).unwrap();
        assert_eq!(core.prefetch_module().as_deref(), expected);
        assert_eq!(core.prefetch_module().as_deref(), expected);
        assert_eq!(
            serde_json::to_value(core.state()).unwrap(),
            before,
            "prediction must not consume RNG or IDs"
        );
    }
}

#[test]
fn prefetch_prediction_stops_at_uncertain_or_terminal_control_flow() {
    for stop in [
        Terminator::Branch {
            condition: Expr::Const {
                value: Value::Bool(true),
            },
            yes: "probe1".into(),
            no: "probe1".into(),
        },
        Terminator::Switch {
            value: Expr::Const {
                value: Value::I32(0),
            },
            cases: BTreeMap::from([("0".into(), "probe1".into())]),
            default: "probe1".into(),
        },
        Terminator::Goto {
            target: "probe0".into(),
        },
        Terminator::Return { value: None },
        Terminator::End {
            outcome: "done".into(),
        },
        Terminator::Call {
            function: "main".into(),
            args: BTreeMap::new(),
            next: "probe1".into(),
            result: None,
        },
        Terminator::Interact {
            choice: "route".into(),
            branches: BTreeMap::from([
                ("walk".into(), "walk_begin".into()),
                ("stay".into(), "stay_begin".into()),
            ]),
            on_empty: "failed".into(),
        },
    ] {
        assert!(prediction_fixture(1, Some(stop))
            .prefetch_module()
            .is_none());
    }
}
