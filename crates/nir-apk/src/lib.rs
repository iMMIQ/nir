//! Host-side Android APK assembly: binary manifest encoding, ZIP packaging
//! and APK Signature Scheme v2 signing, in pure Rust so novel authors can
//! build installable APKs without Java or the Android SDK.
//!
//! ```no_run
//! # use nir_apk::*;
//! # fn main() -> anyhow::Result<()> {
//! let manifest = ManifestSpec {
//!     package: "one.nir.g7f3c2d9a".to_owned(),
//!     version_code: 1,
//!     version_name: "1.0.0".to_owned(),
//!     label: "星之梦".to_owned(),
//!     min_sdk: 26,
//!     target_sdk: 29,
//!     lib_name: "player".to_owned(),
//! };
//! let entries = vec![(
//!     "lib/arm64-v8a/libplayer.so".to_owned(),
//!     EntrySource::File(std::path::PathBuf::from("libplayer.so")),
//! )];
//! let key = key::SigningIdentity::generate()?;
//! build_apk(std::path::Path::new("game.apk"), &manifest, &entries, &key)?;
//! # Ok(())
//! # }
//! ```
#![forbid(unsafe_code)]

pub mod axml;
pub(crate) mod der;
pub mod key;
pub mod sign;
pub mod zip;

use anyhow::{ensure, Context, Result};
use axml::{Attr, AttrValue, Element};
use std::path::{Path, PathBuf};

/// Native library alignment for STORED `lib/**` entries (page size).
const LIB_ALIGNMENT: u16 = 4096;

/// Manifest fields for the generated `AndroidManifest.xml`.
pub struct ManifestSpec {
    /// Java package name, e.g. `one.nir.g0123abcd`; each dot-separated
    /// segment must start with a lowercase ASCII letter.
    pub package: String,
    /// `android:versionCode`, >= 1.
    pub version_code: u32,
    /// `android:versionName`, arbitrary short string.
    pub version_name: String,
    /// `android:label`; arbitrary UTF-16 (game titles are often Chinese).
    pub label: String,
    /// `android:minSdkVersion`; v2-only signing requires >= 24.
    pub min_sdk: u32,
    /// `android:targetSdkVersion`.
    pub target_sdk: u32,
    /// Native library stem (`player` becomes the `android.app.lib_name`
    /// meta-data value; the entry itself is passed in `entries`).
    pub lib_name: String,
}

/// Content source for one APK entry; `File` streams from disk so large games
/// never need to fit in memory.
pub enum EntrySource {
    Bytes(Vec<u8>),
    File(PathBuf),
}

/// `android:configChanges` bitmask for the activity. Covers
/// orientation|screenSize|smallestScreenSize|screenLayout|keyboard|
/// keyboardHidden|navigation|density|uiMode; verified against aapt2 output
/// (see tests). Named per `android.content.pm.ActivityInfo`.
const CONFIG_CHANGES_KEYBOARD: u32 = 0x0000_0010;
const CONFIG_CHANGES_KEYBOARD_HIDDEN: u32 = 0x0000_0020;
const CONFIG_CHANGES_NAVIGATION: u32 = 0x0000_0040;
const CONFIG_CHANGES_ORIENTATION: u32 = 0x0000_0080;
const CONFIG_CHANGES_SCREEN_LAYOUT: u32 = 0x0000_0100;
const CONFIG_CHANGES_UI_MODE: u32 = 0x0000_0200;
const CONFIG_CHANGES_SCREEN_SIZE: u32 = 0x0000_0400;
const CONFIG_CHANGES_SMALLEST_SCREEN_SIZE: u32 = 0x0000_0800;
const CONFIG_CHANGES_DENSITY: u32 = 0x0000_1000;
const CONFIG_CHANGES: u32 = CONFIG_CHANGES_ORIENTATION
    | CONFIG_CHANGES_SCREEN_SIZE
    | CONFIG_CHANGES_SMALLEST_SCREEN_SIZE
    | CONFIG_CHANGES_SCREEN_LAYOUT
    | CONFIG_CHANGES_KEYBOARD
    | CONFIG_CHANGES_KEYBOARD_HIDDEN
    | CONFIG_CHANGES_NAVIGATION
    | CONFIG_CHANGES_DENSITY
    | CONFIG_CHANGES_UI_MODE;

