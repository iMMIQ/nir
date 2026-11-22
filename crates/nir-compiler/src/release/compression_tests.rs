use super::*;
use std::io::Read;

#[test]
fn large_asset_catalogs_do_not_pin_the_whole_story_for_one_image() {
    let mut source: Program =
        serde_json::from_str(include_str!("../../../../fixtures/rain.json")).unwrap();
    let mut nodes = vec![];
    for ordinal in 0..130 {
        let id = format!("catalog_image_{ordinal:03}");
        source
            .assets
            .insert(id.clone(), source.assets["bg.station"].clone());
        let mut node = source.scenes["station"][0].clone();
        node.id = id.clone();
        node.asset = Some(id);
        nodes.push(node);
    }
    source.scenes.insert("catalog_images".into(), nodes);
    for (id, asset) in &mut source.assets {
        asset.object = nir_content::digest(id.as_bytes());
        if asset.kind == AssetKind::Image {
            asset.decoded_bytes = asset.width as u64 * asset.height as u64 * 4;
        }
    }
    let media = source
        .assets
        .iter()
        .map(|(id, a)| (id.clone(), a.object.clone()))
        .collect();
    for plan in source
        .locale_config
        .ui
        .values_mut()
        .chain(source.locale_config.text.values_mut())
    {
        plan.digest = LocaleFontPlan::digest_for(&plan.fonts, &media);
    }
    let reference = crate::compile(&source).unwrap();
    let out = tempfile::tempdir().unwrap();
    fs::create_dir(out.path().join("objects")).unwrap();
    let mut objects = BTreeMap::new();
    let runtime = package_runtime(out.path(), &mut objects, &source, &reference).unwrap();
    let root = &runtime.executable.program;
    let chunks: BTreeSet<_> = (0..130)
        .map(|ordinal| {
            root.assets[&format!("catalog_image_{ordinal:03}")]
                .catalog
                .clone()
        })
        .collect();
    assert_eq!(chunks.len(), 3);
    let keys = root.asset_catalogs(["catalog_image_000"]);
    assert_eq!(keys.len(), 1);
    let key = keys.into_iter().next().unwrap();
    let digest = root.content_requirement(&key).unwrap().digest;
    let bytes = fs::read(out.path().join(format!("objects/{digest}.json"))).unwrap();
    let object = nir_content::parse_runtime_object(root, &key, &bytes).unwrap();
    let RuntimeObject::Catalog(catalog) = &object else {
        panic!("expected asset catalog")
    };
    assert!(catalog.assets.contains_key("catalog_image_000"));
    assert!(catalog.assets.len() <= 64);
    let view = nir_core::ValidatedProgram::from_runtime(root.clone())
        .unwrap()
        .install_batch(vec![(key, object, bytes.len() as u64)])
        .unwrap();
    assert_eq!(view.residency().resident_blocks, 1);
    for report in runtime.catalogs.values() {
        let bytes = fs::read(out.path().join(format!("objects/{}.json", report.object))).unwrap();
        let catalog: AssetCatalog = serde_json::from_slice(&bytes).unwrap();
        assert!(catalog.assets.len() <= 64);
        for id in catalog.assets.keys() {
            assert_eq!(root.assets[id].catalog, catalog.catalog);
        }
    }
}

