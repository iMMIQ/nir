use nir_format::*;
use nir_player::*;
use std::collections::{BTreeMap, BTreeSet};

fn object<T: serde::Serialize>(objects: &mut BTreeMap<String, Vec<u8>>, value: &T) -> String {
    let bytes = serde_json::to_vec(value).unwrap();
    let hash = nir_content::digest(&bytes);
    objects.insert(hash.clone(), bytes);
    hash
}

fn bundled() -> (RuntimeProgram, BTreeMap<String, Vec<u8>>) {
    let mut p: Program = serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    for (id, asset) in &mut p.assets {
        asset.object = nir_content::digest(id.as_bytes());
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
    let recipes = p
        .cues
        .keys()
        .map(|cue| {
            let mut assets = nir_content::cue_assets(&p, cue);
            assets.retain(|id| p.assets[id].kind != AssetKind::Font);
            (cue.clone(), assets)
        })
        .collect();
    let static_content = object(
        &mut objects,
        &ModuleStatic {
            sprite_timelines: Default::default(),
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
        timeline_owners: Default::default(),
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
        catalogs,
        default_locale: p.default_locale,
        title_scene: p.title_scene.or_else(|| p.scenes.keys().next().cloned()),
        title_nodes,
        theme: p.theme,
        player: p.player,
    };
    (root, objects)
}

fn action(p: &mut Player, action: UiAction) -> Vec<AppCommand> {
    p.pump(
        vec![AppEvent::Action {
            action,
            interaction: p.current_interaction(),
            sequence: p.core().state().last_input + 1,
            session: p.generation.session,
        }],
        1000,
    )
}
fn drain(
    p: &mut Player,
    initial: Vec<AppCommand>,
    objects: &BTreeMap<String, Vec<u8>>,
) -> BTreeSet<ContentKey> {
    let mut q = initial;
    let mut requested = BTreeSet::new();
    for _ in 0..100 {
        if q.is_empty() {
            return requested;
        }
        let mut next = vec![];
        for command in q {
            let events = match command {
                AppCommand::GetContent {
                    request,
                    objects: requirements,
                    ..
                } => {
                    requested.extend(requirements.iter().filter_map(|r| r.key.clone()));
                    vec![AppEvent::ContentReady {
                        request,
                        objects: requirements
                            .iter()
                            .map(|r| objects[&r.hash].clone())
                            .collect(),
                    }]
                }
                AppCommand::GetAssets {
                    request,
                    assets,
                    descriptors,
                    ..
                } => {
                    assert_eq!(
                        assets.iter().cloned().collect::<BTreeSet<_>>(),
                        descriptors.keys().cloned().collect()
                    );
                    assets
                        .into_iter()
                        .map(|asset| AppEvent::AssetReady { request, asset })
                        .collect()
                }
                AppCommand::PreparePresentation { request } => {
                    vec![AppEvent::PresentationReady { request }]
                }
                AppCommand::PrepareLocale { request, .. } => {
                    vec![AppEvent::LocaleReady { request }]
                }
                _ => vec![],
            };
            if !events.is_empty() {
                next.extend(p.pump(events, 1000));
            }
        }
        q = next;
    }
    panic!("preparation did not converge: {:?}", p.diagnostic);
}

#[test]
fn consecutive_cues_commit_before_the_next_asset_catalog_is_ready() {
    let (mut root, mut objects) = bundled();
    let function: Function = serde_json::from_value(serde_json::json!({
        "entry":"start","blocks":{
            "start":{"terminator":{"type":"activate","cue":"opening","next":"enter"}},
            "enter":{"terminator":{"type":"activate","cue":"enter","next":"wait"}},
            "wait":{"terminator":{"type":"await","conditions":[{"task":"stage","milestone":{"type":"finished"}}],"next":"done","on_cancelled":"done","on_failed":"done"}},
            "done":{"terminator":{"type":"end","outcome":"completed"}}
        }
    })).unwrap();
    let signature = FunctionSignature::from(&function);
    let code = object(
        &mut objects,
        &ModuleCode {
            format: 2,
            module: "story".into(),
            functions: BTreeMap::from([("main".into(), function)]),
        },
    );
    let module = root.modules.get_mut("story").unwrap();
    module.code = code;
    module.functions = BTreeMap::from([("main".into(), signature.clone())]);
    root.function_index = BTreeMap::from([(
        "main".into(),
        RuntimeFunctionIndex {
            module: "story".into(),
            execution_module: None,
            signature,
        },
    )]);
    let declarations: ModuleStatic =
        serde_json::from_slice(&objects[&root.modules["story"].static_content]).unwrap();
    root.title_scene = Some("station".into());
    root.title_nodes = declarations.scenes["station"].clone();
    let mut p = Player::new_runtime(root, "release".into(), "Test".into(), None).unwrap();
    let boot = p.pump(vec![], 1000);
    drain(&mut p, boot, &objects);
    assert!(p.asset_descriptor("actor.aki").is_none());
    let boot_memory = p.memory_used();
    let mut queue = action(&mut p, UiAction::NewGame);
    let mut held = None;
    for _ in 0..20 {
        let mut next = vec![];
        for command in queue {
            let events = match command {
                AppCommand::GetContent {
                    request,
                    objects: requirements,
                    ..
                } => {
                    if requirements.iter().any(|r| matches!(&r.key, Some(ContentKey::Catalog{catalog}) if catalog=="actor.aki")) {
                        held = Some((request,requirements));
                        continue;
                    }
                    vec![AppEvent::ContentReady {
                        request,
                        objects: requirements
                            .iter()
                            .map(|r| objects[&r.hash].clone())
                            .collect(),
                    }]
                }
                AppCommand::GetAssets {
                    request, assets, ..
                } => assets
                    .into_iter()
                    .map(|asset| AppEvent::AssetReady { request, asset })
                    .collect(),
                AppCommand::PreparePresentation { request } => {
                    vec![AppEvent::PresentationReady { request }]
                }
                AppCommand::PrepareLocale { request, .. } => {
                    vec![AppEvent::LocaleReady { request }]
                }
                _ => vec![],
            };
            if !events.is_empty() {
                next.extend(p.pump(events, 1000));
            }
        }
        queue = next;
        if held.is_some() {
            break;
        }
    }
    let (request, requirements) = held.expect("next cue must request its own catalog");
    assert_eq!(p.error, None);
    assert_eq!(p.diagnostic, None);
    assert_eq!(p.core().state().pending.as_ref().unwrap().cue, "enter");
    assert!(p
        .core()
        .state()
        .scene
        .iter()
        .any(|node| node.asset.as_deref() == Some("bg.station")));
    assert!(p.asset_descriptor("actor.aki").is_none());
    assert!(
        p.memory_used() > boot_memory,
        "committed music remains admitted while the next catalog loads"
    );
    let ready = p.pump(
        vec![AppEvent::ContentReady {
            request,
            objects: requirements
                .iter()
                .map(|r| objects[&r.hash].clone())
                .collect(),
        }],
        1000,
    );
    drain(&mut p, ready, &objects);
    assert_eq!(p.error, None);
    assert!(p.core().state().pending.is_none());
    assert!(p.retained_assets().contains("actor.aki"));
}

#[test]
fn completed_choice_history_restores_without_loading_the_old_chapter_bodies() {
    use nir_core::{HistoryChoice, HistoryChoiceOption, HistoryChoiceResolution, HistoryEntry};
    let (mut root, mut objects) = bundled();
    let source: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    let module = "old".to_owned();
    let function: Function = serde_json::from_value(serde_json::json!({
        "entry":"end","blocks":{"end":{"terminator":{"type":"end","outcome":"old"}}}
    }))
    .unwrap();
    let signature = FunctionSignature::from(&function);
    let code = object(
        &mut objects,
        &ModuleCode {
            format: 2,
            module: module.clone(),
            functions: BTreeMap::from([("old.entry".into(), function)]),
        },
    );
    let contracts: BTreeMap<_, _> = ["walk", "stay"]
        .into_iter()
        .map(|id| (format!("old.{id}"), source.texts[id].clone()))
        .collect();
    let mut choice = source.choices["route"].clone();
    for option in &mut choice.options {
        option.text = format!("old.{}", option.text);
    }
    let static_content = object(
        &mut objects,
        &ModuleStatic {
            sprite_timelines: Default::default(),
            format: 2,
            module: module.clone(),
            scenes: BTreeMap::new(),
            cues: BTreeMap::new(),
            choices: BTreeMap::from([("old.route".into(), choice)]),
            text_contracts: contracts.clone(),
            activation_recipes: BTreeMap::new(),
        },
    );
    let locales = source
        .locales
        .iter()
        .map(|(locale, texts)| {
            let hash = object(
                &mut objects,
                &ModuleTexts {
                    format: 2,
                    module: module.clone(),
                    locale: locale.clone(),
                    texts: ["walk", "stay"]
                        .into_iter()
                        .map(|id| (format!("old.{id}"), texts[id].clone()))
                        .collect(),
                },
            );
            (locale.clone(), hash)
        })
        .collect();
    root.modules.insert(
        module.clone(),
        ModuleIndex {
            functions: BTreeMap::from([("old.entry".into(), signature.clone())]),
            texts: contracts.keys().cloned().collect(),
            code,
            static_content,
            locales,
        },
    );
    root.function_index.insert(
        "old.entry".into(),
        RuntimeFunctionIndex {
            execution_module: None,
            module: module.clone(),
            signature,
        },
    );
    root.choice_owners
        .insert("old.route".into(), module.clone());
    for (id, c) in &contracts {
        root.text_owners.insert(id.clone(), module.clone());
        root.text_contracts.insert(
            id.clone(),
            RuntimeTextIdentity {
                images: vec![],
                module: module.clone(),
                meaning_revision: c.meaning_revision,
                source_revision: c.source_revision,
                contract_revision: c.contract_revision,
                contract_digest: c.contract_digest.clone(),
            },
        );
    }
    let mut original =
        Player::new_runtime(root.clone(), "release".into(), "Test".into(), None).unwrap();
    let boot = original.pump(vec![], 1000);
    drain(&mut original, boot, &objects);
    let start = action(&mut original, UiAction::NewGame);
    drain(&mut original, start, &objects);
    let mut snapshot = original.core().snapshot();
    let interaction = snapshot.next_id;
    snapshot.next_id += 1;
    let choice = HistoryChoice {
        id: "old.route".into(),
        resolution: HistoryChoiceResolution::Selected {
            option: "stay".into(),
        },
        options: ["walk", "stay"]
            .into_iter()
            .map(|id| {
                let c = &contracts[&format!("old.{id}")];
                HistoryChoiceOption {
                    id: id.into(),
                    text_id: format!("old.{id}"),
                    label: format!("Frozen {id}"),
                    enabled: true,
                    meaning_revision: c.meaning_revision,
                    source_revision: c.source_revision,
                    contract_digest: c.contract_digest.clone(),
                }
            })
            .collect(),
    };
    let c = &contracts["old.stay"];
    snapshot.history.push(HistoryEntry {
        images: vec![],
        interaction,
        text_id: "old.stay".into(),
        meaning_revision: c.meaning_revision,
        source_revision: c.source_revision,
        contract_digest: c.contract_digest.clone(),
        locale: snapshot.locale.clone(),
        font_plan_digest: root.locale_config.text[&snapshot.locale].digest.clone(),
        speaker: String::new(),
        speaker_id: String::new(),
        text: "Frozen stay".into(),
        voices: vec![],
        choice: Some(choice),
    });
    let mut restored = Player::new_runtime(root, "release".into(), "Test".into(), None).unwrap();
    let boot = restored.pump(vec![], 1000);
    let boot_keys = drain(&mut restored, boot, &objects);
    let commands = restored.pump(
        vec![AppEvent::Loaded {
            envelope: Box::new(SaveEnvelope {
                format: 1,
                slot: 0,
                revision: 1,
                digest: nir_content::digest(&serde_json::to_vec(&snapshot).unwrap()),
                snapshot: snapshot.clone(),
            }),
        }],
        1000,
    );
    let keys = drain(&mut restored, commands, &objects);
    assert!(restored.error.is_none(), "{:?}", restored.diagnostic);
    assert!(restored.core().state().fault.is_none());
    assert_eq!(
        restored.core().state().history.len(),
        snapshot.history.len()
    );
    assert_eq!(restored.core().state().variables, snapshot.variables);
    for key in boot_keys.iter().chain(&keys) {
        assert!(
            !matches!(key, ContentKey::Code { module } | ContentKey::Static { module } | ContentKey::Text { module,.. } if module=="old"),
            "history must not retain/fetch old module bodies: {key:?}"
        );
    }
    let commands = action(&mut restored, UiAction::Continue);
    drain(&mut restored, commands, &objects);
    let before = serde_json::to_value(restored.core().snapshot()).unwrap();
    let commands = action(&mut restored, UiAction::History);
    let keys = drain(&mut restored, commands, &objects);
    assert!(keys.is_empty());
    let rows = restored.model().history;
    let h = rows.iter().find(|h| h.choice.is_some()).unwrap();
    assert_eq!(h.text, "Frozen stay");
    assert_eq!(
        h.choice,
        Some(nir_presentation::HistoryChoiceKind::Selected)
    );
    assert_eq!(
        serde_json::to_value(restored.core().snapshot()).unwrap(),
        before
    );
}

#[test]
fn runtime_boot_loads_catalogs_then_execution_loads_independent_module_blocks() {
    let (root, objects) = bundled();
    let mut p = Player::new_runtime(root, "release".into(), "Test".into(), None).unwrap();
    let initial = p.pump(vec![], 1000);
    assert!(!initial
        .iter()
        .any(|c| matches!(c, AppCommand::GetAssets { .. })));
    let boot = drain(&mut p, initial, &objects);
    assert!(!boot.is_empty());
    assert!(boot
        .iter()
        .all(|key| matches!(key, ContentKey::Catalog { .. })));
    assert!(p.error.is_none(), "{:?}", p.diagnostic);
    assert!(!p.is_loading());
    let before = p.core().snapshot();
    let start = action(&mut p, UiAction::NewGame);
    assert_eq!(p.core().state().next_id, before.next_id);
    assert_eq!(p.core().state().tick_us, before.tick_us);
    let requested = drain(&mut p, start, &objects);
    assert!(requested.contains(&ContentKey::Static {
        module: "story".into()
    }));
    assert!(requested.contains(&ContentKey::Code {
        module: "story".into()
    }));
    assert!(requested.contains(&ContentKey::Text {
        module: "story".into(),
        locale: "zh-Hans".into()
    }));
    assert!(!requested.contains(&ContentKey::Text {
        module: "story".into(),
        locale: "en".into()
    }));
    assert!(p.error.is_none(), "{:?}", p.diagnostic);
    assert!(p.core().dialogue().is_some());
    let playing = p.content_residency();
    assert!(playing.pinned_blocks >= 3);
    let title = action(&mut p, UiAction::Title);
    drain(&mut p, title, &objects);
    let title = p.content_residency();
    assert_eq!(title.resident_blocks, playing.resident_blocks);
    assert!(title.pinned_blocks < playing.pinned_blocks);
    assert!(
        title.unpinned_bytes > 0,
        "M2.1 retains bytes but releases inactive content pins"
    );
}

#[test]
fn corrupt_catalog_preserves_live_state_and_retry_uses_the_same_content_identity() {
    let (root, objects) = bundled();
    let mut p = Player::new_runtime(root, "release".into(), "Test".into(), None).unwrap();
    let before = serde_json::to_value(p.core().snapshot()).unwrap();
    let commands = p.pump(vec![], 1000);
    let (request, requirements) = commands
        .into_iter()
        .find_map(|c| match c {
            AppCommand::GetContent {
                request, objects, ..
            } => Some((request, objects)),
            _ => None,
        })
        .unwrap();
    p.pump(
        vec![AppEvent::ContentReady {
            request,
            objects: requirements.iter().map(|_| b"{}".to_vec()).collect(),
        }],
        1000,
    );
    assert!(p.error.is_some());
    assert_eq!(serde_json::to_value(p.core().snapshot()).unwrap(), before);
    let retry = action(&mut p, UiAction::Retry);
    let keys = drain(&mut p, retry, &objects);
    assert_eq!(
        keys,
        requirements.iter().filter_map(|r| r.key.clone()).collect()
    );
    assert!(p.error.is_none(), "{:?}", p.diagnostic);
    assert!(!p.is_loading());
}

#[test]
fn cold_restore_prepares_canonical_definitions_and_original_dialogue_locale() {
    let (root, objects) = bundled();
    let mut original =
        Player::new_runtime(root.clone(), "release".into(), "Test".into(), None).unwrap();
    let boot = original.pump(vec![], 1000);
    drain(&mut original, boot, &objects);
    let start = action(&mut original, UiAction::NewGame);
    drain(&mut original, start, &objects);
    let snapshot = original.core().snapshot();
    assert!(original.core().dialogue().is_some());
    let preferences = root.player.preferences("en".into(), "en".into());
    let mut restored =
        Player::new_runtime(root, "release".into(), "Test".into(), Some(preferences)).unwrap();
    let boot = restored.pump(vec![], 1000);
    drain(&mut restored, boot, &objects);
    let envelope = SaveEnvelope {
        format: 1,
        slot: 0,
        revision: 1,
        digest: nir_content::digest(&serde_json::to_vec(&snapshot).unwrap()),
        snapshot,
    };
    let load = restored.pump(
        vec![AppEvent::Loaded {
            envelope: Box::new(envelope),
        }],
        1000,
    );
    let keys = drain(&mut restored, load, &objects);
    assert!(restored.error.is_none(), "{:?}", restored.diagnostic);
    assert!(keys.contains(&ContentKey::Static {
        module: "story".into()
    }));
    assert!(keys.contains(&ContentKey::Code {
        module: "story".into()
    }));
    assert!(keys.contains(&ContentKey::Text {
        module: "story".into(),
        locale: "zh-Hans".into()
    }));
    assert_eq!(restored.core().dialogue().unwrap().1.locale, "zh-Hans");
    assert_eq!(restored.effective_text_locale, "en");
    let resident = restored.content_residency();
    assert!(resident.blocks.iter().any(|block| block.key
        == ContentKey::Text {
            module: "story".into(),
            locale: "en".into()
        }));
    assert!(
        !resident.blocks.iter().any(|block| block.key
            == ContentKey::Text {
                module: "story".into(),
                locale: "zh-Hans".into()
            }),
        "original dialogue text is only needed in the verification scratch view"
    );
    assert_eq!(
        restored.core().state().variables,
        original.core().state().variables
    );
    assert_eq!(
        restored.core().state().tick_us,
        original.core().state().tick_us
    );
    assert_eq!(
        restored.core().state().frames.last().unwrap().op_id,
        original.core().state().frames.last().unwrap().op_id
    );
}

#[test]
fn cold_restore_prepares_bound_voice_timer_metadata_even_after_the_voice_ended() {
    for finished in [false, true] {
        let (mut root, mut objects) = bundled();
        root.requires
            .extend(["text.voice-binding.v1".into(), "text.voice-timer.v1".into()]);
        let asset = "audio.voice".to_owned();
        let module = root.modules.get_mut("story").unwrap();
        let mut code: ModuleCode = serde_json::from_slice(&objects[&module.code]).unwrap();
        code.functions
            .get_mut("main")
            .unwrap()
            .blocks
            .get_mut("wait_intro")
            .unwrap()
            .ops
            .push(Op {
                id: "sampled-voice-binding".into(),
                operation: Operation::DialogueVoice {
                    task: "line".into(),
                    voice: Some("spoken".into()),
                    wait: VoiceWaitPolicy::SampledRemaining,
                },
            });
        module.code = object(&mut objects, &code);
        let mut data: ModuleStatic =
            serde_json::from_slice(&objects[&module.static_content]).unwrap();
        data.cues.get_mut("intro").unwrap().effects.push(EffectDef {
            id: "spoken".into(),
            scope: Scope::Session,
            effect: Effect::Audio {
                asset: asset.clone(),
                bus: AudioBus::Voice,
                looped: false,
                loop_region: None,
                gain: 1.,
            },
        });
        data.activation_recipes
            .get_mut("intro")
            .unwrap()
            .insert(asset.clone());
        module.static_content = object(&mut objects, &data);
        root.task_owners.insert("spoken".into(), "story".into());
        let mut original =
            Player::new_runtime(root.clone(), "release".into(), "Test".into(), None).unwrap();
        let commands = original.pump(vec![], 1000);
        drain(&mut original, commands, &objects);
        let commands = action(&mut original, UiAction::NewGame);
        drain(&mut original, commands, &objects);
        let voice = original.core().state().handles["spoken"];
        if finished {
            original.pump(
                vec![AppEvent::AudioEnded {
                    domain: TimeDomain::Story,
                    task: voice,
                    session: original.generation.session,
                }],
                1000,
            );
        }
        let snapshot = original.core().snapshot();
        assert_eq!(
            original
                .core()
                .dialogue()
                .unwrap()
                .1
                .reading
                .as_ref()
                .unwrap()
                .voice,
            Some(voice)
        );
        let envelope = SaveEnvelope {
            format: 1,
            slot: 0,
            revision: 1,
            digest: nir_content::digest(&serde_json::to_vec(&snapshot).unwrap()),
            snapshot,
        };
        let catalog = ContentKey::Catalog {
            catalog: root.assets[&asset].catalog.clone(),
        };
        let mut restored =
            Player::new_runtime(root, "release".into(), "Test".into(), None).unwrap();
        let commands = restored.pump(vec![], 1000);
        let boot_keys = drain(&mut restored, commands, &objects);
        assert!(
            !boot_keys.contains(&catalog),
            "voice metadata is cold at title"
        );
        let commands = restored.pump(
            vec![AppEvent::Loaded {
                envelope: Box::new(envelope),
            }],
            1000,
        );
        let keys = drain(&mut restored, commands, &objects);
        assert!(restored.error.is_none(), "{:?}", restored.diagnostic);
        assert!(
            keys.contains(&catalog),
            "duration must be prepared before core restore"
        );
        assert!(!restored.is_loading());
        assert_eq!(
            restored.retained_assets().contains(&asset),
            !finished,
            "an ended binding needs catalog metadata, not decoded media"
        );
        assert_eq!(
            restored.core().state().tick_us,
            original.core().state().tick_us
        );
        assert_eq!(
            restored.core().state().variables,
            original.core().state().variables
        );
    }
}

#[test]
fn history_voice_loads_only_its_catalog_and_keeps_cancelled_decode_budget_until_ack() {
    let (mut root, mut objects) = bundled();
    let source: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    let (source_id, contract) = source.texts.iter().next().unwrap();
    let module = "old".to_owned();
    let text_id = "old.remembered".to_owned();
    let function_id = "old.entry".to_owned();
    let function: Function = serde_json::from_value(serde_json::json!({
        "entry":"end", "blocks":{"end":{"ops":[],"terminator":{"type":"end","outcome":"old"}}}
    }))
    .unwrap();
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
            sprite_timelines: Default::default(),
            format: 2,
            module: module.clone(),
            scenes: BTreeMap::new(),
            cues: BTreeMap::new(),
            choices: BTreeMap::new(),
            text_contracts: BTreeMap::from([(text_id.clone(), contract.clone())]),
            activation_recipes: BTreeMap::new(),
        },
    );
    let locales = source
        .locales
        .iter()
        .map(|(locale, texts)| {
            let hash = object(
                &mut objects,
                &ModuleTexts {
                    format: 2,
                    module: module.clone(),
                    locale: locale.clone(),
                    texts: BTreeMap::from([(text_id.clone(), texts[source_id].clone())]),
                },
            );
            (locale.clone(), hash)
        })
        .collect();
    root.modules.insert(
        module.clone(),
        ModuleIndex {
            functions: BTreeMap::from([(function_id.clone(), signature.clone())]),
            texts: BTreeSet::from([text_id.clone()]),
            code,
            static_content,
            locales,
        },
    );
    root.function_index.insert(
        function_id,
        RuntimeFunctionIndex {
            execution_module: None,
            module: module.clone(),
            signature,
        },
    );
    root.text_owners.insert(text_id.clone(), module.clone());
    root.text_contracts.insert(
        text_id.clone(),
        RuntimeTextIdentity {
            images: vec![],
            module,
            source_revision: contract.source_revision,
            contract_revision: contract.contract_revision,
            meaning_revision: contract.meaning_revision,
            contract_digest: contract.contract_digest.clone(),
        },
    );
    root.requires.push("text.voice-binding.v1".into());
    let voice_asset = "old.voice".to_owned();
    let mut audio = source
        .assets
        .values()
        .find(|a| a.kind == AssetKind::Audio)
        .unwrap()
        .clone();
    audio.object = nir_content::digest(voice_asset.as_bytes());
    audio.decoded_bytes = 1_048_576;
    audio.duration_us = Micros(1_000_000);
    let voice_catalog = object(
        &mut objects,
        &AssetCatalog {
            format: 2,
            catalog: voice_asset.clone(),
            assets: BTreeMap::from([(voice_asset.clone(), audio.clone())]),
        },
    );
    root.catalogs.insert(voice_asset.clone(), voice_catalog);
    root.assets.insert(
        voice_asset.clone(),
        AssetIndexEntry {
            kind: AssetKind::Audio,
            object: audio.object.clone(),
            catalog: voice_asset.clone(),
        },
    );
    let mut original =
        Player::new_runtime(root.clone(), "release".into(), "Test".into(), None).unwrap();
    let boot = original.pump(vec![], 1000);
    drain(&mut original, boot, &objects);
    let start = action(&mut original, UiAction::NewGame);
    drain(&mut original, start, &objects);
    let mut snapshot = original.core().snapshot();
    snapshot.history.push(nir_core::HistoryEntry {
        images: vec![],
        choice: None,
        interaction: original.core().dialogue().unwrap().1.interaction,
        voices: vec![nir_core::HistoryVoice {
            instance: *snapshot.handles.values().next().unwrap(),
            asset: voice_asset.clone(),
            gain: 0.75,
        }],
        text_id,
        source_revision: contract.source_revision,
        meaning_revision: contract.meaning_revision,
        contract_digest: contract.contract_digest.clone(),
        locale: "zh-Hans".into(),
        font_plan_digest: root.locale_config.text["zh-Hans"].digest.clone(),
        speaker: String::new(),
        speaker_id: String::new(),
        text: "Frozen old history".into(),
    });
    let mut restored = Player::new_runtime(root, "release".into(), "Test".into(), None).unwrap();
    let boot = restored.pump(vec![], 1000);
    drain(&mut restored, boot, &objects);
    let envelope = SaveEnvelope {
        format: 1,
        slot: 0,
        revision: 1,
        digest: nir_content::digest(&serde_json::to_vec(&snapshot).unwrap()),
        snapshot,
    };
    let load = restored.pump(
        vec![AppEvent::Loaded {
            envelope: Box::new(envelope),
        }],
        1000,
    );
    let keys = drain(&mut restored, load, &objects);
    assert!(restored.error.is_none(), "{:?}", restored.diagnostic);
    assert!(keys.iter().all(|key| match key {
        ContentKey::Static { module }
        | ContentKey::Code { module }
        | ContentKey::Text { module, .. } => module != "old",
        ContentKey::Catalog { .. } => true,
    }));
    assert_eq!(
        restored.core().state().history.last().unwrap().text,
        "Frozen old history"
    );
    assert!(
        !keys.contains(&ContentKey::Catalog {
            catalog: voice_asset.clone()
        }),
        "loading a save must not download all history voices"
    );
    action(&mut restored, UiAction::Continue);
    action(&mut restored, UiAction::History);
    let state = serde_json::to_value(restored.core().snapshot()).unwrap();
    let entry = restored.core().state().history.len() - 1;
    let replay = action(&mut restored, UiAction::HistoryVoice { entry });
    let (request, requirements) = replay
        .iter()
        .find_map(|command| match command {
            AppCommand::GetContent {
                request, objects, ..
            } => Some((*request, objects.clone())),
            _ => None,
        })
        .unwrap();
    assert_eq!(requirements.len(), 1);
    assert_eq!(
        requirements[0].key,
        Some(ContentKey::Catalog {
            catalog: voice_asset.clone()
        })
    );
    let failed = restored.pump(
        vec![AppEvent::ContentFailed {
            request,
            message: "offline".into(),
        }],
        1000,
    );
    assert!(!failed
        .iter()
        .any(|c| matches!(c, AppCommand::GetAssets { .. })));
    assert!(restored.history_voice_model().unwrap().failed);
    assert!(restored.error.is_none());
    assert!(!restored.is_loading());
    let replay = action(&mut restored, UiAction::HistoryVoice { entry });
    let request = replay
        .iter()
        .find_map(|c| match c {
            AppCommand::GetContent { request, .. } => Some(*request),
            _ => None,
        })
        .unwrap();
    let prepared = restored.pump(
        vec![AppEvent::ContentReady {
            request,
            objects: requirements
                .iter()
                .map(|r| objects[&r.hash].clone())
                .collect(),
        }],
        1000,
    );
    let request = prepared
        .iter()
        .find_map(|c| match c {
            AppCommand::GetAssets {
                request,
                assets,
                descriptors,
                ..
            } => {
                assert_eq!(assets, &vec![voice_asset.clone()]);
                assert_eq!(descriptors[&voice_asset].decoded_bytes, audio.decoded_bytes);
                Some(*request)
            }
            _ => None,
        })
        .unwrap();
    let reserved = restored.memory_used();
    let cancelled = action(&mut restored, UiAction::StopHistoryVoice);
    assert!(cancelled
        .iter()
        .any(|c| matches!(c, AppCommand::CancelAssets { request: id } if *id == request)));
    assert_eq!(
        restored.memory_used(),
        reserved,
        "physical cancellation has not completed"
    );
    assert!(!restored.accepts_resource(request));
    let stale = restored.pump(
        vec![AppEvent::AssetReady {
            request,
            asset: voice_asset.clone(),
        }],
        1000,
    );
    assert!(!stale.iter().any(|c| matches!(
        c,
        AppCommand::AudioStart {
            domain: TimeDomain::ForegroundUi,
            ..
        }
    )));
    restored.pump(vec![AppEvent::AssetsCancelled { request }], 1000);
    assert_eq!(
        restored.memory_used() + audio.decoded_bytes + audio.bytes,
        reserved
    );
    action(&mut restored, UiAction::Close);
    assert_eq!(
        serde_json::to_value(restored.core().snapshot()).unwrap(),
        state
    );
}

