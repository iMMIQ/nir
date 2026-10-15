use nir_compiler::*;
use nir_core::{Core, CoreInput, ValidatedProgram};
use nir_format::Value;
use serde_json::{json, Value as Json};
use std::{fs, path::Path};

fn copy_tree(src: &Path, dst: &Path) {
    nir_compiler::copy_tree(src, dst).unwrap();
}

fn project() -> tempfile::TempDir {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/rain-letters");
    let dir = tempfile::tempdir().unwrap();
    copy_tree(&source, dir.path());
    dir
}

fn test_sdk() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
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
        fs::write(dir.path().join(name), format!("packaging fixture: {name}")).unwrap();
    }
    dir
}

fn module_toml(id: &str) -> String {
    format!(
        "id = \"{id}\"\nmodule_format = 1\nsources = [\"story.nir.json\"]\ntext_contracts = \"texts/contracts.json\"\ntext_revisions = \"texts/revisions.json\"\n\n[exports]\nstart = \"main\"\n\n[text_bundles]\nen = \"texts/en.json\"\nzh-Hans = \"texts/zh-Hans.json\"\n"
    )
}

fn add_second_module(root: &Path) {
    let game_path = root.join("game.toml");
    let game = fs::read_to_string(&game_path).unwrap();
    let game = game
        .replace(
            "modules = [\"content/ch01/module.toml\"]",
            "shared = [\"content/common/shared.nir.json\"]\nmodules = [\"content/ch01/module.toml\", \"content/ch02/module.toml\"]",
        )
        .replace("title_scene = \"station\"", "title_scene = \"ch01.station\"");
    fs::write(game_path, game).unwrap();

    let ch01_story_path = root.join("content/ch01/story.nir.json");
    let mut ch01: Json = serde_json::from_slice(&fs::read(&ch01_story_path).unwrap()).unwrap();
    let shared = json!({
        "fragment_format": 1,
        "variables": ch01["variables"].clone(),
    });
    let shared_dir = root.join("content/common");
    fs::create_dir_all(&shared_dir).unwrap();
    fs::write(
        shared_dir.join("shared.nir.json"),
        serde_json::to_vec_pretty(&shared).unwrap(),
    )
    .unwrap();
    ch01.as_object_mut().unwrap().remove("variables");
    ch01["functions"] = json!({
        "main": {
            "entry": "start",
            "blocks": {
                "start": {
                    "ops": [],
                    "terminator": {
                        "type": "call",
                        "function": "helper",
                        "args": {"x": {"type": "const", "value": {"type": "i32", "value": 7}}},
                        "next": "cross",
                        "result": "affection"
                    }
                },
                "cross": {
                    "ops": [],
                    "terminator": {
                        "type": "call",
                        "function": "ch02.start",
                        "args": {},
                        "next": "end",
                        "result": "affection"
                    }
                },
                "end": {"ops": [], "terminator": {"type": "end", "outcome": "complete"}}
            }
        },
        "helper": {
            "params": {"x": "i32"},
            "returns": "i32",
            "entry": "return",
            "blocks": {
                "return": {
                    "ops": [],
                    "terminator": {"type": "return", "value": {"type": "var", "name": "x"}}
                }
            }
        }
    });
    fs::write(&ch01_story_path, serde_json::to_vec_pretty(&ch01).unwrap()).unwrap();

    let ch02_dir = root.join("content/ch02");
    fs::create_dir_all(ch02_dir.join("texts")).unwrap();
    let mut ch02 = ch01.clone();
    ch02["functions"] = json!({
        "main": {
            "returns": "i32",
            "entry": "return",
            "blocks": {
                "return": {
                    "ops": [],
                    "terminator": {"type": "return", "value": {"type": "const", "value": {"type": "i32", "value": 42}}}
                }
            }
        }
    });
    fs::write(
        ch02_dir.join("story.nir.json"),
        serde_json::to_vec_pretty(&ch02).unwrap(),
    )
    .unwrap();
    fs::copy(
        root.join("content/ch01/texts/contracts.json"),
        ch02_dir.join("texts/contracts.json"),
    )
    .unwrap();
    fs::copy(
        root.join("content/ch01/texts/revisions.json"),
        ch02_dir.join("texts/revisions.json"),
    )
    .unwrap();
    for locale in ["en", "zh-Hans"] {
        fs::copy(
            root.join(format!("content/ch01/texts/{locale}.json")),
            ch02_dir.join(format!("texts/{locale}.json")),
        )
        .unwrap();
    }
    fs::write(ch02_dir.join("module.toml"), module_toml("ch02")).unwrap();
}