#[test]
fn large_code_packages_keep_shared_tasks_and_validate_each_runtime_module() {
    let mut source: Program =
        serde_json::from_str(include_str!("../../../../fixtures/rain.json")).unwrap();
    for (id, asset) in &mut source.assets {
        asset.object = nir_content::digest(id.as_bytes());
    }
    let media = source
        .assets
        .iter()
        .map(|(id, asset)| (id.clone(), asset.object.clone()))
        .collect();
    for plan in source
        .locale_config
        .ui
        .values_mut()
        .chain(source.locale_config.text.values_mut())
    {
        plan.digest = LocaleFontPlan::digest_for(&plan.fonts, &media);
    }
    source
        .variables
        .insert("payload".into(), nir_format::Value::String(String::new()));
    for ordinal in 0..32 {
        let ops: Vec<_> = (0..100).map(|at| serde_json::json!({
            "id": format!("f{ordinal}_op{at}"),
            "operation":{"type":"assign","target":"payload","value":{"type":"const","value":{"type":"string","value":"x".repeat(1024)}}}
        })).collect();
        let mut function = source.functions[&source.entry].clone();
        for block in function.blocks.values_mut() {
            for op in &mut block.ops {
                op.id = format!("f{ordinal}_{}", op.id);
            }
        }
        function
            .blocks
            .get_mut(&function.entry.clone())
            .unwrap()
            .ops
            .extend(
                ops.into_iter()
                    .map(|op| serde_json::from_value(op).unwrap()),
            );
        source
            .functions
            .insert(format!("large_{ordinal}"), function);
    }
    let reference = crate::compile(&source).unwrap();
    let out = tempfile::tempdir().unwrap();
    fs::create_dir(out.path().join("objects")).unwrap();
    let mut objects = BTreeMap::new();
    for id in source.assets.keys() {
        object(
            out.path(),
            &mut objects,
            id.as_bytes(),
            "bin",
            "application/octet-stream",
        )
        .unwrap();
    }
    let runtime = package_runtime(out.path(), &mut objects, &source, &reference).unwrap();
    let root = &runtime.executable.program;
    assert!(root.modules.len() > 2);
    assert_eq!(root.function_index.len(), source.functions.len());
    assert_eq!(root.function_index[&source.entry].module, "_legacy");
    assert!(root.task_owners.values().all(|owner| owner == "_legacy"));
    assert!(root.text_owners.values().all(|owner| owner == "_legacy"));
    for (module, index) in &root.modules {
        if module != "_legacy" {
            assert!(index.texts.is_empty());
            assert!(runtime.modules[module].code_bytes < 2 * 1024 * 1024);
        }
    }
    let child = root
        .modules
        .keys()
        .find(|module| *module != "_legacy")
        .unwrap();
    let mut closure = ClosureBuilder::default();
    add_module_payload(
        &mut closure,
        &objects,
        &runtime,
        child,
        "en",
        "en",
        "cold code-package entry",
    )
    .unwrap();
    let closure = closure.finish();
    assert!(closure
        .objects
        .contains_key(&runtime.modules["_legacy"].static_content));
    assert!(closure
        .objects
        .contains_key(&runtime.modules["_legacy"].locales["en"].object));
    assert!(!closure
        .objects
        .contains_key(&runtime.modules["_legacy"].code));
    validate_runtime_packages(out.path(), &runtime.executable).unwrap();
    let root_view = nir_core::ValidatedProgram::from_runtime(root.clone()).unwrap();
    for (module, index) in &root.modules {
        let mut keys = std::collections::BTreeSet::from([ContentKey::Code {
            module: module.clone(),
        }]);
        keys.extend(root.content_prerequisites(&ContentKey::Code {
            module: module.clone(),
        }));
        let batch = keys
            .into_iter()
            .map(|key| {
                let digest = root.content_requirement(&key).unwrap().digest;
                let bytes = fs::read(out.path().join(format!("objects/{digest}.json"))).unwrap();
                let object = nir_content::parse_runtime_object(root, &key, &bytes).unwrap();
                (key, object, bytes.len() as u64)
            })
            .collect();
        let view = root_view.install_batch(batch).unwrap();
        assert!(index
            .functions
            .keys()
            .all(|id| view.program().functions.contains_key(id)));
    }
}

