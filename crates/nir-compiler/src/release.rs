use crate::{compile, load_project, runtime_roots, GameManifest, LoadedProject};
use anyhow::{bail, Context, Result};
use nir_format::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
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
        "index.html",
        "bootstrap.js",
        "THIRD-PARTY.txt",
        "compiler.sha256",
    ] {
        let b = fs::read(sdk.join(f))
            .with_context(|| format!("E_SDK_MISSING: {f}; run cargo xtask sdk"))?;
        files.insert(f.into(), nir_content::digest(&b));
    }
    Ok(SdkManifest {
        format: 1,
        compiler_version: env!("CARGO_PKG_VERSION").into(),
        files,
    })
}
pub fn resolve(root: &Path, sdk: &Path) -> Result<GameLock> {
    let p = load_project(root)?;
    let m = sdk_manifest(sdk)?;
    let lock = GameLock {
        format: 1,
        engine_api: p.manifest.engine.api,
        capability_profile: p.manifest.engine.capability_profile,
        sdk_digest: nir_content::digest(&serde_json::to_vec(&m)?),
        sdk: m,
    };
    fs::write(root.join("game.lock"), toml::to_string_pretty(&lock)?)?;
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
fn object(
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
#[derive(Debug, Serialize)]
pub struct BuildReport {
    pub release: String,
    pub game_id: String,
    pub total_bytes: u64,
    pub objects: usize,
    pub resources: Vec<String>,
    pub excluded_resources: Vec<String>,
    pub provenance: BTreeMap<String, String>,
    pub engine_build: String,
    pub resolved_config: crate::ResolvedConfig,
}
pub fn build(root: &Path, sdk: &Path, out: &Path, locked: bool) -> Result<BuildReport> {
    let p = load_project(root)?;
    let lock = if locked {
        check_lock(&p, sdk)?
    } else {
        resolve(root, sdk)?
    };
    fs::create_dir_all(out.join("objects"))?;
    fs::create_dir_all(out.join("releases"))?;
    fs::create_dir_all(out.join("channels"))?;
    let mut objects = BTreeMap::new();
    let roots = runtime_roots(&p.program);
    let mut program = p.program.clone();
    program.assets.retain(|id, _| roots.contains(id));
    for id in &roots {
        let a = &program.assets[id];
        let (ext, mime) = match a.kind {
            AssetKind::Image => ("png", "image/png"),
            AssetKind::Audio => ("wav", "audio/wav"),
            AssetKind::Font => ("otf", "font/otf"),
        };
        object(out, &mut objects, &p.media[id], ext, mime)?;
    }
    let executable = compile(&program)?;
    let program_hash = object(
        out,
        &mut objects,
        &serde_json::to_vec(&executable)?,
        "json",
        "application/json",
    )?;
    // Glue receives the exact WASM URL explicitly; no hashed import rewriting after hashing.
    let wasm = object(
        out,
        &mut objects,
        &fs::read(sdk.join("player_web_bg.wasm"))?,
        "wasm",
        "application/wasm",
    )?;
    let js = object(
        out,
        &mut objects,
        &fs::read(sdk.join("player_web.js"))?,
        "js",
        "text/javascript",
    )?;
    let host = object(
        out,
        &mut objects,
        &fs::read(sdk.join("host.js"))?,
        "js",
        "text/javascript",
    )?;
    let mut notices = fs::read_to_string(sdk.join("THIRD-PARTY.txt"))?;
    for name in &p.manifest.inputs.notices {
        let path = crate::relative(&p.root, &p.root, name)?;
        notices.push_str(&format!("\n\n=== {name} ===\n"));
        notices.push_str(&fs::read_to_string(path)?);
    }
    let notice = object(
        out,
        &mut objects,
        notices.as_bytes(),
        "txt",
        "text/plain; charset=utf-8",
    )?;
    fs::write(out.join("NOTICE.txt"), &notices)?;
    let manifest = ReleaseManifest {
        format: 1,
        game_id: program.game_id.clone(),
        title: p.manifest.game.title.clone(),
        version: p.manifest.game.version.clone(),
        engine_build: lock.sdk_digest.clone(),
        program: program_hash,
        objects: objects.clone(),
        engine: EngineFiles { js, wasm, host },
        notices: vec![notice],
    };
    nir_content::validate_release(&manifest)?;
    let bytes = serde_json::to_vec(&manifest)?;
    let release = nir_content::digest(&bytes);
    fs::write(out.join(format!("releases/{release}.json")), bytes)?;
    for name in ["index.html", "bootstrap.js"] {
        fs::copy(sdk.join(name), out.join(name))?;
    }
    // Atomic local channel replacement, after every referenced object exists.
    let tmp = out.join("channels/stable.json.tmp");
    fs::write(
        &tmp,
        serde_json::to_vec(&serde_json::json!({"format":1,"release":release}))?,
    )?;
    fs::rename(tmp, out.join("channels/stable.json"))?;
    let report = BuildReport {
        release,
        game_id: program.game_id,
        total_bytes: objects.values().map(|o| o.bytes).sum(),
        objects: objects.len(),
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
    };
    let reports = root.join("reports");
    fs::create_dir_all(&reports)?;
    fs::write(
        reports.join("build.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    Ok(report)
}
pub fn copy_tree(src: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    let mut entries: Vec<_> = fs::read_dir(src)?.collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let ty = e.file_type()?;
        let name = e.file_name();
        if ["dist", "reports", ".nir", "game.lock"]
            .iter()
            .any(|v| name == *v)
        {
            continue;
        }
        if ty.is_symlink() {
            bail!("E_TEMPLATE: symlinks not permitted");
        }
        if ty.is_dir() {
            copy_tree(&e.path(), &dest.join(name))?;
        } else {
            fs::copy(e.path(), dest.join(name))?;
        }
    }
    Ok(())
}
pub fn init(dest: &Path, sdk: &Path, id: &str) -> Result<()> {
    if dest.exists() {
        bail!("E_EXISTS: destination already exists");
    }
    let source = sdk.join("template");
    if !source.is_dir() {
        bail!("E_TEMPLATE: build the SDK template first");
    }
    copy_tree(&source, dest)?;
    let path = dest.join("game.toml");
    let mut m: GameManifest = toml::from_str(&fs::read_to_string(&path)?)?;
    m.game.id = id.into();
    fs::write(path, toml::to_string_pretty(&m)?)?;
    fs::write(
        dest.join(".gitignore"),
        ".nir/\ndist/\nreports/\ngame.local.toml\n",
    )?;
    Ok(())
}
pub fn default_sdk() -> PathBuf {
    if let Some(path) = std::env::var_os("NIR_SDK") {
        return path.into();
    }
    if let Ok(exe) = std::env::current_exe() {
        let path = exe.parent().unwrap().join("sdk");
        if path.is_dir() {
            return path;
        }
    }
    PathBuf::from("dist/sdk")
}
