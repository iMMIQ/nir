use nir_compiler::*;
use std::fs;
use std::path::{Path, PathBuf};
fn source() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/rain-letters")
}
fn project() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    copy_tree(&source(), d.path()).unwrap();
    d
}
#[test]
fn choice_only_image_states_survive_release_asset_pruning() {
    let d = project();
    let catalog = d.path().join("assets/catalog.toml");
    let mut assets = fs::read_to_string(&catalog).unwrap();
    for state in ["normal", "hover", "disabled"] {
        assets.push_str(&format!("\n[[assets]]\nid = \"choice.{state}\"\nkind = \"image\"\nsource = \"source/aki.png\"\nrights = \"CC0-1.0\"\nexpected_size = [290,530]\n"));
    }
    fs::write(catalog, assets).unwrap();
    let story = d.path().join("content/ch01/story.nir.json");
    let mut fragment: serde_json::Value =
        serde_json::from_slice(&fs::read(&story).unwrap()).unwrap();
    for option in fragment["choices"]["route"]["options"]
        .as_array_mut()
        .unwrap()
    {
        option["image"] = serde_json::json!({"asset":"choice.normal","hover_asset":"choice.hover",
            "disabled_asset":"choice.disabled","rect":[120,120,200,150]});
    }
    fs::write(story, serde_json::to_vec(&fragment).unwrap()).unwrap();
    let sdk = test_sdk();
    resolve(d.path(), sdk.path()).unwrap();
    let out = d.path().join("dist/choice-images");
    let report = build(d.path(), sdk.path(), &out, true).unwrap();
    let release: nir_format::ReleaseManifest = serde_json::from_slice(
        &fs::read(out.join(format!("releases/{}.json", report.release))).unwrap(),
    )
    .unwrap();
    let executable: nir_format::RuntimeExecutable = serde_json::from_slice(
        &fs::read(out.join(&release.objects[&release.program].path)).unwrap(),
    )
    .unwrap();
    for state in ["normal", "hover", "disabled"] {
        assert!(executable
            .program
            .assets
            .contains_key(&format!("choice.{state}")));
    }
    assert!(executable
        .program
        .requires
        .iter()
        .any(|cap| cap == "choice.disabled-image.v1"));
    nir_core::ValidatedProgram::from_runtime(executable.program).unwrap();
}

#[test]
fn inherited_image_activation_recipes_match_eager_and_lazy_release_paths() {
    let d = project();
    let story = d.path().join("content/ch01/story.nir.json");
    let mut fragment: serde_json::Value =
        serde_json::from_slice(&fs::read(&story).unwrap()).unwrap();
    fragment["cues"]["opening"]["effects"][0]["effect"]["inherit_images"] =
        serde_json::json!(["background"]);
    fs::write(story, serde_json::to_vec(&fragment).unwrap()).unwrap();
    let sdk = test_sdk();
    resolve(d.path(), sdk.path()).unwrap();
    let out = d.path().join("dist/inherited-images");
    let report = build(d.path(), sdk.path(), &out, true).unwrap();
    let release: nir_format::ReleaseManifest = serde_json::from_slice(
        &fs::read(out.join(format!("releases/{}.json", report.release))).unwrap(),
    )
    .unwrap();
    let executable: nir_format::RuntimeExecutable = serde_json::from_slice(
        &fs::read(out.join(&release.objects[&release.program].path)).unwrap(),
    )
    .unwrap();
    assert!(executable
        .program
        .requires
        .iter()
        .any(|cap| cap == "stage.inherit-image.v1"));
    nir_core::ValidatedProgram::from_runtime(executable.program).unwrap();
}

#[test]
fn authored_history_voice_is_opt_in_inferred_and_gated_in_source_and_runtime() {
    for flow in [false, true] {
        let d = project();
        let path = d.path().join("themes/rain/theme.toml");
        let original = fs::read_to_string(&path).unwrap();
        let content = if flow {
            "{type = \"history_flow\", voice_controls = true, size = 24, line_height = 36, gap = 12, wheel_step = 48, page_step = 120, max_visible = 32, color = [1,1,1,1]}"
        } else {
            "{type = \"history_window\", voice_controls = true, offset_local = \"offset\", limit = 4, row_height = 80, size = 24, color = [1,1,1,1]}"
        };
        let theme=format!("{original}\n[image_menus.title]\nbackground = \"bg.station\"\nbuttons = []\n[image_menus.title.locals.offset]\ntype = \"int\"\ninitial = 0\nmin = 0\nmax = 999\n[[image_menus.title.elements]]\nid = \"records\"\nrect = [40,40,600,360]\ncontent = {content}\n");
        fs::write(&path, &theme).unwrap();
        let loaded = load_project(d.path()).unwrap();
        assert!(loaded
            .program
            .requires
            .iter()
            .any(|c| c == "ui.menu-history-voice.v1"));
        compile(&loaded.program).unwrap();
        let mut missing = loaded.program.clone();
        missing.requires.retain(|c| c != "ui.menu-history-voice.v1");
        assert_eq!(
            nir_core::ValidatedProgram::new(missing).unwrap_err().code,
            "E_CAPABILITY"
        );
        let sdk = test_sdk();
        resolve(d.path(), sdk.path()).unwrap();
        let out = d.path().join("dist/history-voice");
        let report = build(d.path(), sdk.path(), &out, true).unwrap();
        let release: nir_format::ReleaseManifest = serde_json::from_slice(
            &fs::read(out.join(format!("releases/{}.json", report.release))).unwrap(),
        )
        .unwrap();
        let executable: nir_format::RuntimeExecutable = serde_json::from_slice(
            &fs::read(out.join(&release.objects[&release.program].path)).unwrap(),
        )
        .unwrap();
        nir_core::ValidatedProgram::from_runtime(executable.program.clone()).unwrap();
        let mut root = executable.program;
        root.requires.retain(|c| c != "ui.menu-history-voice.v1");
        assert_eq!(
            nir_core::ValidatedProgram::from_runtime(root)
                .unwrap_err()
                .code,
            "E_CAPABILITY"
        );
        fs::write(
            &path,
            theme.replace("voice_controls = true", "voice_controls = false"),
        )
        .unwrap();
        let legacy = load_project(d.path()).unwrap();
        assert!(!legacy
            .program
            .requires
            .iter()
            .any(|c| c == "ui.menu-history-voice.v1"));
        let serialized =
            serde_json::to_value(&legacy.program.theme.image_menus["title"].elements[0].content)
                .unwrap();
        assert!(serialized.get("voice_controls").is_none());
        compile(&legacy.program).unwrap();
    }
}
#[test]
fn checks_and_scenarios() {
    let p = load_project(&source()).unwrap();
    let e = compile(&p.program).unwrap();
    validate_executable(&e).unwrap();
    assert_eq!(test_project(&source()).unwrap(), ["walk", "stay"]);
}
#[test]
fn rejects_path_escape() {
    let d = project();
    let path = d.path().join("game.toml");
    let s = fs::read_to_string(&path)
        .unwrap()
        .replace("assets/catalog.toml", "../catalog.toml");
    fs::write(path, s).unwrap();
    assert!(load_project(d.path())
        .unwrap_err()
        .to_string()
        .contains("E_PATH"));
}
#[test]
fn rejects_symlink_escape() {
    let d = project();
    #[cfg(unix)]
    let external = tempfile::NamedTempFile::new().unwrap();
    let path = d.path().join("assets/source/station.png");
    fs::remove_file(&path).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(external.path(), path).unwrap();
        assert!(load_project(d.path())
            .unwrap_err()
            .to_string()
            .contains("E_PATH_ESCAPE"));
    }
}
#[test]
fn rejects_duplicate_fragment_identity() {
    let d = project();
    let path = d.path().join("content/ch01/module.toml");
    let s = fs::read_to_string(&path).unwrap().replace(
        "[\"story.nir.json\"]",
        "[\"story.nir.json\", \"story.nir.json\"]",
    );
    fs::write(path, s).unwrap();
    assert!(load_project(d.path())
        .unwrap_err()
        .to_string()
        .contains("E_DUPLICATE"));
}
#[test]
fn rejects_lfs_pointer() {
    let d = project();
    fs::write(
        d.path().join("assets/source/station.png"),
        "version https://git-lfs.github.com/spec/v1\noid sha256:0\nsize 10",
    )
    .unwrap();
    assert!(load_project(d.path())
        .unwrap_err()
        .to_string()
        .contains("E_LFS_POINTER"));
}
#[test]
fn rejects_wrong_dimensions() {
    let d = project();
    let path = d.path().join("assets/catalog.toml");
    let s = fs::read_to_string(&path)
        .unwrap()
        .replace("1280, 720", "99, 99");
    fs::write(path, s).unwrap();
    assert!(load_project(d.path())
        .unwrap_err()
        .to_string()
        .contains("E_ASSET_SIZE"));
}
#[test]
fn missing_font_glyph_rejected() {
    let d = project();
    let path = d.path().join("content/ch01/texts/en.json");
    let s = fs::read_to_string(&path)
        .unwrap()
        .replace("You came after all.", "You came after all. 🐈");
    fs::write(path, s).unwrap();
    text_review(d.path(), "arrival", "en").unwrap();
    assert!(load_project(d.path())
        .unwrap_err()
        .to_string()
        .contains("E_FONT_COVERAGE"));
}