/// `android:theme` reference to `@android:style/Theme.DeviceDefault.NoActionBar.Fullscreen`
/// (0x0103012a), verified against the platform 34 framework resource table.
const THEME_DEVICE_DEFAULT_NO_ACTION_BAR_FULLSCREEN: u32 = 0x0103_012a;
/// `singleTask` launch mode, the integer aapt2 compiles the enum name to.
const LAUNCH_MODE_SINGLE_TASK: u32 = 2;

/// Encodes the `AndroidManifest.xml` for the given spec (validates first).
pub fn manifest_bytes(manifest: &ManifestSpec) -> Result<Vec<u8>> {
    validate_manifest(manifest)?;
    axml::encode(&manifest_element(manifest))
}

/// Assembles an installable APK: generates the binary `AndroidManifest.xml`
/// (NativeActivity launcher, `hasCode=false`, `extractNativeLibs=true`),
/// packages the entries, signs with APK Signature Scheme v2, and writes
/// atomically to `out`.
///
/// Entries are written in the given order. Names under `lib/` are STORED
/// uncompressed and page-aligned (required by some loaders for uncompressed
/// native code); everything else is DEFLATE-compressed. The caller supplies
/// `lib/arm64-v8a/lib<lib_name>.so` in `entries`.
pub fn build_apk(
    out: &Path,
    manifest: &ManifestSpec,
    entries: &[(String, EntrySource)],
    key: &key::SigningIdentity,
) -> Result<()> {
    validate_manifest(manifest)?;
    let manifest_xml = manifest_bytes(manifest)?;
    let mut names = std::collections::HashSet::new();
    for (name, _) in entries {
        validate_entry_name(name)?;
        ensure!(
            names.insert(name.clone()),
            "E_APK_ENTRY: duplicate entry {name:?}"
        );
    }
    ensure!(
        entries.len() < zip::MAX_ENTRIES as usize,
        "E_APK_ENTRY_COUNT: too many entries"
    );

    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("E_APK_OUTPUT: {}", parent.display()))?;
        }
    }
    let mut temporary = out.as_os_str().to_owned();
    temporary.push(".part");
    let temporary = PathBuf::from(temporary);
    let outcome = assemble(&temporary, &manifest_xml, entries, key);
    if let Err(error) = outcome {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    std::fs::rename(&temporary, out).with_context(|| format!("E_APK_OUTPUT: {}", out.display()))?;
    Ok(())
}

/// Writes contents, central directory, signing block and EOCD to `temporary`.
fn assemble(
    temporary: &Path,
    manifest_xml: &[u8],
    entries: &[(String, EntrySource)],
    key: &key::SigningIdentity,
) -> Result<()> {
    let file = std::fs::File::create(temporary)
        .with_context(|| format!("E_APK_OUTPUT: {}", temporary.display()))?;
    let mut writer = zip::ZipWriter::new(file);
    writer.add_bytes("AndroidManifest.xml", zip::Method::Store, manifest_xml)?;
    for (name, source) in entries {
        let stored = name.starts_with("lib/");
        let method = if stored {
            zip::Method::Store
        } else {
            zip::Method::Deflate
        };
        let alignment = u16::from(stored) * LIB_ALIGNMENT;
        match source {
            EntrySource::Bytes(bytes) => {
                writer.add_bytes_aligned(name, method, bytes, alignment)?
            }
            EntrySource::File(path) => {
                zip::add_file_aligned(&mut writer, name, method, path, alignment)?
            }
        }
    }
    let (central_directory, contents_length) = writer.finish()?;
    let entry_count = entries.len() as u32 + 1;

    // Digest view: the EOCD with its central-directory offset pointing at the
    // (future) signing block, as the v2 scheme requires while digesting.
    let eocd_digest_view = zip::eocd(entry_count, central_directory.len() as u64, contents_length)?;
    let digest = {
        let mut reader = std::fs::File::open(temporary).context("E_APK_READ")?;
        sign::content_digest(
            &mut reader,
            contents_length,
            &central_directory,
            &eocd_digest_view,
        )?
    };
    let block = sign::signing_block(key, &digest);
    let eocd = zip::eocd(
        entry_count,
        central_directory.len() as u64,
        contents_length + block.len() as u64,
    )?;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(temporary)
        .context("E_APK_OUTPUT")?;
    use std::io::Write;
    file.write_all(&block).context("E_APK_WRITE")?;
    file.write_all(&central_directory).context("E_APK_WRITE")?;
    file.write_all(&eocd).context("E_APK_WRITE")?;
    file.sync_all().context("E_APK_WRITE")?;
    Ok(())
}