#[test]
fn hundred_module_story_loads_incrementally_and_budget_rejection_preserves_old_view() {
    let (mut root, mut objects) = bundled();
    let mut keys = vec![];
    let mut code_bytes = 0;
    for index in 0..100 {
        let module = format!("bulk{index:03}");
        let function_id = format!("{module}.entry");
        let function: Function = serde_json::from_value(serde_json::json!({
            "entry":"end", "blocks":{"end":{"ops":[],"terminator":{"type":"end","outcome":"x".repeat(200_000)}}}
        })).unwrap();
        let signature = FunctionSignature::from(&function);
        let code = object(
            &mut objects,
            &ModuleCode {
                format: 2,
                module: module.clone(),
                functions: BTreeMap::from([(function_id.clone(), function)]),
            },
        );
        code_bytes += objects[&code].len();
        let static_content = object(
            &mut objects,
            &ModuleStatic {
                sprite_timelines: Default::default(),
                format: 2,
                module: module.clone(),
                scenes: BTreeMap::new(),
                cues: BTreeMap::new(),
                choices: BTreeMap::new(),
                text_contracts: BTreeMap::new(),
                activation_recipes: BTreeMap::new(),
            },
        );
        root.modules.insert(
            module.clone(),
            ModuleIndex {
                functions: BTreeMap::from([(function_id.clone(), signature.clone())]),
                texts: BTreeSet::new(),
                code,
                static_content,
                locales: BTreeMap::new(),
            },
        );
        root.function_index.insert(
            function_id,
            RuntimeFunctionIndex {
                execution_module: None,
                module: module.clone(),
                signature,
            },
        );
        keys.push(vec![
            ContentKey::Static {
                module: module.clone(),
            },
            ContentKey::Code { module },
        ]);
    }
    assert!(code_bytes > MAX_INPUT_BYTES);
    assert!(serde_json::to_vec(&root).unwrap().len() < MAX_INPUT_BYTES);
    let mut view = nir_core::ValidatedProgram::from_runtime(root.clone()).unwrap();
    assert_eq!(view.residency().resident_blocks, 0);
    let mut baseline = None;
    let mut rejected = false;
    for requirements in keys {
        let batch = requirements
            .iter()
            .map(|key| {
                let hash = root.content_requirement(key).unwrap().digest;
                let bytes = &objects[&hash];
                (
                    key.clone(),
                    nir_content::parse_runtime_object(&root, key, bytes).unwrap(),
                    bytes.len() as u64,
                )
            })
            .collect();
        let before = view.residency();
        match view.install_batch(batch) {
            Ok(next) => {
                if let Some(first) = &baseline {
                    let first: &nir_core::ValidatedProgram = first;
                    assert!(std::ptr::eq(
                        first.program().functions.get("bulk000.entry").unwrap(),
                        next.program().functions.get("bulk000.entry").unwrap()
                    ));
                } else {
                    baseline = Some(next.clone());
                }
                view = next;
                assert!(view.residency().resident_bytes <= MAX_INPUT_BYTES as u64);
            }
            Err(error) => {
                assert_eq!(error.code, "E_RESIDENCY_BUDGET");
                assert_eq!(view.residency(), before);
                assert!(requirements.iter().all(|key| !view.is_resident(key)));
                rejected = true;
                break;
            }
        }
    }
    assert!(
        rejected,
        "M2.1 must enforce a finite resident budget without eviction"
    );
}