#[test]
fn identical_scene_templates_keep_activations_and_module_ownership() {
    let mut source: Program =
        serde_json::from_str(include_str!("../../../../fixtures/rain.json")).unwrap();
    let nodes = source.scenes.values().next().unwrap().clone();
    source.scenes = BTreeMap::from([
        ("a.first".into(), nodes.clone()),
        ("a.repeat".into(), nodes.clone()),
        ("b.same".into(), nodes.clone()),
    ]);
    let mut changed = nodes;
    changed[0].x += 1.;
    source.scenes.insert("a.changed".into(), changed);
    source.title_scene = Some("a.repeat".into());
    let present = |scene: &str, duration| EffectDef {
        id: "stage".into(),
        scope: Scope::Session,
        effect: Effect::StagePresent {
            dialogue_visible: None,
            inherit_images: Vec::new(),
            inherit_image_geometry: Vec::new(),
            scene: scene.into(),
            transition: Default::default(),
            duration_us: Micros(duration),
        },
    };
    source.cues = BTreeMap::from([(
        "a.show".into(),
        Cue {
            effects: vec![EffectDef {
                id: "sequence".into(),
                scope: Scope::Session,
                effect: Effect::Sequence {
                    children: vec![present("a.first", 100), present("a.repeat", 200)],
                },
            }],
        },
    )]);
    let optimized =
        coalesce_scene_templates(&source, &BTreeSet::from(["a".into(), "b".into()])).unwrap();
    assert_eq!(optimized.scenes.len(), 3);
    assert!(optimized.scenes.contains_key("a.changed"));
    assert!(optimized.scenes.contains_key("b.same"));
    assert_eq!(optimized.title_scene.as_deref(), Some("a.first"));
    let children = optimized.cues["a.show"].effects[0]
        .effect
        .compose_children()
        .unwrap();
    assert_eq!(children.len(), 2);
    for (child, duration) in children.iter().zip([100, 200]) {
        assert!(
            matches!(&child.effect, Effect::StagePresent { scene, duration_us, .. } if scene == "a.first" && duration_us.0 == duration)
        );
    }
    assert!(
        source.scenes.contains_key("a.repeat"),
        "author declarations stay unchanged"
    );
}

#[test]
fn inline_only_images_belong_to_the_text_module_catalog() {
    let mut program: Program =
        serde_json::from_str(include_str!("../../../../fixtures/rain.json")).unwrap();
    program.texts.get_mut("intro").unwrap().images = vec![nir_format::TextImageContract {
        id: "inline".into(),
        asset: "inline-only".into(),
    }];
    let consumers = asset_consumers(
        &program,
        &BTreeMap::new(),
        &BTreeMap::new(),
        &BTreeMap::new(),
        &BTreeMap::from([("intro".into(), "chapter".into())]),
        &BTreeMap::new(),
    );
    assert_eq!(
        consumers["inline-only"],
        BTreeSet::from(["module:chapter".into()])
    );
    assert!(!consumers["inline-only"].contains("bootstrap"));
}

#[test]
fn gzip_is_reproducible_and_preserves_object_identity() {
    let out = tempfile::tempdir().unwrap();
    fs::create_dir(out.path().join("objects")).unwrap();
    let mut objects = BTreeMap::new();
    let bytes = b"compressible release content ".repeat(100);
    let hash = object(out.path(), &mut objects, &bytes, "json", "application/json").unwrap();
    let descriptor = &objects[&hash];
    assert_eq!(descriptor.bytes, bytes.len() as u64);
    let sidecar = out.path().join(format!("{}.gz", descriptor.path));
    let first = fs::read(&sidecar).unwrap();
    assert!(first.len() < bytes.len());
    assert_eq!(&first[4..8], &[0; 4]);
    let mut decoded = Vec::new();
    flate2::read::GzDecoder::new(first.as_slice())
        .read_to_end(&mut decoded)
        .unwrap();
    assert_eq!(decoded, bytes);
    object(out.path(), &mut objects, &bytes, "json", "application/json").unwrap();
    assert_eq!(fs::read(sidecar).unwrap(), first);
    let tiny = object(out.path(), &mut objects, b"{}", "json", "application/json").unwrap();
    assert!(!out
        .path()
        .join(format!("{}.gz", objects[&tiny].path))
        .exists());
    let media = object(out.path(), &mut objects, &bytes, "png", "image/png").unwrap();
    assert!(!out
        .path()
        .join(format!("{}.gz", objects[&media].path))
        .exists());
}