#[test]
fn assembles_namespaced_modules_and_links_export_calls_with_shared_state() {
    let project = project();
    add_second_module(project.path());

    let loaded = load_project(project.path()).unwrap();
    let p = &loaded.program;
    assert_eq!(p.entry, "ch01.main");
    assert_eq!(p.variables.len(), 1);
    assert_eq!(p.variables["affection"], Value::I32(0));
    assert!(p.functions.contains_key("ch01.helper"));
    assert!(p.functions.contains_key("ch02.main"));
    assert!(matches!(
        &p.functions["ch01.main"].blocks["cross"].terminator,
        nir_format::Terminator::Call { function, result, .. }
            if function == "ch02.main" && result.as_deref() == Some("affection")
    ));
    assert!(p.texts.contains_key("ch01.arrival"));
    assert!(p.texts.contains_key("ch02.arrival"));
    assert!(p.modules["ch01"].texts.contains("ch01.arrival"));
    assert!(p.modules["ch02"].texts.contains("ch02.arrival"));
    assert_eq!(p.modules["ch01"].functions.len(), 2);
    assert_eq!(p.modules["ch02"].functions.len(), 1);
    assert!(p
        .modules
        .values()
        .all(|m| m.code.is_empty() && m.locales.is_empty()));

    let mut core = Core::new(
        ValidatedProgram::new(p.clone()).unwrap(),
        "multi-test".into(),
        "en".into(),
    )
    .unwrap();
    assert_eq!(core.state().variables["affection"], Value::I32(0));
    core.step(CoreInput::Time { delta_us: 0 }, 10_000);
    assert_eq!(core.state().outcome.as_deref(), Some("complete"));
    assert_eq!(core.state().variables["affection"], Value::I32(42));
}

#[test]
fn module_audio_references_survive_nested_composition_and_runtime_packaging() {
    for namespaced in [false, true] {
        let project = project();
        if namespaced {
            add_second_module(project.path());
        }
        let story = project.path().join("content/ch01/story.nir.json");
        let mut fragment: Json = serde_json::from_slice(&fs::read(&story).unwrap()).unwrap();
        fragment["cues"]["stop_music"] = json!({"effects": [{
            "id": "stop-chain", "scope": "session", "effect": {
                "type": "sequence", "children": [{
                    "id": "stop-group", "scope": "session", "effect": {
                        "type": "parallel_all", "children": [{
                            "id": "stop-child", "scope": "session", "effect": {
                                "type": "audio_stop", "target": "music", "duration_us": "10000"
                            }
                        }]
                    }
                }]
            }
        }]});
        let fade = json!({
            "id": "fade", "scope": "session", "effect": {
                "type": "tween", "target": {"type": "audio_instance", "task": "music", "property": "gain"},
                "to": 0.5, "duration_us": "10000"
            }
        });
        fragment["cues"]["fade_music"] = json!({"effects": [fade.clone()]});
        fragment["cues"]["fade_composed"] = json!({"effects": [{
            "id": "fade-chain", "scope": "session", "effect": {
                "type": "parallel_all", "children": [{
                    "id": "fade-group", "scope": "session", "effect": {
                        "type": "sequence", "children": [fade]
                    }
                }]
            }
        }]});
        fs::write(&story, serde_json::to_vec(&fragment).unwrap()).unwrap();

        let loaded = load_project(project.path()).unwrap();
        compile(&loaded.program).unwrap();
        let prefix = if namespaced { "ch01." } else { "" };
        let cue = &loaded.program.cues[&format!("{prefix}stop_music")];
        let children = cue.effects[0].effect.compose_children().unwrap();
        let child = &children[0].effect.compose_children().unwrap()[0];
        assert_eq!(child.id, format!("{prefix}stop-child"));
        assert!(
            matches!(&child.effect, nir_format::Effect::AudioStop { target, .. }
            if target == &format!("{prefix}music"))
        );

        let sdk = test_sdk();
        build_profile(
            project.path(),
            sdk.path(),
            &project.path().join("dist/audio-composition"),
            "dev",
            false,
            false,
            &OptimizeOptions::none(),
        )
        .unwrap();
    }
}