#[test]
fn locale_config_is_required_and_rejects_invalid_font_plans() {
    let d = project();
    let game = d.path().join("game.toml");
    let manifest = fs::read_to_string(&game).unwrap();
    fs::write(
        &game,
        manifest.replace("locales = \"config/locales.toml\"\n", ""),
    )
    .unwrap();
    assert!(load_project(d.path())
        .unwrap_err()
        .to_string()
        .contains("E_LOCALE_CONFIG"));

    let d = project();
    let locales = d.path().join("config/locales.toml");
    let config = fs::read_to_string(&locales).unwrap();
    fs::write(
        &locales,
        config.replacen(
            "zh-Hans = [\"font.reader\"]",
            "zh-Hans = [\"font.reader\", \"font.reader\"]",
            1,
        ),
    )
    .unwrap();
    assert!(load_project(d.path())
        .unwrap_err()
        .to_string()
        .contains("E_FONT_PLAN"));

    let d = project();
    let locales = d.path().join("config/locales.toml");
    let config = fs::read_to_string(&locales).unwrap();
    fs::write(
        &locales,
        config.replace("default_text = \"zh-Hans\"", "default_text = \"fr\""),
    )
    .unwrap();
    assert!(load_project(d.path())
        .unwrap_err()
        .to_string()
        .contains("E_LOCALE_CONFIG"));

    let d = project();
    let locales = d.path().join("config/locales.toml");
    let config = fs::read_to_string(&locales).unwrap();
    fs::write(&locales, config.replace("[ui]", "unknown = true\n\n[ui]")).unwrap();
    assert!(load_project(d.path())
        .unwrap_err()
        .to_string()
        .contains("unknown field"));
}

#[test]
fn locale_text_plans_must_cover_all_bundles_and_only_use_fonts() {
    let d = project();
    let locales = d.path().join("config/locales.toml");
    let config = fs::read_to_string(&locales).unwrap();
    let without_english_text = config.replace(
        "[text]\nzh-Hans = [\"font.reader\"]\nen = [\"font.latin\", \"font.reader\"]",
        "[text]\nzh-Hans = [\"font.reader\"]",
    );
    fs::write(&locales, without_english_text).unwrap();
    assert!(load_project(d.path())
        .unwrap_err()
        .to_string()
        .contains("E_TRANSLATION"));

    let d = project();
    let locales = d.path().join("config/locales.toml");
    let config = fs::read_to_string(&locales).unwrap();
    fs::write(
        &locales,
        config.replacen(
            "zh-Hans = [\"font.reader\"]",
            "zh-Hans = [\"bg.station\"]",
            1,
        ),
    )
    .unwrap();
    assert!(load_project(d.path())
        .unwrap_err()
        .to_string()
        .contains("E_FONT_PLAN"));
}
#[test]
fn unsupported_module_count_rejected() {
    let d = project();
    let path = d.path().join("game.toml");
    let s = fs::read_to_string(&path)
        .unwrap()
        .replace("[\"content/ch01/module.toml\"]", "[]");
    fs::write(path, s).unwrap();
    assert!(load_project(d.path())
        .unwrap_err()
        .to_string()
        .contains("E_CAPABILITY"));
}

fn test_sdk() -> tempfile::TempDir {
    // These bytes exercise packaging identity only. Browser tests use the real SDK.
    let d = tempfile::tempdir().unwrap();
    for name in [
        "player_web.js",
        "player_web_bg.wasm",
        "host.js",
        "runtime-worker.js",
        "asset-worker.js",
        "index.html",
        "bootstrap.js",
        "THIRD-PARTY.txt",
        "compiler.sha256",
    ] {
        fs::write(d.path().join(name), format!("packaging fixture: {name}")).unwrap();
    }
    d
}
#[test]
fn reproducible_release_lock_drift_and_corruption() {
    let d = project();
    let sdk = test_sdk();
    resolve(d.path(), sdk.path()).unwrap();
    let a = build(d.path(), sdk.path(), &d.path().join("dist/a"), true).unwrap();
    let b = build(d.path(), sdk.path(), &d.path().join("dist/b"), true).unwrap();
    assert_eq!(a.release, b.release);
    let manifest: nir_format::ReleaseManifest = serde_json::from_slice(
        &fs::read(d.path().join(format!("dist/a/releases/{}.json", a.release))).unwrap(),
    )
    .unwrap();
    for (hash, object) in &manifest.objects {
        nir_content::verify(
            &fs::read(d.path().join("dist/a").join(&object.path)).unwrap(),
            hash,
        )
        .unwrap();
    }
    let object = manifest.objects.values().next().unwrap();
    fs::write(d.path().join("dist/a").join(&object.path), b"corrupt").unwrap();
    assert!(build(d.path(), sdk.path(), &d.path().join("dist/a"), true)
        .unwrap_err()
        .to_string()
        .contains("E_DIGEST"));
    fs::write(sdk.path().join("host.js"), b"drift").unwrap();
    assert!(check_lock(&load_project(d.path()).unwrap(), sdk.path())
        .unwrap_err()
        .to_string()
        .contains("E_LOCK_DRIFT"));
}

