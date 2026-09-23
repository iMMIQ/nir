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
    #[serde(default = "runtime_preset")]
    pub runtime_preset: String,
}
fn runtime_preset() -> String {
    "web-standard".into()
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Inputs {
    pub modules: Vec<String>,
    pub asset_catalogs: Vec<String>,
    pub theme: String,
    #[serde(default)]
    pub locales: Option<String>,
    #[serde(default)]
    pub player: Option<String>,
    #[serde(default)]
    pub scenarios: Vec<String>,
    #[serde(default)]
    pub notices: Vec<String>,
}
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Module {
    module_format: u32,
    id: String,
    sources: Vec<String>,
    pub(crate) text_contracts: String,
    #[serde(default)]
    pub(crate) text_revisions: Option<String>,
    exports: BTreeMap<String, String>,
    pub(crate) text_bundles: BTreeMap<String, String>,
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
    #[serde(default)]
    font: Option<crate::FontRecipe>,
}
#[derive(Debug)]
pub struct LoadedProject {
    pub root: PathBuf,
    pub manifest: GameManifest,
    pub program: Program,
    pub media: BTreeMap<String, Vec<u8>>,
    pub provenance: BTreeMap<String, String>,
    pub resolved_config: crate::ResolvedConfig,
    pub fonts: BTreeMap<String, crate::FontReport>,
    pub font_notices: BTreeMap<String, String>,
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
pub(crate) fn json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = read(path)?;
    nir_content::parse(&bytes, &path.display().to_string()).map_err(|mut d| {
        // Source-position reparsing belongs to author tools, never the WASM loader.
        if d.code == "E_SCHEMA" {
            if let Err(original) = serde_json::from_slice::<T>(&bytes) {
                if let Some(details) = &mut d.details {
                    details.source = Some(SourceRef {
                        file: path.display().to_string(),
                        line: original.line(),
                        column: original.column(),
                        pointer: String::new(),
                    });
                }
            }
        }
        anyhow::Error::new(d)
    })
}
pub(crate) fn toml_file<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = read(path)?;
    let text = std::str::from_utf8(&bytes)?;
    toml::from_str(text).map_err(|e: toml::de::Error| {
        let mut d = Diagnostic::new("E_TOML", path.display().to_string(), e.message()).classified(
            ErrorDomain::Content,
            "parse",
            "toml",
            vec![Recovery::FixContent],
        );
        if let Some(span) = e.span() {
            let prefix = &bytes[..span.start];
            d.details.as_mut().unwrap().source = Some(SourceRef {
                file: path.display().to_string(),
                line: prefix.iter().filter(|b| **b == b'\n').count() + 1,
                column: prefix.len()
                    - prefix
                        .iter()
                        .rposition(|b| *b == b'\n')
                        .map_or(0, |p| p + 1)
                    + 1,
                pointer: String::new(),
            });
        }
        anyhow::Error::new(d)
    })
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
    let locale_path = manifest.inputs.locales.as_deref().ok_or_else(|| {
        anyhow!("E_LOCALE_CONFIG: add inputs.locales = \"config/locales.toml\" and define UI/text font plans")
    }).and_then(|name| relative(&root, &root, name))?;
    let locale_manifest: crate::LocaleManifest = toml_file(&locale_path)?;
    let locale_config = locale_manifest.resolve()?;
    if locale_config.default_text != manifest.game.source_locale {
        bail!("E_LOCALE_DEFAULT: game.source_locale must match config/locales.toml default_text");
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
    let (theme, player, resolved_config) = crate::config::resolve_config(&root, &manifest)?;
    let texts = crate::texts::compiled_texts(&root)?;
    let mut program = Program {
        format: FORMAT_VERSION,
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
        texts: texts.contracts,
        locales: texts.locales,
        locale_config: locale_config.clone(),
        assets: BTreeMap::new(),
        default_locale: manifest.game.source_locale.clone(),
        title_scene: manifest.game.title_scene.clone(),
        theme,
        player,
    };
    let mut sources = crate::diagnostics::SourceIndex::default();
    for source in &module.sources {
        let path = relative(&root, base, source)?;
        let f: Fragment = json(&path)?;
        sources.fragment(&root, &path, &read(&path)?);
        if f.fragment_format != 1 {
            bail!("E_FRAGMENT: {}", path.display());
        }
        merge(&mut program.variables, f.variables, &path)?;
        merge(&mut program.functions, f.functions, &path)?;
        merge(&mut program.scenes, f.scenes, &path)?;
        merge(&mut program.cues, f.cues, &path)?;
        merge(&mut program.choices, f.choices, &path)?;
    }
    if locale_config.text.keys().collect::<BTreeSet<_>>()
        != program.locales.keys().collect::<BTreeSet<_>>()
    {
        bail!("E_TRANSLATION: config/locales.toml text plans must exactly match complete text bundles");
    }
    let character_sets = crate::fonts::characters_by_plan(&program, &manifest.game.title)?;
    let mut font_characters: BTreeMap<String, BTreeSet<char>> = BTreeMap::new();
    for (plans, sets) in [
        (&locale_config.ui, &character_sets.ui),
        (&locale_config.text, &character_sets.text),
    ] {
        for (locale, plan) in plans {
            let chars = sets
                .get(locale)
                .ok_or_else(|| anyhow!("E_LOCALE_CONFIG: no character set for {locale}"))?;
            for font in &plan.fonts {
                font_characters
                    .entry(font.clone())
                    .or_default()
                    .extend(chars);
            }
        }
    }
    let mut fonts = BTreeMap::new();
    let mut font_notices = BTreeMap::new();
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
            let mut bytes = read(&source_path)?;
            if let Some(recipe) = &source.font {
                if source.kind != AssetKind::Font {
                    bail!("E_FONT_RECIPE: font recipe requires kind = font");
                }
                let license_path = relative(&root, path.parent().unwrap(), &recipe.license)?;
                let license = String::from_utf8(read(&license_path)?)
                    .context("E_FONT_LICENSE: expected UTF-8 license")?;
                if license.trim().is_empty() {
                    bail!("E_FONT_LICENSE: empty license for {}", source.id);
                }
                let chars = font_characters.get(&source.id).ok_or_else(|| {
                    anyhow!(
                        "E_FONT_PLAN_UNUSED: font {} is not referenced by a UI or text locale",
                        source.id
                    )
                })?;
                let (prepared, mut report) = crate::fonts::prepare(&root, &bytes, recipe, chars)
                    .with_context(|| format!("font {} ({local})", source.id))?;
                report.source = local.clone();
                report.license = license_path.strip_prefix(&root)?.to_string_lossy().into();
                report.license_digest = nir_content::digest(license.as_bytes());
                font_notices.insert(source.id.clone(), license);
                fonts.insert(source.id.clone(), report);
                bytes = prepared;
            }
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
    let asset_objects: BTreeMap<_, _> = program
        .assets
        .iter()
        .map(|(id, asset)| (id.clone(), asset.object.clone()))
        .collect();
    for plan in program
        .locale_config
        .ui
        .values_mut()
        .chain(program.locale_config.text.values_mut())
    {
        plan.digest = LocaleFontPlan::digest_for(&plan.fonts, &asset_objects);
    }
    crate::fonts::coverage_by_plan(&program, &media, &character_sets)?;
    // Revision depends on canonical source content, never local paths or iteration order.
    program.revision = nir_content::digest(&serde_json::to_vec(&program)?);
    ValidatedProgram::new(program.clone()).map_err(|d| sources.annotate(d))?;
    for source in &manifest.inputs.scenarios {
        relative(&root, &root, source)?;
    }
    Ok(LoadedProject {
        root,
        manifest,
        program,
        media,
        provenance,
        resolved_config,
        fonts,
        font_notices,
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
        format: FORMAT_VERSION,
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
        (
            "texts",
            schemars::schema_for!(BTreeMap<String,crate::AuthorTextDoc>),
        ),
        (
            "text-contracts",
            schemars::schema_for!(BTreeMap<String,crate::AuthorTextContract>),
        ),
        (
            "text-revisions",
            schemars::schema_for!(crate::TextRevisions),
        ),
        ("theme", schemars::schema_for!(crate::ThemeManifest)),
        ("theme-tokens", schemars::schema_for!(crate::ThemeTokens)),
        ("player", schemars::schema_for!(crate::PlayerConfig)),
        ("locales", schemars::schema_for!(crate::LocaleManifest)),
        ("program", schemars::schema_for!(Program)),
        ("diagnostic", schemars::schema_for!(Diagnostic)),
    ];
    for (name, schema) in schemas {
        fs::write(
            out.join(format!("{name}.schema.json")),
            serde_json::to_vec_pretty(&schema)?,
        )?;
    }
    Ok(())
}
