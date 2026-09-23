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
    #[serde(default)]
    pub shared: Vec<String>,
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
    pub(crate) module_format: u32,
    pub(crate) id: String,
    pub(crate) sources: Vec<String>,
    pub(crate) text_contracts: String,
    #[serde(default)]
    pub(crate) text_revisions: Option<String>,
    pub(crate) exports: BTreeMap<String, String>,
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
struct ModuleFragments {
    module: Module,
    functions: BTreeMap<String, Function>,
    scenes: BTreeMap<String, Vec<Node>>,
    cues: BTreeMap<String, Cue>,
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

fn module_key(module: &str, id: &str, namespaced: bool) -> String {
    if namespaced {
        format!("{module}.{id}")
    } else {
        id.to_owned()
    }
}

pub(crate) fn valid_module_id(id: &str) -> bool {
    !id.is_empty()
        && !id.contains('.')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn qualify_local_refs(module: &str, f: &mut Function, namespaced: bool) {
    if !namespaced {
        return;
    }
    for block in f.blocks.values_mut() {
        for op in &mut block.ops {
            op.id = format!("{module}.{}", op.id);
            match &mut op.operation {
                Operation::TaskControl { task, .. } | Operation::DialogueContinue { task } => {
                    *task = format!("{module}.{task}");
                }
                _ => {}
            }
        }
        match &mut block.terminator {
            Terminator::Activate { cue, .. } => *cue = format!("{module}.{cue}"),
            Terminator::Interact { choice, .. } => *choice = format!("{module}.{choice}"),
            Terminator::Await { conditions, .. } => {
                for condition in conditions {
                    condition.task = format!("{module}.{}", condition.task);
                }
            }
            _ => {}
        }
    }
}

fn qualify_cue_refs(module: &str, cue: &mut Cue, namespaced: bool) {
    if !namespaced {
        return;
    }
    for def in &mut cue.effects {
        def.id = format!("{module}.{}", def.id);
        match &mut def.effect {
            Effect::StagePresent { scene, .. } => *scene = format!("{module}.{scene}"),
            Effect::Dialogue { text, speaker, .. } => {
                *text = format!("{module}.{text}");
                if !speaker.is_empty() {
                    *speaker = format!("{module}.{speaker}");
                }
            }
            _ => {}
        }
    }
}

fn qualify_choice_refs(module: &str, choice: &mut Choice, namespaced: bool) {
    if namespaced {
        for option in &mut choice.options {
            option.text = format!("{module}.{}", option.text);
        }
    }
}

fn resolve_call(
    module: &Module,
    function: &str,
    modules: &BTreeMap<String, Module>,
    local_functions: &BTreeSet<String>,
    namespaced: bool,
) -> Result<String> {
    if !namespaced {
        return Ok(function.to_owned());
    }
    for (target_id, target) in modules {
        if let Some((prefix, alias)) = function.split_once('.') {
            if prefix == target_id {
                let local = target.exports.get(alias).ok_or_else(|| {
                    anyhow!("E_EXPORT: {target_id}.{alias} is not an exported function")
                })?;
                return Ok(format!("{target_id}.{local}"));
            }
        }
    }
    if local_functions.contains(function) {
        return Ok(format!("{}.{}", module.id, function));
    }
    bail!("E_FUNCTION: {} has no local function {function}", module.id)
}

fn load_module_specs(root: &Path, manifest: &GameManifest) -> Result<Vec<(PathBuf, Module)>> {
    if manifest.inputs.modules.is_empty() {
        bail!("E_CAPABILITY: project must list at least one module");
    }
    if manifest.inputs.modules.len() > 4096 {
        bail!("E_LIMIT: too many modules");
    }
    let mut out = Vec::new();
    let mut ids = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for name in &manifest.inputs.modules {
        let path = relative(root, root, name)?;
        if !paths.insert(path.clone()) {
            bail!("E_DUPLICATE: module file {}", path.display());
        }
        let module: Module = toml_file(&path)?;
        if module.module_format != 1 || !valid_module_id(&module.id) {
            bail!(
                "E_MODULE: unsupported identity/format in {}",
                path.display()
            );
        }
        if !ids.insert(module.id.clone()) {
            bail!("E_MODULE: duplicate module id {}", module.id);
        }
        for alias in module.exports.keys() {
            if alias.is_empty()
                || alias.contains('.')
                || !alias
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            {
                bail!("E_EXPORT: export names must be nonempty and contain no dot");
            }
        }
        out.push((path, module));
    }
    Ok(out)
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
    let module_specs = load_module_specs(&root, &manifest)?;
    let namespaced = module_specs.len() > 1;
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
    let entry_module = &module_specs[0].1;
    let entry_local = entry_module.exports.get("start").ok_or_else(|| {
        anyhow!(
            "E_EXPORT: first module {} is missing start export",
            entry_module.id
        )
    })?;
    let entry_module_id = entry_module.id.clone();
    let entry_local = entry_local.clone();
    let (theme, player, resolved_config) = crate::config::resolve_config(&root, &manifest)?;
    let texts = crate::texts::compiled_texts(&root, namespaced)?;
    let entry = module_key(&entry_module_id, &entry_local, namespaced);
    let mut program = Program {
        format: FORMAT_VERSION,
        game_id: manifest.game.id.clone(),
        revision: String::new(),
        entry,
        requires: CAPABILITIES.iter().map(|s| s.to_string()).collect(),
        stage: manifest.stage.clone(),
        variables: BTreeMap::new(),
        functions: BTreeMap::new(),
        modules: BTreeMap::new(),
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
    let mut fragment_paths = BTreeSet::new();
    for name in &manifest.inputs.shared {
        let path = relative(&root, &root, name)?;
        if !fragment_paths.insert(path.clone()) {
            bail!("E_DUPLICATE: shared fragment {}", path.display());
        }
        let f: Fragment = json(&path)?;
        sources.fragment(&root, &path, &read(&path)?);
        if f.fragment_format != 1 {
            bail!("E_FRAGMENT: {}", path.display());
        }
        if !f.functions.is_empty()
            || !f.scenes.is_empty()
            || !f.cues.is_empty()
            || !f.choices.is_empty()
        {
            bail!("E_SHARED_FRAGMENT: shared fragments currently contain global variables only");
        }
        merge(&mut program.variables, f.variables, &path)?;
    }
    let mut loaded_modules = Vec::new();
    for (module_path, module) in module_specs {
        let base = module_path.parent().unwrap();
        let mut loaded = ModuleFragments {
            module: module.clone(),
            functions: BTreeMap::new(),
            scenes: BTreeMap::new(),
            cues: BTreeMap::new(),
            choices: BTreeMap::new(),
        };
        for source in &module.sources {
            let path = relative(&root, base, source)?;
            if !fragment_paths.insert(path.clone()) {
                bail!(
                    "E_DUPLICATE: fragment {} is listed more than once",
                    path.display()
                );
            }
            let f: Fragment = json(&path)?;
            sources.fragment_in_module(
                &root,
                &path,
                &read(&path)?,
                namespaced.then_some(module.id.as_str()),
            );
            if f.fragment_format != 1 {
                bail!("E_FRAGMENT: {}", path.display());
            }
            if namespaced && !f.variables.is_empty() {
                bail!(
                    "E_SHARED_VARIABLE: declare cross-module variables in inputs.shared ({})",
                    path.display()
                );
            }
            if !namespaced {
                merge(&mut program.variables, f.variables, &path)?;
            }
            merge(&mut loaded.functions, f.functions, &path)?;
            merge(&mut loaded.scenes, f.scenes, &path)?;
            merge(&mut loaded.cues, f.cues, &path)?;
            merge(&mut loaded.choices, f.choices, &path)?;
        }
        loaded_modules.push(loaded);
    }
    let module_map: BTreeMap<_, _> = loaded_modules
        .iter()
        .map(|m| (m.module.id.clone(), m.module.clone()))
        .collect();
    let function_names: BTreeMap<String, BTreeSet<String>> = loaded_modules
        .iter()
        .map(|m| (m.module.id.clone(), m.functions.keys().cloned().collect()))
        .collect();
    for loaded in &loaded_modules {
        let locals = &function_names[&loaded.module.id];
        for (alias, function) in &loaded.module.exports {
            if !locals.contains(function) {
                bail!(
                    "E_EXPORT: {}.{alias} points to missing local function {function}",
                    loaded.module.id
                );
            }
        }
    }
    if !function_names[&entry_module_id].contains(&entry_local) {
        bail!(
            "E_EXPORT: {}.start points to missing local function {entry_local}",
            entry_module_id
        );
    }
    for loaded in loaded_modules {
        let local_names = &function_names[&loaded.module.id];
        for (id, mut function) in loaded.functions {
            if namespaced {
                for block in function.blocks.values_mut() {
                    if let Terminator::Call { function: call, .. } = &mut block.terminator {
                        *call = resolve_call(&loaded.module, call, &module_map, local_names, true)?;
                    }
                }
            }
            qualify_local_refs(&loaded.module.id, &mut function, namespaced);
            let id = module_key(&loaded.module.id, &id, namespaced);
            if program
                .functions
                .insert(id.clone(), function.clone())
                .is_some()
            {
                bail!("E_DUPLICATE: function {id}");
            }
            program
                .modules
                .entry(loaded.module.id.clone())
                .or_default()
                .functions
                .insert(id, FunctionSignature::from(&function));
        }
        for (id, nodes) in loaded.scenes {
            let key = module_key(&loaded.module.id, &id, namespaced);
            if program.scenes.insert(key.clone(), nodes).is_some() {
                bail!("E_DUPLICATE: scene {key}");
            }
        }
        for (id, mut cue) in loaded.cues {
            qualify_cue_refs(&loaded.module.id, &mut cue, namespaced);
            let key = module_key(&loaded.module.id, &id, namespaced);
            if program.cues.insert(key.clone(), cue).is_some() {
                bail!("E_DUPLICATE: cue {key}");
            }
        }
        for (id, mut choice) in loaded.choices {
            qualify_choice_refs(&loaded.module.id, &mut choice, namespaced);
            let key = module_key(&loaded.module.id, &id, namespaced);
            if program.choices.insert(key.clone(), choice).is_some() {
                bail!("E_DUPLICATE: choice {key}");
            }
        }
    }
    for (module, texts) in texts.module_texts {
        program.modules.entry(module).or_default().texts = texts;
    }
    if namespaced {
        if let Some(scene) = &program.title_scene {
            if !program.scenes.contains_key(scene) {
                let matches: Vec<_> = program
                    .scenes
                    .keys()
                    .filter(|id| id.rsplit('.').next() == Some(scene.as_str()))
                    .cloned()
                    .collect();
                if matches.len() == 1 {
                    program.title_scene = matches.into_iter().next();
                } else {
                    bail!(
                        "E_SCENE: multi-module title_scene must identify one module scene: {scene}"
                    );
                }
            }
        }
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
