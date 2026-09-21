use nir_compiler::*;
use std::{
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
    assert!(load_project(d.path())
        .unwrap_err()
        .to_string()
        .contains("E_FONT_COVERAGE"));
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
