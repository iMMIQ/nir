//! Integration tests for full APK assembly, verified with the crate's own
//! decoder/verifier and — when present on the machine — aapt2, apksigner and
//! Python's zipfile (external checks skip with a printed note).

use nir_apk::axml::{self, DecodedElement, DecodedValue};
use nir_apk::{
    build_apk, key::SigningIdentity, manifest_bytes, sign, zip, EntrySource, ManifestSpec,
};
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("nir-apk-tests");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn manifest_spec() -> ManifestSpec {
    ManifestSpec {
        package: "one.nir.g0123abcd".to_owned(),
        version_code: 3,
        version_name: "1.0.3".to_owned(),
        label: "星之梦 ～Planetarium～".to_owned(),
        min_sdk: 26,
        target_sdk: 29,
        lib_name: "player".to_owned(),
    }
}

/// Deterministic pseudo-random library payload (15 MiB, incompressible enough
/// to exercise multi-chunk digests and streaming through the STORE path).
fn write_library(path: &Path) -> Vec<u8> {
    let mut state = 0x1234_5678_u32;
    let bytes: Vec<u8> = (0..15 * 1024 * 1024)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8
        })
        .collect();
    std::fs::write(path, &bytes).unwrap();
    bytes
}

type Assets = (Vec<(String, EntrySource)>, Vec<(String, Vec<u8>)>);

/// One in-memory asset (Chinese prose) and one streamed from disk.
fn write_assets(dir: &Path) -> Assets {
    std::fs::create_dir_all(dir.join("data/objects")).unwrap();
    let story = "少年睁开眼睛，穹顶的星空缓缓转动。".repeat(2000);
    let table: Vec<u8> = (0..40_000u32).map(|i| (i % 7) as u8).collect();
    std::fs::write(dir.join("data/objects/table.bin"), &table).unwrap();
    let entries = vec![
        (
            "assets/data/objects/story.json".to_owned(),
            EntrySource::Bytes(story.clone().into_bytes()),
        ),
        (
            "assets/data/objects/table.bin".to_owned(),
            EntrySource::File(dir.join("data/objects/table.bin")),
        ),
    ];
    let expected = vec![
        (
            "assets/data/objects/story.json".to_owned(),
            story.into_bytes(),
        ),
        ("assets/data/objects/table.bin".to_owned(), table),
    ];
    (entries, expected)
}

fn build_full_apk(tag: &str, out: &Path, key: &SigningIdentity) -> Vec<(String, Vec<u8>)> {
    let library = scratch(&format!("{tag}-libplayer.so"));
    write_library(&library);
    let (assets, expected) = write_assets(&scratch(&format!("{tag}-stage")));
    let entries = vec![(
        "lib/arm64-v8a/libplayer.so".to_owned(),
        EntrySource::File(library),
    )]
    .into_iter()
    .chain(assets)
    .collect::<Vec<_>>();
    build_apk(out, &manifest_spec(), &entries, key).expect("E_TEST_BUILD");
    expected
}

/// Attributes aapt2 adds on its own; the equality check ignores them.
const AAPT2_EXTRA_ATTRS: &[&str] = &[
    "compileSdkVersion",
    "compileSdkVersionCodename",
    "platformBuildVersionCode",
    "platformBuildVersionName",
];

fn strip_aapt2_extras(element: &mut DecodedElement) {
    element
        .attrs
        .retain(|attr| !AAPT2_EXTRA_ATTRS.contains(&attr.name.as_str()));
    for child in &mut element.children {
        strip_aapt2_extras(child);
    }
}

fn decoded_manifest(spec: &ManifestSpec) -> axml::DecodedXml {
    axml::decode(&manifest_bytes(spec).expect("E_TEST_ENCODE")).expect("E_TEST_DECODE")
}

/// Compares our encoding against aapt2's for the same source manifest:
/// identical element tree, attribute namespaces/values and resource-map IDs
/// (ignoring the build-metadata attributes aapt2 injects).
fn assert_matches_aapt2(spec: &ManifestSpec, fixture: &str) {
    let reference_bytes = std::fs::read(fixtures().join(fixture)).expect("E_TEST_FIXTURE");
    let reference = axml::decode(&reference_bytes).expect("E_TEST_DECODE_AAPT2");
    let reference_map = reference.resource_map.clone();
    let mut reference_root = reference.root.clone();
    strip_aapt2_extras(&mut reference_root);
    let ours = decoded_manifest(spec);
    let mut ours_root = ours.root.clone();
    strip_aapt2_extras(&mut ours_root);
    assert_eq!(ours_root, reference_root, "element tree differs from aapt2");
    // The resource map covers the attribute-name strings in pool order;
    // ours must equal aapt2's minus the compileSdk entries we do not emit.
    let expected_map: Vec<u32> = reference_map
        .iter()
        .copied()
        .filter(|id| *id != 0x0101_0572 && *id != 0x0101_0573)
        .collect();
    assert_eq!(
        ours.resource_map, expected_map,
        "resource map differs from aapt2"
    );
}