#[test]
fn runtime_boot_does_not_request_an_unselected_locales_font_catalog() {
    let (mut root, mut objects) = bundled();
    let source: Program =
        serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
    let mut font = source.assets["font.reader"].clone();
    font.object = "b".repeat(64);
    font.decoded_bytes = 1;
    let catalog = "english-font".to_owned();
    let hash = object(
        &mut objects,
        &AssetCatalog {
            format: 2,
            catalog: catalog.clone(),
            assets: BTreeMap::from([("font.english".into(), font.clone())]),
        },
    );
    root.catalogs.insert(catalog.clone(), hash);
    root.assets.insert(
        "font.english".into(),
        AssetIndexEntry {
            kind: AssetKind::Font,
            object: font.object,
            catalog: catalog.clone(),
        },
    );
    let asset_objects = root
        .assets
        .iter()
        .map(|(id, asset)| (id.clone(), asset.object.clone()))
        .collect();
    for plans in [&mut root.locale_config.ui, &mut root.locale_config.text] {
        let plan = plans.get_mut("en").unwrap();
        plan.fonts = vec!["font.english".into()];
        plan.digest = LocaleFontPlan::digest_for(&plan.fonts, &asset_objects);
    }
    let default_catalog = root.assets["font.reader"].catalog.clone();
    let preferences = root.player.preferences("en".into(), "en".into());
    let mut p =
        Player::new_runtime(root, "release".into(), "Test".into(), Some(preferences)).unwrap();
    let initial = p.pump(vec![], 1000);
    let keys = drain(&mut p, initial, &objects);
    assert!(p.error.is_none(), "{:?}", p.diagnostic);
    assert!(keys.contains(&ContentKey::Catalog { catalog }));
    assert!(!keys.contains(&ContentKey::Catalog {
        catalog: default_catalog
    }));
    assert!(p.retained_descriptors().contains_key("font.english"));
    assert!(!p.retained_descriptors().contains_key("font.reader"));
}