#[test]
fn staged_release_has_hashed_fixed_launch_and_no_channel() {
    let project = project();
    let sdk = test_sdk();
    resolve(project.path(), sdk.path()).unwrap();
    let out = project.path().join("dist/staged");
    assert!(build_profile(
        project.path(),
        sdk.path(),
        &out,
        "release",
        false,
        false,
        &OptimizeOptions::default()
    )
    .unwrap_err()
    .to_string()
    .contains("E_RELEASE_LOCK"));
    let report = build_profile(
        project.path(),
        sdk.path(),
        &out,
        "release",
        true,
        false,
        &OptimizeOptions::default(),
    )
    .unwrap();
    assert!(project.path().join("reports/build.json").exists());
    assert!(project.path().join("reports/dependencies.json").exists());
    assert!(!out.join("channels/stable.json").exists());
    let bytes = fs::read(out.join(format!("releases/{}.json", report.release))).unwrap();
    nir_content::verify(&bytes, &report.release).unwrap();
    let release: nir_format::ReleaseManifest = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(release.format, 1);
    assert_eq!(release.profile, "release");
    for (name, hash) in [
        ("index.html", &release.launch.html),
        ("bootstrap.js", &release.launch.bootstrap),
    ] {
        let fixed = fs::read(out.join(format!("releases/{}/{name}", report.release))).unwrap();
        let object = fs::read(out.join(&release.objects[hash].path)).unwrap();
        assert_eq!(fixed, object);
        nir_content::verify(&fixed, hash).unwrap();
    }
}

#[test]
fn release_splits_runtime_packages_and_reports_locale_closures() {
    let d = project();
    let sdk = test_sdk();
    resolve(d.path(), sdk.path()).unwrap();

    let first_out = d.path().join("dist/first");
    let first = build(d.path(), sdk.path(), &first_out, true).unwrap();
    let first_release: nir_format::ReleaseManifest = serde_json::from_slice(
        &fs::read(first_out.join(format!("releases/{}.json", first.release))).unwrap(),
    )
    .unwrap();
    let first_exe: nir_format::RuntimeExecutable = serde_json::from_slice(
        &fs::read(first_out.join(&first_release.objects[&first_release.program].path)).unwrap(),
    )
    .unwrap();
    assert_eq!(first_exe.format, 2);
    assert_eq!(first_exe.program.format, 2);
    let root_json = serde_json::to_value(&first_exe.program).unwrap();
    for full_body in ["functions", "scenes", "cues", "choices", "texts"] {
        assert!(
            root_json.get(full_body).is_none(),
            "root retained {full_body}"
        );
    }
    assert!(!root_json["locales"].is_object());
    assert!(!root_json["title_nodes"].as_array().unwrap().is_empty());
    assert_eq!(root_json["title_scene"], "station");
    assert!(root_json["assets"]
        .as_object()
        .unwrap()
        .values()
        .all(|asset| {
            let mut fields: Vec<_> = asset
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            fields.sort_unstable();
            fields == ["catalog", "kind", "object"]
        }));
    let module_id = first_exe.program.modules.keys().next().unwrap().clone();
    let first_index = &first_exe.program.modules[&module_id];
    let first_code = first_index.code.clone();
    let first_static = first_index.static_content.clone();
    let first_zh = first_index.locales["zh-Hans"].clone();
    let first_en = first_index.locales["en"].clone();
    for hash in [&first_static, &first_code, &first_zh, &first_en] {
        let object = &first_release.objects[hash];
        nir_content::verify(&fs::read(first_out.join(&object.path)).unwrap(), hash).unwrap();
    }
    let module_static: nir_format::ModuleStatic = serde_json::from_slice(
        &fs::read(first_out.join(&first_release.objects[&first_static].path)).unwrap(),
    )
    .unwrap();
    assert_eq!(module_static.format, 2);
    assert_eq!(module_static.module, module_id);
    assert!(!module_static.scenes.is_empty());
    assert_eq!(
        module_static
            .scenes
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        first_exe
            .program
            .scene_owners
            .iter()
            .filter(|(_, owner)| *owner == &module_id)
            .map(|(scene, _)| scene.clone())
            .collect()
    );
    let module_code: nir_format::ModuleCode = serde_json::from_slice(
        &fs::read(first_out.join(&first_release.objects[&first_code].path)).unwrap(),
    )
    .unwrap();
    assert_eq!(module_code.format, 2);
    assert_eq!(module_code.module, module_id);
    assert_eq!(module_code.functions.len(), first_index.functions.len());

    // The English-only font is absent from the Chinese boot closure, while the
    // selected English boot closure includes both its catalog and media.
    let dependencies: serde_json::Value =
        serde_json::from_slice(&fs::read(d.path().join("reports/dependencies.json")).unwrap())
            .unwrap();
    let zh_boot = &dependencies["boot"]["zh-Hans"]["zh-Hans"];
    let en_boot = &dependencies["boot"]["en"]["en"];
    let latin = &first_exe.program.assets["font.latin"];
    let latin_catalog = &first_exe.program.catalogs[&latin.catalog];
    assert!(zh_boot["objects"].get(&latin.object).is_none());
    assert!(en_boot["objects"].get(&latin.object).is_some());
    assert!(en_boot["objects"].get(latin_catalog).is_some());
    assert!(zh_boot["objects"].get(&first_release.program).is_some());
    assert!(zh_boot["category_bytes"]["engine"].as_u64().unwrap() > 0);
    for path in ["index.html", "bootstrap.js", "channels/stable.json"] {
        assert!(zh_boot["files"].get(path).is_some());
    }
    let release_path = format!("releases/{}.json", first.release);
    assert!(zh_boot["files"].get(release_path.as_str()).is_some());

    let english_path = d.path().join("content/ch01/texts/en.json");
    let mut english: serde_json::Value =
        serde_json::from_slice(&fs::read(&english_path).unwrap()).unwrap();
    let text_id = {
        let (text_id, doc) = english.as_object_mut().unwrap().iter_mut().next().unwrap();
        let revised_text = format!("{} Revised", doc["spans"][0]["text"].as_str().unwrap());
        doc["spans"][0]["text"] = serde_json::Value::String(revised_text);
        text_id.clone()
    };
    fs::write(&english_path, serde_json::to_vec_pretty(&english).unwrap()).unwrap();
    text_review(d.path(), &text_id, "en").unwrap();

    let second_out = d.path().join("dist/second");
    let second = build(d.path(), sdk.path(), &second_out, true).unwrap();
    let second_release: nir_format::ReleaseManifest = serde_json::from_slice(
        &fs::read(second_out.join(format!("releases/{}.json", second.release))).unwrap(),
    )
    .unwrap();
    let second_exe: nir_format::RuntimeExecutable = serde_json::from_slice(
        &fs::read(second_out.join(&second_release.objects[&second_release.program].path)).unwrap(),
    )
    .unwrap();
    let second_index = &second_exe.program.modules[&module_id];
    assert_eq!(second_index.code, first_code);
    assert_eq!(second_index.locales["zh-Hans"], first_zh);
    assert_ne!(second_index.locales["en"], first_en);
    assert!(first.module_packages[&module_id].locales.contains_key("en"));
}

