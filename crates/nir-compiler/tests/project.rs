use nir_compiler::*;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
fn source() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/rain-letters")
}
fn project() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    copy_tree(&source(), d.path()).unwrap();
    d
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
fn release_splits_module_code_and_text_without_cross_locale_invalidation() {
    let d = project();
    let sdk = test_sdk();
    resolve(d.path(), sdk.path()).unwrap();

    let first_out = d.path().join("dist/first");
    let first = build(d.path(), sdk.path(), &first_out, true).unwrap();
    let first_release: nir_format::ReleaseManifest = serde_json::from_slice(
        &fs::read(first_out.join(format!("releases/{}.json", first.release))).unwrap(),
    )
    .unwrap();
    let first_exe: nir_format::Executable = serde_json::from_slice(
        &fs::read(first_out.join(&first_release.objects[&first_release.program].path)).unwrap(),
    )
    .unwrap();
    nir_content::validate_executable(&first_exe).unwrap();
    assert!(first_exe.program.functions.is_empty());
    assert!(first_exe.program.locales.values().all(BTreeMap::is_empty));
    let module_id = first_exe.program.modules.keys().next().unwrap().clone();
    let first_index = &first_exe.program.modules[&module_id];
    let first_code = first_index.code.clone();
    let first_zh = first_index.locales["zh-Hans"].clone();
    let first_en = first_index.locales["en"].clone();
    for hash in [&first_code, &first_zh, &first_en] {
        let object = &first_release.objects[hash];
        nir_content::verify(&fs::read(first_out.join(&object.path)).unwrap(), hash).unwrap();
    }
    let module_code: nir_format::ModuleCode = serde_json::from_slice(
        &fs::read(first_out.join(&first_release.objects[&first_code].path)).unwrap(),
    )
    .unwrap();
    assert_eq!(module_code.module, module_id);
    assert_eq!(module_code.functions.len(), first_index.functions.len());

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
    let second_exe: nir_format::Executable = serde_json::from_slice(
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
    for messages in [
        include_str!("../../nir-presentation/messages/en.ftl"),
        include_str!("../../nir-presentation/messages/zh-Hans.ftl"),
    ] {
        for c in messages.chars().filter(|c| !c.is_whitespace()) {
            assert!(face.glyph_index(c).is_some(), "missing UI glyph: {c}");
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