#[test]
fn authenticated_but_invalid_static_recipe_rejects_the_entire_module_batch() {
    let (mut root, mut objects) = bundled();
    let old_hash = root.modules["story"].static_content.clone();
    let mut declarations: ModuleStatic = serde_json::from_slice(&objects[&old_hash]).unwrap();
    declarations
        .activation_recipes
        .values_mut()
        .find(|assets| !assets.is_empty())
        .unwrap()
        .clear();
    let hash = object(&mut objects, &declarations);
    root.modules.get_mut("story").unwrap().static_content = hash;
    let entry = root.entry.clone();
    let mut p = Player::new_runtime(root, "release".into(), "Test".into(), None).unwrap();
    let boot = p.pump(vec![], 1000);
    drain(&mut p, boot, &objects);
    let before = p.content_residency().resident_bytes;
    let start = action(&mut p, UiAction::NewGame);
    let state = serde_json::to_value(p.core().snapshot()).unwrap();
    drain(&mut p, start, &objects);
    assert!(
        p.error.is_some(),
        "a matching hash must not bypass semantic recipe validation"
    );
    assert_eq!(p.content_residency().resident_bytes, before);
    assert!(p.core().program().functions.get(&entry).is_none());
    assert_eq!(serde_json::to_value(p.core().snapshot()).unwrap(), state);
}

