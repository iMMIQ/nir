use anyhow::{anyhow, bail, Context, Result};
use nir_core::ValidatedProgram;
use nir_format::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GameManifest {
    pub project_format: u32,
    pub game: Game,
    pub engine: Engine,
    pub stage: Stage,
    pub inputs: Inputs,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Game {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub version: String,
    pub source_locale: String,
    #[serde(default)]
    pub title_scene: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Engine {
    pub api: String,
    pub capability_profile: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Inputs {
    pub modules: Vec<String>,
    pub asset_catalogs: Vec<String>,
    pub theme: String,
    #[serde(default)]
    pub scenarios: Vec<String>,
    #[serde(default)]
    pub notices: Vec<String>,
}
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Module {
    module_format: u32,
    id: String,
    sources: Vec<String>,
    text_contracts: String,
    exports: BTreeMap<String, String>,
    text_bundles: BTreeMap<String, String>,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Fragment {
    fragment_format: u32,
    #[serde(default)]
    variables: BTreeMap<String, Value>,
    #[serde(default)]
    functions: BTreeMap<String, Function>,
    #[serde(default)]
    scenes: BTreeMap<String, Vec<Node>>,
    #[serde(default)]
    cues: BTreeMap<String, Cue>,
    #[serde(default)]
    choices: BTreeMap<String, Choice>,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Catalog {
    format: u32,
    assets: Vec<AssetSource>,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct AssetSource {
    id: String,
    kind: AssetKind,
    source: String,
    rights: String,
    #[serde(default)]
    expected_size: Option<[u32; 2]>,
}
#[derive(Debug)]
pub struct LoadedProject {
    pub root: PathBuf,
    pub manifest: GameManifest,
    pub program: Program,
    pub media: BTreeMap<String, Vec<u8>>,
    pub provenance: BTreeMap<String, String>,
}
pub fn relative(root: &Path, base: &Path, name: &str) -> Result<PathBuf> {
    let name_path = Path::new(name);
    if name.is_empty()
        || name.contains('\\')
        || name.contains(':')
        || name_path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        bail!("E_PATH: expected project-relative path: {name}");
    }
    let root = fs::canonicalize(root)?;
    let joined = base.join(name_path);
    let resolved = fs::canonicalize(&joined)
        .with_context(|| format!("E_FILE_MISSING: {}", joined.display()))?;
    if !resolved.starts_with(&root) {
        bail!("E_PATH_ESCAPE: {}", joined.display());
    }
    if !resolved.is_file() {
        bail!("E_FILE: {} is not a file", joined.display());
    }
    Ok(resolved)
}
fn read(path: &Path) -> Result<Vec<u8>> {
    let len = fs::metadata(path)?.len();
    if len > 64 * 1024 * 1024 {
        bail!("E_LIMIT: source file {} exceeds 64 MiB", path.display());
    }
    let b = fs::read(path)?;
    if b.starts_with(b"version https://git-lfs.github.com/spec/v1") {
        bail!(
            "E_LFS_POINTER: fetch the media bytes for {}",
            path.display()
        );
    }
    Ok(b)
}
fn json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    Ok(nir_content::parse(
        &read(path)?,
        &path.display().to_string(),
    )?)
}
fn toml_file<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    toml::from_str(std::str::from_utf8(&read(path)?)?)
        .with_context(|| format!("E_TOML: {}", path.display()))
}
fn merge<T>(dest: &mut BTreeMap<String, T>, src: BTreeMap<String, T>, file: &Path) -> Result<()> {
    for (id, item) in src {
        if dest.insert(id.clone(), item).is_some() {
            bail!("E_DUPLICATE: {}: {id}", file.display());
        }
    }
    Ok(())
}
pub fn load_project(root: &Path) -> Result<LoadedProject> {
    let root = fs::canonicalize(root)?;
    let manifest: GameManifest = toml_file(&root.join("game.toml"))?;
    for name in &manifest.inputs.notices {
        read(&relative(&root, &root, name)?)?;
    }
    if manifest.project_format != 1
        || manifest.engine.api != "nir-player/0.1"
        || manifest.engine.capability_profile != "web-v1"
    {
        bail!("E_VERSION: unsupported project/engine profile");
    }
    if manifest.inputs.modules.len() != 1 {
        bail!("E_CAPABILITY: this release supports exactly one module");
    }
    if manifest.game.slug.is_empty()
        || !manifest
            .game
            .slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        bail!("E_SLUG: expected lowercase ASCII slug");
    }
    let module_path = relative(&root, &root, &manifest.inputs.modules[0])?;
    let module: Module = toml_file(&module_path)?;
    let base = module_path.parent().unwrap();
    if module.module_format != 1 || module.id.contains('.') || module.id.is_empty() {
        bail!("E_MODULE: unsupported identity/format");
    }
    let entry = module
        .exports
        .get("start")
        .ok_or_else(|| anyhow!("E_EXPORT: missing start export"))?
        .clone();
    let mut program = Program {
        format: 1,
        game_id: manifest.game.id.clone(),
        revision: String::new(),
        entry,
        requires: CAPABILITIES.iter().map(|s| s.to_string()).collect(),
        stage: manifest.stage.clone(),
        variables: BTreeMap::new(),
        functions: BTreeMap::new(),
        scenes: BTreeMap::new(),
        cues: BTreeMap::new(),
        choices: BTreeMap::new(),
        texts: json(&relative(&root, base, &module.text_contracts)?)?,
        locales: BTreeMap::new(),
        assets: BTreeMap::new(),
        default_locale: manifest.game.source_locale.clone(),
        title_scene: manifest.game.title_scene.clone(),
        theme: json(&relative(&root, &root, &manifest.inputs.theme)?)?,
    };
    for source in &module.sources {
        let path = relative(&root, base, source)?;
        let f: Fragment = json(&path)?;
        if f.fragment_format != 1 {
            bail!("E_FRAGMENT: {}", path.display());
        }
        merge(&mut program.variables, f.variables, &path)?;
        merge(&mut program.functions, f.functions, &path)?;
        merge(&mut program.scenes, f.scenes, &path)?;
        merge(&mut program.cues, f.cues, &path)?;
        merge(&mut program.choices, f.choices, &path)?;
    }
    for (locale, path) in &module.text_bundles {
        program
            .locales
            .insert(locale.clone(), json(&relative(&root, base, path)?)?);
    }
    let mut media = BTreeMap::new();
    let mut provenance = BTreeMap::new();
    let mut normalized = BTreeMap::new();
    for catalog in &manifest.inputs.asset_catalogs {
        let path = relative(&root, &root, catalog)?;
        let catalog: Catalog = toml_file(&path)?;
        if catalog.format != 1 {
            bail!("E_CATALOG_VERSION: {}", path.display());
        }
        for source in catalog.assets {
            if source.rights.trim().is_empty() {
                bail!("E_RIGHTS: {} lacks source/permission record", source.id);
            }
            let source_path = relative(&root, path.parent().unwrap(), &source.source)?;
            let local = source_path
                .strip_prefix(&root)?
                .to_string_lossy()
                .to_string();
            let normalized_path = local.nfc().collect::<String>().to_lowercase();
            if let Some(old) = normalized.insert(normalized_path, local.clone()) {
                if old != local {
                    bail!("E_PATH_COLLISION: {old} / {local}");
                }
            }
            let bytes = read(&source_path)?;
            let mut asset = Asset {
                kind: source.kind,
                object: nir_content::digest(&bytes),
                bytes: bytes.len() as u64,
                width: 0,
                height: 0,
                duration_us: Micros(0),
                decoded_bytes: 0,
            };
            match asset.kind {
                AssetKind::Image => {
                    let img = image::ImageReader::new(std::io::Cursor::new(&bytes))
                        .with_guessed_format()?
                        .into_dimensions()
                        .context("E_IMAGE: dimensions")?;
                    if img.0 > 8192 || img.1 > 8192 || img.0 == 0 || img.1 == 0 {
                        bail!("E_LIMIT: image {} dimensions", source.id);
                    }
                    asset.width = img.0;
                    asset.height = img.1;
                    asset.decoded_bytes = img.0 as u64 * img.1 as u64 * 4;
                    if source.expected_size.is_some_and(|s| s != [img.0, img.1]) {
                        bail!("E_ASSET_SIZE: {} is {}x{}", source.id, img.0, img.1);
                    }
                    image::load_from_memory(&bytes).context("E_IMAGE: corrupt image")?;
                }
                AssetKind::Audio => {
                    let (duration, decoded) = wav_info(&bytes)?;
                    asset.duration_us = Micros(duration);
                    asset.decoded_bytes = decoded;
                }
                AssetKind::Font => {
                    ttf_parser::Face::parse(&bytes, 0)
                        .map_err(|_| anyhow!("E_FONT: invalid font {}", source.id))?;
                    asset.decoded_bytes = asset.bytes * 4;
                }
            }
            if program.assets.insert(source.id.clone(), asset).is_some() {
                bail!("E_DUPLICATE: asset {}", source.id);
            }
            provenance.insert(source.id.clone(), source.rights);
            media.insert(source.id, bytes);
        }
    }
    if let Some(title) = &program.title_scene {
        if !program.scenes.contains_key(title) {
            bail!("E_SCENE: title scene {title}");
        }
    }
    validate_font_coverage(&program, &media)?;
    // Revision depends on canonical source content, never local paths or iteration order.
    program.revision = nir_content::digest(&serde_json::to_vec(&program)?);
    ValidatedProgram::new(program.clone())?;
    for source in &manifest.inputs.scenarios {
        relative(&root, &root, source)?;
    }
    Ok(LoadedProject {
        root,
        manifest,
        program,
        media,
        provenance,
    })
}
fn wav_info(b: &[u8]) -> Result<(u64, u64)> {
    if b.len() < 44 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        bail!("E_AUDIO_FORMAT: v0.1 expects PCM WAV");
    }
    let mut pos = 12;
    let mut rate = 0u32;
    let mut channels = 0u16;
    let mut bits = 0u16;
    let mut data = 0usize;
    while pos + 8 <= b.len() {
        let n = u32::from_le_bytes(b[pos + 4..pos + 8].try_into()?) as usize;
        let start = pos + 8;
        let end = start
            .checked_add(n)
            .ok_or_else(|| anyhow!("E_AUDIO_FORMAT: size overflow"))?;
        if end > b.len() {
            bail!("E_AUDIO_FORMAT: truncated chunk");
        }
        match &b[pos..pos + 4] {
            b"fmt " => {
                if n < 16 || u16::from_le_bytes(b[start..start + 2].try_into()?) != 1 {
                    bail!("E_AUDIO_FORMAT: PCM required");
                }
                channels = u16::from_le_bytes(b[start + 2..start + 4].try_into()?);
                rate = u32::from_le_bytes(b[start + 4..start + 8].try_into()?);
                bits = u16::from_le_bytes(b[start + 14..start + 16].try_into()?);
            }
            b"data" => data = n,
            _ => {}
        }
        pos = end + (n % 2);
    }
    if !(1..=2).contains(&channels) || !(8000..=96000).contains(&rate) || bits != 16 || data == 0 {
        bail!("E_AUDIO_FORMAT: expected mono/stereo 16-bit PCM");
    }
    let frames = data as u64 / (channels as u64 * 2);
    Ok((
        frames * 1_000_000 / rate as u64,
        frames * channels as u64 * 4,
    ))
}
fn validate_font_coverage(p: &Program, media: &BTreeMap<String, Vec<u8>>) -> Result<()> {
    let fonts: Vec<_> = p
        .assets
        .iter()
        .filter(|(_, a)| a.kind == AssetKind::Font)
        .filter_map(|(id, _)| ttf_parser::Face::parse(&media[id], 0).ok())
        .collect();
    if fonts.is_empty() {
        bail!("E_FONT: register a font asset");
    }
    let mut chars = BTreeSet::new();
    for docs in p.locales.values() {
        for d in docs.values() {
            for span in &d.spans {
                if let Span::Text { text, .. } = span {
                    chars.extend(text.chars());
                }
            }
        }
    }
    for v in p.variables.values() {
        if let Value::String(s) = v {
            chars.extend(s.chars());
        }
    }
    let missing: Vec<_> = chars
        .into_iter()
        .filter(|c| !c.is_whitespace() && !fonts.iter().any(|f| f.glyph_index(*c).is_some()))
        .take(12)
        .collect();
    if !missing.is_empty() {
        bail!("E_FONT_COVERAGE: missing glyphs {missing:?}; add an appropriately licensed font");
    }
    Ok(())
}
pub fn compile(p: &Program) -> Result<Executable> {
    ValidatedProgram::new(p.clone())?;
    let mut addresses = vec![];
    let mut resume_map = BTreeMap::new();
    let mut semantic_cost_map = vec![];
    for (fid, f) in &p.functions {
        for (bid, b) in &f.blocks {
            for op in 0..=b.ops.len() {
                let a = Address {
                    function: fid.clone(),
                    block: bid.clone(),
                    op,
                    stable_id: b
                        .ops
                        .get(op)
                        .map(|o| o.id.clone())
                        .unwrap_or_else(|| "@terminator".into()),
                };
                resume_map.insert(a.key(), addresses.len() as u32);
                addresses.push(a);
                semantic_cost_map.push(1);
            }
        }
    }
    let activation_recipes = p
        .cues
        .keys()
        .map(|c| (c.clone(), nir_content::cue_assets(p, c)))
        .collect();
    let e = Executable {
        format: 1,
        program: p.clone(),
        addresses,
        resume_map,
        semantic_cost_map,
        activation_recipes,
    };
    validate_executable(&e)?;
    Ok(e)
}
pub fn validate_executable(e: &Executable) -> Result<()> {
    ValidatedProgram::new(e.program.clone())?;
    nir_content::validate_executable(e)?;
    Ok(())
}
pub fn runtime_roots(p: &Program) -> BTreeSet<String> {
    let mut roots = BTreeSet::new();
    for cue in p.cues.keys() {
        roots.extend(nir_content::cue_assets(p, cue));
    }
    if let Some(scene) = &p.title_scene {
        roots.extend(p.scenes[scene].iter().filter_map(|n| n.asset.clone()));
    }
    roots
}

pub fn write_schemas(out: &Path) -> Result<()> {
    fs::create_dir_all(out)?;
    let schemas = [
        ("game", schemars::schema_for!(GameManifest)),
        ("module", schemars::schema_for!(Module)),
        ("fragment", schemars::schema_for!(Fragment)),
        ("assets", schemars::schema_for!(Catalog)),
        ("texts", schemars::schema_for!(BTreeMap<String,TextDoc>)),
        (
            "text-contracts",
            schemars::schema_for!(BTreeMap<String,TextContract>),
        ),
        ("theme", schemars::schema_for!(Theme)),
        ("program", schemars::schema_for!(Program)),
    ];
    for (name, schema) in schemas {
        fs::write(
            out.join(format!("{name}.schema.json")),
            serde_json::to_vec_pretty(&schema)?,
        )?;
    }
    Ok(())
}
