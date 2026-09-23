use nir_compiler::*;
use std::{
    fs,
    path::{Path, PathBuf},
};
fn template() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../templates/minimal")
}
fn project() -> tempfile::TempDir {
    let p = tempfile::tempdir().unwrap();
    copy_tree(&template(), p.path()).unwrap();
    p
}
fn replace(p: &Path, name: &str, from: &str, to: &str) {
    let file = p.join(name);
    let s = fs::read_to_string(&file).unwrap();
    assert!(s.contains(from));
    fs::write(file, s.replace(from, to)).unwrap();
}
fn sdk() -> tempfile::TempDir {
    let p = tempfile::tempdir().unwrap();
    for n in [
        "player_web.js",
        "player_web_bg.wasm",
        "host.js",
        "index.html",
        "bootstrap.js",
        "THIRD-PARTY.txt",
        "compiler.sha256",
    ] {
        fs::write(p.path().join(n), n).unwrap();
    }
    p
}
#[test]
fn minimal_routes_and_independent_initialization() {
    assert_eq!(test_project(&template()).unwrap(), ["garden", "home"]);
    let sdk = sdk();
    copy_tree(&template(), &sdk.path().join("templates/minimal")).unwrap();
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    init_template(&a, sdk.path(), "test.a", "minimal").unwrap();
    init_template(&b, sdk.path(), "test.b", "minimal").unwrap();
    assert_ne!(
        load_project(&a).unwrap().program.game_id,
        load_project(&b).unwrap().program.game_id
    );
    assert!(init_template(&a, sdk.path(), "test.a", "minimal").is_err());
    assert!(init_template(&root.path().join("bad"), sdk.path(), "x", "../minimal").is_err());
}
#[test]
fn subset_cache_reproducibility_corruption_and_new_characters() {
    let p = project();
    let a = load_project(p.path()).unwrap();
    let r = &a.fonts["font.reader"];
    assert!(!r.cache_hit);
    assert!(r.output_bytes < r.source_bytes / 20);
    let b = load_project(p.path()).unwrap();
    assert!(b.fonts["font.reader"].cache_hit);
    assert_eq!(a.media, b.media);
    let cached = p
        .path()
        .join(format!(".nir/cache/fonts/{}.font", r.cache_key));
    fs::write(cached, b"corrupt").unwrap();
    let c = load_project(p.path()).unwrap();
    assert!(!c.fonts["font.reader"].cache_hit);
    assert_eq!(a.media, c.media);
    let font = ttf_parser::Face::parse(&a.media["font.reader"], 0).unwrap();
    for c in "春天校园简体中文−←→↗0123456789".chars() {
        assert!(font.glyph_index(c).is_some(), "{c}");
    }
    assert!(font.glyph_index('鲸').is_none());
    replace(p.path(), "content/main/texts/zh-Hans.json", "春天", "鲸鱼");
    text_update(p.path(), "intro", false).unwrap();
    text_review(p.path(), "intro", "en").unwrap();
    let changed = load_project(p.path()).unwrap();
    assert!(!changed.fonts["font.reader"].cache_hit);
    assert_ne!(r.object, changed.fonts["font.reader"].object);
    assert!(ttf_parser::Face::parse(&changed.media["font.reader"], 0)
        .unwrap()
        .glyph_index('鲸')
        .is_some());
    fs::remove_dir_all(p.path().join(".nir")).unwrap();
    assert_eq!(changed.media, load_project(p.path()).unwrap().media);
}
#[test]
fn title_literals_reserved_chars_full_mode_and_coverage_errors() {
    let p = project();
    replace(p.path(), "game.toml", "新的故事", "霜雪之夜");
    replace(
        p.path(),
        "assets/catalog.toml",
        "extra_characters = \"\"",
        "extra_characters = \"麒麟\"",
    );
    let fragment = p.path().join("content/main/story.nir.json");
    let mut j: serde_json::Value = serde_json::from_slice(&fs::read(&fragment).unwrap()).unwrap();
    j["variables"] = serde_json::json!({"name":{"type":"string","value":"青"}});
    j["functions"]["main"]["blocks"]["intro"]["ops"] = serde_json::json!([{"id":"rename","operation":{"type":"assign","target":"name","value":{"type":"const","value":{"type":"string","value":"玄武"}}}}]);
    fs::write(fragment, serde_json::to_vec_pretty(&j).unwrap()).unwrap();
    let scenario = p.path().join("tests/garden.toml");
    let scenario_text = fs::read_to_string(&scenario).unwrap()
        + "\n[expect.variables.name]\ntype=\"string\"\nvalue=\"玄武\"\n";
    fs::write(&scenario, &scenario_text).unwrap();
    assert!(test_project(p.path()).is_ok());
    fs::write(&scenario, scenario_text.replace("玄武", "wrong")).unwrap();
    assert!(test_project(p.path()).is_err());
    let a = load_project(p.path()).unwrap();
    let font = ttf_parser::Face::parse(&a.media["font.reader"], 0).unwrap();
    for c in "霜雪之夜麒麟玄武".chars() {
        assert!(font.glyph_index(c).is_some(), "{c}");
    }
    replace(
        p.path(),
        "assets/catalog.toml",
        "mode = \"subset\"",
        "mode = \"full\"",
    );
    let full = load_project(p.path()).unwrap();
    assert!(full.fonts["font.reader"].retained_characters > 40000);
    assert!(ttf_parser::Face::parse(&full.media["font.reader"], 0)
        .unwrap()
        .glyph_index('鲸')
        .is_some());
    replace(p.path(), "assets/catalog.toml", "麒麟", "🐈");
    let e = format!("{:#}", load_project(p.path()).unwrap_err());
    assert!(
        e.contains("E_FONT_COVERAGE") && e.contains("U+1F408"),
        "{e}"
    );
}
#[test]
fn rejects_invalid_recipe_font_face_license_and_cache_paths() {
    for (name, from, to, code) in [
        (
            "assets/catalog.toml",
            "mode = \"subset\"",
            "mode = \"unknown\"",
            "E_TOML",
        ),
        (
            "assets/catalog.toml",
            "face_index = 0",
            "face_index = 5",
            "E_FONT",
        ),
        (
            "assets/catalog.toml",
            "face_index = 0",
            "surprise = true",
            "E_TOML",
        ),
        (
            "assets/catalog.toml",
            "fonts/OFL.txt",
            "../OFL.txt",
            "E_PATH",
        ),
        (
            "assets/catalog.toml",
            "fonts/OFL.txt",
            "fonts/missing.txt",
            "E_FILE_MISSING",
        ),
        (
            "assets/catalog.toml",
            "kind = \"font\"",
            "kind = \"image\"",
            "E_FONT_RECIPE",
        ),
        (
            "content/main/texts/en.json",
            "Flowers bloom",
            "🐈 Flowers bloom",
            "E_FONT_COVERAGE",
        ),
    ] {
        let p = project();
        replace(p.path(), name, from, to);
        if name.ends_with("en.json") {
            text_review(p.path(), "intro", "en").unwrap();
        }
        let e = format!("{:#}", load_project(p.path()).unwrap_err());
        assert!(e.contains(code), "{e}");
    }
    let p = project();
    fs::write(p.path().join("assets/fonts/OFL.txt"), "").unwrap();
    assert!(format!("{:#}", load_project(p.path()).unwrap_err()).contains("E_FONT_LICENSE"));
    let p = project();
    fs::write(
        p.path().join("assets/fonts/NotoSansCJKsc-Regular.otf"),
        b"broken",
    )
    .unwrap();
    assert!(format!("{:#}", load_project(p.path()).unwrap_err()).contains("E_FONT"));
    #[cfg(unix)]
    {
        let p = project();
        let external = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(external.path(), p.path().join(".nir")).unwrap();
        assert!(format!("{:#}", load_project(p.path()).unwrap_err()).contains("E_PATH_ESCAPE"));
    }
}
#[test]
fn subset_preserves_shaping_positions_and_outlines() {
    let p = project();
    replace(
        p.path(),
        "assets/catalog.toml",
        "extra_characters = \"\"",
        "extra_characters = \"office ffi fi á 漢語\"",
    );
    let loaded = load_project(p.path()).unwrap();
    let original = fs::read(p.path().join("assets/fonts/NotoSansCJKsc-Regular.otf")).unwrap();
    let subset = &loaded.media["font.reader"];
    for text in ["office ffi fi", "a\u{301}", "漢語"] {
        let shape = |bytes: &[u8]| {
            let face = rustybuzz::Face::from_slice(bytes, 0).unwrap();
            let mut buffer = rustybuzz::UnicodeBuffer::new();
            buffer.push_str(text);
            buffer.guess_segment_properties();
            let shaped = rustybuzz::shape(&face, &[], buffer);
            shaped
                .glyph_infos()
                .iter()
                .zip(shaped.glyph_positions())
                .map(|(i, p)| {
                    assert_ne!(i.glyph_id, 0);
                    let box_ = face
                        .glyph_bounding_box(rustybuzz::ttf_parser::GlyphId(i.glyph_id as u16))
                        .map(|r| (r.x_min, r.y_min, r.x_max, r.y_max));
                    (
                        i.cluster,
                        p.x_advance,
                        p.y_advance,
                        p.x_offset,
                        p.y_offset,
                        box_,
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(shape(&original), shape(subset), "{text}");
        if text.starts_with("office") {
            assert!(
                shape(subset).len() < text.chars().count(),
                "ligatures must actually form"
            );
        }
    }
    for tag in [b"GSUB", b"GPOS", b"GDEF"] {
        assert!(ttf_parser::Face::parse(subset, 0)
            .unwrap()
            .raw_face()
            .table(ttf_parser::Tag::from_bytes(tag))
            .is_some());
    }
}
#[test]
fn release_contains_generated_font_and_license_not_master_or_cache() {
    let p = project();
    let sdk = sdk();
    resolve(p.path(), sdk.path()).unwrap();
    let a = build(p.path(), sdk.path(), &p.path().join("dist/a"), true).unwrap();
    let b = build(p.path(), sdk.path(), &p.path().join("dist/b"), true).unwrap();
    assert_eq!(a.release, b.release);
    let r = &a.fonts["font.reader"];
    assert!(r.cache_hit);
    let manifest: nir_format::ReleaseManifest = serde_json::from_slice(
        &fs::read(p.path().join(format!("dist/a/releases/{}.json", a.release))).unwrap(),
    )
    .unwrap();
    assert!(manifest.objects.contains_key(&r.object));
    assert!(!manifest.objects.contains_key(&r.source_digest));
    assert!(fs::read_to_string(p.path().join("dist/a/NOTICE.txt"))
        .unwrap()
        .contains("SIL OPEN FONT LICENSE"));
    let object = r.object.clone();
    replace(
        p.path(),
        "assets/fonts/OFL.txt",
        "PREAMBLE",
        "PREAMBLE\nTest annotation",
    );
    let c = build(p.path(), sdk.path(), &p.path().join("dist/c"), true).unwrap();
    assert_ne!(a.release, c.release);
    assert_eq!(c.fonts["font.reader"].object, object);
    assert!(c.fonts["font.reader"].cache_hit);
}

#[test]
fn truetype_collection_face_extraction_and_unsupported_tables() {
    // Construct a valid one-face TTC from the independent static glyf fixture.
    let bytes = include_bytes!("fonts/ABeeZee-Regular.ttf");
    let mut collection = b"ttcf\0\x01\0\0\0\0\0\x01\0\0\0\x10".to_vec();
    collection.extend_from_slice(bytes);
    let count = u16::from_be_bytes(bytes[4..6].try_into().unwrap()) as usize;
    for i in 0..count {
        let pos = 16 + 12 + 16 * i + 8;
        let old = u32::from_be_bytes(collection[pos..pos + 4].try_into().unwrap());
        collection[pos..pos + 4].copy_from_slice(&(old + 16).to_be_bytes());
    }
    let p = project();
    fs::write(p.path().join("assets/latin.ttc"), collection).unwrap();
    fs::write(
        p.path().join("assets/latin-license.txt"),
        include_bytes!("fonts/OFL.txt"),
    )
    .unwrap();
    let file = p.path().join("assets/catalog.toml");
    let catalog = fs::read_to_string(&file).unwrap();
    fs::write(&file,format!("{catalog}\n[[assets]]\nid=\"font.latin\"\nkind=\"font\"\nsource=\"latin.ttc\"\nrights=\"OFL-1.1\"\n[assets.font]\nmode=\"subset\"\nlicense=\"latin-license.txt\"\nextra_characters=\"á\"\n")).unwrap();
    let locales = p.path().join("config/locales.toml");
    let config = fs::read_to_string(&locales).unwrap().replace(
        "en = [\"font.reader\"]",
        "en = [\"font.latin\", \"font.reader\"]",
    );
    fs::write(locales, config).unwrap();
    let a = load_project(p.path()).unwrap();
    let font = &a.media["font.latin"];
    assert_eq!(&font[..4], b"\0\x01\0\0");
    let face = ttf_parser::Face::parse(font, 0).unwrap();
    assert!(face.tables().glyf.is_some());
    assert!(face.glyph_index('á').is_some());
    let sdk = sdk();
    resolve(p.path(), sdk.path()).unwrap();
    let report = build(p.path(), sdk.path(), &p.path().join("dist"), true).unwrap();
    let manifest: nir_format::ReleaseManifest = serde_json::from_slice(
        &fs::read(
            p.path()
                .join(format!("dist/releases/{}.json", report.release)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        manifest.objects[&a.program.assets["font.latin"].object].media_type,
        "font/ttf"
    );
    // AAT layout is outside this compiler profile and must not silently vanish.
    let mut bad = bytes.to_vec();
    for i in 0..count {
        let pos = 12 + 16 * i;
        if &bad[pos..pos + 4] == b"GSUB" {
            bad[pos..pos + 4].copy_from_slice(b"morx");
        }
    }
    let mut records: Vec<_> = bad[12..12 + 16 * count]
        .as_chunks::<16>()
        .0
        .iter()
        .map(|r| r.to_vec())
        .collect();
    records.sort_by_key(|r| r[..4].to_vec());
    for (i, r) in records.iter().enumerate() {
        bad[12 + 16 * i..12 + 16 * (i + 1)].copy_from_slice(r);
    }
    fs::write(p.path().join("assets/latin.ttc"), bad).unwrap();
    assert!(format!("{:#}", load_project(p.path()).unwrap_err()).contains("E_FONT_CAPABILITY"));
}