#[test]
fn staged_restore_failure_retries_and_cancelled_completion_cannot_install() {
    let (root, objects) = bundled();
    let mut original =
        Player::new_runtime(root.clone(), "release".into(), "Test".into(), None).unwrap();
    let boot = original.pump(vec![], 1000);
    drain(&mut original, boot, &objects);
    let start = action(&mut original, UiAction::NewGame);
    drain(&mut original, start, &objects);
    let snapshot = original.core().snapshot();
    let envelope = SaveEnvelope {
        format: 1,
        slot: 0,
        revision: 1,
        digest: nir_content::digest(&serde_json::to_vec(&snapshot).unwrap()),
        snapshot,
    };
    let mut restored = Player::new_runtime(root, "release".into(), "Test".into(), None).unwrap();
    let boot = restored.pump(vec![], 1000);
    drain(&mut restored, boot, &objects);
    let before = serde_json::to_value(restored.core().snapshot()).unwrap();
    let resident_before = restored.content_residency().resident_bytes;
    let load = restored.pump(
        vec![AppEvent::Loaded {
            envelope: Box::new(envelope.clone()),
        }],
        1000,
    );
    let (request, requirements) = load
        .into_iter()
        .find_map(|command| match command {
            AppCommand::GetContent {
                request, objects, ..
            } => Some((request, objects)),
            _ => None,
        })
        .unwrap();
    assert!(requirements
        .iter()
        .all(|r| !matches!(r.key, Some(ContentKey::Code { .. }))));
    restored.pump(
        vec![AppEvent::ContentReady {
            request,
            objects: requirements.iter().map(|_| b"{}".to_vec()).collect(),
        }],
        1000,
    );
    assert!(restored.error.is_some());
    assert_eq!(
        serde_json::to_value(restored.core().snapshot()).unwrap(),
        before
    );
    assert_eq!(restored.content_residency().resident_bytes, resident_before);
    let retry = action(&mut restored, UiAction::Retry);
    drain(&mut restored, retry, &objects);
    assert!(restored.error.is_none(), "{:?}", restored.diagnostic);
    assert!(restored.core().dialogue().is_some());

    let load = restored.pump(
        vec![AppEvent::Loaded {
            envelope: Box::new(envelope),
        }],
        1000,
    );
    let (request, requirements) = load
        .into_iter()
        .find_map(|command| match command {
            AppCommand::GetContent {
                request, objects, ..
            } => Some((request, objects)),
            _ => None,
        })
        .unwrap();
    let title = action(&mut restored, UiAction::Title);
    drain(&mut restored, title, &objects);
    let before = serde_json::to_value(restored.core().snapshot()).unwrap();
    let resident_before = restored.content_residency().resident_bytes;
    assert!(!restored.accepts_content(request));
    let late = restored.pump(
        vec![AppEvent::ContentReady {
            request,
            objects: requirements
                .iter()
                .map(|r| objects[&r.hash].clone())
                .collect(),
        }],
        1000,
    );
    drain(&mut restored, late, &objects);
    assert_eq!(restored.screen, nir_presentation::Screen::Title);
    assert_eq!(
        serde_json::to_value(restored.core().snapshot()).unwrap(),
        before
    );
    assert_eq!(restored.content_residency().resident_bytes, resident_before);
    assert!(restored.error.is_none(), "{:?}", restored.diagnostic);
}

