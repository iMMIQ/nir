use super::*;

/// Android entry. The AndroidApp handle supplies the storage roots and APK
/// assets; `native_library` is the installed libplayer.so path used for the
/// whole-player digest attestation (None skips the check).
#[cfg(target_os = "android")]
pub fn run_android(
    app: android_activity::AndroidApp,
    native_library: Option<PathBuf>,
) -> Result<()> {
    let files = app
        .internal_data_path()
        .context("E_STORAGE_ROOT: internal data path unavailable")?;
    let root = extract_content(&app, &files)?;
    // Saves and exports live in the external files dir (Android/data/<pkg>/
    // files, visible over USB) so the documented backup path works; the rare
    // device without one falls back to internal storage.
    let external = app.external_data_path().unwrap_or_else(|| files.clone());
    let data = external.join("NIR/games");
    let exports = external.join("exports");
    fs::create_dir_all(&exports).ok();
    let bundle = Arc::new(Bundle::open(&root)?);
    // Same helper-thread attestation as the desktop: the installed native
    // library is hashed while the event loop and GPU come up, and the verdict
    // gates runtime construction in resumed().
    let expected_player = bundle.manifest.player.clone();
    let exe_check = native_library.map(|path| {
        std::thread::spawn(move || -> Result<()> {
            nir_content::verify(&fs::read(path)?, &expected_player)?;
            Ok(())
        })
    });
    let event_loop = winit::event_loop::EventLoop::builder()
        .with_android_app(app)
        .build()?;
    let mut app = App {
        bundle,
        data,
        exports,
        runtime: None,
        error: None,
        smoke: None,
        smoke_step: 0,
        started: Instant::now(),
        smoke_advance: Instant::now(),
        advances: 0,
        save_at_dialogue: false,
        hidden: false,
        exe_check,
    };
    event_loop.run_app(&mut app)?;
    if let Some(error) = app.error {
        return Err(error);
    }
    Ok(())
}

/// Write an extracted content-addressed asset via temp + rename without
/// per-file `fsync` stalls; `release.txt` still uses `atomic_write` at the end
/// so updates commit atomically while keeping cold extraction fast.
#[cfg(target_os = "android")]
fn extract_write(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Remove obsolete extracted files left behind by prior APK updates under
/// content-addressed trees (`objects/`, `releases/`).
#[cfg(target_os = "android")]
fn prune_stale_files(dir: &std::path::Path, expected: &BTreeSet<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            prune_stale_files(&path, expected);
            let _ = fs::remove_dir(&path);
        } else if !expected.contains(&path) {
            let _ = fs::remove_file(&path);
        }
    }
}

/// Streams the packaged `data/` tree out of the APK into filesDir once.
/// Unchanged content-addressed files (same name and size) are skipped, so an
/// app update only copies content that actually changed; the digests
/// Bundle::open re-verifies still cover the extracted copies. `release.txt`
/// is written last via `atomic_write` and stale objects/releases from older
/// versions are pruned to avoid leaking internal storage.
#[cfg(target_os = "android")]
fn extract_content(app: &android_activity::AndroidApp, files: &std::path::Path) -> Result<PathBuf> {
    use std::{ffi::CString, io::Read};
    fn walk(
        manager: &ndk::asset::AssetManager,
        apk_dir: &str,
        out_dir: &std::path::Path,
        expected: &mut BTreeSet<PathBuf>,
        deferred_release: &mut Option<(PathBuf, Vec<u8>)>,
    ) -> Result<()> {
        let dir_c = CString::new(apk_dir.to_owned())
            .with_context(|| format!("E_PACKAGE_PATH: {apk_dir}"))?;
        let Some(mut dir) = manager.open_dir(&dir_c) else {
            bail!("E_PACKAGE_ROOT: {apk_dir} missing from APK assets");
        };
        let mut names = Vec::new();
        while let Some(name) = dir.with_next(|c| c.to_bytes().to_vec()) {
            names.push(name);
        }
        for name in names {
            let name = String::from_utf8_lossy(&name).into_owned();
            if name.is_empty() || name.contains(['\\', ':']) || name.split('/').any(|s| s == "..") {
                bail!("E_PACKAGE_PATH: invalid asset name {name:?}");
            }
            let apk_path = format!("{apk_dir}/{name}");
            let out_path = out_dir.join(&name);
            let asset_c = CString::new(apk_path.clone())
                .with_context(|| format!("E_PACKAGE_PATH: {apk_path}"))?;
            if let Some(mut asset) = manager.open(&asset_c) {
                expected.insert(out_path.clone());
                let len = asset.length();
                // Only the content-addressed trees are immutable; the release
                // pointer is fixed-name and fixed-size, so skipping it on a
                // size match would pin every update to the old release.
                let addressable = apk_path != "data/release.txt";
                if addressable
                    && matches!(fs::metadata(&out_path), Ok(m) if m.len() as usize == len)
                {
                    continue;
                }
                let mut bytes = Vec::with_capacity(len);
                asset
                    .read_to_end(&mut bytes)
                    .with_context(|| format!("E_PACKAGE_READ: {apk_path}"))?;
                if addressable {
                    extract_write(&out_path, &bytes)?;
                } else {
                    *deferred_release = Some((out_path, bytes));
                }
            } else {
                fs::create_dir_all(&out_path)
                    .with_context(|| format!("E_PACKAGE_PATH: {out_path:?}"))?;
                walk(manager, &apk_path, &out_path, expected, deferred_release)?;
            }
        }
        Ok(())
    }
    let root = files.join("data");
    fs::create_dir_all(&root).context("E_PACKAGE_ROOT")?;
    let mut expected = BTreeSet::new();
    let mut deferred_release = None;
    walk(
        &app.asset_manager(),
        "data",
        &root,
        &mut expected,
        &mut deferred_release,
    )?;
    if let Some((path, bytes)) = deferred_release {
        atomic_write(&path, &bytes)?;
    }
    for subdir in ["objects", "releases"] {
        prune_stale_files(&root.join(subdir), &expected);
    }
    Ok(root)
}
