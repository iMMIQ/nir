use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SdkManifest {
    pub format: u32,
    pub compiler_version: String,
    pub files: BTreeMap<String, String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GameLock {
    pub format: u32,
    pub engine_api: String,
    pub capability_profile: String,
    pub sdk_digest: String,
    pub sdk: SdkManifest,
}
pub fn sdk_manifest(sdk: &Path) -> Result<SdkManifest> {
    let mut files = BTreeMap::new();
    for f in [
        "player_web.js",
        "player_web_bg.wasm",
        "host.js",
        "runtime-worker.js",
        "asset-worker.js",
        "index.html",
        "bootstrap.js",
        "style.css",
        "THIRD-PARTY.txt",
        "compiler.sha256",
    ] {
        let b = fs::read(sdk.join(f))
            .with_context(|| format!("E_SDK_MISSING: {f}; run cargo xtask sdk"))?;
        files.insert(f.into(), nir_content::digest(&b));
    }
    let native = sdk.join("windows/player-windows.exe");
    if native.try_exists()? {
        files.insert(
            "windows/player-windows.exe".into(),
            nir_content::digest(&fs::read(native)?),
        );
    }
    let native = sdk.join("linux/player-linux");
    if native.try_exists()? {
        files.insert(
            "linux/player-linux".into(),
            nir_content::digest(&fs::read(native)?),
        );
    }
    let native = sdk.join("android/lib/arm64-v8a/libplayer.so");
    if native.try_exists()? {
        files.insert(
            "android/lib/arm64-v8a/libplayer.so".into(),
            nir_content::digest(&fs::read(native)?),
        );
    }
    Ok(SdkManifest {
        format: 1,
        compiler_version: env!("CARGO_PKG_VERSION").into(),
        files,
    })
}
pub fn resolve(root: &Path, sdk: &Path) -> Result<GameLock> {
    let p = load_project(root)?;
    resolve_loaded(&p, sdk)
}
pub(super) fn resolve_loaded(p: &LoadedProject, sdk: &Path) -> Result<GameLock> {
    let m = sdk_manifest(sdk)?;
    let lock = GameLock {
        format: 1,
        engine_api: p.manifest.engine.api.clone(),
        capability_profile: p.manifest.engine.capability_profile.clone(),
        sdk_digest: nir_content::digest(&serde_json::to_vec(&m)?),
        sdk: m,
    };
    fs::write(p.root.join("game.lock"), toml::to_string_pretty(&lock)?)?;
    Ok(lock)
}
pub fn check_lock(p: &LoadedProject, sdk: &Path) -> Result<GameLock> {
    let lock: GameLock = toml::from_str(
        &fs::read_to_string(p.root.join("game.lock"))
            .context("E_LOCK_MISSING: run novelc resolve explicitly")?,
    )?;
    let current = sdk_manifest(sdk)?;
    if lock.format != 1
        || lock.sdk != current
        || lock.engine_api != p.manifest.engine.api
        || lock.capability_profile != p.manifest.engine.capability_profile
        || lock.sdk_digest != nir_content::digest(&serde_json::to_vec(&current)?)
    {
        bail!("E_LOCK_DRIFT: SDK or engine requirements changed; run novelc resolve");
    }
    Ok(lock)
}
pub(super) fn object(
    out: &Path,
    objects: &mut BTreeMap<String, Object>,
    bytes: &[u8],
    ext: &str,
    mime: &str,
) -> Result<String> {
    let hash = nir_content::digest(bytes);
    let rel = format!("objects/{hash}.{ext}");
    let path = out.join(&rel);
    if path.exists() {
        nir_content::verify(&fs::read(&path)?, &hash)?;
    } else {
        fs::write(&path, bytes)?;
    }
    // Transport variants never participate in the immutable object identity.
    if matches!(ext, "wasm" | "js" | "json") {
        let mut encoder = flate2::GzBuilder::new()
            .mtime(0)
            .write(Vec::new(), flate2::Compression::new(6));
        encoder.write_all(bytes)?;
        let compressed = encoder.finish()?;
        let sidecar = out.join(format!("{rel}.gz"));
        if compressed.len() < bytes.len() {
            // Replace atomically: a preview may be serving this same output.
            let temporary = out.join(format!("{rel}.gz.next"));
            fs::write(&temporary, compressed)?;
            fs::rename(temporary, sidecar)?;
        } else if sidecar.try_exists()? {
            fs::remove_file(sidecar)?;
        }
    }
    objects.insert(
        hash.clone(),
        Object {
            path: rel,
            bytes: bytes.len() as u64,
            media_type: mime.into(),
        },
    );
    Ok(hash)
}
