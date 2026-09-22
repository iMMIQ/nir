use nir_compiler::*;
use nir_format::*;
use serde_json::{json, Value as Json};
use std::{
    fs,
    path::{Path, PathBuf},
};
fn source() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../templates/minimal")
}
fn project() -> tempfile::TempDir {
    let p = tempfile::tempdir().unwrap();
    copy_tree(&source(), p.path()).unwrap();
    p
}
fn file(root: &Path, name: &str) -> PathBuf {
    root.join("content/main/texts").join(name)
}
fn read(root: &Path, name: &str) -> Json {
    serde_json::from_slice(&fs::read(file(root, name)).unwrap()).unwrap()
}
fn edit(root: &Path, name: &str, f: impl FnOnce(&mut Json)) {
    let mut j = read(root, name);
    f(&mut j);
    fs::write(file(root, name), serde_json::to_vec_pretty(&j).unwrap()).unwrap();
}
fn codes(root: &Path) -> Vec<String> {
    text_status(root)
        .unwrap()
        .issues
        .into_iter()
        .map(|i| i.code)
        .collect()
}
fn source_edit(root: &Path) {
    edit(root, "zh-Hans.json", |j| {
        j["intro"]["spans"][0]["text"] = json!("春天的花开了。")
    });
}
#[test]
fn copy_edit_requires_source_update_then_explicit_translation_review() {
    let p = project();
    let r = p.path();
    let before = load_project(r).unwrap();
    source_edit(r);
    assert!(codes(r).contains(&"E_TEXT_SOURCE_CHANGED".into()));
    let d = diagnostic(&load_project(r).unwrap_err());
    assert_eq!(d.code, "E_TEXT_SOURCE_CHANGED");
    let at = d.details.unwrap().source.unwrap();
    assert_eq!(at.file, "content/main/texts/zh-Hans.json");
    assert_eq!(at.pointer, "/intro");
    assert!(at.line > 1);
    assert!(text_review(r, "intro", "en").is_err());
    text_update(r, "intro", false).unwrap();
    let c = &read(r, "contracts.json")["intro"];
    assert_eq!(c["source_revision"], 2);
    assert_eq!(c["contract_revision"], 1);
    assert_eq!(c["meaning_revision"], 1);
    assert!(codes(r).contains(&"E_TRANSLATION_STALE".into()));
    // Hand-copying version numbers never substitutes for explicit review.
    edit(r, "en.json", |j| j["intro"]["source_revision"] = json!(2));
    assert!(codes(r).contains(&"E_TRANSLATION_UNREVIEWED".into()));
    text_review(r, "intro", "en").unwrap();
    assert!(text_status(r).unwrap().ready);
    let after = load_project(r).unwrap();
    assert_ne!(before.program.revision, after.program.revision);
    assert_eq!(
        before.program.texts["intro"].contract_digest,
        after.program.texts["intro"].contract_digest
    );
    assert_eq!(
        before.program.texts["intro"].meaning_revision,
        after.program.texts["intro"].meaning_revision
    );
    let snapshot = fs::read(file(r, "revisions.json")).unwrap();
    text_update(r, "intro", false).unwrap();
    text_review(r, "intro", "en").unwrap();
    assert_eq!(snapshot, fs::read(file(r, "revisions.json")).unwrap());
    edit(r, "en.json", |j| {
        j["intro"]["spans"][0]["text"] = json!("A revised translation.")
    });
    assert!(codes(r).contains(&"E_TRANSLATION_UNREVIEWED".into()));
    text_review(r, "intro", "en").unwrap();
    assert!(text_status(r).unwrap().ready);
}
#[test]
fn semantic_contract_changes_force_bump_and_reject_gate_and_parameter_mismatch() {
    let p = project();
    let r = p.path();
    edit(r, "contracts.json", |j| {
        j["intro"]["gates"] = json!(["one", "two"])
    });
    edit(r, "zh-Hans.json", |j| {
        j["intro"]["spans"].as_array_mut().unwrap().extend([
            json!({"type":"gate","id":"one"}),
            json!({"type":"gate","id":"two"}),
        ]);
    });
    assert!(format!("{:#}", text_update(r, "intro", false).unwrap_err()).contains("E_TEXT_MEANING"));
    text_update(r, "intro", true).unwrap();
    assert_eq!(read(r, "contracts.json")["intro"]["meaning_revision"], 2);
    assert!(format!("{:#}", text_review(r, "intro", "en").unwrap_err()).contains("E_GATE"));
    edit(r, "en.json", |j| {
        j["intro"]["spans"].as_array_mut().unwrap().extend([
            json!({"type":"gate","id":"two"}),
            json!({"type":"gate","id":"one"}),
        ]);
    });
    assert!(text_review(r, "intro", "en").is_err());
    edit(r, "en.json", |j| {
        j["intro"]["spans"].as_array_mut().unwrap().swap(1, 2)
    });
    text_review(r, "intro", "en").unwrap();
    assert!(text_status(r).unwrap().ready);
    edit(r, "contracts.json", |j| {
        j["intro"]["params"] = json!({"name":"string"})
    });
    assert!(text_update(r, "intro", true).is_err());
    edit(r, "zh-Hans.json", |j| {
        j["intro"]["spans"]
            .as_array_mut()
            .unwrap()
            .push(json!({"type":"param","id":"name","name":"name"}))
    });
    text_update(r, "intro", true).unwrap();
    assert!(text_review(r, "intro", "en").is_err());
    edit(r, "en.json", |j| {
        j["intro"]["spans"]
            .as_array_mut()
            .unwrap()
            .push(json!({"type":"param","id":"name","name":"name"}))
    });
    text_review(r, "intro", "en").unwrap();
    // Type change invalidates review even when Span names/Gate order are unchanged.
    edit(r, "contracts.json", |j| {
        j["intro"]["params"]["name"] = json!("i32")
    });
    assert!(text_update(r, "intro", false).is_err());
    text_update(r, "intro", true).unwrap();
    assert!(codes(r).contains(&"E_TRANSLATION_STALE".into()));
}
#[test]
fn report_collects_missing_extra_and_unknown_fields_fail_closed() {
    let p = project();
    let r = p.path();
    edit(r, "en.json", |j| {
        j.as_object_mut().unwrap().remove("intro");
        j["extra"] = j["garden"].clone();
    });
    let status = text_status(r).unwrap();
    assert!(!status.ready);
    assert!(codes(r).contains(&"E_TRANSLATION_MISSING".into()));
    assert!(codes(r).contains(&"E_TEXT_CONTRACT".into()));
    edit(r, "contracts.json", |j| {
        j["intro"]["unsupported"] = json!(true)
    });
    assert!(text_status(r).is_err());
}
#[test]
fn new_text_and_explicit_meaning_bump_without_byte_changes() {
    let p = project();
    let r = p.path();
    edit(
        r,
        "contracts.json",
        |j| j["new"] = json!({"source_revision":1,"contract_revision":1,"meaning_revision":1}),
    );
    edit(r, "zh-Hans.json", |j| j["new"] = j["intro"].clone());
    edit(r, "en.json", |j| j["new"] = j["intro"].clone());
    text_update(r, "new", false).unwrap();
    assert!(!text_status(r).unwrap().ready);
    text_review(r, "new", "en").unwrap();
    assert!(text_status(r).unwrap().ready);
    text_update(r, "new", true).unwrap();
    assert_eq!(read(r, "contracts.json")["new"]["meaning_revision"], 2);
    assert!(!text_status(r).unwrap().ready);
    assert!(text_update(r, "absent", true).is_err());
    assert!(text_review(r, "new", "zh-Hans").is_err());
    assert!(text_review(r, "new", "unknown").is_err());
}
#[test]
fn runtime_rechecks_digests_and_semantic_read_identity_survives_copy_edits() {
    let p = project();
    let r = p.path();
    let original = load_project(r).unwrap().program;
    assert_eq!(original.format, 1);
    let mut legacy = serde_json::to_value(&original).unwrap();
    let contract = legacy["texts"]["intro"].as_object_mut().unwrap();
    for field in [
        "source_revision",
        "contract_revision",
        "meaning_revision",
        "contract_digest",
    ] {
        contract.remove(field);
    }
    contract.insert("revision".into(), json!(1));
    assert!(serde_json::from_value::<Program>(legacy).is_err());
    let mut tampered = original.clone();
    tampered.texts.get_mut("intro").unwrap().meaning_revision += 1;
    assert_eq!(
        nir_core::ValidatedProgram::new(tampered).unwrap_err().code,
        "E_TEXT_REVISION"
    );
    let mut tampered = original.clone();
    tampered
        .locales
        .get_mut("en")
        .unwrap()
        .get_mut("intro")
        .unwrap()
        .contract_digest = "0".repeat(64);
    assert!(nir_core::ValidatedProgram::new(tampered).is_err());
    let read_key = |p: Program| {
        let mut core = nir_core::Core::new(
            nir_core::ValidatedProgram::new(p).unwrap(),
            "release".into(),
            "zh-Hans".into(),
        )
        .unwrap();
        for seq in 1..20 {
            let input = if let Some(p) = &core.state().pending {
                nir_core::CoreInput::Prepared { activation: p.id }
            } else if let Some((_, d)) = core.dialogue() {
                nir_core::CoreInput::Advance {
                    interaction: d.interaction,
                    sequence: seq,
                }
            } else {
                nir_core::CoreInput::None
            };
            for intent in core.step(input, 100).intents {
                if let nir_core::CoreIntent::ProfileMerge { key } = intent {
                    return key;
                }
            }
        }
        panic!("did not finish intro")
    };
    let key = read_key(original.clone());
    source_edit(r);
    text_update(r, "intro", false).unwrap();
    text_review(r, "intro", "en").unwrap();
    assert_eq!(key, read_key(load_project(r).unwrap().program));
    text_update(r, "intro", true).unwrap();
    text_review(r, "intro", "en").unwrap();
    assert_ne!(key, read_key(load_project(r).unwrap().program));
    let core = nir_core::Core::new(
        nir_core::ValidatedProgram::new(original.clone()).unwrap(),
        "old-release".into(),
        "zh-Hans".into(),
    )
    .unwrap();
    let mut snapshot = core.state().clone();
    snapshot.format = 99;
    assert!(nir_core::Core::restore(
        nir_core::ValidatedProgram::new(original).unwrap(),
        snapshot,
        "old-release"
    )
    .is_err());
}
#[test]
fn transaction_recovery_is_explicit_and_preserves_concurrent_edits() {
    let p = project();
    let r = p.path();
    fs::create_dir_all(r.join(".nir")).unwrap();
    let path = file(r, "en.json");
    let before = fs::read(&path).unwrap();
    edit(r, "en.json", |j| {
        j["intro"]["spans"][0]["text"] = json!("Changed")
    });
    let after = fs::read(&path).unwrap();
    let journal = r.join(".nir/text-transaction.json");
    fs::write(
        &journal,
        serde_json::to_vec(
            &json!([{"path":"content/main/texts/en.json","before":String::from_utf8(before.clone()).unwrap(),"after":String::from_utf8(after.clone()).unwrap()}]),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(format!("{:#}", text_status(r).unwrap_err()).contains("E_TEXT_TRANSACTION"));
    fs::write(&path, b"external edit").unwrap();
    assert!(text_recover(r).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"external edit");
    fs::write(&path, after).unwrap();
    text_recover(r).unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(text_status(r).unwrap().ready);
}
fn make_legacy(r: &Path) {
    for name in ["contracts.json", "zh-Hans.json", "en.json"] {
        edit(r, name, |j| {
            for v in j.as_object_mut().unwrap().values_mut() {
                let o = v.as_object_mut().unwrap();
                let rev = o.remove("source_revision").unwrap();
                o.remove("contract_revision");
                o.remove("meaning_revision");
                o.insert("revision".into(), rev);
            }
        });
    }
    fs::remove_file(file(r, "revisions.json")).unwrap();
    let path = r.join("content/main/module.toml");
    let text = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .filter(|s| !s.starts_with("text_revisions"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(path, text).unwrap();
}
#[test]
fn migration_preserves_source_ids_and_meaning_and_requires_a_new_destination() {
    let p = project();
    let r = p.path();
    make_legacy(r);
    let before = fs::read(file(r, "zh-Hans.json")).unwrap();
    assert!(format!("{:#}", load_project(r).unwrap_err()).contains("E_TEXT_MIGRATION"));
    let target = tempfile::tempdir().unwrap();
    let out = target.path().join("migrated");
    text_migrate(r, &out).unwrap();
    assert_eq!(before, fs::read(file(r, "zh-Hans.json")).unwrap());
    assert!(text_status(&out).unwrap().ready);
    assert_eq!(test_project(&out).unwrap(), ["garden", "home"]);
    assert_eq!(read(&out, "contracts.json")["intro"]["meaning_revision"], 1);
    assert!(!out.join("game.lock").exists());
    assert!(text_migrate(r, &out).is_err());
    assert!(text_migrate(r, &r.join("child")).is_err());
    edit(r, "en.json", |j| j["intro"]["revision"] = json!(7));
    let bad = target.path().join("bad");
    assert!(text_migrate(r, &bad).is_err());
    assert!(!bad.exists());
}

#[test]
fn forged_manual_versions_and_overflow_never_partially_write() {
    let p = project();
    let r = p.path();
    source_edit(r);
    edit(r, "contracts.json", |j| {
        j["intro"]["source_revision"] = json!(2)
    });
    assert!(text_update(r, "intro", false).is_err());
    assert!(!text_status(r).unwrap().ready);
    edit(r, "contracts.json", |j| {
        j["intro"]["source_revision"] = json!(u32::MAX)
    });
    edit(r, "zh-Hans.json", |j| {
        j["intro"]["source_revision"] = json!(u32::MAX)
    });
    edit(r, "revisions.json", |j| {
        j["texts"]["intro"]["source_revision"] = json!(u32::MAX)
    });
    let contract = fs::read(file(r, "contracts.json")).unwrap();
    let ledger = fs::read(file(r, "revisions.json")).unwrap();
    assert!(format!("{:#}", text_update(r, "intro", false).unwrap_err()).contains("overflow"));
    assert_eq!(contract, fs::read(file(r, "contracts.json")).unwrap());
    assert_eq!(ledger, fs::read(file(r, "revisions.json")).unwrap());
    assert!(!r.join(".nir/text-transaction.json").exists());
}