#[test]
fn runtime_rejects_missing_or_rewritten_index_and_recipe() {
    let p = load_project(&source()).unwrap();
    let original = compile(&p.program).unwrap();
    let mut e = original.clone();
    e.addresses[0].stable_id = "wrong".into();
    assert!(nir_content::validate_executable(&e).is_err());
    let mut e = original.clone();
    e.activation_recipes.clear();
    assert!(nir_content::validate_executable(&e).is_err());
    let mut e = original;
    e.addresses.pop();
    assert!(nir_content::validate_executable(&e).is_err());
}

#[test]
fn window_transition_masks_join_the_release_roots_and_capability() {
    // A mid-block mask reveal belongs to no cue recipe: its identity must
    // still ship in the release root index, and the capability stays declared
    // only while an op actually uses a styled reveal.
    let d = project();
    let catalog = d.path().join("assets/catalog.toml");
    let mut s = fs::read_to_string(&catalog).unwrap();
    s.push_str("\n[[assets]]\nid = \"mask.pattern\"\nkind = \"image\"\nsource = \"source/station.png\"\nrights = \"CC0-1.0\"\nexpected_size = [1280, 720]\n");
    fs::write(&catalog, s).unwrap();
    let path = d.path().join("content/ch01/story.nir.json");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["functions"]["main"]["blocks"]["intro"]["ops"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"id":"window.reveal","operation":{
            "type":"dialogue_visibility","visible":false,
            "transition":{"type":"mask","asset":"mask.pattern","channel":"alpha"},
            "duration_us":"500000"}}));
    fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    let p = load_project(d.path()).unwrap();
    assert!(p
        .program
        .requires
        .iter()
        .any(|c| c == "text.window-transition.v1"));
    assert!(runtime_roots(&p.program).contains("mask.pattern"));

    let base = load_project(&source()).unwrap();
    assert!(!base
        .program
        .requires
        .iter()
        .any(|c| c == "text.window-transition.v1"));
    assert!(!runtime_roots(&base.program).contains("mask.pattern"));
}
#[test]
fn source_diagnostic_locates_reference_without_changing_program_identity() {
    let d = project();
    let path = d.path().join("content/ch01/story.nir.json");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let (scene, nodes) = value["scenes"]
        .as_object_mut()
        .unwrap()
        .iter_mut()
        .next()
        .unwrap();
    let scene = scene.clone();
    nodes[0]["asset"] = serde_json::json!("missing.private.test");
    let text = serde_json::to_string_pretty(&value).unwrap();
    fs::write(&path, &text).unwrap();
    let error = load_project(d.path()).unwrap_err();
    let diag = diagnostic(&error);
    assert_eq!(diag.code, "E_ASSET_TYPE");
    let details = diag.details.unwrap();
    let source = details.source.unwrap();
    assert_eq!(source.file, "content/ch01/story.nir.json");
    assert_eq!(source.pointer, format!("/scenes/{scene}/0/asset"));
    let line = text.lines().nth(source.line - 1).unwrap();
    assert!(line[source.column - 1..].starts_with("\"missing.private.test\""));
    assert!(details
        .references
        .contains(&"missing.private.test".to_string()));
    assert!(details.hint.unwrap().contains("catalog"));
    assert!(!serde_json::to_string(&diagnostic(&error))
        .unwrap()
        .contains(d.path().to_str().unwrap()));
}
#[test]
fn strict_parse_reports_real_line_and_preserves_old_diagnostic_wire_format() {
    let d = nir_content::parse::<nir_format::Program>(b"{\n \"bad\": ]", "story.json").unwrap_err();
    let s = d.details.unwrap().source.unwrap();
    assert_eq!(s.line, 2);
    assert!(s.column > 1);
    let old: nir_format::Diagnostic =
        serde_json::from_str(r#"{"code":"E_TEST","location":"f/b/1","message":"cause"}"#).unwrap();
    assert!(old.details.is_none());
}

#[test]
fn template_font_covers_bundled_diagnostic_messages() {
    let bytes = fs::read(source().join("assets/source/reader.otf")).unwrap();
    let face = ttf_parser::Face::parse(&bytes, 0).unwrap();
    let master = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../templates/minimal/assets/fonts/NotoSansCJKsc-Regular.otf"),
    )
    .unwrap();
    let master_face = ttf_parser::Face::parse(&master, 0).unwrap();
    for messages in [
        include_str!("../../nir-presentation/messages/en.ftl"),
        include_str!("../../nir-presentation/messages/zh-Hans.ftl"),
    ] {
        for line in messages.lines() {
            // rain-letters has no Japanese text plan and never displays this
            // option. New imports use the minimal template's full master.
            let face = if line.starts_with("language-ja =") {
                &master_face
            } else {
                &face
            };
            for c in line.chars().filter(|c| !c.is_whitespace()) {
                assert!(face.glyph_index(c).is_some(), "missing UI glyph: {c}");
            }
        }
    }
}

#[test]
fn author_schema_error_retains_parser_position() {
    let d = project();
    let path = d.path().join("themes/rain/tokens.json");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["unsupported_core_field"] = serde_json::json!(true);
    fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    let error = diagnostic(&load_project(d.path()).unwrap_err());
    assert_eq!(error.code, "E_SCHEMA");
    let source = error.details.unwrap().source.unwrap();
    assert!(source.file.ends_with("themes/rain/tokens.json"));
    assert!(source.line > 1 && source.column > 0);
}

#[test]
fn resolves_author_configuration_with_per_field_sources() {
    let d = project();
    fs::write(
        d.path().join("config/player.toml"),
        "format = 1\n[defaults]\nfont_scale = 1.2\nauto_delay_us = \"2500000\"\n",
    )
    .unwrap();
    let p = load_project(d.path()).unwrap();
    assert_eq!(p.program.player.font_scale, 1.2);
    assert_eq!(p.program.player.auto_delay_us.0, 2_500_000);
    assert!(p.program.player.prefetch_content);
    assert!(!p.program.player.prefetch_media);
    assert_eq!(
        p.resolved_config["player.prefetch_media"].source,
        "builtin:web-standard"
    );
    assert_eq!(
        p.resolved_config["player.prefetch_content"].source,
        "builtin:web-standard"
    );
    assert_eq!(
        p.resolved_config["player.font_scale"].source,
        "config/player.toml#/defaults/font_scale"
    );
    assert_eq!(
        p.resolved_config["player.bgm_volume"].source,
        "builtin:web-standard"
    );
    assert_eq!(
        p.resolved_config["theme.slots.dialogue.main"].value,
        "builtin.dialogue"
    );
    assert_eq!(
        p.resolved_config["theme.tokens.panel"].source,
        "themes/rain/tokens.json#/panel"
    );
    let encoded = serde_json::to_string(&p.program).unwrap();
    assert!(!encoded.contains("config/player.toml"));
    assert!(!serde_json::to_string(&p.resolved_config)
        .unwrap()
        .contains(d.path().to_str().unwrap()));
    // Local configuration cannot override work semantics or presentation defaults.
    fs::write(
        d.path().join("game.local.toml"),
        "[defaults]\nfont_scale = 0.8\n",
    )
    .unwrap();
    assert_eq!(
        load_project(d.path()).unwrap().program.revision,
        p.program.revision
    );
    fs::write(
        d.path().join("config/player.toml"),
        "format = 1\n[defaults]\nprefetch_content = false\nprefetch_media = true\n",
    )
    .unwrap();
    let configured = load_project(d.path()).unwrap();
    assert!(!configured.program.player.prefetch_content);
    assert!(configured.program.player.prefetch_media);
    assert_eq!(
        configured.resolved_config["player.prefetch_media"].source,
        "config/player.toml#/defaults/prefetch_media"
    );
    assert_eq!(
        configured.resolved_config["player.prefetch_content"].source,
        "config/player.toml#/defaults/prefetch_content"
    );
}