#[test]
fn manifest_matches_aapt2_reference() {
    // Fixture built with `aapt2 link -I android.jar (platform 34)` from
    // AndroidManifest.theme.source.xml: the same manifest this crate emits,
    // including android:theme="@android:style/Theme.DeviceDefault.NoActionBar.Fullscreen".
    // (AndroidManifest.source.xml is the identical manifest without the theme,
    // kept as the human-readable input for regenerating the fixture.)
    assert_matches_aapt2(&manifest_spec(), "AndroidManifest.theme.aapt2.bin");
}

#[test]
fn full_build_round_trips() {
    let key = SigningIdentity::from_seed(&[0x42; 32]).unwrap();
    let out = scratch("round-trip.apk");
    let expected = build_full_apk("rt", &out, &key);
    let apk = std::fs::read(&out).unwrap();
    assert!(apk.len() > 10 * 1024 * 1024);
    // Own ZIP reader: structure, CRCs, decompression.
    let entries = zip::read_entries(&apk).unwrap();
    assert_eq!(entries.len(), 4); // manifest + lib + 2 assets
    let manifest_entry = entries
        .iter()
        .find(|e| e.name == "AndroidManifest.xml")
        .unwrap();
    assert_eq!(manifest_entry.method, 0);
    let lib_entry = entries
        .iter()
        .find(|e| e.name == "lib/arm64-v8a/libplayer.so")
        .unwrap();
    assert_eq!(lib_entry.method, 0);
    // Page-aligned uncompressed native code.
    let extra_len = u16::from_le_bytes(
        apk[lib_entry.local_offset as usize + 28..lib_entry.local_offset as usize + 30]
            .try_into()
            .unwrap(),
    ) as usize;
    let data_start = lib_entry.local_offset as usize + 30 + lib_entry.name.len() + extra_len;
    assert_eq!(data_start % 4096, 0);
    assert_eq!(
        zip::extract_entry(&apk, lib_entry).unwrap(),
        std::fs::read(scratch("rt-libplayer.so")).unwrap()
    );
    for (name, bytes) in &expected {
        let entry = entries.iter().find(|e| &e.name == name).unwrap();
        assert_eq!(entry.method, 8, "{name} should be deflated");
        assert_eq!(zip::extract_entry(&apk, entry).unwrap(), *bytes);
    }
    // The manifest decodes and carries the spec.
    let manifest = zip::extract_entry(&apk, manifest_entry).unwrap();
    let decoded = axml::decode(&manifest).unwrap();
    let label = decoded
        .root
        .children
        .iter()
        .find(|c| c.name == "application")
        .unwrap()
        .attrs
        .iter()
        .find(|a| a.name == "label")
        .unwrap();
    assert_eq!(
        label.value,
        DecodedValue::String("星之梦 ～Planetarium～".to_owned())
    );
    // Own v2 verifier.
    let verified = sign::verify_apk(&out).unwrap();
    assert_eq!(verified.signers, 1);
    assert_eq!(verified.certificate_der, key.certificate_der());
}