/// Validates manifest fields, mirroring aapt's package rules.
fn validate_manifest(manifest: &ManifestSpec) -> Result<()> {
    ensure!(!manifest.package.is_empty(), "E_APK_PACKAGE: empty");
    for segment in manifest.package.split('.') {
        ensure!(
            !segment.is_empty()
                && segment
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_lowercase())
                && segment
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "E_APK_PACKAGE: {:?} is not a lowercase java package name",
            manifest.package
        );
    }
    ensure!(manifest.version_code >= 1, "E_APK_VERSION_CODE");
    ensure!(
        manifest.min_sdk >= 24,
        "E_APK_SDK: v2-only signing needs min_sdk >= 24"
    );
    ensure!(
        manifest.target_sdk >= manifest.min_sdk,
        "E_APK_SDK: target_sdk below min_sdk"
    );
    ensure!(
        !manifest.lib_name.is_empty()
            && manifest
                .lib_name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
        "E_APK_LIB_NAME: {:?} must be a library stem",
        manifest.lib_name
    );
    Ok(())
}

/// Validates an APK entry path: relative, forward slashes only, no parent or
/// current-directory segments, no drive letters or control characters.
fn validate_entry_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty() && !name.starts_with('/') && !name.ends_with('/'),
        "E_APK_ENTRY: {name:?} must be a relative path"
    );
    ensure!(
        !name.contains('\\') && !name.contains(':') && !name.chars().any(|c| c.is_control()),
        "E_APK_ENTRY: {name:?} contains forbidden characters"
    );
    for segment in name.split('/') {
        ensure!(
            !segment.is_empty() && segment != "." && segment != "..",
            "E_APK_ENTRY: {name:?} has an invalid segment"
        );
    }
    ensure!(name != "AndroidManifest.xml", "E_APK_ENTRY: reserved name");
    Ok(())
}