#[test]
fn rejects_unsupported_theme_contracts_and_unsafe_props() {
    for (old, new, code) in [
        ("builtin.reader", "custom.javascript", "E_THEME_CONTRACT"),
        ("builtin.dialogue\"", "builtin.choice\"", "E_TOML"),
        ("choice.main", "overlay.phone", "E_TOML"),
        ("font_size = 23.0", "font_size = 0.0", "E_THEME_PROPS"),
        ("height = 240.0", "height = nan", "E_THEME_PROPS"),
        ("tokens.json", "../../../../tokens.json", "E_PATH"),
        (
            "padding = 24.0",
            "padding = 24.0\nhide_menu = true",
            "E_TOML",
        ),
    ] {
        let d = project();
        let path = d.path().join("themes/rain/theme.toml");
        fs::write(&path, fs::read_to_string(&path).unwrap().replace(old, new)).unwrap();
        let error = load_project(d.path()).unwrap_err().to_string();
        assert!(error.contains(code), "{old} -> {new}: {error}");
    }
}

#[test]
fn rejects_unknown_runtime_and_player_overrides() {
    for text in [
        "format = 2",
        "format = 1\n[defaults]\nbgm_volume = 2.0",
        "format = 1\n[defaults]\nauto_delay_us = \"0\"",
        "format = 1\n[defaults]\nrelease_gates = true",
    ] {
        let d = project();
        fs::write(d.path().join("config/player.toml"), text).unwrap();
        assert!(load_project(d.path()).is_err(), "accepted {text}");
    }
    let d = project();
    let path = d.path().join("game.toml");
    fs::write(
        &path,
        fs::read_to_string(&path)
            .unwrap()
            .replace("web-standard", "webgl-worker"),
    )
    .unwrap();
    assert!(load_project(d.path())
        .unwrap_err()
        .to_string()
        .contains("E_RUNTIME_PRESET"));
}

#[test]
fn legacy_tokens_and_new_components_share_validation_at_runtime() {
    let d = project();
    let path = d.path().join("game.toml");
    fs::write(
        &path,
        fs::read_to_string(&path)
            .unwrap()
            .replace("themes/rain/theme.toml", "themes/rain/tokens.json"),
    )
    .unwrap();
    let mut p = load_project(d.path()).unwrap().program;
    let mut invisible = p.clone();
    invisible.theme.text = invisible.theme.panel;
    assert_eq!(
        nir_core::ValidatedProgram::new(invisible).unwrap_err().code,
        "E_THEME_CONTRAST"
    );
    assert_eq!(
        p.theme.slots.dialogue,
        nir_format::DialogueComponent::Bottom
    );
    p.theme.dialogue.font_size = f32::INFINITY;
    assert_eq!(
        nir_core::ValidatedProgram::new(p).unwrap_err().code,
        "E_THEME_PROPS"
    );
}

#[test]
fn theme_edit_changes_release_without_changing_locked_sdk() {
    let d = project();
    let sdk = test_sdk();
    let lock = resolve(d.path(), sdk.path()).unwrap();
    let before = build(d.path(), sdk.path(), &d.path().join("dist/a"), true).unwrap();
    let path = d.path().join("themes/rain/theme.toml");
    fs::write(
        &path,
        fs::read_to_string(&path)
            .unwrap()
            .replace("builtin.dialogue\"", "builtin.dialogue.top\"")
            .replace("builtin.choice\"", "builtin.choice.compact\""),
    )
    .unwrap();
    let after = build(d.path(), sdk.path(), &d.path().join("dist/b"), true).unwrap();
    assert_ne!(before.release, after.release);
    assert_eq!(before.engine_build, after.engine_build);
    assert_eq!(
        lock,
        check_lock(&load_project(d.path()).unwrap(), sdk.path()).unwrap()
    );
    assert_eq!(
        after.release,
        build(d.path(), sdk.path(), &d.path().join("dist/c"), true)
            .unwrap()
            .release
    );
}

#[test]
fn fixed_auto_delay_configuration_emits_only_its_used_capability() {
    let d = project();
    let legacy = load_project(d.path()).unwrap();
    assert!(!legacy
        .program
        .requires
        .iter()
        .any(|c| c == "player.auto-delay-policy.v1"));
    fs::write(
        d.path().join("config/player.toml"),
        "format = 1\n[defaults]\nauto_delay_policy = \"fixed\"\nauto_delay_us = \"0\"\n",
    )
    .unwrap();
    let fixed = load_project(d.path()).unwrap();
    assert_eq!(fixed.program.player.auto_delay_us.0, 0);
    assert!(fixed
        .program
        .requires
        .iter()
        .any(|c| c == "player.auto-delay-policy.v1"));
}

#[test]
fn loop_region_capability_tracks_story_compositions_and_menu_music() {
    for mode in ["plain", "composed", "menu"] {
        let d = project();
        assert!(!load_project(d.path())
            .unwrap()
            .program
            .requires
            .iter()
            .any(|cap| cap == "audio.loop-region.v1"));
        if mode == "menu" {
            let path = d.path().join("themes/rain/theme.toml");
            let mut theme = fs::read_to_string(&path).unwrap();
            theme.push_str("\n[image_menus.title]\nbackground = \"bg.station\"\nbuttons = []\n[image_menus.title.effects.music]\nasset = \"audio.bgm\"\nloop_region = { start_us = \"200000\", end_us = \"600000\" }\n");
            fs::write(path, theme).unwrap();
        } else {
            let path = d.path().join("content/ch01/story.nir.json");
            let mut content: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            let music = content["cues"]["opening"]["effects"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|definition| definition["id"] == "music")
                .unwrap();
            music["effect"]["loop_region"] =
                serde_json::json!({"start_us":"200000","end_us":"600000"});
            if mode == "composed" {
                let child = music.clone();
                *music = serde_json::json!({"id":"music-chain","scope":"session","effect":{"type":"sequence","children":[child]}});
            }
            fs::write(path, serde_json::to_vec(&content).unwrap()).unwrap();
        }
        let loaded = load_project(d.path()).unwrap();
        assert!(
            loaded
                .program
                .requires
                .iter()
                .any(|cap| cap == "audio.loop-region.v1"),
            "{mode}"
        );
        compile(&loaded.program).unwrap();
    }
}

#[test]
fn reading_menu_actions_emit_only_their_used_capability() {
    let d = project();
    assert!(!load_project(d.path())
        .unwrap()
        .program
        .requires
        .iter()
        .any(|c| c == "ui.menu-reading.v1"));
    let path = d.path().join("themes/rain/theme.toml");
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str("\n[image_menus.title]\nbackground = \"bg.station\"\nbuttons = []\n[[image_menus.title.elements]]\nid = \"auto\"\nrect = [0,0,300,80]\ncontent = { type = \"hit_region\", label = \"Auto\", action = {type = \"reading\", mode = \"auto\"} }\n");
    fs::write(path, text).unwrap();
    let loaded = load_project(d.path()).unwrap();
    assert!(loaded
        .program
        .requires
        .iter()
        .any(|c| c == "ui.menu-reading.v1"));
    compile(&loaded.program).unwrap();
}