#[test]
fn player_evicts_and_reloads_returned_modules_without_replaying_calls() {
    let (mut root, mut objects) = bundled();
    root.player.prefetch_content = false;
    let mut blocks = BTreeMap::new();
    for index in 0..11 {
        let module_index = index % 10;
        blocks.insert(
            format!("call{index}"),
            Block {
                ops: vec![],
                terminator: Terminator::Call {
                    function: format!("chapter{module_index}.entry"),
                    args: BTreeMap::new(),
                    next: if index == 10 {
                        "end".into()
                    } else {
                        format!("call{}", index + 1)
                    },
                    result: None,
                },
            },
        );
    }
    blocks.insert(
        "end".into(),
        Block {
            ops: vec![],
            terminator: Terminator::End {
                outcome: "reloaded".into(),
            },
        },
    );
    let driver = Function {
        params: BTreeMap::new(),
        locals: BTreeMap::new(),
        returns: None,
        entry: "call0".into(),
        blocks,
    };
    let owner = root.function_index[&root.entry].module.clone();
    let module = root.modules.get_mut(&owner).unwrap();
    let mut code: ModuleCode = serde_json::from_slice(&objects[&module.code]).unwrap();
    code.functions.insert(root.entry.clone(), driver.clone());
    module.code = object(&mut objects, &code);
    module
        .functions
        .insert(root.entry.clone(), FunctionSignature::from(&driver));
    root.function_index.get_mut(&root.entry).unwrap().signature = FunctionSignature::from(&driver);
    let mut chapter_hashes = Vec::new();
    for index in 0..10 {
        let name = format!("chapter{index}");
        let function_id = format!("{name}.entry");
        let function = Function {
            params: BTreeMap::new(),
            locals: BTreeMap::new(),
            returns: None,
            entry: "return".into(),
            blocks: BTreeMap::from([(
                "return".into(),
                Block {
                    ops: vec![],
                    terminator: Terminator::Return { value: None },
                },
            )]),
        };
        let signature = FunctionSignature::from(&function);
        let package = ModuleCode {
            format: 2,
            module: name.clone(),
            functions: BTreeMap::from([(function_id.clone(), function)]),
        };
        // Valid encoded whitespace makes the cache budget observable without
        // constructing an artificial multi-megabyte runtime value.
        let mut bytes = serde_json::to_vec(&package).unwrap();
        bytes.resize(2 * 1024 * 1024, b' ');
        let hash = nir_content::digest(&bytes);
        objects.insert(hash.clone(), bytes);
        chapter_hashes.push(hash.clone());
        let static_content = object(
            &mut objects,
            &ModuleStatic {
                sprite_timelines: Default::default(),
                format: 2,
                module: name.clone(),
                scenes: BTreeMap::new(),
                cues: BTreeMap::new(),
                choices: BTreeMap::new(),
                text_contracts: BTreeMap::new(),
                activation_recipes: BTreeMap::new(),
            },
        );
        root.modules.insert(
            name.clone(),
            ModuleIndex {
                functions: BTreeMap::from([(function_id.clone(), signature.clone())]),
                texts: BTreeSet::new(),
                code: hash,
                static_content,
                locales: BTreeMap::new(),
            },
        );
        root.function_index.insert(
            function_id,
            RuntimeFunctionIndex {
                execution_module: None,
                module: name,
                signature,
            },
        );
    }
    let mut player = Player::new_runtime(root, "release".into(), "Test".into(), None).unwrap();
    let boot = player.pump(vec![], 1000);
    drain(&mut player, boot, &objects);
    let mut commands = action(&mut player, UiAction::NewGame);
    let mut requested = Vec::new();
    for _ in 0..100 {
        if commands.is_empty() {
            break;
        }
        let mut next = Vec::new();
        for command in commands {
            match command {
                AppCommand::GetContent {
                    request,
                    objects: requirements,
                    priority,
                    ..
                } => {
                    assert_eq!(priority, ContentPriority::Required);
                    requested.extend(requirements.iter().map(|r| r.hash.clone()));
                    next.extend(player.pump(
                        vec![AppEvent::ContentReady {
                                request,
                                objects: requirements
                                    .iter()
                                    .map(|r| objects[&r.hash].clone())
                                    .collect(),
                            }],
                        1000,
                    ));
                }
                other => {
                    drain(&mut player, vec![other], &objects);
                }
            }
        }
        commands = next;
        assert!(player.error.is_none(), "{:?}", player.diagnostic);
        let residency = player.content_residency();
        assert!(residency.resident_bytes <= residency.budget_bytes);
    }
    assert_eq!(player.core().state().outcome.as_deref(), Some("reloaded"));
    assert_eq!(
        requested
            .iter()
            .filter(|hash| **hash == chapter_hashes[0])
            .count(),
        2
    );
    for hash in &chapter_hashes[1..] {
        assert_eq!(
            requested
                .iter()
                .filter(|requested| *requested == hash)
                .count(),
            1
        );
    }
    assert!(player.content_residency().resident_bytes < 20 * 1024 * 1024);
}

