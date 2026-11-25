//! Zero-boilerplate NIR v2 project compiler, auto-optimizer, and multi-target packager.
//!
//! A complete game project needs ONLY:
//! - `game.toml` (~10 lines)
//! - `scripts/*.nirs`
//! - `assets/` (auto-discovered: PNG/WebP/JPG/WAV/MP3/OGG/FLAC/TTF/OTF/MP4/WebM)
//! - optional `locales/<lang>.toml` & `ui/theme.toml`
#![forbid(unsafe_code)]

pub mod delta;
pub mod ktx2_enc;
pub mod nirs;
pub mod optimize;
pub mod opus_enc;

use anyhow::{bail, Context, Result};
use nir_ir::*;
use optimize::{
    build_embedded_game_cjk_font, decode_to_rgba, detect_asset_kind_and_codec,
    effective_webp_quality, encode_webp_rgba_q, fp_audio_cache_path_for_bus, fp_image_cache_path,
    generate_gallery_thumbnail, optimize_audio_for_bus, optimize_decoded_pixels, optimize_image,
    read_audio_cache_meta_only, read_cached_payload, read_image_cache_meta_only, sha256_bytes,
    stream_cached_payload, subset_font, write_audio_cache, write_image_cache, CachedPayloadRef,
    OptimizeConfig, RawPixels, OPTIMIZER_VERSION,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectManifest {
    pub game: GameSection,
    pub theme: ThemeSection,
    pub android: AndroidSection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GameSection {
    pub id: String,
    pub title: String,
    pub version: String,
    pub locale: String,
    pub stage: [u32; 2],
    pub save_slots: u32,
    pub default_reveal_ms: u32,
    pub title_bg: Option<String>,
    pub title_bgm: Option<String>,
    pub font: Option<String>,
}

impl Default for GameSection {
    fn default() -> Self {
        Self {
            id: "org.example.story".into(),
            title: "Untitled Story".into(),
            version: "0.1.0".into(),
            locale: "zh-Hans".into(),
            stage: [1280, 720],
            save_slots: 12,
            default_reveal_ms: 28,
            title_bg: None,
            title_bgm: None,
            font: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeSection {
    pub accent: Option<String>,
    pub dialogue_bg: Option<String>,
    pub font_size: Option<f32>,
    pub line_height: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AndroidSection {
    pub version_code: u32,
    pub min_sdk: u32,
    pub target_sdk: u32,
    /// Optional Android screen orientation (`sensorLandscape`, `fullSensor`, `landscape`, `portrait`).
    pub orientation: Option<String>,
}

impl Default for AndroidSection {
    fn default() -> Self {
        Self {
            version_code: 1,
            min_sdk: 26,
            target_sdk: 34,
            orientation: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildSummary {
    pub game_id: String,
    pub title: String,
    pub scripts: usize,
    pub ops: usize,
    pub texts: usize,
    pub total_assets_scanned: usize,
    pub packed_assets: usize,
    pub pruned_assets: usize,
    pub trimmed_sprites: usize,
    pub delta_patched: usize,
    pub generated_thumbs: usize,
    pub saved_bytes: i64,
    pub nirpak_bytes: usize,
}

pub struct BuiltBundle {
    pub program: NirProgram,
    pub media_blobs: Vec<Vec<u8>>,
    pub nirpak: Vec<u8>,
    pub summary: BuildSummary,
}

/// Direct decoded raster pixels supplied to `build_direct_program` (avoids PNG round-trip).
pub enum DirectPixels {
    Rgb {
        width: u32,
        height: u32,
        data: Vec<u8>,
    },
    Rgba {
        width: u32,
        height: u32,
        data: Vec<u8>,
    },
}

/// Asset input for `build_direct_program`, decoupled from filesystem layout or importer crates.
pub struct DirectAssetSource {
    pub name: String,
    pub kind: AssetKind,
    pub codec: MediaCodec,
    pub loader: Arc<dyn Fn() -> Result<Vec<u8>> + Send + Sync>,
    /// Optional direct pixel decoder bypassing PNG encode/decode round-trips.
    pub pixel_loader: Option<Arc<dyn Fn() -> Result<DirectPixels> + Send + Sync>>,
    /// Optional `(file_len, mtime_nanos)` fingerprint enabling instant pre-decode cache hits!
    pub source_fingerprint: Option<(u64, u64)>,
}

struct DiscoveredAsset {
    name: String,
    path: PathBuf,
    kind: AssetKind,
    codec: MediaCodec,
}

/// Scans `root/assets/**` automatically — zero manual `catalog.toml` required!
fn scan_assets(root: &Path) -> Result<Vec<DiscoveredAsset>> {
    let assets_dir = root.join("assets");
    if !assets_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    collect_files(&assets_dir, &mut files)?;
    files.sort();
    let mut out = Vec::new();
    for path in files {
        let Some((kind, codec)) = detect_asset_kind_and_codec(&path) else {
            continue;
        };
        let rel = path.strip_prefix(&assets_dir)?;
        let rel_no_ext = rel.with_extension("");
        let name = rel_no_ext
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        out.push(DiscoveredAsset {
            name,
            path,
            kind,
            codec,
        });
    }
    Ok(out)
}

fn scan_scripts(root: &Path) -> Result<Vec<nirs::ScriptFile>> {
    let scripts_dir = root.join("scripts");
    if !scripts_dir.is_dir() {
        bail!(
            "E_PROJECT: missing `scripts/` directory in {}",
            root.display()
        );
    }
    let mut files = Vec::new();
    collect_files(&scripts_dir, &mut files)?;
    files.retain(|p| p.extension().and_then(|e| e.to_str()) == Some("nirs"));
    // Put `main.nirs` first so default entry starts at PC 0 cleanly!
    files.sort_by(|a, b| {
        let a_main = a.file_name().and_then(|s| s.to_str()) == Some("main.nirs");
        let b_main = b.file_name().and_then(|s| s.to_str()) == Some("main.nirs");
        b_main.cmp(&a_main).then_with(|| a.cmp(b))
    });
    if files.is_empty() {
        bail!("E_PROJECT: no `.nirs` script files found under scripts/");
    }
    files
        .into_iter()
        .map(|p| {
            let rel = p.strip_prefix(&scripts_dir).unwrap_or(&p);
            let name = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            nirs::load_script_file(&p, name)
        })
        .collect()
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        if p.is_dir() {
            collect_files(&p, out)?;
        } else if p.is_file() {
            out.push(p);
        }
    }
    Ok(())
}

/// Compiles, optimizes, tree-shakes, and packs a NIR v2 project into `.nirpak`!
pub fn compile_project(root: &Path, opt: &OptimizeConfig) -> Result<BuiltBundle> {
    compile_project_with_compression(root, opt, flate2::Compression::default())
}

/// Ultra-fast compilation preset for `novelc dev` live hot-reload:
/// skips transcoding, reads image headers only, and uses fast zlib compression.
pub fn compile_project_fast(root: &Path) -> Result<BuiltBundle> {
    let opt = OptimizeConfig {
        enabled: false,
        ..Default::default()
    };
    compile_project_with_compression(root, &opt, flate2::Compression::fast())
}

fn compile_project_with_compression(
    root: &Path,
    opt: &OptimizeConfig,
    compression: flate2::Compression,
) -> Result<BuiltBundle> {
    let manifest_path = root.join("game.toml");
    let manifest: ProjectManifest = if manifest_path.is_file() {
        toml::from_str(&fs::read_to_string(&manifest_path)?)
            .with_context(|| format!("parsing {}", manifest_path.display()))?
    } else {
        ProjectManifest::default()
    };

    let discovered = scan_assets(root)?;
    let total_scanned = discovered.len();
    let resolver_pairs: Vec<(String, AssetId)> = discovered
        .iter()
        .enumerate()
        .map(|(i, a)| (a.name.clone(), AssetId(i as u32)))
        .collect();
    let resolver = nirs::AssetResolver::from_assets(&resolver_pairs);

    let scripts = scan_scripts(root)?;
    let mut compiled = nirs::compile_scripts(&scripts, &resolver, manifest.game.stage)?;

    // Resolve optional title/theme/font assets
    let mut used = compiled.used_assets.clone();
    let mut resolve_opt = |name: &Option<String>| -> Option<AssetId> {
        let n = name.as_deref()?;
        let id = resolver.resolve(n)?;
        used.insert(id);
        Some(id)
    };
    let title_bg = resolve_opt(&manifest.game.title_bg);
    let title_bgm = resolve_opt(&manifest.game.title_bgm);
    let dialogue_bg = resolve_opt(&manifest.theme.dialogue_bg);

    // Auto-pick bundled fonts: explicit `game.font` OR any discovered font under `assets/fonts/`!
    let mut fonts = Vec::new();
    if let Some(fid) = resolve_opt(&manifest.game.font) {
        fonts.push(fid);
    } else {
        for (i, a) in discovered.iter().enumerate() {
            if a.kind == AssetKind::Font {
                let id = AssetId(i as u32);
                fonts.push(id);
                used.insert(id);
            }
        }
    }

    // Build optional translations from `locales/<lang>.toml`
    let mut texts_by_locale = BTreeMap::new();
    texts_by_locale.insert(manifest.game.locale.clone(), compiled.texts.clone());
    let mut locales = vec![manifest.game.locale.clone()];
    let locales_dir = root.join("locales");
    if locales_dir.is_dir() {
        for entry in fs::read_dir(&locales_dir)? {
            let p = entry?.path();
            if p.extension().and_then(|e| e.to_str()) == Some("toml") {
                if let Some(lang) = p.file_stem().and_then(|s| s.to_str()) {
                    let map: BTreeMap<String, String> = toml::from_str(&fs::read_to_string(&p)?)?;
                    let localized: Vec<TextEntry> = compiled
                        .texts
                        .iter()
                        .enumerate()
                        .map(|(idx, orig)| {
                            let key = format!("t{idx}");
                            map.get(&key)
                                .map(|s| TextEntry::plain(s.clone()))
                                .unwrap_or_else(|| orig.clone())
                        })
                        .collect();
                    if !locales.iter().any(|l| l == lang) {
                        locales.push(lang.to_owned());
                    }
                    texts_by_locale.insert(lang.to_owned(), localized);
                }
            }
        }
    }

    // Tree-shaking: remap only used assets to dense sequential IDs 0..N!
    let mut remap = BTreeMap::<AssetId, AssetId>::new();
    let mut kept_indices = Vec::new();
    for old_id in &used {
        let new_id = AssetId(kept_indices.len() as u32);
        remap.insert(*old_id, new_id);
        kept_indices.push(old_id.0 as usize);
    }
    remap_ops_assets(&mut compiled.ops, &remap);
    let remap_opt = |o: Option<AssetId>| o.and_then(|id| remap.get(&id).copied());
    let ui_fonts: Vec<AssetId> = fonts
        .iter()
        .filter_map(|id| remap.get(id).copied())
        .collect();

    let mut ui = UiTheme::default();
    if let Some(hex) = &manifest.theme.accent {
        if let Some(c) = nirs::parse_hex_color(hex) {
            ui.accent = c;
        }
    }
    ui.dialogue_bg = remap_opt(dialogue_bg);
    if let Some(fsz) = manifest.theme.font_size {
        ui.font_size = fsz;
    }
    if let Some(lh) = manifest.theme.line_height {
        ui.line_height = lh;
    }

    // Collect characters directly without cloning NirProgram!
    let has_fonts = kept_indices
        .iter()
        .any(|&idx| discovered[idx].kind == AssetKind::Font);
    let used_chars = if opt.enabled && opt.subset_fonts && has_fonts {
        collect_all_chars_parts(&manifest.game.title, &compiled.strings, &texts_by_locale)
    } else {
        std::collections::BTreeSet::new()
    };

    // Optimize all kept assets in parallel across CPU cores (preserving deterministic ID order)!
    let cache_dir = root.join(".nir").join("cache");
    struct AssetWorkOutput {
        bytes: Vec<u8>,
        codec: MediaCodec,
        width: u32,
        height: u32,
        trim: Option<SpriteTrim>,
        duration_us: u64,
        sample_rate: u32,
        channels: u16,
        loop_region: Option<AudioLoopRegion>,
        saved_bytes: i64,
        features: u32,
        trimmed: bool,
    }

    // Infer per-asset audio bus so BGM receives higher mono floor & MDCT bias and skips silence trim.
    let remapped_title_bgm = remap_opt(title_bgm);
    let inferred_buses = infer_audio_buses_from_ops(remapped_title_bgm, &compiled.ops);

    let optimize_one = |new_idx: usize, orig_idx: usize| -> Result<AssetWorkOutput> {
        let src = &discovered[orig_idx];
        let raw =
            fs::read(&src.path).with_context(|| format!("reading asset {}", src.path.display()))?;
        let mut features = 0u32;
        let mut trimmed = false;
        let out = match src.kind {
            AssetKind::Image => {
                let res = optimize_image(&raw, src.codec, opt, &cache_dir)?;
                let saved_bytes = res.saved_bytes;
                match res.codec {
                    MediaCodec::Webp => features |= FEATURE_WEBP,
                    MediaCodec::Ktx2 => features |= FEATURE_KTX2,
                    _ => {}
                }
                if res.trim.is_some() {
                    features |= FEATURE_TRIMMED;
                    trimmed = true;
                }
                AssetWorkOutput {
                    bytes: res.bytes,
                    codec: res.codec,
                    width: res.width,
                    height: res.height,
                    trim: res.trim,
                    duration_us: 0,
                    sample_rate: 0,
                    channels: 0,
                    loop_region: None,
                    saved_bytes,
                    features,
                    trimmed,
                }
            }
            AssetKind::Audio => {
                let bus = inferred_buses.get(&AssetId(new_idx as u32)).copied();
                let res = optimize_audio_for_bus(&raw, src.codec, bus, opt, &cache_dir)?;
                let saved_bytes = res.saved_bytes;
                match res.codec {
                    MediaCodec::Mp3 => features |= FEATURE_MP3,
                    MediaCodec::Ogg => features |= FEATURE_OGG,
                    MediaCodec::Opus => features |= FEATURE_OPUS,
                    _ => {}
                }
                AssetWorkOutput {
                    bytes: res.bytes,
                    codec: res.codec,
                    width: 0,
                    height: 0,
                    trim: None,
                    duration_us: res.duration_us,
                    sample_rate: res.sample_rate,
                    channels: res.channels,
                    loop_region: res.loop_region,
                    saved_bytes,
                    features,
                    trimmed,
                }
            }
            AssetKind::Font => {
                let orig_len = raw.len() as i64;
                let sub = subset_font(&raw, &used_chars, opt, &cache_dir)?;
                let saved_bytes = orig_len - sub.len() as i64;
                AssetWorkOutput {
                    bytes: sub,
                    codec: src.codec,
                    width: 0,
                    height: 0,
                    trim: None,
                    duration_us: 0,
                    sample_rate: 0,
                    channels: 0,
                    loop_region: None,
                    saved_bytes,
                    features,
                    trimmed,
                }
            }
            AssetKind::Video => {
                features |= FEATURE_VIDEO;
                AssetWorkOutput {
                    bytes: raw,
                    codec: src.codec,
                    width: 0,
                    height: 0,
                    trim: None,
                    duration_us: 0,
                    sample_rate: 0,
                    channels: 0,
                    loop_region: None,
                    saved_bytes: 0,
                    features,
                    trimmed: false,
                }
            }
            AssetKind::Raw => AssetWorkOutput {
                bytes: raw,
                codec: src.codec,
                width: 0,
                height: 0,
                trim: None,
                duration_us: 0,
                sample_rate: 0,
                channels: 0,
                loop_region: None,
                saved_bytes: 0,
                features: 0,
                trimmed: false,
            },
        };
        Ok(out)
    };

    let worker_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(kept_indices.len().max(1));

    let results: Vec<Result<AssetWorkOutput>> = if worker_count <= 1 || kept_indices.len() <= 1 {
        kept_indices
            .iter()
            .enumerate()
            .map(|(new_idx, &orig_idx)| optimize_one(new_idx, orig_idx))
            .collect()
    } else {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Mutex;
        let next_task = AtomicUsize::new(0);
        let slots: Vec<Mutex<Option<Result<AssetWorkOutput>>>> =
            (0..kept_indices.len()).map(|_| Mutex::new(None)).collect();
        std::thread::scope(|s| {
            for _ in 0..worker_count {
                s.spawn(|| loop {
                    let i = next_task.fetch_add(1, Ordering::Relaxed);
                    if i >= kept_indices.len() {
                        break;
                    }
                    let res = optimize_one(i, kept_indices[i]);
                    *slots[i].lock().unwrap_or_else(|e| e.into_inner()) = Some(res);
                });
            }
        });
        slots
            .into_iter()
            .map(|m| m.into_inner().unwrap_or_else(|e| e.into_inner()).unwrap())
            .collect()
    };

    let mut strings = compiled.strings;
    let mut assets_meta = Vec::with_capacity(kept_indices.len());
    let mut media_blobs = Vec::with_capacity(kept_indices.len());
    let mut features = 0u32;
    let mut trimmed_sprites = 0usize;
    let mut saved_bytes = 0i64;

    for (new_idx, (&orig_idx, res)) in kept_indices.iter().zip(results).enumerate() {
        let src = &discovered[orig_idx];
        let out = res?;
        let name_str = StrId(strings.len() as u32);
        strings.push(src.name.clone());
        features |= out.features;
        saved_bytes += out.saved_bytes;
        if out.trimmed {
            trimmed_sprites += 1;
        }
        assets_meta.push(AssetMeta {
            id: AssetId(new_idx as u32),
            name: name_str,
            kind: src.kind,
            codec: out.codec,
            width: out.width,
            height: out.height,
            trim: out.trim,
            duration_us: out.duration_us,
            sample_rate: out.sample_rate,
            channels: out.channels,
            loop_region: out.loop_region,
            offset: 0,
            size: out.bytes.len() as u64,
            delta: None,
        });
        media_blobs.push(out.bytes);
    }

    let mut program = NirProgram {
        features,
        meta: GameMeta {
            game_id: manifest.game.id.clone(),
            title: manifest.game.title.clone(),
            version: manifest.game.version.clone(),
            stage_width: manifest.game.stage[0],
            stage_height: manifest.game.stage[1],
            default_locale: manifest.game.locale.clone(),
            locales,
            entry_func: FuncId(0),
            save_slots: manifest.game.save_slots,
            default_reveal_us: manifest.game.default_reveal_ms as u64 * 1000,
            title_bg: remap_opt(title_bg),
            title_bgm: remap_opt(title_bgm),
            ui_fonts: ui_fonts.clone(),
            text_fonts: ui_fonts,
        },
        strings,
        vars: compiled.vars,
        funcs: compiled.funcs,
        labels: compiled.labels,
        ops: compiled.ops,
        texts: texts_by_locale,
        ui,
        assets: assets_meta,
        asset_lookup: BTreeMap::new(),
        timelines: Vec::new(),
        menus: Vec::new(),
    };

    let mut backings: Vec<BlobBacking> = media_blobs
        .into_iter()
        .map(|b| {
            let h = sha256_bytes(&b);
            BlobBacking::Mem {
                bytes: b,
                sha256: h,
            }
        })
        .collect();
    let generated_thumbs = run_gallery_thumbs_pass(&mut program, &mut backings, &cache_dir, opt);
    let generated_fonts = run_auto_embed_font_pass(&mut program, &mut backings, &cache_dir, opt);
    let (delta_patched, delta_saved) = run_delta_pass(&mut program, &mut backings, &cache_dir, opt);
    saved_bytes += delta_saved;

    let mut media_blobs = Vec::with_capacity(backings.len());
    for bk in &backings {
        media_blobs.push(bk.read_all()?);
    }
    let blob_refs: Vec<&[u8]> = media_blobs.iter().map(Vec::as_slice).collect();
    let nirpak = encode_nirpak_with_compression(&program, &blob_refs, compression)?;

    let summary = BuildSummary {
        game_id: program.meta.game_id.clone(),
        title: program.meta.title.clone(),
        scripts: scripts.len(),
        ops: program.ops.len(),
        texts: compiled.texts.len(),
        total_assets_scanned: total_scanned,
        packed_assets: program.assets.len(),
        pruned_assets: total_scanned.saturating_sub(
            program
                .assets
                .len()
                .saturating_sub(generated_thumbs + generated_fonts),
        ),
        trimmed_sprites,
        delta_patched,
        generated_thumbs,
        saved_bytes,
        nirpak_bytes: nirpak.len(),
    };

    Ok(BuiltBundle {
        program,
        media_blobs,
        nirpak,
        summary,
    })
}

/// Extracts all dialogue strings from a project into `locales/<lang>.toml` for easy translation.
pub fn extract_translations(root: &Path, lang: &str) -> Result<PathBuf> {
    let opt = OptimizeConfig {
        enabled: false,
        ..Default::default()
    };
    let built = compile_project(root, &opt)?;
    let locales_dir = root.join("locales");
    fs::create_dir_all(&locales_dir)?;
    let out_path = locales_dir.join(format!("{lang}.toml"));
    let mut existing: BTreeMap<String, String> = if out_path.is_file() {
        toml::from_str(&fs::read_to_string(&out_path)?).unwrap_or_default()
    } else {
        BTreeMap::new()
    };
    let default_texts = built
        .program
        .texts
        .get(&built.program.meta.default_locale)
        .cloned()
        .unwrap_or_default();
    for (i, entry) in default_texts.iter().enumerate() {
        let key = format!("t{i}");
        existing.entry(key).or_insert_with(|| {
            entry
                .spans
                .iter()
                .filter_map(|s| match s {
                    RichSpan::Text(t) | RichSpan::Emphasis(t) | RichSpan::Color { text: t, .. } => {
                        Some(t.as_str())
                    }
                    RichSpan::Ruby { base, .. } => Some(base.as_str()),
                    _ => None,
                })
                .collect()
        });
    }
    fs::write(&out_path, toml::to_string_pretty(&existing)?)?;
    Ok(out_path)
}

/// Packages a clean modern Web build (single entry, zero redirects, zero pure-JS SHA-256).
pub fn package_web(built: &BuiltBundle, sdk_web_dir: &Path, out_dir: &Path) -> Result<()> {
    fs::create_dir_all(out_dir)?;
    for name in [
        "index.html",
        "style.css",
        "app.js",
        "manifest.webmanifest",
        "sw.js",
        "player_web.js",
        "player_web_bg.wasm",
        "fallback-cjk.otf",
        "fallback-cjk.ttf",
    ] {
        let src = sdk_web_dir.join(name);
        if src.is_file() {
            fs::copy(&src, out_dir.join(name))?;
        }
    }
    fs::write(out_dir.join("game.nirpak"), &built.nirpak)?;
    Ok(())
}

/// Packages a native desktop bundle (copies native player binary + single `game.nirpak` directly!).
pub fn package_native(
    built: &BuiltBundle,
    player_binary: &Path,
    out_dir: &Path,
    binary_name: &str,
) -> Result<()> {
    fs::create_dir_all(out_dir)?;
    if player_binary.is_file() {
        let dst = out_dir.join(binary_name);
        fs::copy(player_binary, &dst)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&dst, fs::Permissions::from_mode(0o755));
        }
    }
    fs::write(out_dir.join("game.nirpak"), &built.nirpak)?;
    Ok(())
}

/// Packages a zero-Gradle signed Android APK with `assets/game.nirpak` stored uncompressed
/// (page-aligned for direct zero-extraction `AAsset` / `mmap` access!).
pub fn package_android(
    root: &Path,
    built: &BuiltBundle,
    libplayer_so: &Path,
    out_apk: &Path,
) -> Result<()> {
    let manifest_path = root.join("game.toml");
    let manifest: ProjectManifest = if manifest_path.is_file() {
        toml::from_str(&fs::read_to_string(&manifest_path)?).unwrap_or_default()
    } else {
        ProjectManifest::default()
    };

    let key_path = root.join("android-signing.pem");
    let key = if key_path.is_file() {
        nir_apk::key::SigningIdentity::load(&key_path)?
    } else {
        let k = nir_apk::key::SigningIdentity::generate()?;
        k.save_new(&key_path)?;
        nir_apk::key::SigningIdentity::load(&key_path)?
    };

    let entries = [
        (
            "lib/arm64-v8a/libplayer.so",
            nir_apk::EntryRef::File(libplayer_so),
        ),
        (
            "assets/game.nirpak",
            nir_apk::EntryRef::Bytes(&built.nirpak),
        ),
    ];

    let pkg_suffix: String = built
        .program
        .meta
        .game_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let package_name = format!("org.nir.g{pkg_suffix}");

    let apk_spec = nir_apk::ManifestSpec {
        package: package_name,
        label: built.program.meta.title.clone(),
        version_code: manifest.android.version_code,
        version_name: built.program.meta.version.clone(),
        min_sdk: manifest.android.min_sdk,
        target_sdk: manifest.android.target_sdk,
        lib_name: "player".into(),
        screen_orientation: manifest
            .android
            .orientation
            .as_deref()
            .and_then(nir_apk::ScreenOrientation::parse),
    };
    if let Some(parent) = out_apk.parent() {
        fs::create_dir_all(parent)?;
    }
    nir_apk::build_apk_borrowed(out_apk, &apk_spec, &entries, &key)?;
    Ok(())
}

/// Backing storage for one optimized media asset during build (disk cache file OR small in-RAM buffer).
enum BlobBacking {
    Cache(CachedPayloadRef),
    Mem { bytes: Vec<u8>, sha256: [u8; 32] },
}

impl BlobBacking {
    fn info(&self) -> StreamingBlobInfo {
        match self {
            Self::Cache(c) => StreamingBlobInfo {
                size: c.payload_len,
                hash: c.sha256,
            },
            Self::Mem { bytes, sha256 } => StreamingBlobInfo {
                size: bytes.len() as u64,
                hash: *sha256,
            },
        }
    }

    fn read_all(&self) -> Result<Vec<u8>> {
        match self {
            Self::Cache(c) => read_cached_payload(c),
            Self::Mem { bytes, .. } => Ok(bytes.clone()),
        }
    }

    fn stream_to(&self, writer: &mut impl Write) -> std::io::Result<()> {
        match self {
            Self::Cache(c) => stream_cached_payload(c, writer),
            Self::Mem { bytes, .. } => writer.write_all(bytes),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct CachedDeltaPlanEntry {
    asset_idx: usize,
    base_idx: usize,
    x: u32,
    y: u32,
    tile_mask: Vec<u8>,
    patch_w: u32,
    patch_h: u32,
    cache_file_name: String,
    saved_bytes: i64,
}

#[derive(Serialize, Deserialize)]
struct CachedTrimBaseEntry {
    asset_idx: usize,
    trim: SpriteTrim,
    cache_file_name: String,
    orig_size: u64,
}

#[derive(Serialize, Deserialize)]
struct CachedTrimDeltaPlan {
    bases: Vec<CachedTrimBaseEntry>,
    patches: Vec<CachedDeltaPlanEntry>,
    patch_trims: Vec<SpriteTrim>,
    net_saved: i64,
}

/// Global thumbnail-clustered, disk-cached 16×16 tile-masked star-graph differential compression pass.
///
/// 1. **Untrimmed CGs/BGs & Sprites**: groups same-dimension images, computes tiny 2.3 KB
///    thumbnails (zero RAM accumulation), clusters globally into semantically similar groups of
///    `<= 64` (medoid-first, eliminating cross-chunk misses), and runs Top-4 base delta compression.
/// 2. **Trimmed Standing Sprites**: groups alpha-trimmed sprites by `(orig_w, orig_h)`, clusters
///    them via canvas-space thumbnails, aligns each star group onto its tight union bounding box,
///    and adopts differential patches whenever total group bytes (including base re-encode) drop
///    by `>= 12%`!
fn run_delta_pass(
    program: &mut NirProgram,
    backings: &mut [BlobBacking],
    cache_dir: &Path,
    opt: &OptimizeConfig,
) -> (usize, i64) {
    if !opt.enabled || !opt.delta_images || opt.ktx2_textures {
        return (0, 0);
    }

    let mut total_patched = 0usize;
    let mut total_saved = 0i64;
    const MAX_CLUSTER: usize = 64;

    // --- Pass 1: Untrimmed same-dimension images with global semantic thumbnail clustering ---
    let mut buckets: BTreeMap<(u32, u32), Vec<(String, usize)>> = BTreeMap::new();
    for (idx, meta) in program.assets.iter().enumerate() {
        if meta.kind == AssetKind::Image
            && meta.codec != MediaCodec::Ktx2
            && meta.trim.is_none()
            && meta.width >= 128
            && meta.height >= 128
        {
            buckets
                .entry((meta.width, meta.height))
                .or_default()
                .push((program.str(meta.name).to_owned(), idx));
        }
    }

    for ((w, h), mut items) in buckets {
        if items.len() < 2 {
            continue;
        }
        items.sort_by(|a, b| a.0.cmp(&b.0));

        // Bucket-level cache key lets warm rebuilds skip BOTH thumbnail decode AND pixel diff!
        let mut hasher = Sha256::new();
        hasher.update(OPTIMIZER_VERSION.as_bytes());
        hasher.update(b":deltabucket_v2:");
        hasher.update(w.to_le_bytes());
        hasher.update(h.to_le_bytes());
        hasher.update([opt.webp_quality, opt.lossless_images as u8]);
        for &(_, idx) in &items {
            let info = backings[idx].info();
            hasher.update(info.hash);
            hasher.update(info.size.to_le_bytes());
        }
        let plan_key = format!("{:x}", hasher.finalize());
        let plan_path = cache_dir.join(format!("bp_{plan_key}.json"));

        if let Ok(plan_bytes) = fs::read(&plan_path) {
            if let Ok(entries) = serde_json::from_slice::<Vec<CachedDeltaPlanEntry>>(&plan_bytes) {
                let mut all_present = true;
                let mut resolved = Vec::with_capacity(entries.len());
                for e in entries {
                    let p = cache_dir.join(&e.cache_file_name);
                    let orig_len = backings
                        .get(e.asset_idx)
                        .map_or(0, |b| b.info().size as i64);
                    if let Some(meta) = read_image_cache_meta_only(&p, orig_len) {
                        resolved.push((e, meta.cache_ref));
                    } else {
                        all_present = false;
                        break;
                    }
                }
                if all_present {
                    for (e, cref) in resolved {
                        let meta = &mut program.assets[e.asset_idx];
                        meta.codec = MediaCodec::Webp;
                        meta.size = cref.payload_len;
                        meta.delta = Some(ImageDelta {
                            base: AssetId(e.base_idx as u32),
                            x: e.x,
                            y: e.y,
                            tile_mask: e.tile_mask,
                        });
                        backings[e.asset_idx] = BlobBacking::Cache(cref);
                        total_patched += 1;
                        total_saved += e.saved_bytes;
                    }
                    continue;
                }
            }
        }

        // Cold pass: extract compact 32×18 thumbnails (one image in RAM at a time) & cluster globally.
        let mut valid_idxs: Vec<usize> = Vec::with_capacity(items.len());
        let mut thumbs: Vec<Vec<u8>> = Vec::with_capacity(items.len());
        for &(_, idx) in &items {
            let codec = program.assets[idx].codec;
            let Ok(bytes) = backings[idx].read_all() else {
                continue;
            };
            let Some((dw, dh, rgba, _)) = decode_to_rgba(&bytes, codec) else {
                continue;
            };
            if dw == w && dh == h {
                thumbs.push(delta::compute_thumb(&rgba, dw as usize, dh as usize));
                valid_idxs.push(idx);
            }
        }
        if valid_idxs.len() < 2 {
            continue;
        }
        let clusters = delta::cluster_by_thumbs(&thumbs, MAX_CLUSTER);
        drop(thumbs);

        let mut plan_entries: Vec<CachedDeltaPlanEntry> = Vec::new();
        for cluster in clusters {
            if cluster.len() < 2 {
                continue;
            }
            let mut candidates = Vec::with_capacity(cluster.len());
            for local in cluster {
                let idx = valid_idxs[local];
                let codec = program.assets[idx].codec;
                let Ok(bytes) = backings[idx].read_all() else {
                    continue;
                };
                let orig_len = bytes.len();
                let Some((dw, dh, rgba, has_alpha)) = decode_to_rgba(&bytes, codec) else {
                    continue;
                };
                if dw == w && dh == h {
                    candidates.push(delta::DeltaCandidate {
                        asset_idx: idx,
                        width: dw,
                        height: dh,
                        has_alpha,
                        orig_encoded_len: orig_len,
                        rgba,
                    });
                }
            }
            if candidates.len() < 2 {
                continue;
            }
            let any_alpha = candidates.iter().any(|c| c.has_alpha);
            let q = effective_webp_quality(w, h, any_alpha, opt);
            let deltas = delta::compress_delta_candidates(&candidates, q, opt.lossless_images);
            drop(candidates);

            for dr in deltas {
                let orig_len = backings[dr.asset_idx].info().size as i64;
                let saved = orig_len - dr.patch_webp.len() as i64;
                let patch_key = format!("dl_{:x}.nioc", Sha256::digest(&dr.patch_webp));
                let patch_path = cache_dir.join(&patch_key);
                let cref = write_image_cache(
                    &patch_path,
                    MediaCodec::Webp,
                    dr.patch_width,
                    dr.patch_height,
                    None,
                    &dr.patch_webp,
                );
                plan_entries.push(CachedDeltaPlanEntry {
                    asset_idx: dr.asset_idx,
                    base_idx: dr.delta.base.0 as usize,
                    x: dr.delta.x,
                    y: dr.delta.y,
                    tile_mask: dr.delta.tile_mask.clone(),
                    patch_w: dr.patch_width,
                    patch_h: dr.patch_height,
                    cache_file_name: patch_key,
                    saved_bytes: saved,
                });
                let meta = &mut program.assets[dr.asset_idx];
                meta.codec = MediaCodec::Webp;
                meta.size = cref.payload_len;
                meta.delta = Some(dr.delta);
                backings[dr.asset_idx] = BlobBacking::Cache(cref);
                total_patched += 1;
                total_saved += saved;
            }
        }
        let _ = fs::create_dir_all(cache_dir);
        if let Ok(json) = serde_json::to_vec(&plan_entries) {
            let tmp = plan_path.with_extension(format!("json.{}.tmp", std::process::id()));
            if fs::write(&tmp, &json).is_ok() {
                let _ = fs::rename(&tmp, &plan_path);
            }
        }
    }

    // --- Pass 2: Trimmed Standing-Sprite Union-BBox Delta ---
    let mut trim_buckets: BTreeMap<(u32, u32), Vec<(String, usize, SpriteTrim)>> = BTreeMap::new();
    for (idx, meta) in program.assets.iter().enumerate() {
        if meta.kind == AssetKind::Image && meta.codec != MediaCodec::Ktx2 && meta.delta.is_none() {
            if let Some(t) = meta.trim {
                if t.orig_w >= 128 && t.orig_h >= 128 && t.w >= 64 && t.h >= 64 {
                    trim_buckets
                        .entry((t.orig_w, t.orig_h))
                        .or_default()
                        .push((program.str(meta.name).to_owned(), idx, t));
                }
            }
        }
    }

    const MAX_TRIM_CLUSTER: usize = 48;
    for ((orig_w, orig_h), mut items) in trim_buckets {
        if items.len() < 2 {
            continue;
        }
        items.sort_by(|a, b| a.0.cmp(&b.0));

        let mut hasher = Sha256::new();
        hasher.update(OPTIMIZER_VERSION.as_bytes());
        hasher.update(b":trimdelta_v1:");
        hasher.update(orig_w.to_le_bytes());
        hasher.update(orig_h.to_le_bytes());
        hasher.update([opt.webp_quality, opt.lossless_images as u8]);
        for &(_, idx, t) in &items {
            let info = backings[idx].info();
            hasher.update(info.hash);
            hasher.update(info.size.to_le_bytes());
            hasher.update(t.x.to_le_bytes());
            hasher.update(t.y.to_le_bytes());
            hasher.update(t.w.to_le_bytes());
            hasher.update(t.h.to_le_bytes());
        }
        let plan_key = format!("{:x}", hasher.finalize());
        let plan_path = cache_dir.join(format!("tp_{plan_key}.json"));

        if let Ok(plan_bytes) = fs::read(&plan_path) {
            if let Ok(plan) = serde_json::from_slice::<CachedTrimDeltaPlan>(&plan_bytes) {
                if plan.patches.len() == plan.patch_trims.len() {
                    let mut all_present = true;
                    let mut resolved_bases = Vec::with_capacity(plan.bases.len());
                    for b in &plan.bases {
                        let p = cache_dir.join(&b.cache_file_name);
                        if let Some(m) = read_image_cache_meta_only(&p, b.orig_size as i64) {
                            resolved_bases.push((b.asset_idx, b.trim, m.cache_ref));
                        } else {
                            all_present = false;
                            break;
                        }
                    }
                    let mut resolved_patches = Vec::with_capacity(plan.patches.len());
                    if all_present {
                        for (e, &ptrim) in plan.patches.iter().zip(plan.patch_trims.iter()) {
                            let p = cache_dir.join(&e.cache_file_name);
                            let orig_len = backings
                                .get(e.asset_idx)
                                .map_or(0, |b| b.info().size as i64);
                            if let Some(m) = read_image_cache_meta_only(&p, orig_len) {
                                resolved_patches.push((e, ptrim, m.cache_ref));
                            } else {
                                all_present = false;
                                break;
                            }
                        }
                    }
                    if all_present {
                        for (b_idx, trim, cref) in resolved_bases {
                            let meta = &mut program.assets[b_idx];
                            meta.codec = MediaCodec::Webp;
                            meta.width = trim.w;
                            meta.height = trim.h;
                            meta.trim = Some(trim);
                            meta.size = cref.payload_len;
                            backings[b_idx] = BlobBacking::Cache(cref);
                        }
                        for (e, ptrim, cref) in resolved_patches {
                            let meta = &mut program.assets[e.asset_idx];
                            meta.codec = MediaCodec::Webp;
                            meta.width = ptrim.w;
                            meta.height = ptrim.h;
                            meta.trim = Some(ptrim);
                            meta.size = cref.payload_len;
                            meta.delta = Some(ImageDelta {
                                base: AssetId(e.base_idx as u32),
                                x: e.x,
                                y: e.y,
                                tile_mask: e.tile_mask.clone(),
                            });
                            backings[e.asset_idx] = BlobBacking::Cache(cref);
                            total_patched += 1;
                        }
                        total_saved += plan.net_saved;
                        continue;
                    }
                }
            }
        }

        // Cold trimmed-sprite clustering via full-canvas 32×18 thumbnails.
        let mut valid_items: Vec<(usize, SpriteTrim)> = Vec::with_capacity(items.len());
        let mut thumbs: Vec<Vec<u8>> = Vec::with_capacity(items.len());
        for &(_, idx, trim) in &items {
            let codec = program.assets[idx].codec;
            let Ok(bytes) = backings[idx].read_all() else {
                continue;
            };
            let Some((dw, dh, rgba, _)) = decode_to_rgba(&bytes, codec) else {
                continue;
            };
            if dw == trim.w && dh == trim.h {
                thumbs.push(delta::compute_trimmed_thumb(&rgba, trim));
                valid_items.push((idx, trim));
            }
        }
        if valid_items.len() < 2 {
            continue;
        }
        let clusters = delta::cluster_by_thumbs(&thumbs, MAX_TRIM_CLUSTER);
        drop(thumbs);

        let mut plan = CachedTrimDeltaPlan {
            bases: Vec::new(),
            patches: Vec::new(),
            patch_trims: Vec::new(),
            net_saved: 0,
        };

        for cluster in clusters {
            if cluster.len() < 2 {
                continue;
            }
            // Compute tight union bounding box of this cluster's sprites.
            let mut ux0 = u32::MAX;
            let mut uy0 = u32::MAX;
            let mut ux1 = 0u32;
            let mut uy1 = 0u32;
            let mut max_member_area = 0u64;
            for &local in &cluster {
                let (_, t) = valid_items[local];
                ux0 = ux0.min(t.x);
                uy0 = uy0.min(t.y);
                ux1 = ux1.max(t.x.saturating_add(t.w));
                uy1 = uy1.max(t.y.saturating_add(t.h));
                max_member_area = max_member_area.max(t.w as u64 * t.h as u64);
            }
            let uw = ux1.saturating_sub(ux0);
            let uh = uy1.saturating_sub(uy0);
            let union_area = uw as u64 * uh as u64;
            if uw < 128
                || uh < 128
                || ux1 > orig_w
                || uy1 > orig_h
                || union_area > max_member_area.saturating_mul(4)
            {
                continue;
            }
            let union_trim = SpriteTrim {
                x: ux0,
                y: uy0,
                w: uw,
                h: uh,
                orig_w,
                orig_h,
            };

            let mut candidates: Vec<delta::DeltaCandidate> = Vec::with_capacity(cluster.len());
            let mut orig_trims: BTreeMap<usize, SpriteTrim> = BTreeMap::new();
            let mut orig_sizes: BTreeMap<usize, usize> = BTreeMap::new();
            for &local in &cluster {
                let (idx, t) = valid_items[local];
                let codec = program.assets[idx].codec;
                let Ok(bytes) = backings[idx].read_all() else {
                    continue;
                };
                let orig_len = bytes.len();
                let Some((dw, dh, rgba, _)) = decode_to_rgba(&bytes, codec) else {
                    continue;
                };
                if dw != t.w || dh != t.h {
                    continue;
                }
                let mut canvas = vec![0u8; uw as usize * uh as usize * 4];
                let dx = (t.x - ux0) as usize;
                let dy = (t.y - uy0) as usize;
                let row_bytes = t.w as usize * 4;
                for y in 0..t.h as usize {
                    let src_off = y * row_bytes;
                    let dst_off = ((dy + y) * uw as usize + dx) * 4;
                    canvas[dst_off..dst_off + row_bytes]
                        .copy_from_slice(&rgba[src_off..src_off + row_bytes]);
                }
                orig_trims.insert(idx, t);
                orig_sizes.insert(idx, orig_len);
                candidates.push(delta::DeltaCandidate {
                    asset_idx: idx,
                    width: uw,
                    height: uh,
                    has_alpha: true,
                    orig_encoded_len: orig_len,
                    rgba: canvas,
                });
            }
            if candidates.len() < 2 {
                continue;
            }

            let q = effective_webp_quality(uw, uh, true, opt);
            let deltas = delta::compress_delta_candidates(&candidates, q, opt.lossless_images);
            if deltas.is_empty() {
                continue;
            }

            // Group patches by base keyframe so each star group pays for its own union-bbox base re-encode.
            let mut by_base: BTreeMap<usize, Vec<delta::DeltaResult>> = BTreeMap::new();
            for dr in deltas {
                by_base
                    .entry(dr.delta.base.0 as usize)
                    .or_default()
                    .push(dr);
            }
            let cand_by_asset: BTreeMap<usize, &delta::DeltaCandidate> =
                candidates.iter().map(|c| (c.asset_idx, c)).collect();

            for (base_idx, group_deltas) in by_base {
                let Some(&base_cand) = cand_by_asset.get(&base_idx) else {
                    continue;
                };
                let base_orig_trim = orig_trims[&base_idx];
                let base_orig_size = orig_sizes[&base_idx];

                let reencoded_base = if base_orig_trim != union_trim {
                    Some(encode_webp_rgba_q(
                        &base_cand.rgba,
                        uw,
                        uh,
                        q,
                        opt.lossless_images,
                    ))
                } else {
                    None
                };
                let new_base_len = reencoded_base
                    .as_ref()
                    .map_or(base_orig_size, Vec::len);

                let orig_group_bytes: usize = base_orig_size
                    + group_deltas
                        .iter()
                        .map(|d| orig_sizes[&d.asset_idx])
                        .sum::<usize>();
                let new_group_bytes: usize = new_base_len
                    + group_deltas
                        .iter()
                        .map(|d| d.patch_webp.len() + d.delta.tile_mask.len())
                        .sum::<usize>();

                // Adopt this star group only if net savings (including union-bbox base overhead) >= 12%!
                if new_group_bytes * 100 > orig_group_bytes * 88 {
                    continue;
                }

                if let Some(base_webp) = reencoded_base {
                    let base_key = format!("tb_{:x}.nioc", Sha256::digest(&base_webp));
                    let base_path = cache_dir.join(&base_key);
                    let base_cref = write_image_cache(
                        &base_path,
                        MediaCodec::Webp,
                        uw,
                        uh,
                        Some(union_trim),
                        &base_webp,
                    );
                    plan.bases.push(CachedTrimBaseEntry {
                        asset_idx: base_idx,
                        trim: union_trim,
                        cache_file_name: base_key,
                        orig_size: base_orig_size as u64,
                    });
                    let bmeta = &mut program.assets[base_idx];
                    bmeta.codec = MediaCodec::Webp;
                    bmeta.width = uw;
                    bmeta.height = uh;
                    bmeta.trim = Some(union_trim);
                    bmeta.size = base_cref.payload_len;
                    backings[base_idx] = BlobBacking::Cache(base_cref);
                }

                let mut group_patch_disk_bytes = 0usize;
                for dr in group_deltas {
                    let orig_len = orig_sizes[&dr.asset_idx] as i64;
                    let patch_len = dr.patch_webp.len();
                    group_patch_disk_bytes += patch_len;
                    let patch_key = format!("dl_{:x}.nioc", Sha256::digest(&dr.patch_webp));
                    let patch_path = cache_dir.join(&patch_key);
                    let cref = write_image_cache(
                        &patch_path,
                        MediaCodec::Webp,
                        dr.patch_width,
                        dr.patch_height,
                        Some(union_trim),
                        &dr.patch_webp,
                    );
                    plan.patches.push(CachedDeltaPlanEntry {
                        asset_idx: dr.asset_idx,
                        base_idx,
                        x: dr.delta.x,
                        y: dr.delta.y,
                        tile_mask: dr.delta.tile_mask.clone(),
                        patch_w: dr.patch_width,
                        patch_h: dr.patch_height,
                        cache_file_name: patch_key,
                        saved_bytes: orig_len - patch_len as i64,
                    });
                    plan.patch_trims.push(union_trim);
                    let meta = &mut program.assets[dr.asset_idx];
                    meta.codec = MediaCodec::Webp;
                    meta.width = uw;
                    meta.height = uh;
                    meta.trim = Some(union_trim);
                    meta.size = cref.payload_len;
                    meta.delta = Some(dr.delta);
                    backings[dr.asset_idx] = BlobBacking::Cache(cref);
                    total_patched += 1;
                }
                let group_net_saved =
                    orig_group_bytes as i64 - (new_base_len + group_patch_disk_bytes) as i64;
                plan.net_saved += group_net_saved;
                total_saved += group_net_saved;
            }
        }

        let _ = fs::create_dir_all(cache_dir);
        if let Ok(json) = serde_json::to_vec(&plan) {
            let tmp = plan_path.with_extension(format!("json.{}.tmp", std::process::id()));
            if fs::write(&tmp, &json).is_ok() {
                let _ = fs::rename(&tmp, &plan_path);
            }
        }
    }

    if total_patched > 0 {
        program.features |= FEATURE_DELTA_IMAGE | FEATURE_WEBP;
    }
    (total_patched, total_saved)
}

/// Auto-embeds a compact game-subset CJK font (~150–300 KB) when `opt.auto_embed_cjk_font` is set
/// and the game bundles no fonts, covering every character in the game plus all built-in NIR UI
/// strings so Web cold start never downloads the 6.3 MB Pan-CJK fallback font!
fn run_auto_embed_font_pass(
    program: &mut NirProgram,
    backings: &mut Vec<BlobBacking>,
    cache_dir: &Path,
    opt: &OptimizeConfig,
) -> usize {
    if !opt.enabled
        || !opt.subset_fonts
        || !opt.auto_embed_cjk_font
        || !program.meta.ui_fonts.is_empty()
        || !program.meta.text_fonts.is_empty()
    {
        return 0;
    }
    let mut chars = program.collect_all_chars();
    chars.extend(nir_ir::ui_messages::EN_FTL.chars());
    chars.extend(nir_ir::ui_messages::ZH_HANS_FTL.chars());
    for c in ' '..='~' {
        chars.insert(c);
    }
    for c in "　「」『』（）【】《》、。！？…—・～：；％＋－×÷".chars() {
        chars.insert(c);
    }
    let Some((font_bytes, codec)) = build_embedded_game_cjk_font(&chars, opt, cache_dir) else {
        return 0;
    };
    let new_id = AssetId(program.assets.len() as u32);
    let name_str = StrId(program.strings.len() as u32);
    program.strings.push("__embedded/cjk_subset".into());
    let sha = sha256_bytes(&font_bytes);
    let sz = font_bytes.len() as u64;
    program.assets.push(AssetMeta {
        id: new_id,
        name: name_str,
        kind: AssetKind::Font,
        codec,
        width: 0,
        height: 0,
        trim: None,
        duration_us: 0,
        sample_rate: 0,
        channels: 0,
        loop_region: None,
        offset: 0,
        size: sz,
        delta: None,
    });
    backings.push(BlobBacking::Mem {
        bytes: font_bytes,
        sha256: sha,
    });
    program.meta.ui_fonts.push(new_id);
    program.meta.text_fonts.push(new_id);
    1
}

/// Auto-generates compact `256×144` WebP thumbnails for `program.ui.gallery` items whose
/// `thumb` currently points at a full-resolution CG (> 320×180), slashing Gallery VRAM by ~96%!
fn run_gallery_thumbs_pass(
    program: &mut NirProgram,
    backings: &mut Vec<BlobBacking>,
    cache_dir: &Path,
    opt: &OptimizeConfig,
) -> usize {
    if !opt.enabled || !opt.gallery_thumbnails || program.ui.gallery.is_empty() {
        return 0;
    }
    let mut thumb_map: BTreeMap<AssetId, AssetId> = BTreeMap::new();
    let mut generated = 0usize;

    for idx in 0..program.ui.gallery.len() {
        let Some(orig_id) = program.ui.gallery[idx].thumb else {
            continue;
        };
        if let Some(&mapped) = thumb_map.get(&orig_id) {
            program.ui.gallery[idx].thumb = Some(mapped);
            continue;
        }
        let Some(orig_meta) = program.assets.get(orig_id.0 as usize).cloned() else {
            continue;
        };
        if orig_meta.kind != AssetKind::Image
            || orig_meta.delta.is_some()
            || (orig_meta.width <= 320 && orig_meta.height <= 180)
        {
            continue;
        }
        let Ok(bytes) = backings[orig_id.0 as usize].read_all() else {
            continue;
        };
        let Some((tw, th, thumb_webp)) =
            generate_gallery_thumbnail(&bytes, orig_meta.codec, cache_dir)
        else {
            continue;
        };
        let new_id = AssetId(program.assets.len() as u32);
        let name_str = StrId(program.strings.len() as u32);
        let orig_name = program.str(orig_meta.name).to_owned();
        program.strings.push(format!("__thumb/{orig_name}"));
        let thumb_key = format!("th_{:x}.nioc", Sha256::digest(&thumb_webp));
        let cref = write_image_cache(
            &cache_dir.join(&thumb_key),
            MediaCodec::Webp,
            tw,
            th,
            None,
            &thumb_webp,
        );
        program.assets.push(AssetMeta {
            id: new_id,
            name: name_str,
            kind: AssetKind::Image,
            codec: MediaCodec::Webp,
            width: tw,
            height: th,
            trim: None,
            duration_us: 0,
            sample_rate: 0,
            channels: 0,
            loop_region: None,
            offset: 0,
            size: cref.payload_len,
            delta: None,
        });
        backings.push(BlobBacking::Cache(cref));
        program.features |= FEATURE_WEBP;
        thumb_map.insert(orig_id, new_id);
        program.ui.gallery[idx].thumb = Some(new_id);
        generated += 1;
    }
    generated
}

struct PreparedDirectBuild {
    program: NirProgram,
    backings: Vec<BlobBacking>,
    total_scanned: usize,
    trimmed_sprites: usize,
    delta_patched: usize,
    generated_thumbs: usize,
    generated_fonts: usize,
    saved_bytes: i64,
}

fn prepare_direct_build(
    mut program: NirProgram,
    raw_assets: Vec<DirectAssetSource>,
    extra_keep: &BTreeSet<AssetId>,
    cache_dir: &Path,
    opt: &OptimizeConfig,
) -> Result<PreparedDirectBuild> {
    let total_scanned = raw_assets.len();

    // 1. Collect all referenced AssetIds across ops, meta, ui, timelines, menus, and asset_lookup.
    let mut used = collect_program_assets(&program);
    for &id in extra_keep {
        if (id.0 as usize) < total_scanned {
            used.insert(id);
        }
    }
    used.retain(|id| (id.0 as usize) < total_scanned);

    // 2. Build dense remap table 0..N for kept assets.
    let mut remap = BTreeMap::<AssetId, AssetId>::new();
    let mut kept_indices = Vec::with_capacity(used.len());
    for old_id in &used {
        let new_id = AssetId(kept_indices.len() as u32);
        remap.insert(*old_id, new_id);
        kept_indices.push(old_id.0 as usize);
    }

    // 3. Remap every AssetId reference in the program.
    remap_program_assets(&mut program, &remap);

    // 4. Collect characters once if any font is kept and font subsetting is enabled.
    let has_fonts = kept_indices
        .iter()
        .any(|&idx| raw_assets[idx].kind == AssetKind::Font);
    let used_chars = if opt.enabled && opt.subset_fonts && has_fonts {
        program.collect_all_chars()
    } else {
        BTreeSet::new()
    };

    // 5. Load & optimize kept assets with bounded concurrency (max 8 workers) and disk-cache spill!
    struct DirectAssetOut {
        backing: BlobBacking,
        codec: MediaCodec,
        width: u32,
        height: u32,
        trim: Option<SpriteTrim>,
        duration_us: u64,
        sample_rate: u32,
        channels: u16,
        loop_region: Option<AudioLoopRegion>,
        saved_bytes: i64,
        features: u32,
        trimmed: bool,
    }

    // Infer per-asset AudioBus (BGM > SFX > Voice) after remapping so Opus/MP3 optimization is bus-aware!
    let inferred_buses = infer_audio_buses(&program);

    let optimize_one = |new_idx: usize, orig_idx: usize| -> Result<DirectAssetOut> {
        let src = &raw_assets[orig_idx];
        let mut features = 0u32;
        let mut trimmed = false;
        let bus = inferred_buses.get(&AssetId(new_idx as u32)).copied();

        // Ultra-fast warm rebuild path: check source-fingerprint cache BEFORE touching disk/decoder!
        if opt.enabled {
            if let Some(fp) = src.source_fingerprint {
                match src.kind {
                    AssetKind::Image => {
                        let fp_path = fp_image_cache_path(cache_dir, &src.name, fp, opt);
                        if let Some(meta) = read_image_cache_meta_only(&fp_path, fp.0 as i64) {
                            match meta.codec {
                                MediaCodec::Webp => features |= FEATURE_WEBP,
                                MediaCodec::Ktx2 => features |= FEATURE_KTX2,
                                _ => {}
                            }
                            if meta.trim.is_some() {
                                features |= FEATURE_TRIMMED;
                                trimmed = true;
                            }
                            return Ok(DirectAssetOut {
                                backing: BlobBacking::Cache(meta.cache_ref),
                                codec: meta.codec,
                                width: meta.width,
                                height: meta.height,
                                trim: meta.trim,
                                duration_us: 0,
                                sample_rate: 0,
                                channels: 0,
                                loop_region: None,
                                saved_bytes: meta.saved_bytes,
                                features,
                                trimmed,
                            });
                        }
                    }
                    AssetKind::Audio => {
                        let fp_path =
                            fp_audio_cache_path_for_bus(cache_dir, &src.name, fp, bus, opt);
                        if let Some(meta) = read_audio_cache_meta_only(&fp_path, fp.0 as i64) {
                            match meta.codec {
                                MediaCodec::Mp3 => features |= FEATURE_MP3,
                                MediaCodec::Ogg => features |= FEATURE_OGG,
                                MediaCodec::Opus => features |= FEATURE_OPUS,
                                _ => {}
                            }
                            return Ok(DirectAssetOut {
                                backing: BlobBacking::Cache(meta.cache_ref),
                                codec: meta.codec,
                                width: 0,
                                height: 0,
                                trim: None,
                                duration_us: meta.duration_us,
                                sample_rate: meta.sample_rate,
                                channels: meta.channels,
                                loop_region: None,
                                saved_bytes: meta.saved_bytes,
                                features,
                                trimmed: false,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }

        // Direct pixel path (eliminates `.gal -> PNG -> decode PNG -> WebP` round-trip!)
        if src.kind == AssetKind::Image {
            if let Some(px_loader) = &src.pixel_loader {
                let pixels = (px_loader)()
                    .with_context(|| format!("decoding direct image `{}`", src.name))?;
                let orig_len = src.source_fingerprint.map_or(0, |(len, _)| len as i64);
                let (res, _) = match &pixels {
                    DirectPixels::Rgb {
                        width,
                        height,
                        data,
                    } => optimize_decoded_pixels(
                        RawPixels::Rgb {
                            width: *width,
                            height: *height,
                            data,
                        },
                        orig_len,
                        opt,
                    )?,
                    DirectPixels::Rgba {
                        width,
                        height,
                        data,
                    } => optimize_decoded_pixels(
                        RawPixels::Rgba {
                            width: *width,
                            height: *height,
                            data,
                        },
                        orig_len,
                        opt,
                    )?,
                };
                match res.codec {
                    MediaCodec::Webp => features |= FEATURE_WEBP,
                    MediaCodec::Ktx2 => features |= FEATURE_KTX2,
                    _ => {}
                }
                if res.trim.is_some() {
                    features |= FEATURE_TRIMMED;
                    trimmed = true;
                }
                let backing = if let Some(fp) = src.source_fingerprint.filter(|_| opt.enabled) {
                    let fp_path = fp_image_cache_path(cache_dir, &src.name, fp, opt);
                    BlobBacking::Cache(write_image_cache(
                        &fp_path, res.codec, res.width, res.height, res.trim, &res.bytes,
                    ))
                } else {
                    let sha = sha256_bytes(&res.bytes);
                    BlobBacking::Mem {
                        bytes: res.bytes,
                        sha256: sha,
                    }
                };
                return Ok(DirectAssetOut {
                    backing,
                    codec: res.codec,
                    width: res.width,
                    height: res.height,
                    trim: res.trim,
                    duration_us: 0,
                    sample_rate: 0,
                    channels: 0,
                    loop_region: None,
                    saved_bytes: res.saved_bytes,
                    features,
                    trimmed,
                });
            }
        }

        let raw = (src.loader)().with_context(|| format!("loading direct asset `{}`", src.name))?;
        let out = match src.kind {
            AssetKind::Image => {
                let res = optimize_image(&raw, src.codec, opt, cache_dir)?;
                match res.codec {
                    MediaCodec::Webp => features |= FEATURE_WEBP,
                    MediaCodec::Ktx2 => features |= FEATURE_KTX2,
                    _ => {}
                }
                if res.trim.is_some() {
                    features |= FEATURE_TRIMMED;
                    trimmed = true;
                }
                let backing = if let Some(fp) = src.source_fingerprint.filter(|_| opt.enabled) {
                    let fp_path = fp_image_cache_path(cache_dir, &src.name, fp, opt);
                    BlobBacking::Cache(write_image_cache(
                        &fp_path, res.codec, res.width, res.height, res.trim, &res.bytes,
                    ))
                } else {
                    let sha = sha256_bytes(&res.bytes);
                    BlobBacking::Mem {
                        bytes: res.bytes,
                        sha256: sha,
                    }
                };
                DirectAssetOut {
                    backing,
                    codec: res.codec,
                    width: res.width,
                    height: res.height,
                    trim: res.trim,
                    duration_us: 0,
                    sample_rate: 0,
                    channels: 0,
                    loop_region: None,
                    saved_bytes: res.saved_bytes,
                    features,
                    trimmed,
                }
            }
            AssetKind::Audio => {
                let res = optimize_audio_for_bus(&raw, src.codec, bus, opt, cache_dir)?;
                match res.codec {
                    MediaCodec::Mp3 => features |= FEATURE_MP3,
                    MediaCodec::Ogg => features |= FEATURE_OGG,
                    MediaCodec::Opus => features |= FEATURE_OPUS,
                    _ => {}
                }
                let backing = if let Some(fp) = src.source_fingerprint.filter(|_| opt.enabled) {
                    let fp_path = fp_audio_cache_path_for_bus(cache_dir, &src.name, fp, bus, opt);
                    BlobBacking::Cache(write_audio_cache(
                        &fp_path,
                        res.codec,
                        res.duration_us,
                        res.sample_rate,
                        res.channels,
                        &res.bytes,
                    ))
                } else {
                    let sha = sha256_bytes(&res.bytes);
                    BlobBacking::Mem {
                        bytes: res.bytes,
                        sha256: sha,
                    }
                };
                DirectAssetOut {
                    backing,
                    codec: res.codec,
                    width: 0,
                    height: 0,
                    trim: None,
                    duration_us: res.duration_us,
                    sample_rate: res.sample_rate,
                    channels: res.channels,
                    loop_region: res.loop_region,
                    saved_bytes: res.saved_bytes,
                    features,
                    trimmed: false,
                }
            }
            AssetKind::Font => {
                let orig_len = raw.len() as i64;
                let sub = subset_font(&raw, &used_chars, opt, cache_dir)?;
                let saved = orig_len - sub.len() as i64;
                let sha = sha256_bytes(&sub);
                DirectAssetOut {
                    backing: BlobBacking::Mem {
                        bytes: sub,
                        sha256: sha,
                    },
                    codec: src.codec,
                    width: 0,
                    height: 0,
                    trim: None,
                    duration_us: 0,
                    sample_rate: 0,
                    channels: 0,
                    loop_region: None,
                    saved_bytes: saved,
                    features: 0,
                    trimmed: false,
                }
            }
            AssetKind::Video => {
                features |= FEATURE_VIDEO;
                let sha = sha256_bytes(&raw);
                DirectAssetOut {
                    backing: BlobBacking::Mem {
                        bytes: raw,
                        sha256: sha,
                    },
                    codec: src.codec,
                    width: 0,
                    height: 0,
                    trim: None,
                    duration_us: 0,
                    sample_rate: 0,
                    channels: 0,
                    loop_region: None,
                    saved_bytes: 0,
                    features,
                    trimmed: false,
                }
            }
            AssetKind::Raw => {
                let sha = sha256_bytes(&raw);
                DirectAssetOut {
                    backing: BlobBacking::Mem {
                        bytes: raw,
                        sha256: sha,
                    },
                    codec: src.codec,
                    width: 0,
                    height: 0,
                    trim: None,
                    duration_us: 0,
                    sample_rate: 0,
                    channels: 0,
                    loop_region: None,
                    saved_bytes: 0,
                    features: 0,
                    trimmed: false,
                }
            }
        };
        Ok(out)
    };

    // Cap active worker threads to 8 to keep peak build memory flat (< 250 MiB even on 15k-asset games!)
    let worker_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(8)
        .min(kept_indices.len().max(1));

    let results: Vec<Result<DirectAssetOut>> = if worker_count <= 1 || kept_indices.len() <= 1 {
        kept_indices
            .iter()
            .enumerate()
            .map(|(new_idx, &orig_idx)| optimize_one(new_idx, orig_idx))
            .collect()
    } else {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Mutex;
        let next_task = AtomicUsize::new(0);
        let slots: Vec<Mutex<Option<Result<DirectAssetOut>>>> =
            (0..kept_indices.len()).map(|_| Mutex::new(None)).collect();
        std::thread::scope(|s| {
            for _ in 0..worker_count {
                s.spawn(|| loop {
                    let i = next_task.fetch_add(1, Ordering::Relaxed);
                    if i >= kept_indices.len() {
                        break;
                    }
                    let res = optimize_one(i, kept_indices[i]);
                    *slots[i].lock().unwrap_or_else(|e| e.into_inner()) = Some(res);
                });
            }
        });
        slots
            .into_iter()
            .map(|m| m.into_inner().unwrap_or_else(|e| e.into_inner()).unwrap())
            .collect()
    };

    let mut assets_meta = Vec::with_capacity(kept_indices.len());
    let mut backings = Vec::with_capacity(kept_indices.len());
    let mut features = 0u32;
    let mut trimmed_sprites = 0usize;
    let mut saved_bytes = 0i64;

    for (new_idx, (&orig_idx, res)) in kept_indices.iter().zip(results).enumerate() {
        let src = &raw_assets[orig_idx];
        let out = res?;
        let name_str = StrId(program.strings.len() as u32);
        program.strings.push(src.name.clone());
        features |= out.features;
        saved_bytes += out.saved_bytes;
        if out.trimmed {
            trimmed_sprites += 1;
        }
        let sz = out.backing.info().size;
        assets_meta.push(AssetMeta {
            id: AssetId(new_idx as u32),
            name: name_str,
            kind: src.kind,
            codec: out.codec,
            width: out.width,
            height: out.height,
            trim: out.trim,
            duration_us: out.duration_us,
            sample_rate: out.sample_rate,
            channels: out.channels,
            loop_region: out.loop_region,
            offset: 0,
            size: sz,
            delta: None,
        });
        backings.push(out.backing);
    }
    program.features = features;
    program.assets = assets_meta;

    // 6. Auto-generate gallery thumbnails & compact CJK font first, then run global/trimmed delta pass!
    let generated_thumbs = run_gallery_thumbs_pass(&mut program, &mut backings, cache_dir, opt);
    let generated_fonts = run_auto_embed_font_pass(&mut program, &mut backings, cache_dir, opt);
    let (delta_patched, delta_saved) = run_delta_pass(&mut program, &mut backings, cache_dir, opt);
    saved_bytes += delta_saved;

    Ok(PreparedDirectBuild {
        program,
        backings,
        total_scanned,
        trimmed_sprites,
        delta_patched,
        generated_thumbs,
        generated_fonts,
        saved_bytes,
    })
}

/// Streaming direct-to-file `.nirpak` builder: never holds all media blobs or the full archive
/// buffer in RAM (slashes peak RSS on multi-thousand-asset games by ~85-90%!).
pub fn build_direct_program_to_file(
    program: NirProgram,
    raw_assets: Vec<DirectAssetSource>,
    extra_keep: &BTreeSet<AssetId>,
    cache_dir: &Path,
    opt: &OptimizeConfig,
    dest_pak: &Path,
) -> Result<BuildSummary> {
    let mut prep = prepare_direct_build(program, raw_assets, extra_keep, cache_dir, opt)?;
    let infos: Vec<StreamingBlobInfo> = prep.backings.iter().map(BlobBacking::info).collect();

    if let Some(parent) = dest_pak.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = dest_pak.with_extension("nirpak.tmp");
    let file = fs::File::create(&tmp_path)
        .with_context(|| format!("creating `{}`", tmp_path.display()))?;
    let mut writer = BufWriter::with_capacity(256 * 1024, file);
    let (written, patched_assets) = write_nirpak_streaming(
        &prep.program,
        &infos,
        flate2::Compression::default(),
        &mut writer,
        |idx, w| prep.backings[idx].stream_to(w),
    )?;
    drop(writer);
    fs::rename(&tmp_path, dest_pak)?;
    prep.program.assets = patched_assets;

    let text_count = prep
        .program
        .texts
        .get(&prep.program.meta.default_locale)
        .map_or(0, Vec::len);
    let base_packed = prep
        .program
        .assets
        .len()
        .saturating_sub(prep.generated_thumbs + prep.generated_fonts);
    Ok(BuildSummary {
        game_id: prep.program.meta.game_id,
        title: prep.program.meta.title,
        scripts: prep.program.funcs.len(),
        ops: prep.program.ops.len(),
        texts: text_count,
        total_assets_scanned: prep.total_scanned,
        packed_assets: prep.program.assets.len(),
        pruned_assets: prep.total_scanned.saturating_sub(base_packed),
        trimmed_sprites: prep.trimmed_sprites,
        delta_patched: prep.delta_patched,
        generated_thumbs: prep.generated_thumbs,
        saved_bytes: prep.saved_bytes,
        nirpak_bytes: written as usize,
    })
}

/// Optimizes, tree-shakes, remaps, and packs a direct-to-IR `NirProgram` + lazy raw assets
/// into an in-memory `BuiltBundle` (used by Web/Native/Android packagers and tests).
pub fn build_direct_program(
    program: NirProgram,
    raw_assets: Vec<DirectAssetSource>,
    extra_keep: &BTreeSet<AssetId>,
    cache_dir: &Path,
    opt: &OptimizeConfig,
) -> Result<BuiltBundle> {
    build_direct_program_with_compression(
        program,
        raw_assets,
        extra_keep,
        cache_dir,
        opt,
        flate2::Compression::default(),
    )
}

pub fn build_direct_program_with_compression(
    program: NirProgram,
    raw_assets: Vec<DirectAssetSource>,
    extra_keep: &BTreeSet<AssetId>,
    cache_dir: &Path,
    opt: &OptimizeConfig,
    compression: flate2::Compression,
) -> Result<BuiltBundle> {
    let mut prep = prepare_direct_build(program, raw_assets, extra_keep, cache_dir, opt)?;
    let infos: Vec<StreamingBlobInfo> = prep.backings.iter().map(BlobBacking::info).collect();

    let mut nirpak = Vec::new();
    let (_, patched_assets) =
        write_nirpak_streaming(&prep.program, &infos, compression, &mut nirpak, |idx, w| {
            prep.backings[idx].stream_to(w)
        })?;
    prep.program.assets = patched_assets;

    let mut media_blobs = Vec::with_capacity(prep.backings.len());
    for bk in &prep.backings {
        media_blobs.push(bk.read_all()?);
    }

    let text_count = prep
        .program
        .texts
        .get(&prep.program.meta.default_locale)
        .map_or(0, Vec::len);
    let base_packed = prep
        .program
        .assets
        .len()
        .saturating_sub(prep.generated_thumbs + prep.generated_fonts);

    let summary = BuildSummary {
        game_id: prep.program.meta.game_id.clone(),
        title: prep.program.meta.title.clone(),
        scripts: prep.program.funcs.len(),
        ops: prep.program.ops.len(),
        texts: text_count,
        total_assets_scanned: prep.total_scanned,
        packed_assets: prep.program.assets.len(),
        pruned_assets: prep.total_scanned.saturating_sub(base_packed),
        trimmed_sprites: prep.trimmed_sprites,
        delta_patched: prep.delta_patched,
        generated_thumbs: prep.generated_thumbs,
        saved_bytes: prep.saved_bytes,
        nirpak_bytes: nirpak.len(),
    };

    Ok(BuiltBundle {
        program: prep.program,
        media_blobs,
        nirpak,
        summary,
    })
}

fn collect_ui_node_assets(node: &UiNode, out: &mut BTreeSet<AssetId>) {
    match &node.widget {
        UiWidget::Container { children, .. } | UiWidget::Stack { children, .. } => {
            for ch in children {
                collect_ui_node_assets(ch, out);
            }
        }
        UiWidget::Image { asset } => {
            out.insert(*asset);
        }
        UiWidget::Button {
            image, hover_image, ..
        } => {
            if let Some(a) = image {
                out.insert(*a);
            }
            if let Some(a) = hover_image {
                out.insert(*a);
            }
        }
        UiWidget::Text { .. } => {}
    }
}

fn remap_ui_node_assets(node: &mut UiNode, remap: &BTreeMap<AssetId, AssetId>) {
    let m = |id: AssetId| remap.get(&id).copied().unwrap_or(id);
    match &mut node.widget {
        UiWidget::Container { children, .. } | UiWidget::Stack { children, .. } => {
            for ch in children {
                remap_ui_node_assets(ch, remap);
            }
        }
        UiWidget::Image { asset } => *asset = m(*asset),
        UiWidget::Button {
            image, hover_image, ..
        } => {
            *image = image.map(m);
            *hover_image = hover_image.map(m);
        }
        UiWidget::Text { .. } => {}
    }
}

fn collect_ui_node_replay_funcs(node: &UiNode, out: &mut Vec<FuncId>) {
    match &node.widget {
        UiWidget::Container { children, .. } | UiWidget::Stack { children, .. } => {
            for ch in children {
                collect_ui_node_replay_funcs(ch, out);
            }
        }
        UiWidget::Button {
            action: UiAction::StartReplay(fid),
            ..
        } => {
            out.push(*fid);
        }
        _ => {}
    }
}

/// Computes the CFG-reachable bytecode instruction bitmap starting from `meta.entry_func`,
/// every gallery `replay_func`, and every `UiAction::StartReplay` in `ui.title_screen`.
fn compute_reachable_ops(program: &NirProgram) -> Vec<bool> {
    let n = program.ops.len();
    let mut visited = vec![false; n];
    if n == 0 {
        return visited;
    }
    let mut queue = std::collections::VecDeque::new();
    let enqueue = |pc: usize, visited: &mut [bool], queue: &mut std::collections::VecDeque<usize>| {
        if pc < visited.len() && !visited[pc] {
            visited[pc] = true;
            queue.push_back(pc);
        }
    };

    if let Some(f) = program.funcs.get(program.meta.entry_func.0 as usize) {
        enqueue(f.entry_pc as usize, &mut visited, &mut queue);
    } else {
        enqueue(0, &mut visited, &mut queue);
    }
    for g in &program.ui.gallery {
        if let Some(fid) = g.replay_func {
            if let Some(f) = program.funcs.get(fid.0 as usize) {
                enqueue(f.entry_pc as usize, &mut visited, &mut queue);
            }
        }
    }
    if let Some(ts) = &program.ui.title_screen {
        let mut replay_funcs = Vec::new();
        for node in &ts.nodes {
            collect_ui_node_replay_funcs(node, &mut replay_funcs);
        }
        for fid in replay_funcs {
            if let Some(f) = program.funcs.get(fid.0 as usize) {
                enqueue(f.entry_pc as usize, &mut visited, &mut queue);
            }
        }
    }

    while let Some(pc) = queue.pop_front() {
        match &program.ops[pc] {
            Op::Jump(target) => enqueue(*target as usize, &mut visited, &mut queue),
            Op::JumpIfFalse(target) => {
                enqueue(*target as usize, &mut visited, &mut queue);
                enqueue(pc + 1, &mut visited, &mut queue);
            }
            Op::Call { func, .. } => {
                if let Some(f) = program.funcs.get(func.0 as usize) {
                    enqueue(f.entry_pc as usize, &mut visited, &mut queue);
                }
                enqueue(pc + 1, &mut visited, &mut queue);
            }
            Op::Choice {
                items, cancel_pc, ..
            } => {
                for item in items {
                    enqueue(item.target_pc as usize, &mut visited, &mut queue);
                }
                if let Some(cp) = cancel_pc {
                    enqueue(*cp as usize, &mut visited, &mut queue);
                }
                enqueue(pc + 1, &mut visited, &mut queue);
            }
            Op::Ret { .. } | Op::End { .. } => {}
            _ => enqueue(pc + 1, &mut visited, &mut queue),
        }
    }
    visited
}

fn record_bus_priority(map: &mut BTreeMap<AssetId, AudioBus>, id: AssetId, bus: AudioBus) {
    let rank = |b: AudioBus| match b {
        AudioBus::Bgm => 3u8,
        AudioBus::Sfx => 2,
        AudioBus::Voice => 1,
    };
    map.entry(id)
        .and_modify(|cur| {
            if rank(bus) > rank(*cur) {
                *cur = bus;
            }
        })
        .or_insert(bus);
}

fn infer_audio_buses_from_ops(
    title_bgm: Option<AssetId>,
    ops: &[Op],
) -> BTreeMap<AssetId, AudioBus> {
    let mut map = BTreeMap::new();
    if let Some(id) = title_bgm {
        record_bus_priority(&mut map, id, AudioBus::Bgm);
    }
    for op in ops {
        match op {
            Op::AudioPlay { bus, asset, .. } => record_bus_priority(&mut map, *asset, *bus),
            Op::Say { voice: Some(a), .. } => record_bus_priority(&mut map, *a, AudioBus::Voice),
            Op::ChoiceList {
                click_se: Some(a), ..
            } => record_bus_priority(&mut map, *a, AudioBus::Sfx),
            Op::ChoiceMenu {
                hover_se, click_se, ..
            } => {
                if let Some(a) = hover_se {
                    record_bus_priority(&mut map, *a, AudioBus::Sfx);
                }
                if let Some(a) = click_se {
                    record_bus_priority(&mut map, *a, AudioBus::Sfx);
                }
            }
            _ => {}
        }
    }
    map
}

fn infer_audio_buses(program: &NirProgram) -> BTreeMap<AssetId, AudioBus> {
    let mut map = infer_audio_buses_from_ops(program.meta.title_bgm, &program.ops);
    if let Some(ts) = &program.ui.title_screen {
        if let Some(id) = ts.bgm {
            record_bus_priority(&mut map, id, AudioBus::Bgm);
        }
    }
    for tl in &program.timelines {
        for clip in &tl.clips {
            if let TimelineClipKind::Sound { asset, bus, .. } = clip.kind {
                record_bus_priority(&mut map, asset, bus);
            }
        }
    }
    map
}

fn collect_program_assets(program: &NirProgram) -> BTreeSet<AssetId> {
    let mut out = BTreeSet::new();
    if let Some(a) = program.meta.title_bg {
        out.insert(a);
    }
    if let Some(a) = program.meta.title_bgm {
        out.insert(a);
    }
    for &f in program
        .meta
        .ui_fonts
        .iter()
        .chain(program.meta.text_fonts.iter())
    {
        out.insert(f);
    }
    if let Some(a) = program.ui.dialogue_bg {
        out.insert(a);
    }
    if let Some(ts) = &program.ui.title_screen {
        if let Some(a) = ts.background {
            out.insert(a);
        }
        if let Some(a) = ts.bgm {
            out.insert(a);
        }
        for n in &ts.nodes {
            collect_ui_node_assets(n, &mut out);
        }
    }
    for g in &program.ui.gallery {
        if let Some(t) = g.thumb {
            out.insert(t);
        }
        for &a in &g.cg_assets {
            out.insert(a);
        }
    }
    for st in &program.ui.dialogue_styles {
        if let Some(a) = st.dialogue_bg {
            out.insert(a);
        }
    }
    for tl in &program.timelines {
        for clip in &tl.clips {
            match &clip.kind {
                TimelineClipKind::Image { asset, .. } | TimelineClipKind::Sound { asset, .. } => {
                    out.insert(*asset);
                }
                TimelineClipKind::Box { .. } => {}
            }
        }
    }
    for menu in &program.menus {
        if let Some(bg) = menu.background {
            out.insert(bg);
        }
        for item in &menu.items {
            if let Some(a) = item.image {
                out.insert(a);
            }
            if let Some(a) = item.hover_image {
                out.insert(a);
            }
            if let Some(a) = item.disabled_image {
                out.insert(a);
            }
            if let Some(a) = item.preview_image {
                out.insert(a);
            }
        }
    }
    for &id in program.asset_lookup.values() {
        out.insert(id);
    }
    for table in program.texts.values() {
        for entry in table {
            for span in &entry.spans {
                if let RichSpan::InlineImage { asset, .. } = span {
                    out.insert(*asset);
                }
            }
        }
    }
    let reachable = compute_reachable_ops(program);
    for (pc, op) in program.ops.iter().enumerate() {
        if !reachable[pc] {
            continue;
        }
        match op {
            Op::LayerSet { asset: Some(a), .. } | Op::LayerAsset { asset: Some(a), .. } => {
                out.insert(*a);
            }
            Op::StageCommit { transition, .. } | Op::TextWindow { transition, .. } => {
                if let Some(m) = transition.mask_asset() {
                    out.insert(m);
                }
            }
            Op::Say { voice: Some(a), .. } | Op::SetDialoguePortrait(Some(a)) => {
                out.insert(*a);
            }
            Op::Choice { items, .. } => {
                for item in items {
                    if let Some(a) = item.image {
                        out.insert(a);
                    }
                    if let Some(a) = item.hover_image {
                        out.insert(a);
                    }
                    if let Some(a) = item.disabled_image {
                        out.insert(a);
                    }
                    if let Some((a, _)) = item.preview {
                        out.insert(a);
                    }
                }
            }
            Op::ChoiceList {
                click_se: Some(a), ..
            } => {
                out.insert(*a);
            }
            Op::ChoiceMenu {
                hover_se, click_se, ..
            } => {
                if let Some(a) = hover_se {
                    out.insert(*a);
                }
                if let Some(a) = click_se {
                    out.insert(*a);
                }
            }
            Op::AudioPlay { asset, .. } | Op::VideoPlay { asset, .. } => {
                out.insert(*asset);
            }
            _ => {}
        }
    }
    out
}

fn remap_program_assets(program: &mut NirProgram, remap: &BTreeMap<AssetId, AssetId>) {
    let m = |id: AssetId| remap.get(&id).copied().unwrap_or(id);
    remap_ops_assets(&mut program.ops, remap);
    program.meta.title_bg = program.meta.title_bg.map(m);
    program.meta.title_bgm = program.meta.title_bgm.map(m);
    for id in &mut program.meta.ui_fonts {
        *id = m(*id);
    }
    for id in &mut program.meta.text_fonts {
        *id = m(*id);
    }
    program.ui.dialogue_bg = program.ui.dialogue_bg.map(m);
    if let Some(ts) = &mut program.ui.title_screen {
        ts.background = ts.background.map(m);
        ts.bgm = ts.bgm.map(m);
        for n in &mut ts.nodes {
            remap_ui_node_assets(n, remap);
        }
    }
    for g in &mut program.ui.gallery {
        g.thumb = g.thumb.map(m);
        for a in &mut g.cg_assets {
            *a = m(*a);
        }
    }
    for st in &mut program.ui.dialogue_styles {
        st.dialogue_bg = st.dialogue_bg.map(m);
    }
    for tl in &mut program.timelines {
        for clip in &mut tl.clips {
            match &mut clip.kind {
                TimelineClipKind::Image { asset, .. } | TimelineClipKind::Sound { asset, .. } => {
                    *asset = m(*asset);
                }
                TimelineClipKind::Box { .. } => {}
            }
        }
    }
    for menu in &mut program.menus {
        menu.background = menu.background.map(m);
        for item in &mut menu.items {
            item.image = item.image.map(m);
            item.hover_image = item.hover_image.map(m);
            item.disabled_image = item.disabled_image.map(m);
            item.preview_image = item.preview_image.map(m);
        }
    }
    program.asset_lookup.retain(|_, id| remap.contains_key(id));
    for id in program.asset_lookup.values_mut() {
        *id = m(*id);
    }
    for table in program.texts.values_mut() {
        for entry in table {
            for span in &mut entry.spans {
                if let RichSpan::InlineImage { asset, .. } = span {
                    *asset = m(*asset);
                }
            }
        }
    }
}

fn remap_ops_assets(ops: &mut [Op], remap: &BTreeMap<AssetId, AssetId>) {
    let m = |id: AssetId| remap.get(&id).copied().unwrap_or(id);
    for op in ops {
        match op {
            Op::LayerSet { asset, .. } | Op::LayerAsset { asset, .. } => {
                *asset = asset.map(m);
            }
            Op::StageCommit { transition, .. } | Op::TextWindow { transition, .. } => {
                if let Transition::RuleMask { mask, .. } = transition {
                    *mask = m(*mask);
                }
            }
            Op::Say { voice, .. } => {
                *voice = voice.map(m);
            }
            Op::SetDialoguePortrait(asset) => {
                *asset = asset.map(m);
            }
            Op::Choice { items, .. } => {
                for item in items {
                    item.image = item.image.map(m);
                    item.hover_image = item.hover_image.map(m);
                    item.disabled_image = item.disabled_image.map(m);
                    item.preview = item.preview.map(|(a, pos)| (m(a), pos));
                }
            }
            Op::ChoiceList { click_se, .. } => {
                *click_se = click_se.map(m);
            }
            Op::ChoiceMenu {
                hover_se, click_se, ..
            } => {
                *hover_se = hover_se.map(m);
                *click_se = click_se.map(m);
            }
            Op::AudioPlay { asset, .. } | Op::VideoPlay { asset, .. } => {
                *asset = m(*asset);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_boilerplate_project_compiles_trims_and_tree_shakes() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("scripts")).unwrap();
        fs::create_dir_all(dir.path().join("assets/bg")).unwrap();
        fs::create_dir_all(dir.path().join("assets/char")).unwrap();

        // Write minimal game.toml (no catalog.toml, no module.toml, no contracts.json, no game.lock!)
        fs::write(
            dir.path().join("game.toml"),
            r#"
[game]
id = "org.nir.demo"
title = "Demo"
locale = "zh-Hans"
"#,
        )
        .unwrap();

        // Create a background PNG (solid opaque)
        let bg = image::RgbaImage::from_pixel(64, 64, image::Rgba([20, 40, 80, 255]));
        bg.save(dir.path().join("assets/bg/sky.png")).unwrap();

        // Create a sprite PNG with 80% transparent margin -> should auto-trim!
        let mut sp = image::RgbaImage::from_pixel(100, 100, image::Rgba([0, 0, 0, 0]));
        for y in 40..60 {
            for x in 40..60 {
                sp.put_pixel(x, y, image::Rgba([255, 100, 100, 255]));
            }
        }
        sp.save(dir.path().join("assets/char/hero.png")).unwrap();

        // Unused asset -> should be tree-shaken out!
        bg.save(dir.path().join("assets/bg/unused.png")).unwrap();

        // Write clean modern .nirs script
        fs::write(
            dir.path().join("scripts/main.nirs"),
            r#"
character hero "主角" color=#66ccff
var score = 0

label start:
    bg bg/sky with dissolve 300
    show hero char/hero at center with dissolve 200
    hero: "你好，{b}新引擎{/b}！当前分数：{score}"
    $ score += 10
    if score >= 10:
        hero: "分数达标了！"
    endif
    menu:
        - "继续" -> ending
        - "退出" -> ending

label ending:
    end_game good
"#,
        )
        .unwrap();

        let built = compile_project(dir.path(), &OptimizeConfig::default()).unwrap();
        assert_eq!(built.summary.total_assets_scanned, 3);
        assert_eq!(built.summary.packed_assets, 2);
        assert_eq!(built.summary.pruned_assets, 1);
        assert_eq!(built.summary.trimmed_sprites, 1);

        // Verify .nirpak decodes cleanly and runs in nir-vm? Check header!
        let (decoded, media_base) = decode_nirpak_header(&built.nirpak).unwrap();
        assert_eq!(decoded.meta.game_id, "org.nir.demo");
        assert!(decoded.features & FEATURE_TRIMMED != 0);
        assert!(decoded.features & FEATURE_WEBP != 0);
        let hero_meta = decoded
            .assets
            .iter()
            .find(|a| decoded.str(a.name) == "char/hero")
            .unwrap();
        let trim = hero_meta.trim.expect("hero sprite should be auto-trimmed");
        assert_eq!((trim.x, trim.y, trim.w, trim.h), (40, 40, 20, 20));
        assert_eq!((trim.orig_w, trim.orig_h), (100, 100));
        assert!(!nirpak_asset_slice(&built.nirpak, media_base, hero_meta)
            .unwrap()
            .is_empty());

        // Second compile hits metadata-backed disk cache without losing trim/dimensions!
        let cached = compile_project(dir.path(), &OptimizeConfig::default()).unwrap();
        assert_eq!(cached.summary.trimmed_sprites, 1);
        assert_eq!(cached.summary.packed_assets, 2);
        let (cached_prog, _) = decode_nirpak_header(&cached.nirpak).unwrap();
        let cached_hero = cached_prog
            .assets
            .iter()
            .find(|a| cached_prog.str(a.name) == "char/hero")
            .unwrap();
        assert_eq!(cached_hero.trim, Some(trim));

        // Fast dev compile reads dimensions from headers without transcoding.
        let fast = compile_project_fast(dir.path()).unwrap();
        let (fast_prog, _) = decode_nirpak_header(&fast.nirpak).unwrap();
        let fast_sky = fast_prog
            .assets
            .iter()
            .find(|a| fast_prog.str(a.name) == "bg/sky")
            .unwrap();
        assert_eq!((fast_sky.width, fast_sky.height), (64, 64));
    }

    #[test]
    fn build_direct_program_tree_shakes_remaps_and_optimizes() {
        let dir = tempfile::tempdir().unwrap();
        let make_png = |rgba: [u8; 4], border_alpha: bool| -> Vec<u8> {
            let mut img = image::RgbaImage::from_pixel(64, 64, image::Rgba([0, 0, 0, 0]));
            let range = if border_alpha { 20..44 } else { 0..64 };
            for y in range.clone() {
                for x in range.clone() {
                    img.put_pixel(x, y, image::Rgba(rgba));
                }
            }
            let mut buf = Vec::new();
            image::DynamicImage::ImageRgba8(img)
                .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
                .unwrap();
            buf
        };

        let unused_png = make_png([10, 10, 10, 255], false);
        let sprite_png = make_png([255, 80, 40, 255], true);
        let dyn_png = make_png([40, 180, 255, 255], false);

        let raw_assets = vec![
            DirectAssetSource {
                name: "unused".into(),
                kind: AssetKind::Image,
                codec: MediaCodec::Png,
                loader: Arc::new(move || Ok(unused_png.clone())),
                pixel_loader: None,
                source_fingerprint: None,
            },
            DirectAssetSource {
                name: "sprite".into(),
                kind: AssetKind::Image,
                codec: MediaCodec::Png,
                loader: Arc::new(move || Ok(sprite_png.clone())),
                pixel_loader: None,
                source_fingerprint: None,
            },
            DirectAssetSource {
                name: "dyn_bg".into(),
                kind: AssetKind::Image,
                codec: MediaCodec::Png,
                loader: Arc::new(move || Ok(dyn_png.clone())),
                pixel_loader: None,
                source_fingerprint: None,
            },
        ];

        let mut lookup = BTreeMap::new();
        lookup.insert("gfx/dyn_bg.png".into(), AssetId(2));

        let prog = NirProgram {
            features: 0,
            meta: GameMeta {
                game_id: "org.nir.direct".into(),
                title: "Direct".into(),
                version: "1.0".into(),
                stage_width: 1280,
                stage_height: 720,
                default_locale: "en".into(),
                locales: vec!["en".into()],
                entry_func: FuncId(0),
                save_slots: 4,
                default_reveal_us: 0,
                title_bg: None,
                title_bgm: None,
                ui_fonts: vec![],
                text_fonts: vec![],
            },
            strings: vec!["main".into(), "btn".into()],
            vars: vec![],
            funcs: vec![FuncMeta {
                name: StrId(0),
                entry_pc: 0,
                param_count: 0,
                local_count: 0,
            }],
            labels: vec![],
            ops: vec![Op::LayerSet {
                layer: LayerId(0),
                parent: None,
                asset: Some(AssetId(1)),
                transform: Box::new(Transform2D::default()),
            }],
            texts: BTreeMap::from([("en".into(), vec![])]),
            ui: UiTheme::default(),
            assets: vec![],
            asset_lookup: lookup,
            timelines: vec![],
            menus: vec![MenuTemplate {
                size: [1280, 720],
                background: None,
                items: vec![MenuItemDef {
                    name: StrId(1),
                    label: StrId(1),
                    rect: [0.0, 0.0, 64.0, 64.0],
                    image: Some(AssetId(1)),
                    hover_image: None,
                    disabled_image: None,
                    preview_image: None,
                    preview_pos: [0.0, 0.0],
                }],
            }],
        };

        let built = build_direct_program(
            prog,
            raw_assets,
            &BTreeSet::new(),
            &dir.path().join("cache"),
            &OptimizeConfig::default(),
        )
        .unwrap();

        assert_eq!(built.summary.total_assets_scanned, 3);
        assert_eq!(built.summary.packed_assets, 2);
        assert_eq!(built.summary.pruned_assets, 1);
        assert_eq!(built.summary.trimmed_sprites, 1);

        let (decoded, _) = decode_nirpak_header(&built.nirpak).unwrap();
        // AssetId(1) -> remapped to AssetId(0); AssetId(2) -> remapped to AssetId(1)
        assert_eq!(
            decoded.asset_lookup.get("gfx/dyn_bg.png"),
            Some(&AssetId(1))
        );
        assert_eq!(decoded.menus[0].items[0].image, Some(AssetId(0)));
        assert!(matches!(
            decoded.ops[0],
            Op::LayerSet {
                asset: Some(AssetId(0)),
                ..
            }
        ));
    }

    #[test]
    fn streaming_direct_build_generates_gallery_thumbs_and_ktx2() {
        let dir = tempfile::tempdir().unwrap();
        let make_png = |w: u32, h: u32, rgba: [u8; 4]| -> Vec<u8> {
            let img = image::RgbaImage::from_pixel(w, h, image::Rgba(rgba));
            let mut buf = Vec::new();
            image::DynamicImage::ImageRgba8(img)
                .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
                .unwrap();
            buf
        };
        let cg_png = make_png(800, 600, [120, 160, 220, 255]);

        // 1) Streaming build with automatic gallery thumbnail generation
        let raw_assets = vec![DirectAssetSource {
            name: "cg01".into(),
            kind: AssetKind::Image,
            codec: MediaCodec::Png,
            loader: Arc::new({
                let b = cg_png.clone();
                move || Ok(b.clone())
            }),
            pixel_loader: None,
            source_fingerprint: None,
        }];
        let mut ui = UiTheme::default();
        ui.gallery.push(GalleryItem {
            id: StrId(1),
            title: StrId(1),
            thumb: Some(AssetId(0)),
            unlock_flag: None,
            replay_func: None,
            cg_assets: vec![AssetId(0)],
        });
        let prog = NirProgram {
            features: 0,
            meta: GameMeta {
                game_id: "org.nir.streamtest".into(),
                title: "Stream".into(),
                version: "1.0".into(),
                stage_width: 800,
                stage_height: 600,
                default_locale: "en".into(),
                locales: vec!["en".into()],
                entry_func: FuncId(0),
                save_slots: 4,
                default_reveal_us: 0,
                title_bg: None,
                title_bgm: None,
                ui_fonts: vec![],
                text_fonts: vec![],
            },
            strings: vec!["main".into(), "cg01".into()],
            vars: vec![],
            funcs: vec![FuncMeta {
                name: StrId(0),
                entry_pc: 0,
                param_count: 0,
                local_count: 0,
            }],
            labels: vec![],
            ops: vec![Op::End { outcome: StrId(0) }],
            texts: BTreeMap::from([("en".into(), vec![])]),
            ui,
            assets: vec![],
            asset_lookup: BTreeMap::new(),
            timelines: vec![],
            menus: vec![],
        };

        let out_pak = dir.path().join("out.nirpak");
        let summary = build_direct_program_to_file(
            prog.clone(),
            raw_assets,
            &BTreeSet::new(),
            &dir.path().join("cache1"),
            &OptimizeConfig::default(),
            &out_pak,
        )
        .unwrap();
        assert_eq!(summary.generated_thumbs, 1);
        let pak_bytes = fs::read(&out_pak).unwrap();
        let (decoded, _) = decode_nirpak_header(&pak_bytes).unwrap();
        let thumb_id = decoded.ui.gallery[0].thumb.unwrap();
        assert_ne!(thumb_id, decoded.ui.gallery[0].cg_assets[0]);
        let thumb_meta = decoded.asset(thumb_id).unwrap();
        assert!(thumb_meta.width <= 256 && thumb_meta.height <= 144);

        // 2) KTX2 BC1 encoding path
        let raw_ktx = vec![DirectAssetSource {
            name: "cg01".into(),
            kind: AssetKind::Image,
            codec: MediaCodec::Png,
            loader: Arc::new(move || Ok(cg_png.clone())),
            pixel_loader: None,
            source_fingerprint: None,
        }];
        let opt_ktx = OptimizeConfig {
            ktx2_textures: true,
            gallery_thumbnails: false,
            ..OptimizeConfig::default()
        };
        let built_ktx = build_direct_program(
            prog,
            raw_ktx,
            &BTreeSet::new(),
            &dir.path().join("cache2"),
            &opt_ktx,
        )
        .unwrap();
        let (dktx, _) = decode_nirpak_header(&built_ktx.nirpak).unwrap();
        assert_eq!(dktx.assets[0].codec, MediaCodec::Ktx2);
        assert_ne!(dktx.features & FEATURE_KTX2, 0);
    }

    #[test]
    fn trimmed_sprite_union_bbox_delta_and_cfg_pruning_work() {
        let dir = tempfile::tempdir().unwrap();
        // Build 3 standing sprites on a 512x512 canvas with rich body texture + slightly differing
        // trimmed bounding boxes (e.g. ahoge/ribbon + eye expression change).
        let make_sprite = |variant: u8| -> Vec<u8> {
            let mut img = image::RgbaImage::from_pixel(512, 512, image::Rgba([0, 0, 0, 0]));
            // Shared detailed body (non-linear high-entropy pattern like real sprite artwork): x in 140..370, y in 90..480
            for y in 90..480u32 {
                for x in 140..370u32 {
                    let h = x
                        .wrapping_mul(0x9E37_79B1)
                        .wrapping_add(y.wrapping_mul(0x85EB_CA77));
                    let h = (h ^ (h >> 15)).wrapping_mul(0x2C1B_3C6D);
                    let r = (h & 255) as u8;
                    let g = ((h >> 8) & 255) as u8;
                    let b = ((h >> 16) & 255) as u8;
                    img.put_pixel(x, y, image::Rgba([r, g, b, 255]));
                }
            }
            // Variant-specific tiny extension + eye change so each sprite trims to a distinct rect!
            if variant == 1 {
                for y in 82..90u32 {
                    for x in 240..256u32 {
                        img.put_pixel(x, y, image::Rgba([220, 40, 80, 255]));
                    }
                }
                for y in 160..178u32 {
                    for x in 210..270u32 {
                        img.put_pixel(x, y, image::Rgba([250, 220, 200, 255]));
                    }
                }
            } else if variant == 2 {
                for y in 150..168u32 {
                    for x in 370..382u32 {
                        img.put_pixel(x, y, image::Rgba([60, 180, 240, 255]));
                    }
                }
                for y in 180..196u32 {
                    for x in 220..260u32 {
                        img.put_pixel(x, y, image::Rgba([255, 80, 120, 255]));
                    }
                }
            }
            let mut buf = Vec::new();
            image::DynamicImage::ImageRgba8(img)
                .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
                .unwrap();
            buf
        };

        let sp0 = make_sprite(0);
        let sp1 = make_sprite(1);
        let sp2 = make_sprite(2);
        let dead_png = make_sprite(0);

        let mk_src = |name: &str, bytes: Vec<u8>| DirectAssetSource {
            name: name.into(),
            kind: AssetKind::Image,
            codec: MediaCodec::Png,
            loader: Arc::new(move || Ok(bytes.clone())),
            pixel_loader: None,
            source_fingerprint: None,
        };
        let raw_assets = vec![
            mk_src("char/a_01", sp0.clone()),
            mk_src("char/a_02", sp1.clone()),
            mk_src("char/a_03", sp2.clone()),
            mk_src("dead/unreachable", dead_png),
        ];

        // Program uses assets 0, 1, 2 in reachable entry func; asset 3 is ONLY in unreachable pc 4!
        let prog = NirProgram {
            features: 0,
            meta: GameMeta {
                game_id: "org.nir.trimdelta".into(),
                title: "TrimDelta".into(),
                version: "1.0".into(),
                stage_width: 512,
                stage_height: 512,
                default_locale: "en".into(),
                locales: vec!["en".into()],
                entry_func: FuncId(0),
                save_slots: 4,
                default_reveal_us: 0,
                title_bg: None,
                title_bgm: None,
                ui_fonts: vec![],
                text_fonts: vec![],
            },
            strings: vec!["main".into()],
            vars: vec![],
            funcs: vec![FuncMeta {
                name: StrId(0),
                entry_pc: 0,
                param_count: 0,
                local_count: 0,
            }],
            labels: vec![],
            ops: vec![
                Op::LayerSet {
                    layer: LayerId(0),
                    parent: None,
                    asset: Some(AssetId(0)),
                    transform: Box::new(Transform2D::default()),
                },
                Op::LayerAsset {
                    layer: LayerId(0),
                    asset: Some(AssetId(1)),
                },
                Op::LayerAsset {
                    layer: LayerId(0),
                    asset: Some(AssetId(2)),
                },
                Op::End { outcome: StrId(0) },
                // Unreachable dead op referencing AssetId(3) -> must be CFG-pruned!
                Op::LayerAsset {
                    layer: LayerId(0),
                    asset: Some(AssetId(3)),
                },
            ],
            texts: BTreeMap::from([("en".into(), vec![])]),
            ui: UiTheme::default(),
            assets: vec![],
            asset_lookup: BTreeMap::new(),
            timelines: vec![],
            menus: vec![],
        };

        let cache = dir.path().join("cache");
        let opt = OptimizeConfig {
            lossless_images: true,
            ..OptimizeConfig::default()
        };
        let built = build_direct_program(prog.clone(), raw_assets, &BTreeSet::new(), &cache, &opt)
            .unwrap();
        // CFG pruning must drop dead/unreachable (AssetId(3))!
        assert_eq!(built.summary.packed_assets, 3);
        assert_eq!(built.summary.pruned_assets, 1);
        assert_eq!(built.summary.trimmed_sprites, 3);
        // Trimmed sprites must be union-bbox delta-compressed!
        assert!(
            built.summary.delta_patched >= 2,
            "expected >= 2 trimmed sprites delta-patched, got {}",
            built.summary.delta_patched
        );

        let (dec, _) = decode_nirpak_header(&built.nirpak).unwrap();
        assert_ne!(dec.features & FEATURE_DELTA_IMAGE, 0);
        // Verify union trim aligns across base and delta children!
        let mut delta_count = 0usize;
        for a in &dec.assets {
            if let Some(d) = &a.delta {
                delta_count += 1;
                let base = dec.asset(d.base).unwrap();
                assert_eq!(a.trim, base.trim, "delta child & base must share union trim");
                assert_eq!((a.width, a.height), (base.width, base.height));
                assert!(!d.tile_mask.is_empty());
            }
        }
        assert!(delta_count >= 2);

        // Verify warm cache hit reproduces identical delta plan!
        let raw_assets2 = vec![
            mk_src("char/a_01", sp0),
            mk_src("char/a_02", sp1),
            mk_src("char/a_03", sp2),
            mk_src("dead/unreachable", make_sprite(0)),
        ];
        let cached = build_direct_program(prog, raw_assets2, &BTreeSet::new(), &cache, &opt)
            .unwrap();
        assert_eq!(cached.summary.delta_patched, built.summary.delta_patched);
    }

    #[test]
    fn auto_embed_cjk_font_embeds_compact_subset_when_system_font_available() {
        if optimize::find_system_cjk_font().is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let prog = NirProgram {
            features: 0,
            meta: GameMeta {
                game_id: "org.nir.embedfont".into(),
                title: "测试标题".into(),
                version: "1.0".into(),
                stage_width: 1280,
                stage_height: 720,
                default_locale: "zh-Hans".into(),
                locales: vec!["zh-Hans".into()],
                entry_func: FuncId(0),
                save_slots: 4,
                default_reveal_us: 0,
                title_bg: None,
                title_bgm: None,
                ui_fonts: vec![],
                text_fonts: vec![],
            },
            strings: vec!["main".into()],
            vars: vec![],
            funcs: vec![FuncMeta {
                name: StrId(0),
                entry_pc: 0,
                param_count: 0,
                local_count: 0,
            }],
            labels: vec![],
            ops: vec![Op::End { outcome: StrId(0) }],
            texts: BTreeMap::from([(
                "zh-Hans".into(),
                vec![TextEntry::plain("你好世界！日本語テスト。")],
            )]),
            ui: UiTheme::default(),
            assets: vec![],
            asset_lookup: BTreeMap::new(),
            timelines: vec![],
            menus: vec![],
        };
        let opt = OptimizeConfig {
            auto_embed_cjk_font: true,
            ..OptimizeConfig::default()
        };
        let built = build_direct_program(
            prog,
            vec![],
            &BTreeSet::new(),
            &dir.path().join("cache"),
            &opt,
        )
        .unwrap();
        assert_eq!(built.program.meta.ui_fonts.len(), 1);
        assert_eq!(built.program.meta.text_fonts.len(), 1);
        let fid = built.program.meta.ui_fonts[0];
        let blob = &built.media_blobs[fid.0 as usize];
        assert!(
            blob.len() < 400 * 1024,
            "embedded subset font should be tiny (<400KB), got {} bytes",
            blob.len()
        );
        let face = ttf_parser::Face::parse(blob, 0).expect("embedded font must parse cleanly");
        assert!(face.glyph_index('你').is_some());
        assert!(face.glyph_index('世').is_some());
        assert!(face.glyph_index('語').is_some());
        assert!(face.glyph_index('A').is_some());
    }
}