#[test]
fn menu_transition_capability_follows_spatial_style_usage() {
    let d = project();
    // The stock theme declares neither effects capability.
    let requires = |path: &Path| load_project(path).unwrap().program.requires;
    assert!(!requires(d.path()).iter().any(|c| c == "ui.menu-effects.v1"));
    assert!(!requires(d.path())
        .iter()
        .any(|c| c == "ui.menu-transition.v1"));
    let path = d.path().join("themes/rain/theme.toml");
    let mut text = fs::read_to_string(&path).unwrap();
    // A dissolve fade claims only the effects capability; the wipe close adds
    // the reveal capability.
    text.push_str("\n[image_menus.title]\nbackground = \"bg.station\"\nbuttons = []\n[image_menus.title.effects.enter]\nfade_us = \"400000\"\nstyle = {type = \"dissolve\"}\n[image_menus.title.effects.close]\nfade_us = \"300000\"\nstyle = {type = \"wipe\", direction = \"left_to_right\", softness = 0.2}\n");
    fs::write(path, text).unwrap();
    let loaded = load_project(d.path()).unwrap();
    for cap in ["ui.menu-effects.v1", "ui.menu-transition.v1"] {
        assert!(loaded.program.requires.iter().any(|c| c == cap), "{cap}");
    }
    compile(&loaded.program).unwrap();
    // Dropping the spatial style trims the reveal capability but keeps the
    // fade on the legacy path.
    let path = d.path().join("themes/rain/theme.toml");
    let text = fs::read_to_string(&path).unwrap().replace(
        "style = {type = \"wipe\", direction = \"left_to_right\", softness = 0.2}\n",
        "",
    );
    fs::write(path, text).unwrap();
    let trimmed = load_project(d.path()).unwrap();
    assert!(trimmed
        .program
        .requires
        .iter()
        .any(|c| c == "ui.menu-effects.v1"));
    assert!(!trimmed
        .program
        .requires
        .iter()
        .any(|c| c == "ui.menu-transition.v1"));
    compile(&trimmed.program).unwrap();
}

#[test]
fn menu_element_tween_capability_follows_element_animation_usage() {
    let d = project();
    // The stock theme declares no element animations.
    assert!(!load_project(d.path())
        .unwrap()
        .program
        .requires
        .iter()
        .any(|c| c == "ui.menu-element-tween.v1"));
    let path = d.path().join("themes/rain/theme.toml");
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str("\n[image_menus.title]\nbackground = \"bg.station\"\nbuttons = []\n[[image_menus.title.elements]]\nid = \"row\"\nrect = [0,0,300,80]\ncontent = { type = \"hit_region\", label = \"Row\", action = {type = \"close\"} }\n[image_menus.title.effects.enter]\nfade_us = \"100000\"\n[[image_menus.title.effects.elements]]\nelement = \"row\"\nproperty = \"offset_x\"\nfrom = -40.0\nduration_us = \"300000\"\n");
    fs::write(path, text).unwrap();
    let loaded = load_project(d.path()).unwrap();
    for cap in ["ui.menu-effects.v1", "ui.menu-element-tween.v1"] {
        assert!(loaded.program.requires.iter().any(|c| c == cap), "{cap}");
    }
    compile(&loaded.program).unwrap();
    // Dropping the animation trims the element capability but keeps the page
    // on the effects path.
    let path = d.path().join("themes/rain/theme.toml");
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace("\n[[image_menus.title.effects.elements]]\nelement = \"row\"\nproperty = \"offset_x\"\nfrom = -40.0\nduration_us = \"300000\"\n", "");
    fs::write(path, text).unwrap();
    let trimmed = load_project(d.path()).unwrap();
    assert!(trimmed
        .program
        .requires
        .iter()
        .any(|c| c == "ui.menu-effects.v1"));
    assert!(!trimmed
        .program
        .requires
        .iter()
        .any(|c| c == "ui.menu-element-tween.v1"));
    compile(&trimmed.program).unwrap();
}

#[test]
fn media_capabilities_follow_the_packaged_containers() {
    let d = project();
    let sdk = test_sdk();
    resolve(d.path(), sdk.path()).unwrap();
    // load_project seeds the whole capability list; packaging prunes the
    // container capabilities back to the bytes that actually ship.
    assert!(load_project(d.path())
        .unwrap()
        .program
        .requires
        .iter()
        .any(|c| c == "media.webp.v1"));
    let requires_of = |out: &Path, report: &BuildReport| -> Vec<String> {
        let release: nir_format::ReleaseManifest = serde_json::from_slice(
            &fs::read(out.join(format!("releases/{}.json", report.release))).unwrap(),
        )
        .unwrap();
        let bytes = fs::read(out.join(&release.objects[&release.program].path)).unwrap();
        let exe: nir_format::RuntimeExecutable = serde_json::from_slice(&bytes).unwrap();
        exe.program.requires
    };
    let out = d.path().join("dist/converted");
    let report = build_profile(
        d.path(),
        sdk.path(),
        &out,
        "dev",
        false,
        false,
        &OptimizeOptions::default(),
    )
    .unwrap();
    for cap in ["media.webp.v1", "media.mp3.v1"] {
        assert!(
            requires_of(&out, &report).iter().any(|c| c == cap),
            "{cap} missing after default packaging"
        );
    }
    let out = d.path().join("dist/plain");
    let report = build_profile(
        d.path(),
        sdk.path(),
        &out,
        "dev",
        false,
        false,
        &OptimizeOptions::none(),
    )
    .unwrap();
    for cap in ["media.webp.v1", "media.mp3.v1"] {
        assert!(
            !requires_of(&out, &report).iter().any(|c| c == cap),
            "{cap} declared without shipped objects"
        );
    }
}

#[test]
fn stack_and_reading_conditions_keep_required_capabilities_without_actions() {
    let d = project();
    assert!(!load_project(d.path())
        .unwrap()
        .program
        .requires
        .iter()
        .any(|c| c == "ui.menu-stack.v1"));
    let path = d.path().join("themes/rain/theme.toml");
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str("\n[image_menus.title]\nbackground = \"bg.station\"\nbuttons = []\n[[image_menus.title.elements]]\nid = \"list\"\nrect = [0,0,300,80]\nvisible_when = [{type = \"reading_available\", mode = \"auto\", available = false}]\ncontent = {type = \"stack\", gap = 10}\n");
    fs::write(path, text).unwrap();
    let loaded = load_project(d.path()).unwrap();
    for cap in [
        "ui.menu-stack.v1",
        "ui.menu-reading.v1",
        "ui.menu-services.v1",
        "ui.menu-state.v1",
    ] {
        assert!(loaded.program.requires.iter().any(|c| c == cap), "{cap}");
    }
    compile(&loaded.program).unwrap();
}