/// Builds the manifest element tree (attribute ordering is applied by the
/// encoder, mirroring aapt2).
fn manifest_element(manifest: &ManifestSpec) -> Element {
    let mut action = Element::new("action");
    action.attrs.push(Attr::android(
        "name",
        AttrValue::String("android.intent.action.MAIN".to_owned()),
    ));
    let mut category = Element::new("category");
    category.attrs.push(Attr::android(
        "name",
        AttrValue::String("android.intent.category.LAUNCHER".to_owned()),
    ));
    let mut intent_filter = Element::new("intent-filter");
    intent_filter.children.push(action);
    intent_filter.children.push(category);
    let mut meta_data = Element::new("meta-data");
    meta_data.attrs.push(Attr::android(
        "name",
        AttrValue::String("android.app.lib_name".to_owned()),
    ));
    meta_data.attrs.push(Attr::android(
        "value",
        AttrValue::String(manifest.lib_name.clone()),
    ));
    let mut activity = Element::new("activity");
    activity.attrs.push(Attr::android(
        "name",
        AttrValue::String("android.app.NativeActivity".to_owned()),
    ));
    activity
        .attrs
        .push(Attr::android("exported", AttrValue::Bool(true)));
    activity.attrs.push(Attr::android(
        "launchMode",
        AttrValue::IntDec(LAUNCH_MODE_SINGLE_TASK),
    ));
    activity.attrs.push(Attr::android(
        "configChanges",
        AttrValue::IntHex(CONFIG_CHANGES),
    ));
    activity.children.push(meta_data);
    activity.children.push(intent_filter);
    let mut uses_sdk = Element::new("uses-sdk");
    uses_sdk.attrs.push(Attr::android(
        "minSdkVersion",
        AttrValue::IntDec(manifest.min_sdk),
    ));
    uses_sdk.attrs.push(Attr::android(
        "targetSdkVersion",
        AttrValue::IntDec(manifest.target_sdk),
    ));
    let mut application = Element::new("application");
    application.attrs.push(Attr::android(
        "label",
        AttrValue::String(manifest.label.clone()),
    ));
    application
        .attrs
        .push(Attr::android("hasCode", AttrValue::Bool(false)));
    application
        .attrs
        .push(Attr::android("extractNativeLibs", AttrValue::Bool(true)));
    application.attrs.push(Attr::android(
        "theme",
        AttrValue::Reference(THEME_DEVICE_DEFAULT_NO_ACTION_BAR_FULLSCREEN),
    ));
    application.children.push(activity);
    let mut root = Element::new("manifest");
    root.attrs.push(Attr::plain(
        "package",
        AttrValue::String(manifest.package.clone()),
    ));
    root.attrs.push(Attr::android(
        "versionCode",
        AttrValue::IntDec(manifest.version_code),
    ));
    root.attrs.push(Attr::android(
        "versionName",
        AttrValue::String(manifest.version_name.clone()),
    ));
    root.children.push(uses_sdk);
    root.children.push(application);
    root
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_packages() {
        for package in [
            "",
            "One.Nir",
            "one.nir.0abc",
            "one.nir..x",
            ".one",
            "one.",
            "one.nir-x",
        ] {
            let manifest = ManifestSpec {
                package: package.to_owned(),
                version_code: 1,
                version_name: "1".to_owned(),
                label: "x".to_owned(),
                min_sdk: 26,
                target_sdk: 29,
                lib_name: "player".to_owned(),
            };
            assert!(validate_manifest(&manifest).is_err(), "{package}");
        }
    }

    #[test]
    fn rejects_invalid_entry_names() {
        for name in [
            "",
            "/abs",
            "rel/",
            "a//b",
            "../up",
            "a/../b",
            "./here",
            "back\\slash",
            "c:drive",
            "ctrl\u{7}l",
            "AndroidManifest.xml",
        ] {
            assert!(validate_entry_name(name).is_err(), "{name}");
        }
        for name in ["data/objects/abc.json", "lib/arm64-v8a/libplayer.so", "a"] {
            assert!(validate_entry_name(name).is_ok(), "{name}");
        }
    }

    #[test]
    fn manifest_shape() {
        let manifest = ManifestSpec {
            package: "one.nir.g0123abcd".to_owned(),
            version_code: 3,
            version_name: "1.0.3".to_owned(),
            label: "星之梦".to_owned(),
            min_sdk: 26,
            target_sdk: 29,
            lib_name: "player".to_owned(),
        };
        let encoded = axml::encode(&manifest_element(&manifest)).unwrap();
        let decoded = axml::decode(&encoded).unwrap();
        assert_eq!(decoded.root.name, "manifest");
        assert_eq!(decoded.root.children.len(), 2);
        assert_eq!(decoded.root.children[0].name, "uses-sdk");
        let application = &decoded.root.children[1];
        assert_eq!(application.name, "application");
        assert_eq!(application.children.len(), 1);
        let activity = &application.children[0];
        assert_eq!(activity.name, "activity");
        assert_eq!(activity.children.len(), 2);
        assert_eq!(activity.children[0].name, "meta-data");
        assert_eq!(activity.children[1].children.len(), 2);
    }
}