#[test]
fn published_dialogue_images_keep_module_catalogs_and_load_into_runtime() {
    let project = project();
    add_second_module(project.path());
    let catalog_path = project.path().join("assets/catalog.toml");
    let mut catalog = fs::read_to_string(&catalog_path).unwrap();
    for asset in ["mask.only", "portrait.only", "window.only"] {
        catalog.push_str(&format!("\n[[assets]]\nid = \"{asset}\"\nkind = \"image\"\nsource = \"source/station.png\"\nrights = \"CC0-1.0\"\nexpected_size = [1280, 720]\n"));
    }
    fs::write(catalog_path, catalog).unwrap();
    let theme_path = project.path().join("themes/rain/theme.toml");
    let mut theme = fs::read_to_string(&theme_path).unwrap();
    theme.push_str("\n[dialogue_styles.alternate]\ntext = [1.0, 1.0, 1.0, 1.0]\n[dialogue_styles.alternate.dialogue]\nbackground = \"window.only\"\n");
    fs::write(theme_path, theme).unwrap();
    let story = project.path().join("content/ch02/story.nir.json");
    let mut fragment: Json = serde_json::from_slice(&fs::read(&story).unwrap()).unwrap();
    fragment["functions"]["main"]["blocks"]["return"]["ops"] = json!([{
        "id": "window.reveal", "operation": {
            "type": "dialogue_visibility", "visible": false,
            "transition": {"type": "mask", "asset": "mask.only", "channel": "alpha"},
            "duration_us": "500000"
        }
    }]);
    fragment["cues"]["style"] = json!({"effects": [
        {"id": "style", "scope": "session", "effect": {"type": "dialogue_style", "style": "alternate"}}
    ]});
    fragment["cues"]["decorate"] = json!({"effects": [
        {"id": "portrait", "scope": "session", "effect": {
            "type": "dialogue_decoration", "slot": "portrait", "image": {
                "asset": "portrait.only", "size": [1280, 720],
                "placement": {"type": "absolute", "point": [10, 350]}
            }
        }}
    ]});
    fs::write(story, serde_json::to_vec(&fragment).unwrap()).unwrap();
    let sdk = test_sdk();
    let out = project.path().join("dist/dialogue-images");
    let report = build_profile(
        project.path(),
        sdk.path(),
        &out,
        "dev",
        false,
        false,
        &OptimizeOptions::none(),
    )
    .unwrap();
    let release: nir_format::ReleaseManifest = serde_json::from_slice(
        &fs::read(out.join(format!("releases/{}.json", report.release))).unwrap(),
    )
    .unwrap();
    let executable: nir_format::RuntimeExecutable = serde_json::from_slice(
        &fs::read(out.join(&release.objects[&release.program].path)).unwrap(),
    )
    .unwrap();
    let root = &executable.program;
    for asset in ["mask.only", "portrait.only", "window.only"] {
        let catalog = &root.assets[asset].catalog;
        assert!(report.asset_catalogs[catalog]
            .consumers
            .contains(&"module:ch02".to_owned()));
        assert!(!report.asset_catalogs[catalog]
            .consumers
            .contains(&"module:ch01".to_owned()));
    }
    for asset in ["mask.only", "portrait.only"] {
        assert_eq!(
            report.asset_catalogs[&root.assets[asset].catalog].consumers,
            ["module:ch02"]
        );
    }
    let mut keys = root.asset_catalogs(root.assets.keys().map(String::as_str));
    for (module, index) in &root.modules {
        keys.insert(nir_format::ContentKey::Static {
            module: module.clone(),
        });
        keys.insert(nir_format::ContentKey::Code {
            module: module.clone(),
        });
        keys.extend(
            index
                .locales
                .keys()
                .map(|locale| nir_format::ContentKey::Text {
                    module: module.clone(),
                    locale: locale.clone(),
                }),
        );
    }
    let batch = keys
        .into_iter()
        .map(|key| {
            let requirement = root.content_requirement(&key).unwrap();
            let bytes = fs::read(out.join(&release.objects[&requirement.digest].path)).unwrap();
            let object = nir_content::parse_runtime_object(root, &key, &bytes).unwrap();
            (key, object, bytes.len() as u64)
        })
        .collect();
    let view = ValidatedProgram::from_runtime(root.clone())
        .unwrap()
        .install_batch(batch)
        .unwrap();
    assert!(view.window_transition_assets().contains("mask.only"));
    assert!(view.asset("portrait.only").is_some());
}

#[test]
fn cross_module_calls_cannot_target_private_or_missing_exports() {
    let project = project();
    add_second_module(project.path());
    let story = project.path().join("content/ch01/story.nir.json");
    let mut fragment: Json = serde_json::from_slice(&fs::read(&story).unwrap()).unwrap();
    fragment["functions"]["main"]["blocks"]["cross"]["terminator"]["function"] =
        json!("ch02.private");
    fs::write(&story, serde_json::to_vec_pretty(&fragment).unwrap()).unwrap();
    assert!(load_project(project.path())
        .unwrap_err()
        .to_string()
        .contains("E_EXPORT"));
}

#[test]
fn module_variables_must_be_defined_in_shared_fragment() {
    let project = project();
    add_second_module(project.path());
    let story = project.path().join("content/ch02/story.nir.json");
    let mut fragment: Json = serde_json::from_slice(&fs::read(&story).unwrap()).unwrap();
    fragment["variables"] = json!({"only_ch02": {"type": "bool", "value": false}});
    fs::write(&story, serde_json::to_vec_pretty(&fragment).unwrap()).unwrap();
    assert!(load_project(project.path())
        .unwrap_err()
        .to_string()
        .contains("E_SHARED_VARIABLE"));
}