#[test]
fn text_button_project_round_trips_and_keeps_its_used_capability() {
    let d = project();
    assert!(!load_project(d.path())
        .unwrap()
        .program
        .requires
        .iter()
        .any(|c| c == "ui.menu-text-button.v1"));
    let path = d.path().join("themes/rain/theme.toml");
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str("\n[image_menus.title]\nbackground = \"bg.station\"\nbuttons = []\n[[image_menus.title.elements]]\nid = \"caption\"\nrect = [0,0,300,80]\ncontent = {type = \"text_button\", label = \"Action\", size = 26, color = [1,1,1,1], hover_color = [0,1,0,1], disabled_color = [0,0,0,1], action = {type = \"settings\"}}\n");
    fs::write(path, text).unwrap();
    let loaded = load_project(d.path()).unwrap();
    assert!(loaded
        .program
        .requires
        .iter()
        .any(|c| c == "ui.menu-text-button.v1"));
    compile(&loaded.program).unwrap();
}

#[test]
fn readonly_story_aliases_compile_and_require_only_used_capability() {
    let d = project();
    assert!(!load_project(d.path())
        .unwrap()
        .program
        .requires
        .iter()
        .any(|c| c == "ui.menu-story.v1"));
    let path = d.path().join("themes/rain/theme.toml");
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str("\n[image_menus.title]\nbackground = \"bg.station\"\nbuttons = []\nstory_exports = { count = \"affection\" }\n[[image_menus.title.elements]]\nid = \"entry\"\nrect = [0,0,300,80]\nvisible_when = [{type = \"story\", name = \"count\", equals = 0}]\ncontent = {type = \"hit_region\", label = \"Entry\", action = {type = \"settings\"}}\n");
    fs::write(&path, &text).unwrap();
    let loaded = load_project(d.path()).unwrap();
    assert!(loaded
        .program
        .requires
        .iter()
        .any(|c| c == "ui.menu-story.v1"));
    compile(&loaded.program).unwrap();
    fs::write(
        &path,
        text.replace("count = \"affection\"", "count = \"undeclared\""),
    )
    .unwrap();
    assert!(load_project(d.path())
        .and_then(|p| compile(&p.program))
        .is_err());
}

#[test]
fn continuous_history_capability_is_pruned_and_runtime_rechecks_contract() {
    let d = project();
    assert!(!load_project(d.path())
        .unwrap()
        .program
        .requires
        .iter()
        .any(|c| c == "ui.menu-history-flow.v1"));
    let path = d.path().join("themes/rain/theme.toml");
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str("\n[image_menus.title]\nbackground = \"bg.station\"\nbuttons = []\n[[image_menus.title.elements]]\nid = \"history\"\nrect = [40,40,600,360]\ncontent = {type = \"history_flow\", size = 24, line_height = 36, gap = 12, wheel_step = 72, page_step = 180, max_visible = 32, color = [1,1,1,1]}\n");
    fs::write(path, text).unwrap();
    let loaded = load_project(d.path()).unwrap();
    assert!(loaded
        .program
        .requires
        .iter()
        .any(|c| c == "ui.menu-history-flow.v1"));
    assert!(!loaded
        .program
        .requires
        .iter()
        .any(|c| c == "ui.menu-history.v1"));
    compile(&loaded.program).unwrap();
    let sdk = test_sdk();
    resolve(d.path(), sdk.path()).unwrap();
    let out = d.path().join("dist/history-flow");
    let report = build(d.path(), sdk.path(), &out, true).unwrap();
    let release: nir_format::ReleaseManifest = serde_json::from_slice(
        &fs::read(out.join(format!("releases/{}.json", report.release))).unwrap(),
    )
    .unwrap();
    let executable: nir_format::RuntimeExecutable = serde_json::from_slice(
        &fs::read(out.join(&release.objects[&release.program].path)).unwrap(),
    )
    .unwrap();
    nir_core::ValidatedProgram::from_runtime(executable.program.clone()).unwrap();
    let mut root = executable.program.clone();
    root.requires.retain(|c| c != "ui.menu-history-flow.v1");
    assert_eq!(
        nir_core::ValidatedProgram::from_runtime(root)
            .unwrap_err()
            .code,
        "E_CAPABILITY"
    );
    let mut root = executable.program;
    if let nir_format::MenuContent::HistoryFlow { max_visible, .. } =
        &mut root.theme.image_menus.get_mut("title").unwrap().elements[0].content
    {
        *max_visible = 0;
    }
    assert!(nir_core::ValidatedProgram::from_runtime(root).is_err());
}

