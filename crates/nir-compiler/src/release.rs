use crate::{compile, load_project, runtime_roots, GameManifest, LoadedProject, OptimizeOptions};
use anyhow::{bail, Context, Result};
use nir_format::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[cfg(test)]
mod compression_tests;
mod lock;
mod native;
mod pack;
pub use lock::{check_lock, resolve, sdk_manifest, GameLock, SdkManifest};
use lock::{object, resolve_loaded};
pub use native::{build_android, build_linux, build_windows};
use pack::*;
pub use pack::{AssetCatalogBuildReport, BuildReport, ModuleBuildReport, TextBuildReport};

pub fn build(root: &Path, sdk: &Path, out: &Path, locked: bool) -> Result<BuildReport> {
    build_profile(
        root,
        sdk,
        out,
        if locked { "release" } else { "dev" },
        locked,
        true,
        &OptimizeOptions::default(),
    )
}

pub fn build_profile(
    root: &Path,
    sdk: &Path,
    out: &Path,
    profile: &str,
    locked: bool,
    promote: bool,
    optimize: &OptimizeOptions,
) -> Result<BuildReport> {
    if !matches!(profile, "dev" | "release") {
        bail!("E_PROFILE: expected dev or release");
    }
    if profile == "release" && !locked {
        bail!("E_RELEASE_LOCK: release builds require --locked");
    }
    let mut p = load_project(root)?;
    let lock = if locked {
        check_lock(&p, sdk)?
    } else {
        resolve_loaded(&p, sdk)?
    };
    fs::create_dir_all(out.join("objects"))?;
    fs::create_dir_all(out.join("releases"))?;
    fs::create_dir_all(out.join("channels"))?;
    let mut objects = BTreeMap::new();
    let roots = runtime_roots(&p.program);
    // Ship-format conversion runs before compilation so descriptors, catalog
    // digest and capabilities all reflect the packaged bytes.
    let (optimization, stored) = crate::optimize::optimize_media(
        &mut p,
        &roots,
        optimize,
        &root.join(".nir").join("cache").join("optimize"),
    )?;
    // `load_project` seeds requires with the whole capability list and prunes
    // to actual use; the container capabilities follow the packaged bytes, so a
    // release with no WebP/MP3 objects stays playable on older players.
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
        object(out, &mut objects, &p.media[id], ext, mime)?;
    }
    // Compile and validate the complete source representation first. Runtime
    // lowering is a packaging transform and must not hide invalid references.
    let reference = compile(&p.program)?;
    let runtime = package_runtime(out, &mut objects, &p.program, &reference)?;
    validate_runtime_packages(out, &runtime.executable)?;
    let game_id = p.program.game_id.clone();
    let program_hash = object(
        out,
        &mut objects,
        &serde_json::to_vec(&runtime.executable)?,
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
    let runtime_worker = object(
        out,
        &mut objects,
        &fs::read(sdk.join("runtime-worker.js"))?,
        "js",
        "text/javascript",
    )?;
    let asset_worker = object(
        out,
        &mut objects,
        &fs::read(sdk.join("asset-worker.js"))?,
        "js",
        "text/javascript",
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
    let notice = object(
        out,
        &mut objects,
        notices.as_bytes(),
        "txt",
        "text/plain; charset=utf-8",
    )?;
    fs::write(out.join("NOTICE.txt"), &notices)?;
    let html = object(
        out,
        &mut objects,
        &fs::read(sdk.join("index.html"))?,
        "html",
        "text/html; charset=utf-8",
    )?;
    let bootstrap = object(
        out,
        &mut objects,
        &fs::read(sdk.join("bootstrap.js"))?,
        "js",
        "text/javascript",
    )?;
    let style = object(
        out,
        &mut objects,
        &fs::read(sdk.join("style.css"))?,
        "css",
        "text/css; charset=utf-8",
    )?;
    let manifest = ReleaseManifest {
        format: 1,
        profile: profile.into(),
        game_id: game_id.clone(),
        title: p.manifest.game.title.clone(),
        version: p.manifest.game.version.clone(),
        engine_build: lock.sdk_digest.clone(),
        program: program_hash.clone(),
        objects: objects.clone(),
        engine: EngineFiles {
            js: js.clone(),
            wasm: wasm.clone(),
            host: host.clone(),
            runtime_worker: Some(runtime_worker),
            asset_worker: Some(asset_worker),
        },
        launch: LaunchFiles {
            html: html.clone(),
            bootstrap: bootstrap.clone(),
            style: Some(style.clone()),
        },
        notices: vec![notice],
    };
    nir_content::validate_release(&manifest)?;
    let bytes = serde_json::to_vec(&manifest)?;
    let release = nir_content::digest(&bytes);
    let release_path = out.join(format!("releases/{release}.json"));
    if release_path.exists() {
        nir_content::verify(&fs::read(&release_path)?, &release)?;
    } else {
        fs::write(&release_path, &bytes)?;
    }
    let fixed = out.join(format!("releases/{release}"));
    fs::create_dir_all(&fixed)?;
    for (name, hash) in [
        ("index.html", &html),
        ("bootstrap.js", &bootstrap),
        ("style.css", &style),
    ] {
        let source = out.join(&objects[hash].path);
        let target = fixed.join(name);
        if target.exists() {
            nir_content::verify(&fs::read(&target)?, hash)?;
        } else {
            fs::copy(source, target)?;
        }
    }
    for name in ["index.html", "bootstrap.js", "style.css"] {
        fs::copy(sdk.join(name), out.join(name))?;
    }
    for (hash, descriptor) in &objects {
        let bytes = fs::read(out.join(&descriptor.path))?;
        if bytes.len() as u64 != descriptor.bytes {
            bail!("E_OBJECT_SIZE: {hash}");
        }
        nir_content::verify(&bytes, hash)?;
    }
    let dependencies = dependency_report(
        out,
        &release,
        &program_hash,
        &manifest.engine,
        &objects,
        &runtime,
    )?;
    let report = BuildReport {
        release,
        game_id,
        total_bytes: objects.values().map(|o| o.bytes).sum(),
        objects: objects.len(),
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
    let reports = root.join("reports");
    fs::create_dir_all(&reports)?;
    fs::write(
        reports.join("build.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    fs::write(
        reports.join("dependencies.json"),
        serde_json::to_vec_pretty(&dependencies)?,
    )?;
    // A channel is the last write, after the complete immutable graph and reports.
    if promote {
        let tmp = out.join("channels/stable.json.tmp");
        fs::write(
            &tmp,
            serde_json::to_vec(&serde_json::json!({"format":1,"release":report.release}))?,
        )?;
        fs::rename(tmp, out.join("channels/stable.json"))?;
    }
    Ok(report)
}

pub fn copy_tree(src: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    let mut entries: Vec<_> = fs::read_dir(src)?.collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let ty = e.file_type()?;
        let name = e.file_name();
        if [
            "dist",
            "reports",
            ".nir",
            "game.lock",
            ".git",
            "target",
            "node_modules",
            // Machine-local signing identity, never redistributed in templates.
            "android-signing.pem",
        ]
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
    init_template(dest, sdk, id, "minimal")
}
pub fn init_template(dest: &Path, sdk: &Path, id: &str, template: &str) -> Result<()> {
    let directory = match template {
        "web-basic" => "template",
        "minimal" => "templates/minimal",
        _ => bail!("E_TEMPLATE: expected minimal or web-basic"),
    };
    if dest.exists() {
        bail!("E_EXISTS: destination already exists");
    }
    let source = sdk.join(directory);
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
        ".nir/\ndist/\nreports/\ngame.local.toml\nconfig/android-signing.pem\n",
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
