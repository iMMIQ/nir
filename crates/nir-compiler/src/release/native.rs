use super::*;

/// Platform parameters shared by the Windows and Linux native build paths.
pub(super) struct NativeTarget {
    /// SDK-relative player path, e.g. `windows/player-windows.exe`.
    pub(super) player: &'static str,
    /// Error code prefix, host name and executable format label.
    pub(super) code: &'static str,
    pub(super) host: &'static str,
    pub(super) format: &'static str,
    /// Executable magic checked before packaging (PE `MZ`, ELF `\x7fELF`).
    pub(super) magic: &'static [u8],
    /// Intermediate web build directory under the project's `.nir/`.
    pub(super) intermediate: &'static str,
    /// Packaged binary name (`Game.exe` on Windows, `Game` on Linux).
    pub(super) binary: &'static str,
    pub(super) readme: String,
    /// Report file name under the project's `reports/`.
    pub(super) report: &'static str,
}
/// Reuse content lowering, then publish a native-only graph (no HTML, JS or WASM).
pub fn build_windows(
    root: &Path,
    sdk: &Path,
    out: &Path,
    profile: &str,
    locked: bool,
    optimize: &OptimizeOptions,
) -> Result<BuildReport> {
    build_native(
        root,
        sdk,
        out,
        profile,
        locked,
        &NativeTarget {
            player: "windows/player-windows.exe",
            code: "E_SDK_WINDOWS",
            host: "Windows",
            format: "PE",
            magic: b"MZ",
            intermediate: "windows-content",
            binary: "Game.exe",
            readme: "NIR Windows native player\r\nDouble-click Game.exe. Keep the data folder and NOTICE.txt beside it.\r\nWindows 10/11 x64 with a DirectX 12 capable driver is required.\r\nNo browser, WebView2, web server or network connection is required.\r\nSpace/Enter: advance; Esc: menu; F11: fullscreen; mouse: select; wheel: scroll.\r\nSaves: %LOCALAPPDATA%\\NIR\\games\\<game-id-hash>\\<profile>\\releases\\<release>\\\r\nSaves are isolated by exact release; retain older game folders to continue older saves.\r\n".into(),
            report: "windows-build.json",
        },
        optimize,
    )
}
/// Reuse content lowering, then publish a native-only graph (no HTML, JS or WASM).
pub fn build_linux(
    root: &Path,
    sdk: &Path,
    out: &Path,
    profile: &str,
    locked: bool,
    optimize: &OptimizeOptions,
) -> Result<BuildReport> {
    build_native(
        root,
        sdk,
        out,
        profile,
        locked,
        &NativeTarget {
            player: "linux/player-linux",
            code: "E_SDK_LINUX",
            host: "Linux",
            format: "ELF",
            magic: b"\x7fELF",
            intermediate: "linux-content",
            binary: "Game",
            readme: "NIR Linux native player\nRun ./Game from the bundle directory. Keep the data folder and NOTICE.txt beside it.\nx86_64 Linux with glibc 2.39+ and a Vulkan capable driver is required; audio uses ALSA or PulseAudio.\nNo browser, JavaScript, web server or network connection is required.\nSpace/Enter: advance; Esc: menu; F11: fullscreen; mouse: select; wheel: scroll.\nSaves: $XDG_DATA_HOME/NIR/games/<game-id-hash>/<profile>/releases/<release>/ (default ~/.local/share)\nSaves are isolated by exact release; retain older game folders to continue older saves.\n".into(),
            report: "linux-build.json",
        },
        optimize,
    )
}
/// Deterministic `data/<dir>/<file>` walk in sorted name order.
pub(super) fn collect_files(
    dir: &Path,
    prefix: &str,
    out: &mut Vec<(String, nir_apk::EntrySource)>,
) -> Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        if e.file_type()?.is_dir() {
            collect_files(&e.path(), &format!("{prefix}{name}/"), out)?;
        } else {
            out.push((
                format!("{prefix}{name}"),
                nir_apk::EntrySource::File(e.path()),
            ));
        }
    }
    Ok(())
}
/// Lower through the web path, then package the player and the content graph
/// into a signed APK. No Java, Gradle or Android SDK is needed at build time;
/// the manifest, zip and APK Signature Scheme v2 block all come from
/// `nir-apk`.
pub fn build_android(
    root: &Path,
    sdk: &Path,
    out: &Path,
    profile: &str,
    locked: bool,
    optimize: &OptimizeOptions,
) -> Result<BuildReport> {
    let target = NativeTarget {
        player: "android/lib/arm64-v8a/libplayer.so",
        code: "E_SDK_ANDROID",
        host: "an NDK-equipped host",
        format: "ELF",
        magic: b"\x7fELF",
        intermediate: "android-content",
        binary: "Game.apk",
        readme: "NIR Android native player\nInstall Game.apk on an Android 8.0+ arm64 device (adb install -r Game.apk, or open the file on the device and allow installs from this source).\nA Vulkan capable driver is required; no Java runtime, browser or network connection is used.\nTouch: tap to advance and select; back gesture or button: menu/close.\nGame data ships inside the APK and is extracted to the app's private storage on first launch.\nSaves: Android/data/<package>/files/NIR/games/<game-id-hash>/<profile>/releases/<release>/ inside the app's external storage, visible over USB.\nExported saves land in Android/data/<package>/files/exports/; import is not available on this platform.\nReinstalling a newer build over an installed one (adb install -r) keeps saves; uninstalling deletes them.\nKeep config/android-signing.pem: Android refuses to update an app signed with a different key.\n".into(),
        report: "android-build.json",
    };
    let bundle = native_release(root, sdk, profile, locked, &target, optimize)?;
    // The signing identity is project state, not build output: generate once,
    // then reuse forever. Losing it strands installed copies (updates must be
    // signed with the same key), so it lives under config/ beside game.toml.
    // NIR_ANDROID_SIGNING_SEED overrides the file for reproducible sample
    // builds (CI pins it so published sample APKs stay byte-identical and
    // update-compatible); its holder can sign for the sample, which is
    // acceptable only for throwaway example identities.
    let key_path = root.join("config/android-signing.pem");
    let key = if let Ok(seed) = std::env::var("NIR_ANDROID_SIGNING_SEED") {
        if key_path.exists() {
            eprintln!(
                "Warning: NIR_ANDROID_SIGNING_SEED overrides existing {}",
                key_path.display()
            );
        }
        nir_apk::key::SigningIdentity::from_seed_phrase(&seed)
            .context("E_SIGNING_KEY: NIR_ANDROID_SIGNING_SEED is not usable")?
    } else if key_path.exists() {
        nir_apk::key::SigningIdentity::load(&key_path)
            .context("E_SIGNING_KEY: config/android-signing.pem is not a valid signing identity")?
    } else {
        let key = nir_apk::key::SigningIdentity::generate()?;
        fs::create_dir_all(key_path.parent().unwrap())?;
        // Create atomically with mode 0o600 on Unix so no world-readable window
        // exists and a concurrent first build cannot overwrite the winning key.
        let _ = key
            .save_new(&key_path)
            .context("E_SIGNING_KEY: could not write config/android-signing.pem")?;
        nir_apk::key::SigningIdentity::load(&key_path)
            .context("E_SIGNING_KEY: could not reload config/android-signing.pem")?
    };
    // Stage the exact desktop data layout, then zip it as APK assets; the
    // player extracts assets/data/** on first launch and verifies digests.
    // The staging tree is rebuilt from scratch so objects dropped from the
    // game cannot linger from an earlier build (the walk below packages
    // whatever is on disk, not the manifest).
    let stage = root.join(".nir/android-content/package");
    if stage.try_exists()? {
        fs::remove_dir_all(&stage)?;
    }
    write_native_data(&bundle, &stage.join("data"))?;
    // The library streams from the SDK copy (page-aligned for the loader);
    // its digest was checked against the manifest above.
    let mut entries = vec![(
        "lib/arm64-v8a/libplayer.so".to_owned(),
        nir_apk::EntrySource::File(sdk.join(target.player)),
    )];
    collect_files(&stage, "assets/", &mut entries)?;
    // A human-readable notice beside the hashed object copy, so a shared APK
    // carries its license text without unpacking the object graph.
    entries.push((
        "assets/NOTICE.txt".to_owned(),
        nir_apk::EntrySource::File(bundle.intermediate.join("NOTICE.txt")),
    ));
    // Package name is derived from the game id so rebuilds update in place
    // and distinct games never collide; the `g` prefix keeps the segment
    // leading-letter (hex may start with a digit). min/target SDK match the
    // player's AAudio (26) and the no-Java NativeActivity layout (29).
    let android_cfg = fs::read_to_string(root.join("game.toml"))
        .ok()
        .and_then(|s| toml::from_str::<GameManifest>(&s).ok())
        .and_then(|m| m.android);
    let version_code = android_cfg.as_ref().map_or(1, |c| c.version_code).max(1);
    let min_sdk = android_cfg.as_ref().map_or(26, |c| c.min_sdk).max(26);
    let target_sdk = android_cfg
        .as_ref()
        .map_or(29, |c| c.target_sdk)
        .max(min_sdk);
    let manifest = nir_apk::ManifestSpec {
        package: format!(
            "one.nir.g{}",
            &nir_content::digest(bundle.manifest.game_id.as_bytes())[..15]
        ),
        version_code,
        version_name: bundle.manifest.version.clone(),
        label: bundle.manifest.title.clone(),
        min_sdk,
        target_sdk,
        lib_name: "player".to_owned(),
    };
    nir_apk::build_apk(&out.join("Game.apk"), &manifest, &entries, &key)?;
    fs::copy(
        bundle.intermediate.join("NOTICE.txt"),
        out.join("NOTICE.txt"),
    )?;
    fs::write(out.join("README.txt"), target.readme.clone())?;
    fs::write(
        root.join("reports").join(target.report),
        serde_json::to_vec_pretty(&bundle.report)?,
    )?;
    Ok(bundle.report)
}
/// Shared prefix of every native build: read and sanity-check the platform
/// player, lower the content through the web path into the platform's
/// intermediate directory, strip the web-only engine objects, and derive the
/// native release manifest plus the adjusted report the caller completes.
pub(super) struct NativeBundle {
    /// The platform's intermediate web-build directory under `.nir/`.
    pub(super) intermediate: PathBuf,
    pub(super) manifest: NativeRelease,
    pub(super) report: BuildReport,
    pub(super) player: Vec<u8>,
    /// Serialized `NativeRelease`; its digest becomes `report.release`.
    pub(super) release_bytes: Vec<u8>,
}
pub(super) fn native_release(
    root: &Path,
    sdk: &Path,
    profile: &str,
    locked: bool,
    target: &NativeTarget,
    optimize: &OptimizeOptions,
) -> Result<NativeBundle> {
    if !matches!(profile, "dev" | "release") {
        bail!("E_PROFILE: expected dev or release");
    }
    if profile == "release" && !locked {
        bail!("E_RELEASE_LOCK: release builds require --locked");
    }
    let player = fs::read(sdk.join(target.player)).with_context(|| {
        format!(
            "{}: {} player missing; build the SDK on {}",
            target.code, target.host, target.host
        )
    })?;
    anyhow::ensure!(
        player.starts_with(target.magic),
        "{}: invalid {} executable",
        target.code,
        target.format
    );
    let mut p = load_project(root)?;
    let lock = if locked {
        check_lock(&p, sdk)?
    } else {
        resolve_loaded(&p, sdk)?
    };
    let intermediate = root.join(".nir").join(target.intermediate);
    fs::create_dir_all(intermediate.join("objects"))?;
    let mut objects = BTreeMap::new();
    let roots = runtime_roots(&p.program);
    let (optimization, stored) = crate::optimize::optimize_media(
        &mut p,
        &roots,
        optimize,
        &root.join(".nir").join("cache").join("optimize"),
    )?;
    let ships =
        |media: crate::optimize::StoredMedia| stored.values().any(|stored| *stored == media);
    p.program.requires.retain(|cap| {
        (cap != "media.webp.v1" || ships(crate::optimize::StoredMedia::Webp))
            && (cap != "media.mp3.v1" || ships(crate::optimize::StoredMedia::Mp3))
    });
    for id in &roots {
        let a = p
            .program
            .assets
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("E_ASSET_UNDECLARED: runtime dependency {id}"))?;
        let (ext, mime) = match stored.get(id).copied() {
            Some(media) => (media.ext(), media.mime()),
            None => match a.kind {
                AssetKind::Font if p.media[id].starts_with(b"OTTO") => ("otf", "font/otf"),
                AssetKind::Font => ("ttf", "font/ttf"),
                _ => unreachable!("media assets always have a stored format"),
            },
        };
        object(&intermediate, &mut objects, &p.media[id], ext, mime)?;
    }
    let reference = compile(&p.program)?;
    let runtime = package_runtime(&intermediate, &mut objects, &p.program, &reference)?;
    validate_runtime_packages(&intermediate, &runtime.executable)?;
    let game_id = p.program.game_id.clone();
    let program_hash = object(
        &intermediate,
        &mut objects,
        &serde_json::to_vec(&runtime.executable)?,
        "json",
        "application/json",
    )?;
    let mut notices = fs::read_to_string(sdk.join("THIRD-PARTY.txt"))?;
    for name in &p.manifest.inputs.notices {
        let path = crate::relative(&p.root, &p.root, name)?;
        notices.push_str(&format!("\n\n=== {name} ===\n"));
        notices.push_str(&fs::read_to_string(path)?);
    }
    for (id, license) in &p.font_notices {
        if roots.contains(id) {
            notices.push_str(&format!("\n\n=== Font {id} ===\n{license}"));
        }
    }
    object(
        &intermediate,
        &mut objects,
        notices.as_bytes(),
        "txt",
        "text/plain; charset=utf-8",
    )?;
    fs::write(intermediate.join("NOTICE.txt"), &notices)?;
    let manifest = NativeRelease {
        format: 1,
        game_id: game_id.clone(),
        title: p.manifest.game.title.clone(),
        version: p.manifest.game.version.clone(),
        profile: profile.into(),
        engine_build: lock.sdk_digest.clone(),
        player: nir_content::digest(&player),
        program: program_hash,
        objects,
    };
    let release_bytes = serde_json::to_vec(&manifest)?;
    let release = nir_content::digest(&release_bytes);
    let report = BuildReport {
        release,
        game_id,
        total_bytes: manifest.objects.values().map(|o| o.bytes).sum::<u64>() + player.len() as u64,
        objects: manifest.objects.len(),
        module_packages: runtime.modules,
        asset_catalogs: runtime.catalogs,
        resources: roots.iter().cloned().collect(),
        excluded_resources: p
            .program
            .assets
            .keys()
            .filter(|id| !roots.contains(*id))
            .cloned()
            .collect(),
        provenance: p
            .provenance
            .into_iter()
            .filter(|(id, _)| roots.contains(id))
            .collect(),
        engine_build: lock.sdk_digest,
        resolved_config: p.resolved_config,
        fonts: p
            .fonts
            .into_iter()
            .filter(|(id, _)| roots.contains(id))
            .collect(),
        optimization,
    };
    Ok(NativeBundle {
        intermediate,
        manifest,
        report,
        player,
        release_bytes,
    })
}
/// Writes the shared native data tree (objects, release manifests, pointer)
/// into `data/`; both the desktop folders and the Android APK staging area
/// lay the content out exactly this way.
pub(super) fn write_native_data(bundle: &NativeBundle, data: &Path) -> Result<()> {
    fs::create_dir_all(data.join("objects"))?;
    fs::create_dir_all(data.join("releases"))?;
    for (id, object) in &bundle.manifest.objects {
        let bytes = fs::read(bundle.intermediate.join(&object.path))?;
        nir_content::verify(&bytes, id)?;
        fs::write(data.join(&object.path), bytes)?;
    }
    fs::write(
        data.join(format!("releases/{}.json", bundle.report.release)),
        &bundle.release_bytes,
    )?;
    fs::write(
        data.join("release.txt.next"),
        format!("{}\n", bundle.report.release),
    )?;
    fs::rename(data.join("release.txt.next"), data.join("release.txt"))?;
    Ok(())
}
pub(super) fn build_native(
    root: &Path,
    sdk: &Path,
    out: &Path,
    profile: &str,
    locked: bool,
    target: &NativeTarget,
    optimize: &OptimizeOptions,
) -> Result<BuildReport> {
    let bundle = native_release(root, sdk, profile, locked, target, optimize)?;
    write_native_data(&bundle, &out.join("data"))?;
    fs::write(out.join(format!("{}.next", target.binary)), &bundle.player)?;
    fs::rename(
        out.join(format!("{}.next", target.binary)),
        out.join(target.binary),
    )?;
    // Executables packaged on a Unix host must keep the execute bit through
    // archive extraction; Windows hosts have no mode bit to preserve.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(out.join(target.binary), fs::Permissions::from_mode(0o755))
            .with_context(|| format!("{}: mark {} executable", target.code, target.binary))?;
    }
    fs::copy(
        bundle.intermediate.join("NOTICE.txt"),
        out.join("NOTICE.txt"),
    )?;
    fs::write(out.join("README.txt"), target.readme.clone())?;
    fs::write(
        root.join("reports").join(target.report),
        serde_json::to_vec_pretty(&bundle.report)?,
    )?;
    Ok(bundle.report)
}