#[test]
fn new_game_waits_for_in_flight_text_locale_before_freezing_first_dialogue() {
    let (root, objects) = bundled();
    let mut player = Player::new_runtime(root, "release".into(), "Test".into(), None).unwrap();
    let boot = player.pump(vec![], 1000);
    drain(&mut player, boot, &objects);
    let start = action(&mut player, UiAction::NewGame);
    drain(&mut player, start, &objects);
    assert_eq!(player.core().dialogue().unwrap().1.locale, "zh-Hans");
    let change = action(
        &mut player,
        UiAction::TextLocale {
            locale: "en".into(),
        },
    );
    let (old_request, old_objects) = change
        .into_iter()
        .find_map(|command| match command {
            AppCommand::GetContent {
                request, objects, ..
            } => Some((request, objects)),
            _ => None,
        })
        .unwrap();
    assert!(player.locale_pending());
    let mut start = action(&mut player, UiAction::NewGame);
    assert!(player.core().dialogue().is_none());
    assert!(player.core().state().pending.is_none());
    assert!(!player.accepts_content(old_request));
    start.extend(player.pump(
        vec![AppEvent::ContentReady {
        request: old_request,
        objects: old_objects.iter().map(|r| objects[&r.hash].clone()).collect(),
    }],
        1000,
    ));
    drain(&mut player, start, &objects);
    assert!(player.error.is_none(), "{:?}", player.diagnostic);
    assert_eq!(player.effective_text_locale, "en");
    assert_eq!(player.core().dialogue().unwrap().1.locale, "en");
}

#[test]
fn failed_slot_restore_content_can_return_to_the_unchanged_story() {
    let (root, objects) = bundled();
    let mut p = Player::new_runtime(root, "release".into(), "Test".into(), None).unwrap();
    let commands = p.pump(vec![], 1000);
    drain(&mut p, commands, &objects);
    let commands = action(&mut p, UiAction::NewGame);
    drain(&mut p, commands, &objects);
    let snapshot = p.core().snapshot();
    let before = serde_json::to_value(&snapshot).unwrap();
    let envelope = SaveEnvelope {
        format: 1,
        slot: 0,
        revision: 1,
        digest: nir_content::digest(&serde_json::to_vec(&snapshot).unwrap()),
        snapshot,
    };
    action(&mut p, UiAction::Saves);
    let job = action(&mut p, UiAction::Load { slot: 0 })
        .iter()
        .find_map(|c| {
            if let AppCommand::Load { job, .. } = c {
                Some(*job)
            } else {
                None
            }
        })
        .unwrap();
    let commands = p.pump(
        vec![AppEvent::SlotLoaded {
            job,
            envelope: Box::new(envelope),
        }],
        1000,
    );
    let request = commands
        .iter()
        .find_map(|c| {
            if let AppCommand::GetContent { request, .. } = c {
                Some(*request)
            } else {
                None
            }
        })
        .unwrap();
    p.pump(
        vec![AppEvent::ContentFailed {
            request,
            message: "candidate content unavailable".into(),
        }],
        1000,
    );
    assert!(p.error.is_some());
    assert_eq!(
        p.diagnostic
            .as_ref()
            .unwrap()
            .details
            .as_ref()
            .unwrap()
            .request,
        Some(request)
    );
    let commands = action(&mut p, UiAction::Close);
    drain(&mut p, commands, &objects);
    assert!(p.error.is_none());
    assert!(p.diagnostic.is_none());
    assert!(!p.paused());
    assert_eq!(serde_json::to_value(p.core().snapshot()).unwrap(), before);
    assert!(!p.accepts_content(request));
}

#[test]
fn failed_content_retry_coalesces_until_its_next_terminal_and_preserves_live_state() {
    let (root, objects) = bundled();
    let mut p = Player::new_runtime(root, "release".into(), "Test".into(), None).unwrap();
    let before = serde_json::to_value(p.core().snapshot()).unwrap();
    let commands = p.pump(vec![], 1000);
    let request = commands
        .into_iter()
        .find_map(|c| match c {
            AppCommand::GetContent { request, .. } => Some(request),
            _ => None,
        })
        .unwrap();
    p.pump(
        vec![AppEvent::ContentFailed {
            request,
            message: "offline".into(),
        }],
        1000,
    );
    assert!(!p.retrying());
    let commands = action(&mut p, UiAction::Retry);
    let next = commands
        .iter()
        .find_map(|c| match c {
            AppCommand::GetContent { request, .. } => Some(*request),
            _ => None,
        })
        .unwrap();
    assert!(p.status.is_empty());
    assert!(p.is_loading());
    assert!(
        p.error.is_none(),
        "Content retry replaces the old fault with loading"
    );
    for _ in 0..20 {
        let commands = action(&mut p, UiAction::Retry);
        assert!(!commands.iter().any(|c| matches!(
            c,
            AppCommand::GetContent { .. } | AppCommand::CancelContent { .. }
        )));
    }
    assert!(p.accepts_content(next));
    assert_eq!(serde_json::to_value(p.core().snapshot()).unwrap(), before);
    p.pump(
        vec![AppEvent::ContentFailed {
            request: next,
            message: "still offline".into(),
        }],
        1000,
    );
    assert!(!p.retrying());
    let commands = action(&mut p, UiAction::Retry);
    drain(&mut p, commands, &objects);
    assert!(p.error.is_none());
    assert!(!p.retrying());
    assert!(!p.is_loading());
    assert_eq!(serde_json::to_value(p.core().snapshot()).unwrap(), before);
}

#[test]
fn code_package_start_and_cold_restore_load_shared_scope_without_parent_code() {
    let (mut root, mut objects) = bundled();
    let original: ModuleCode =
        serde_json::from_slice(&objects[&root.modules["story"].code]).unwrap();
    let mut function = original.functions["main"].clone();
    for block in function.blocks.values_mut() {
        for op in &mut block.ops {
            op.id = format!("packed_{}", op.id);
        }
    }
    let signature = FunctionSignature::from(&function);
    let code = object(
        &mut objects,
        &ModuleCode {
            format: 2,
            module: "pack".into(),
            functions: BTreeMap::from([("pack_entry".into(), function)]),
        },
    );
    let static_content = object(
        &mut objects,
        &ModuleStatic {
            sprite_timelines: Default::default(),
            format: 2,
            module: "pack".into(),
            scenes: BTreeMap::new(),
            cues: BTreeMap::new(),
            choices: BTreeMap::new(),
            text_contracts: BTreeMap::new(),
            activation_recipes: BTreeMap::new(),
        },
    );
    root.modules.insert(
        "pack".into(),
        ModuleIndex {
            functions: BTreeMap::from([("pack_entry".into(), signature.clone())]),
            texts: BTreeSet::new(),
            code,
            static_content,
            locales: BTreeMap::new(),
        },
    );
    root.function_index.insert(
        "pack_entry".into(),
        RuntimeFunctionIndex {
            module: "pack".into(),
            execution_module: Some("story".into()),
            signature,
        },
    );
    root.entry = "pack_entry".into();
    root.requires.push("story.code-packages.v1".into());
    let mut p = Player::new_runtime(root.clone(), "release".into(), "Test".into(), None).unwrap();
    let boot = p.pump(vec![], 1000);
    drain(&mut p, boot, &objects);
    let start = action(&mut p, UiAction::NewGame);
    let keys = drain(&mut p, start, &objects);
    assert!(p.error.is_none(), "{:?}", p.diagnostic);
    assert!(p.core().dialogue().is_some());
    for key in [
        ContentKey::Static {
            module: "story".into(),
        },
        ContentKey::Static {
            module: "pack".into(),
        },
        ContentKey::Code {
            module: "pack".into(),
        },
        ContentKey::Text {
            module: "story".into(),
            locale: "zh-Hans".into(),
        },
    ] {
        assert!(keys.contains(&key), "missing {key:?}");
    }
    assert!(!keys.contains(&ContentKey::Code {
        module: "story".into()
    }));
    assert!(p.content_residency().blocks.iter().any(|block| block.key
        == ContentKey::Static {
            module: "story".into()
        }
        && !block.pinned_by.is_empty()));
    let snapshot = p.core().snapshot();
    let music = snapshot.handles["music"];
    let preferences = root.player.preferences("en".into(), "en".into());
    let mut restored =
        Player::new_runtime(root, "release".into(), "Test".into(), Some(preferences)).unwrap();
    let boot = restored.pump(vec![], 1000);
    drain(&mut restored, boot, &objects);
    let envelope = SaveEnvelope {
        format: 1,
        slot: 0,
        revision: 1,
        digest: nir_content::digest(&serde_json::to_vec(&snapshot).unwrap()),
        snapshot,
    };
    let load = restored.pump(
        vec![AppEvent::Loaded {
            envelope: Box::new(envelope),
        }],
        1000,
    );
    let keys = drain(&mut restored, load, &objects);
    assert!(restored.error.is_none(), "{:?}", restored.diagnostic);
    assert!(!keys.contains(&ContentKey::Code {
        module: "story".into()
    }));
    assert_eq!(restored.core().state().handles["music"], music);
    assert_eq!(
        serde_json::to_value(&restored.core().dialogue().unwrap().1.spans).unwrap(),
        serde_json::to_value(&p.core().dialogue().unwrap().1.spans).unwrap()
    );
    assert!(keys.contains(&ContentKey::Text {
        module: "story".into(),
        locale: "en".into()
    }));
}