#[test]
fn history_scrollbar_states_enter_asset_closure_and_runtime_rechecks_capability() {
    let d = project();
    assert!(!load_project(d.path())
        .unwrap()
        .program
        .requires
        .iter()
        .any(|c| c == "ui.menu-history-scrollbar.v1"));
    let path = d.path().join("themes/rain/theme.toml");
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str(r#"
[image_menus.title]
background = "bg.station"
buttons = []
[[image_menus.title.elements]]
id = "history"
rect = [40,40,600,360]
content = {type = "history_flow", size = 24, line_height = 36, gap = 12, wheel_step = 72, page_step = 180, max_visible = 32, color = [1,1,1,1]}
[[image_menus.title.elements]]
id = "scroll"
rect = [660,40,32,360]
content = {type = "history_scrollbar", window = "history", label = "History scroll", thumb_height = 24, arrow_height = 16, line_step = 36, track = {asset = "bg.station"}, thumb = {asset = "bg.station", hover_asset = "bg.river", pressed_asset = "actor.aki", disabled_asset = "bg.river"}, decrease = {asset = "bg.station"}, increase = {asset = "bg.station"}}
"#);
    fs::write(&path, &text).unwrap();
    let loaded = load_project(d.path()).unwrap();
    assert!(loaded
        .program
        .requires
        .iter()
        .any(|c| c == "ui.menu-history-scrollbar.v1"));
    let assets = loaded.program.theme.image_menus["title"].image_assets();
    for id in ["bg.station", "bg.river", "actor.aki"] {
        assert!(assets.contains(id));
    }
    compile(&loaded.program).unwrap();
    let sdk = test_sdk();
    resolve(d.path(), sdk.path()).unwrap();
    let out = d.path().join("dist/history-scrollbar");
    let report = build(d.path(), sdk.path(), &out, true).unwrap();
    let release: nir_format::ReleaseManifest = serde_json::from_slice(
        &fs::read(out.join(format!("releases/{}.json", report.release))).unwrap(),
    )
    .unwrap();
    let executable: nir_format::RuntimeExecutable = serde_json::from_slice(
        &fs::read(out.join(&release.objects[&release.program].path)).unwrap(),
    )
    .unwrap();
    nir_core::ValidatedProgram::from_runtime(executable.program.clone()).unwrap();
    let mut root = executable.program.clone();
    root.requires
        .retain(|c| c != "ui.menu-history-scrollbar.v1");
    assert_eq!(
        nir_core::ValidatedProgram::from_runtime(root)
            .unwrap_err()
            .code,
        "E_CAPABILITY"
    );
    let mut root = executable.program;
    if let nir_format::MenuContent::HistoryScrollbar { window, .. } =
        &mut root.theme.image_menus.get_mut("title").unwrap().elements[1].content
    {
        *window = "missing".into();
    }
    assert!(nir_core::ValidatedProgram::from_runtime(root).is_err());
    fs::write(
        &path,
        text.replace("hover_asset = \"bg.river\"", "hover_asset = \"audio.bgm\""),
    )
    .unwrap();
    assert!(load_project(d.path())
        .and_then(|p| compile(&p.program))
        .is_err());
}

#[test]
fn menu_navigation_prunes_unused_capability_and_runtime_rechecks_links() {
    let d = project();
    assert!(!load_project(d.path())
        .unwrap()
        .program
        .requires
        .iter()
        .any(|c| c == "ui.menu-navigation.v1"));
    let path = d.path().join("themes/rain/theme.toml");
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str(
        r#"
[image_menus.title]
background = "bg.station"
buttons = []
[[image_menus.title.elements]]
id = "child"
rect = [40,40,200,60]
content = {type = "hit_region", label = "Child", action = {type = "push_menu", menu = "child"}}
[image_menus.child]
background = "bg.river"
buttons = []
[[image_menus.child.elements]]
id = "back"
rect = [40,40,200,60]
content = {type = "hit_region", label = "Back", action = {type = "back"}}
"#,
    );
    fs::write(&path, text).unwrap();
    let loaded = load_project(d.path()).unwrap();
    assert!(loaded
        .program
        .requires
        .iter()
        .any(|c| c == "ui.menu-navigation.v1"));
    assert!(loaded
        .program
        .requires
        .iter()
        .any(|c| c == "ui.menu-services.v1"));
    compile(&loaded.program).unwrap();
    let sdk = test_sdk();
    resolve(d.path(), sdk.path()).unwrap();
    let out = d.path().join("dist/navigation");
    let report = build(d.path(), sdk.path(), &out, true).unwrap();
    let release: nir_format::ReleaseManifest = serde_json::from_slice(
        &fs::read(out.join(format!("releases/{}.json", report.release))).unwrap(),
    )
    .unwrap();
    let executable: nir_format::RuntimeExecutable = serde_json::from_slice(
        &fs::read(out.join(&release.objects[&release.program].path)).unwrap(),
    )
    .unwrap();
    nir_core::ValidatedProgram::from_runtime(executable.program.clone()).unwrap();
    let mut root = executable.program.clone();
    root.requires.retain(|c| c != "ui.menu-navigation.v1");
    assert_eq!(
        nir_core::ValidatedProgram::from_runtime(root)
            .unwrap_err()
            .code,
        "E_CAPABILITY"
    );
    let mut root = executable.program;
    if let nir_format::MenuContent::HitRegion { action, .. } =
        &mut root.theme.image_menus.get_mut("title").unwrap().elements[0].content
    {
        *action = nir_format::ImageMenuAction::PushMenu {
            menu: "missing".into(),
        };
    }
    assert!(nir_core::ValidatedProgram::from_runtime(root).is_err());
}

#[test]
fn history_availability_has_independent_source_and_runtime_capability() {
    let d = project();
    assert!(!load_project(d.path())
        .unwrap()
        .program
        .requires
        .iter()
        .any(|c| c == "ui.menu-history-availability.v1"));
    let path = d.path().join("themes/rain/theme.toml");
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str(
        r#"
[image_menus.title]
background = "bg.station"
builtin_navigation = false
buttons = []
[[image_menus.title.elements]]
id = "resume"
rect = [40,40,200,60]
enabled_when = [{type = "history_available", available = true}]
content = {type = "hit_region", label = "Resume", action = {type = "new_game"}}
"#,
    );
    fs::write(&path, text).unwrap();
    let loaded = load_project(d.path()).unwrap();
    for capability in [
        "ui.menu-state.v1",
        "ui.menu-services.v1",
        "ui.menu-history-availability.v1",
        "ui.menu-chrome.v1",
    ] {
        assert!(loaded.program.requires.iter().any(|c| c == capability));
    }
    assert!(!loaded
        .program
        .requires
        .iter()
        .any(|c| c == "ui.menu-history-flow.v1"));
    compile(&loaded.program).unwrap();
    for cap in ["ui.menu-history-availability.v1", "ui.menu-chrome.v1"] {
        let mut missing = loaded.program.clone();
        missing.requires.retain(|c| c != cap);
        assert!(compile(&missing).is_err());
    }
    let sdk = test_sdk();
    resolve(d.path(), sdk.path()).unwrap();
    let out = d.path().join("dist/history-availability");
    let report = build(d.path(), sdk.path(), &out, true).unwrap();
    let release: nir_format::ReleaseManifest = serde_json::from_slice(
        &fs::read(out.join(format!("releases/{}.json", report.release))).unwrap(),
    )
    .unwrap();
    let executable: nir_format::RuntimeExecutable = serde_json::from_slice(
        &fs::read(out.join(&release.objects[&release.program].path)).unwrap(),
    )
    .unwrap();
    nir_core::ValidatedProgram::from_runtime(executable.program.clone()).unwrap();
    for cap in ["ui.menu-history-availability.v1", "ui.menu-chrome.v1"] {
        let mut root = executable.program.clone();
        root.requires.retain(|c| c != cap);
        assert_eq!(
            nir_core::ValidatedProgram::from_runtime(root)
                .unwrap_err()
                .code,
            "E_CAPABILITY"
        );
    }
    assert!(!load_project(&source())
        .unwrap()
        .program
        .requires
        .iter()
        .any(|c| c == "ui.menu-chrome.v1"));
}

#[test]
fn offline_long_pcm_audio_reads_with_full_duration_and_unchanged_other_limits() {
    use std::io::Write;
    let d = project();
    let path = d.path().join("assets/source/bgm.wav");
    let payload = 72 * 1024 * 1024u32;
    let mut file = fs::File::create(&path).unwrap();
    let mut header = Vec::new();
    header.extend(b"RIFF");
    header.extend((payload + 36).to_le_bytes());
    header.extend(b"WAVEfmt ");
    header.extend(16u32.to_le_bytes());
    header.extend(1u16.to_le_bytes());
    header.extend(2u16.to_le_bytes());
    header.extend(48000u32.to_le_bytes());
    header.extend(192000u32.to_le_bytes());
    header.extend(4u16.to_le_bytes());
    header.extend(16u16.to_le_bytes());
    header.extend(b"data");
    header.extend(payload.to_le_bytes());
    file.write_all(&header).unwrap();
    file.set_len(payload as u64 + 44).unwrap();
    drop(file);
    let loaded = load_project(d.path()).unwrap();
    let audio = &loaded.program.assets["audio.bgm"];
    assert_eq!(audio.decoded_bytes, payload as u64 * 2);
    assert_eq!(
        audio.duration_us.0,
        (payload as u64 / 4) * 1_000_000 / 48_000
    );
    drop(loaded);
    fs::File::create(d.path().join("assets/source/station.png"))
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    let error = load_project(d.path()).err().unwrap().to_string();
    assert!(
        error.contains("E_LIMIT") && error.contains("station.png"),
        "{error}"
    );
}

#[test]
fn offline_audio_source_above_pcm_and_header_budget_is_rejected_before_read() {
    let d = project();
    fs::File::create(d.path().join("assets/source/bgm.wav"))
        .unwrap()
        .set_len(128 * 1024 * 1024 + 64 * 1024 + 1)
        .unwrap();
    let error = load_project(d.path()).err().unwrap().to_string();
    assert!(
        error.contains("E_LIMIT") && error.contains("bgm.wav"),
        "{error}"
    );
}