#[test]
fn python_zipfile_reads_the_apk() {
    let key = SigningIdentity::from_seed(&[0x43; 32]).unwrap();
    let out = scratch("python-zipfile.apk");
    build_full_apk("py", &out, &key);
    let output = Command::new("python3")
        .arg("-c")
        .arg(
            "import zipfile,sys;\n\
             z=zipfile.ZipFile(sys.argv[1]);\n\
             assert z.testzip() is None, 'bad crc';\n\
             [z.read(n) for n in z.namelist()];\n\
             print('ZIPFILE_OK', len(z.namelist()))",
        )
        .arg(&out)
        .output();
    match output {
        Ok(result) => {
            let stdout = String::from_utf8_lossy(&result.stdout);
            println!("{}", stdout.trim());
            assert!(
                result.status.success() && stdout.contains("ZIPFILE_OK"),
                "python3 zipfile rejected the APK: {}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        Err(error) => println!("note: python3 unavailable, skipping zipfile check ({error})"),
    }
}

fn tool_path(env_var: &str, segments: &[&str]) -> Option<PathBuf> {
    if let Ok(value) = std::env::var(env_var) {
        return Some(PathBuf::from(value));
    }
    let mut path = std::env::var("HOME").ok().map(PathBuf::from)?;
    for segment in segments {
        path.push(segment);
    }
    path.exists().then_some(path)
}

#[test]
fn aapt2_badging_reports_the_game() {
    let key = SigningIdentity::from_seed(&[0x44; 32]).unwrap();
    let out = scratch("aapt2.apk");
    build_full_apk("aapt", &out, &key);
    let Some(aapt2) = tool_path("NIR_APK_AAPT2", &["android-sdk-dl", "android-14", "aapt2"]) else {
        println!("note: aapt2 not found, skipping badging check");
        return;
    };
    let result = Command::new(&aapt2)
        .arg("dump")
        .arg("badging")
        .arg(&out)
        .output()
        .expect("E_TEST_AAPT2");
    let stdout = String::from_utf8_lossy(&result.stdout);
    println!("{}", stdout);
    assert!(result.status.success(), "aapt2 dump badging failed");
    assert!(
        stdout.contains("package: name='one.nir.g0123abcd'"),
        "package"
    );
    assert!(stdout.contains("versionCode='3'"), "versionCode");
    assert!(stdout.contains("versionName='1.0.3'"), "versionName");
    assert!(stdout.contains("sdkVersion:'26'"), "min sdk");
    assert!(stdout.contains("targetSdkVersion:'29'"), "target sdk");
    assert!(
        stdout.contains("application-label:'星之梦 ～Planetium～'") || stdout.contains("星之梦"),
        "label"
    );
    assert!(stdout.contains("native-code: 'arm64-v8a'"), "native code");
    // xmltree must also parse the binary manifest.
    let tree = Command::new(&aapt2)
        .arg("dump")
        .arg("xmltree")
        .arg("--file")
        .arg("AndroidManifest.xml")
        .arg(&out)
        .output()
        .expect("E_TEST_AAPT2");
    assert!(tree.status.success(), "aapt2 dump xmltree failed");
    let tree_out = String::from_utf8_lossy(&tree.stdout);
    assert!(tree_out.contains("android.app.NativeActivity"), "activity");
    // aapt2 renders the singleTask enum as the integer 2.
    assert!(tree_out.contains("launchMode(0x0101001d)=2"), "launchMode");
    assert!(
        tree_out.contains("configChanges(0x0101001f)=0x00001ff0"),
        "configChanges"
    );
    assert!(tree_out.contains("theme(0x01010000)=@0x0103012a"), "theme");
}

#[test]
fn apksigner_verifies_v2_signature() {
    let key = SigningIdentity::from_seed(&[0x45; 32]).unwrap();
    let out = scratch("apksigner.apk");
    build_full_apk("sig", &out, &key);
    let Some(apksigner) = tool_path(
        "NIR_APK_APKSIGNER",
        &["android-sdk-dl", "android-14", "apksigner"],
    ) else {
        println!("note: apksigner not found, skipping signature check");
        return;
    };
    // The script's `#!/bin/bash` may not exist verbatim (NixOS); invoke it
    // through bash from PATH.
    let mut command = Command::new("bash");
    command
        .arg(&apksigner)
        .arg("verify")
        .arg("--verbose")
        .arg("--print-certs")
        .arg(&out);
    if let Some(jdk) = std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join("android-sdk-dl/jdk-17.0.20.1+1-jre"))
    {
        if jdk.exists() {
            command.env("JAVA_HOME", &jdk);
            if let Ok(path) = std::env::var("PATH") {
                let bin = jdk.join("bin");
                command.env("PATH", format!("{}:{path}", bin.display()));
            }
        }
    }
    let result = command.output().expect("E_TEST_APKSIGNER");
    let stdout = String::from_utf8_lossy(&result.stdout);
    println!("{}", stdout);
    if !result.status.success() {
        println!(
            "apksigner stderr: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    assert!(result.status.success(), "apksigner verify failed");
    assert!(
        stdout.contains("Verified using v2 scheme (APK Signature Scheme v2): true"),
        "v2 scheme"
    );
    assert!(stdout.contains("CN=nir"), "certificate subject");
}

#[test]
fn tampered_apk_fails_verification() {
    let key = SigningIdentity::from_seed(&[0x46; 32]).unwrap();
    let out = scratch("tamper-source.apk");
    build_full_apk("tamper", &out, &key);
    let original = std::fs::read(&out).unwrap();
    let entries = zip::read_entries(&original).unwrap();
    let asset = entries
        .iter()
        .find(|e| e.name == "assets/data/objects/table.bin")
        .unwrap();

    // Flip one byte inside the asset's compressed data.
    let mut flipped = original.clone();
    let extra_len = u16::from_le_bytes(
        original[asset.local_offset as usize + 28..asset.local_offset as usize + 30]
            .try_into()
            .unwrap(),
    ) as usize;
    let offset = asset.local_offset as usize + 30 + asset.name.len() + extra_len + 3;
    flipped[offset] ^= 0x01;
    let flipped_path = scratch("tamper-flipped.apk");
    std::fs::write(&flipped_path, &flipped).unwrap();
    match sign::verify_apk(&flipped_path) {
        Ok(_) => panic!("flipped asset byte passed verification"),
        Err(error) => println!("{error:#}"),
    }

    // Corrupt one byte inside the signing block.
    let mut block_corrupt = original.clone();
    let eocd_offset = original.len() - 22;
    let cd_offset = u32::from_le_bytes(
        original[eocd_offset + 16..eocd_offset + 20]
            .try_into()
            .unwrap(),
    ) as usize;
    let block_size =
        u64::from_le_bytes(original[cd_offset - 24..cd_offset - 16].try_into().unwrap()) as usize;
    block_corrupt[cd_offset - block_size + 40] ^= 0x01; // inside the v2 pair
    let corrupt_path = scratch("tamper-block.apk");
    std::fs::write(&corrupt_path, &block_corrupt).unwrap();
    match sign::verify_apk(&corrupt_path) {
        Ok(_) => panic!("corrupted signing block passed verification"),
        Err(error) => println!("{error:#}"),
    }

    // Truncate the tail (EOCD region).
    let mut truncated = original.clone();
    truncated.truncate(truncated.len() - 8);
    let truncated_path = scratch("tamper-truncated.apk");
    std::fs::write(&truncated_path, &truncated).unwrap();
    assert!(sign::verify_apk(&truncated_path).is_err());
}

#[test]
fn rejects_invalid_builds() {
    let key = SigningIdentity::from_seed(&[0x47; 32]).unwrap();
    let out = scratch("invalid.apk");
    let good_entries = vec![(
        "assets/data/x.json".to_owned(),
        EntrySource::Bytes(b"{}".to_vec()),
    )];
    build_apk(&out, &manifest_spec(), &good_entries, &key).unwrap();
    assert!(std::fs::metadata(&out).is_ok());

    let mut bad_package = manifest_spec();
    bad_package.package = "one.nir.0abc".to_owned();
    assert!(build_apk(&out, &bad_package, &good_entries, &key).is_err());

    let absolute = vec![("/etc/passwd".to_owned(), EntrySource::Bytes(b"{}".to_vec()))];
    assert!(build_apk(&out, &manifest_spec(), &absolute, &key).is_err());

    let traversal = vec![(
        "assets/../../escape".to_owned(),
        EntrySource::Bytes(b"{}".to_vec()),
    )];
    assert!(build_apk(&out, &manifest_spec(), &traversal, &key).is_err());

    let duplicate = vec![
        ("assets/a".to_owned(), EntrySource::Bytes(b"1".to_vec())),
        ("assets/a".to_owned(), EntrySource::Bytes(b"2".to_vec())),
    ];
    assert!(build_apk(&out, &manifest_spec(), &duplicate, &key).is_err());

    let missing = vec![(
        "assets/none".to_owned(),
        EntrySource::File(scratch("does-not-exist.bin")),
    )];
    assert!(build_apk(&out, &manifest_spec(), &missing, &key).is_err());
    // The atomic write left no .part artifact behind.
    assert!(!out.with_extension("apk.part").exists());
}

#[test]
fn deterministic_builds_for_the_same_inputs() {
    let key = SigningIdentity::from_seed(&[0x48; 32]).unwrap();
    let first = scratch("deterministic-1.apk");
    let second = scratch("deterministic-2.apk");
    let entries = vec![(
        "assets/data/a.bin".to_owned(),
        EntrySource::Bytes(vec![9u8; 5000]),
    )];
    build_apk(&first, &manifest_spec(), &entries, &key).unwrap();
    build_apk(&second, &manifest_spec(), &entries, &key).unwrap();
    assert_eq!(
        std::fs::read(&first).unwrap(),
        std::fs::read(&second).unwrap(),
        "identical inputs must produce byte-identical APKs"
    );
}