#[test]
fn returned_animation_module_remains_pinned_and_cold_restore_fetches_shared_keyframes() {
    use serde_json::json;
    let (mut root, mut objects) = bundled();
    root.player.prefetch_content = false;
    root.requires.push("stage.sprite-timeline.v1".into());
    let driver:Function=serde_json::from_value(json!({"entry":"start","blocks":{
        "start":{"terminator":{"type":"activate","cue":"opening","next":"movie"}},
        "movie":{"terminator":{"type":"call","function":"old.entry","args":{},"next":"intro"}},
        "intro":{"terminator":{"type":"activate","cue":"intro","next":"wait"}},
        "wait":{"terminator":{"type":"await","conditions":[{"task":"line","milestone":{"type":"finished"}}],"next":"done","on_cancelled":"done","on_failed":"done"}},
        "done":{"terminator":{"type":"end","outcome":"done"}}
    }})).unwrap();
    let module = root.modules.get_mut("story").unwrap();
    let mut code: ModuleCode = serde_json::from_slice(&objects[&module.code]).unwrap();
    code.functions.insert("main".into(), driver.clone());
    module.code = object(&mut objects, &code);
    module
        .functions
        .insert("main".into(), FunctionSignature::from(&driver));
    root.function_index.get_mut("main").unwrap().signature = FunctionSignature::from(&driver);
    let movie: Function = serde_json::from_value(json!({"entry":"show","blocks":{
        "show":{"terminator":{"type":"activate","cue":"old.show","next":"return"}},
        "return":{"terminator":{"type":"return"}}
    }}))
    .unwrap();
    let signature = FunctionSignature::from(&movie);
    let movie_code = ModuleCode {
        format: 2,
        module: "old".into(),
        functions: BTreeMap::from([("old.entry".into(), movie)]),
    };
    let code_hash = object(&mut objects, &movie_code);
    let timeline: SpriteTimeline =
        serde_json::from_value(json!({"id":"old.frames","duration_us":"10000000","tracks":[
            {"node":"picture","frames":[{"at_us":"0","rect":[0,0,32,24],"opacity":1},
                {"at_us":"1000000","rect":[64,0,32,24],"opacity":1}]}
        ]}))
        .unwrap();
    let nodes: Vec<Node> = serde_json::from_value(json!([
        {"id":"movie","x":700,"y":100,"width":0,"height":0,"timeline_binding":"old.frames"},
        {"id":"picture","parent":"movie","asset":"bg.station","x":0,"y":0,"width":32,"height":24}
    ]))
    .unwrap();
    let cue:Cue=serde_json::from_value(json!({"effects":[
        {"id":"old.stage","scope":"session","effect":{"type":"stage_present","scene":"old.scene"}},
        {"id":"old.movie","scope":"session","effect":{"type":"sprite_timeline","timeline":"old.frames","root":"movie","duration_us":"10000000"}}
    ]})).unwrap();
    let package = ModuleStatic {
        format: 2,
        module: "old".into(),
        sprite_timelines: BTreeMap::from([("old.frames".into(), timeline)]),
        scenes: BTreeMap::from([("old.scene".into(), nodes)]),
        cues: BTreeMap::from([("old.show".into(), cue)]),
        choices: BTreeMap::new(),
        text_contracts: BTreeMap::new(),
        activation_recipes: BTreeMap::from([(
            "old.show".into(),
            BTreeSet::from(["bg.station".into()]),
        )]),
    };
    let static_hash = object(&mut objects, &package);
    root.modules.insert(
        "old".into(),
        ModuleIndex {
            functions: BTreeMap::from([("old.entry".into(), signature.clone())]),
            texts: BTreeSet::new(),
            code: code_hash,
            static_content: static_hash,
            locales: BTreeMap::new(),
        },
    );
    root.function_index.insert(
        "old.entry".into(),
        RuntimeFunctionIndex {
            execution_module: None,
            module: "old".into(),
            signature,
        },
    );
    root.scene_owners.insert("old.scene".into(), "old".into());
    root.timeline_owners
        .insert("old.frames".into(), "old".into());
    root.cue_owners.insert("old.show".into(), "old".into());
    for task in ["old.stage", "old.movie"] {
        root.task_owners.insert(task.into(), "old".into());
    }
    let mut player =
        Player::new_runtime(root.clone(), "release".into(), "Test".into(), None).unwrap();
    let boot = player.pump(vec![], 1000);
    drain(&mut player, boot, &objects);
    let start = action(&mut player, UiAction::NewGame);
    drain(&mut player, start, &objects);
    assert!(player.error.is_none(), "{:?}", player.diagnostic);
    assert!(player
        .core()
        .state()
        .frames
        .iter()
        .all(|f| f.function != "old.entry"));
    assert!(player.core().dialogue().is_some());
    let old_static = ContentKey::Static {
        module: "old".into(),
    };
    assert!(player
        .content_residency()
        .blocks
        .iter()
        .any(|b| b.key == old_static && !b.pinned_by.is_empty()));
    assert!(player.content_residency().blocks.iter().any(|b| b.key
        == ContentKey::Code {
            module: "old".into()
        }
        && b.pinned_by.is_empty()));
    let snapshot = player.core().snapshot();
    let music = snapshot.handles["music"];
    let timeline = snapshot.handles["old.movie"];
    assert!(!serde_json::to_string(&snapshot)
        .unwrap()
        .contains("\"tracks\""));
    let mut restored = Player::new_runtime(root, "release".into(), "Test".into(), None).unwrap();
    let boot = restored.pump(vec![], 1000);
    drain(&mut restored, boot, &objects);
    let envelope = SaveEnvelope {
        format: 1,
        slot: 0,
        revision: 1,
        digest: nir_content::digest(&serde_json::to_vec(&snapshot).unwrap()),
        snapshot,
    };
    let load = restored.pump(
        vec![AppEvent::Loaded {
            envelope: Box::new(envelope),
        }],
        1000,
    );
    let keys = drain(&mut restored, load, &objects);
    assert!(restored.error.is_none(), "{:?}", restored.diagnostic);
    assert!(keys.contains(&old_static));
    assert!(!keys.contains(&ContentKey::Code {
        module: "old".into()
    }));
    assert_eq!(restored.core().state().handles["music"], music);
    assert_eq!(restored.core().state().handles["old.movie"], timeline);
    assert_eq!(restored.core().sample_scene(), player.core().sample_scene());
    let title = action(&mut player, UiAction::Title);
    drain(&mut player, title, &objects);
    assert!(player
        .content_residency()
        .blocks
        .iter()
        .any(|b| b.key == old_static && b.pinned_by.is_empty()));
}
