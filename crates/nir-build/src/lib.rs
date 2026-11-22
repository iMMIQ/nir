//! Zero-boilerplate NIR v2 project compiler, auto-optimizer, and multi-target packager.
//!
//! A complete game project needs ONLY:
//! - `game.toml` (~10 lines)
//! - `scripts/*.nirs`
//! - `assets/` (auto-discovered: PNG/WebP/JPG/WAV/MP3/OGG/FLAC/TTF/OTF/MP4/WebM)
//! - optional `locales/<lang>.toml` & `ui/theme.toml`
#![forbid(unsafe_code)]

pub mod nirs;
pub mod optimize;

use anyhow::{bail, Context, Result};
use nir_ir::*;
use optimize::{detect_asset_kind_and_codec, optimize_audio, optimize_image, subset_font, OptimizeConfig};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
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
}

impl Default for AndroidSection {
    fn default() -> Self {
        Self {
            version_code: 1,
            min_sdk: 26,
            target_sdk: 34,
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
    pub saved_bytes: i64,
    pub nirpak_bytes: usize,
}

pub struct BuiltBundle {
    pub program: NirProgram,
    pub media_blobs: Vec<Vec<u8>>,
    pub nirpak: Vec<u8>,
    pub summary: BuildSummary,
}

/// Asset input for `build_direct_program`, decoupled from filesystem layout or importer crates.
pub struct DirectAssetSource {
    pub name: String,
    pub kind: AssetKind,
    pub codec: MediaCodec,
    pub loader: Arc<dyn Fn() -> Result<Vec<u8>> + Send + Sync>,
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
        bail!("E_PROJECT: missing `scripts/` directory in {}", root.display());
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
    let ui_fonts: Vec<AssetId> = fonts.iter().filter_map(|id| remap.get(id).copied()).collect();

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

    let optimize_one = |orig_idx: usize| -> Result<AssetWorkOutput> {
        let src = &discovered[orig_idx];
        let raw = fs::read(&src.path)
            .with_context(|| format!("reading asset {}", src.path.display()))?;
        let mut features = 0u32;
        let mut trimmed = false;
        let out = match src.kind {
            AssetKind::Image => {
                let res = optimize_image(&raw, src.codec, opt, &cache_dir)?;
                let saved_bytes = res.saved_bytes;
                if res.codec == MediaCodec::Webp {
                    features |= FEATURE_WEBP;
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
                let res = optimize_audio(&raw, src.codec, opt, &cache_dir)?;
                let saved_bytes = res.saved_bytes;
                match res.codec {
                    MediaCodec::Mp3 => features |= FEATURE_MP3,
                    MediaCodec::Ogg => features |= FEATURE_OGG,
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
        kept_indices.iter().map(|&idx| optimize_one(idx)).collect()
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
                    let res = optimize_one(kept_indices[i]);
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
        });
        media_blobs.push(out.bytes);
    }

    let program = NirProgram {
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
        pruned_assets: total_scanned.saturating_sub(program.assets.len()),
        trimmed_sprites,
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
        "player_web.js",
        "player_web_bg.wasm",
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
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
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
    };
    if let Some(parent) = out_apk.parent() {
        fs::create_dir_all(parent)?;
    }
    nir_apk::build_apk_borrowed(out_apk, &apk_spec, &entries, &key)?;
    Ok(())
}

/// Optimizes, tree-shakes, remaps, and packs a direct-to-IR `NirProgram` + lazy raw assets
/// into a production `.nirpak` bundle using the exact same parallel optimizer pipeline as `.nirs`.
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
    mut program: NirProgram,
    raw_assets: Vec<DirectAssetSource>,
    extra_keep: &BTreeSet<AssetId>,
    cache_dir: &Path,
    opt: &OptimizeConfig,
    compression: flate2::Compression,
) -> Result<BuiltBundle> {
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

    // 5. Load & optimize kept assets in parallel across CPU cores!
    struct DirectAssetOut {
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

    let optimize_one = |orig_idx: usize| -> Result<DirectAssetOut> {
        let src = &raw_assets[orig_idx];
        let raw = (src.loader)()
            .with_context(|| format!("loading direct asset `{}`", src.name))?;
        let mut features = 0u32;
        let mut trimmed = false;
        let out = match src.kind {
            AssetKind::Image => {
                let res = optimize_image(&raw, src.codec, opt, cache_dir)?;
                if res.codec == MediaCodec::Webp {
                    features |= FEATURE_WEBP;
                }
                if res.trim.is_some() {
                    features |= FEATURE_TRIMMED;
                    trimmed = true;
                }
                DirectAssetOut {
                    bytes: res.bytes,
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
                let res = optimize_audio(&raw, src.codec, opt, cache_dir)?;
                match res.codec {
                    MediaCodec::Mp3 => features |= FEATURE_MP3,
                    MediaCodec::Ogg => features |= FEATURE_OGG,
                    _ => {}
                }
                DirectAssetOut {
                    bytes: res.bytes,
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
                DirectAssetOut {
                    bytes: sub.clone(),
                    codec: src.codec,
                    width: 0,
                    height: 0,
                    trim: None,
                    duration_us: 0,
                    sample_rate: 0,
                    channels: 0,
                    loop_region: None,
                    saved_bytes: orig_len - sub.len() as i64,
                    features: 0,
                    trimmed: false,
                }
            }
            AssetKind::Video => {
                features |= FEATURE_VIDEO;
                DirectAssetOut {
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
            AssetKind::Raw => DirectAssetOut {
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

    let results: Vec<Result<DirectAssetOut>> = if worker_count <= 1 || kept_indices.len() <= 1 {
        kept_indices.iter().map(|&idx| optimize_one(idx)).collect()
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
                    let res = optimize_one(kept_indices[i]);
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
    let mut media_blobs = Vec::with_capacity(kept_indices.len());
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
        });
        media_blobs.push(out.bytes);
    }
    program.features = features;
    program.assets = assets_meta;

    let blob_refs: Vec<&[u8]> = media_blobs.iter().map(Vec::as_slice).collect();
    let nirpak = encode_nirpak_with_compression(&program, &blob_refs, compression)?;
    let text_count = program
        .texts
        .get(&program.meta.default_locale)
        .map_or(0, Vec::len);

    let summary = BuildSummary {
        game_id: program.meta.game_id.clone(),
        title: program.meta.title.clone(),
        scripts: program.funcs.len(),
        ops: program.ops.len(),
        texts: text_count,
        total_assets_scanned: total_scanned,
        packed_assets: program.assets.len(),
        pruned_assets: total_scanned.saturating_sub(program.assets.len()),
        trimmed_sprites,
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
    for op in &program.ops {
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
            Op::ChoiceList { click_se: Some(a), .. } => {
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
    program
        .asset_lookup
        .retain(|_, id| remap.contains_key(id));
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
            },
            DirectAssetSource {
                name: "sprite".into(),
                kind: AssetKind::Image,
                codec: MediaCodec::Png,
                loader: Arc::new(move || Ok(sprite_png.clone())),
            },
            DirectAssetSource {
                name: "dyn_bg".into(),
                kind: AssetKind::Image,
                codec: MediaCodec::Png,
                loader: Arc::new(move || Ok(dyn_png.clone())),
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
        assert_eq!(decoded.asset_lookup.get("gfx/dyn_bg.png"), Some(&AssetId(1)));
        assert_eq!(decoded.menus[0].items[0].image, Some(AssetId(0)));
        assert!(matches!(decoded.ops[0], Op::LayerSet { asset: Some(AssetId(0)), .. }));
    }
}