#[test]
fn text_revision_commands_route_namespaced_ids_to_their_module() {
    let project = project();
    add_second_module(project.path());
    let root = project.path();
    let before = load_project(root).unwrap();
    let old = before.program.texts["ch02.arrival"].contract_digest.clone();

    let path = root.join("content/ch02/texts/zh-Hans.json");
    let mut source: Json = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    source["arrival"]["spans"][0]["text"] = json!("还是你来了。");
    fs::write(&path, serde_json::to_vec_pretty(&source).unwrap()).unwrap();
    assert!(text_status(root)
        .unwrap()
        .issues
        .iter()
        .any(|issue| { issue.code == "E_TEXT_SOURCE_CHANGED" && issue.text_id == "ch02.arrival" }));
    text_update(root, "ch02.arrival", false).unwrap();
    assert!(text_status(root)
        .unwrap()
        .issues
        .iter()
        .any(|issue| { issue.code == "E_TRANSLATION_STALE" && issue.text_id == "ch02.arrival" }));
    text_review(root, "ch02.arrival", "en").unwrap();

    let after = load_project(root).unwrap();
    assert_eq!(after.program.texts["ch02.arrival"].contract_digest, old);
    assert_eq!(after.program.texts["ch02.arrival"].source_revision, 2);
    assert_eq!(after.program.texts["ch01.arrival"].source_revision, 1);
    assert!(text_status(root).unwrap().ready);
}

#[test]
fn release_reports_cyclic_module_closure_and_exact_shared_catalog_consumers() {
    let project = project();
    add_second_module(project.path());

    // Export a helper from ch01, then make ch02 call it. The existing ch01 ->
    // ch02 call now forms a cycle in the module dependency graph.
    let ch01_module = project.path().join("content/ch01/module.toml");
    let text = fs::read_to_string(&ch01_module)
        .unwrap()
        .replace("start = \"main\"", "start = \"main\"\nhelper = \"helper\"");
    fs::write(ch01_module, text).unwrap();

    let ch02_story = project.path().join("content/ch02/story.nir.json");
    let mut ch02: Json = serde_json::from_slice(&fs::read(&ch02_story).unwrap()).unwrap();
    ch02["functions"]["main"] = json!({
        "returns": "i32",
        "entry": "call_ch01",
        "blocks": {
            "call_ch01": {
                "ops": [],
                "terminator": {
                    "type": "call",
                    "function": "ch01.helper",
                    "args": {"x": {"type": "const", "value": {"type": "i32", "value": 1}}},
                    "next": "return",
                    "result": "affection"
                }
            },
            "return": {
                "ops": [],
                "terminator": {"type": "return", "value": {"type": "var", "name": "affection"}}
            }
        }
    });
    fs::write(&ch02_story, serde_json::to_vec_pretty(&ch02).unwrap()).unwrap();

    let sdk = test_sdk();
    let out = project.path().join("dist/cyclic");
    let build = build(project.path(), sdk.path(), &out, false).unwrap();
    let dependencies: Json = serde_json::from_slice(
        &fs::read(project.path().join("reports/dependencies.json")).unwrap(),
    )
    .unwrap();

    assert_eq!(dependencies["module_dependencies"]["ch01"], json!(["ch02"]));
    assert_eq!(dependencies["module_dependencies"]["ch02"], json!(["ch01"]));
    let entry = &dependencies["entry_reachable"]["en"]["en"];
    let entry_objects = entry["objects"].as_object().unwrap();
    assert!(entry_objects.len() >= 2);
    assert_eq!(
        entry["object_count"].as_u64().unwrap() as usize,
        entry_objects.len()
    );
    assert_eq!(
        entry["object_bytes"].as_u64().unwrap(),
        entry_objects
            .values()
            .map(|object| object["bytes"].as_u64().unwrap())
            .sum::<u64>()
    );

    let release: nir_format::ReleaseManifest = serde_json::from_slice(
        &fs::read(out.join(format!("releases/{}.json", build.release))).unwrap(),
    )
    .unwrap();
    let executable: nir_format::RuntimeExecutable = serde_json::from_slice(
        &fs::read(out.join(&release.objects[&release.program].path)).unwrap(),
    )
    .unwrap();
    let title_catalog = &executable.program.assets["bg.station"].catalog;
    assert_eq!(
        build.asset_catalogs[title_catalog].consumers,
        ["bootstrap", "module:ch01", "module:ch02"]
    );
}
