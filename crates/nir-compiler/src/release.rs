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
#[derive(Debug, Serialize)]
pub struct BuildReport {
    pub release: String,
    pub game_id: String,
    pub total_bytes: u64,
    pub objects: usize,
    pub module_packages: BTreeMap<String, ModuleBuildReport>,
    pub asset_catalogs: BTreeMap<String, AssetCatalogBuildReport>,
    pub resources: Vec<String>,
    pub excluded_resources: Vec<String>,
    pub provenance: BTreeMap<String, String>,
    pub engine_build: String,
    pub resolved_config: crate::ResolvedConfig,
    pub fonts: BTreeMap<String, crate::FontReport>,
    pub optimization: crate::OptimizeReport,
}
#[derive(Debug, Serialize)]
pub struct ModuleBuildReport {
    pub code: String,
    pub code_bytes: u64,
    pub static_content: String,
    pub static_bytes: u64,
    pub asset_catalogs: Vec<String>,
    pub locales: BTreeMap<String, TextBuildReport>,
}
#[derive(Debug, Serialize)]
pub struct TextBuildReport {
    pub object: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssetCatalogBuildReport {
    pub object: String,
    pub bytes: u64,
    pub consumers: Vec<String>,
    pub assets: Vec<String>,
}

#[derive(Debug, Serialize)]
struct DependencyReport {
    format: u32,
    game_id: String,
    entry: String,
    module_dependencies: BTreeMap<String, Vec<String>>,
    boot: BTreeMap<String, BTreeMap<String, DependencyClosure>>,
    modules: BTreeMap<String, BTreeMap<String, BTreeMap<String, DependencyClosure>>>,
    entry_reachable: BTreeMap<String, BTreeMap<String, DependencyClosure>>,
    scope: String,
}

#[derive(Debug, Clone, Default, Serialize)]
struct DependencyClosure {
    object_count: usize,
    object_bytes: u64,
    file_count: usize,
    file_bytes: u64,
    total_bytes: u64,
    category_bytes: BTreeMap<String, u64>,
    objects: BTreeMap<String, DependencyObject>,
    files: BTreeMap<String, DependencyFile>,
}

#[derive(Debug, Clone, Serialize)]
struct DependencyObject {
    bytes: u64,
    media_type: String,
    categories: Vec<String>,
    reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct DependencyFile {
    sha256: String,
    bytes: u64,
    media_type: String,
    reason: String,
}

#[derive(Clone, Default)]
struct ClosureBuilder {
    objects: BTreeMap<String, (u64, String, BTreeSet<String>, BTreeSet<String>)>,
    files: BTreeMap<String, DependencyFile>,
}
impl ClosureBuilder {
    fn add_object(
        &mut self,
        objects: &BTreeMap<String, Object>,
        hash: &str,
        category: &str,
        reason: String,
    ) -> Result<()> {
        let object = objects
            .get(hash)
            .ok_or_else(|| anyhow::anyhow!("E_DEPENDENCY_OBJECT: missing {hash}"))?;
        let entry = self.objects.entry(hash.to_owned()).or_insert_with(|| {
            (
                object.bytes,
                object.media_type.clone(),
                BTreeSet::new(),
                BTreeSet::new(),
            )
        });
        entry.2.insert(reason);
        entry.3.insert(category.into());
        Ok(())
    }

    fn add_file(&mut self, path: &str, bytes: &[u8], media_type: &str, reason: &str) {
        self.files.insert(
            path.into(),
            DependencyFile {
                sha256: nir_content::digest(bytes),
                bytes: bytes.len() as u64,
                media_type: media_type.into(),
                reason: reason.into(),
            },
        );
    }

    fn finish(self) -> DependencyClosure {
        let object_bytes = self.objects.values().map(|entry| entry.0).sum();
        let file_bytes = self.files.values().map(|entry| entry.bytes).sum();
        let object_count = self.objects.len();
        let file_count = self.files.len();
        let mut category_bytes = BTreeMap::new();
        for (bytes, _, _, categories) in self.objects.values() {
            for category in categories {
                *category_bytes.entry(category.clone()).or_insert(0) += bytes;
            }
        }
        DependencyClosure {
            object_count,
            object_bytes,
            file_count,
            file_bytes,
            total_bytes: object_bytes + file_bytes,
            category_bytes,
            objects: self
                .objects
                .into_iter()
                .map(|(hash, (bytes, media_type, reasons, categories))| {
                    (
                        hash,
                        DependencyObject {
                            bytes,
                            media_type,
                            categories: categories.into_iter().collect(),
                            reasons: reasons.into_iter().collect(),
                        },
                    )
                })
                .collect(),
            files: self.files,
        }
    }
}

struct RuntimeBuild {
    executable: RuntimeExecutable,
    modules: BTreeMap<String, ModuleBuildReport>,
    catalogs: BTreeMap<String, AssetCatalogBuildReport>,
    module_dependencies: BTreeMap<String, BTreeSet<String>>,
    entry_module: String,
    catalog_assets: BTreeMap<String, BTreeSet<String>>,
}

fn module_indexes(program: &Program) -> Result<BTreeMap<String, ModuleIndex>> {
    if !program.modules.is_empty() {
        return Ok(program.modules.clone());
    }
    // Older source fixtures have no module table. Lower them as one runtime
    // module so the release path still exercises the same package contracts.
    let id = "_legacy".to_owned();
    Ok(BTreeMap::from([(
        id,
        ModuleIndex {
            functions: program
                .functions
                .iter()
                .map(|(id, function)| (id.clone(), FunctionSignature::from(function)))
                .collect(),
            texts: program.texts.keys().cloned().collect(),
            code: String::new(),
            static_content: String::new(),
            locales: BTreeMap::new(),
        },
    )]))
}

fn declaration_owner(id: &str, modules: &BTreeSet<String>) -> Result<String> {
    if modules.len() == 1 {
        return Ok(modules.iter().next().unwrap().clone());
    }
    let owner = modules
        .iter()
        .find(|module| {
            id.strip_prefix(module.as_str())
                .is_some_and(|rest| rest.starts_with('.'))
        })
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("E_MODULE_OWNERSHIP: no module owns {id}"))?;
    Ok(owner)
}

fn asset_consumers(
    program: &Program,
    scene_owners: &BTreeMap<String, String>,
    cue_owners: &BTreeMap<String, String>,
    text_owners: &BTreeMap<String, String>,
    choice_owners: &BTreeMap<String, String>,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut consumers: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (choice, definition) in &program.choices {
        if let Some(module) = choice_owners.get(choice) {
            for image in definition
                .options
                .iter()
                .filter_map(|option| option.image.as_ref())
            {
                for asset in std::iter::once(&image.asset)
                    .chain(image.hover_asset.iter())
                    .chain(image.disabled_asset.iter())
                {
                    consumers
                        .entry(asset.clone())
                        .or_default()
                        .insert(format!("module:{module}"));
                }
            }
        }
    }
    for (text, contract) in &program.texts {
        if let Some(module) = text_owners.get(text) {
            for image in &contract.images {
                consumers
                    .entry(image.asset.clone())
                    .or_default()
                    .insert(format!("module:{module}"));
            }
        }
    }
    for (scene, nodes) in &program.scenes {
        if let Some(module) = scene_owners.get(scene) {
            for asset in nodes.iter().filter_map(|node| node.asset.as_ref()) {
                consumers
                    .entry(asset.clone())
                    .or_default()
                    .insert(format!("module:{module}"));
            }
        }
    }
    for (cue, definition) in &program.cues {
        if let Some(module) = cue_owners.get(cue) {
            for effect in &definition.effects {
                if let Effect::StagePresent { transition, .. } = &effect.effect {
                    if let Some(asset) = transition.asset() {
                        consumers
                            .entry(asset.to_owned())
                            .or_default()
                            .insert(format!("module:{module}"));
                    }
                }
                if let Effect::Audio { asset, .. } = &effect.effect {
                    consumers
                        .entry(asset.clone())
                        .or_default()
                        .insert(format!("module:{module}"));
                }
                if effect.effect.uses_compose() {
                    let mut child_audio = BTreeSet::new();
                    effect.effect.collect_audio_assets(&mut child_audio);
                    for asset in child_audio {
                        consumers
                            .entry(asset)
                            .or_default()
                            .insert(format!("module:{module}"));
                    }
                }
            }
        }
    }
    for asset in program.theme.image_assets() {
        consumers
            .entry(asset)
            .or_default()
            .insert("bootstrap".into());
    }
    for asset in program
        .theme
        .image_menus
        .values()
        .flat_map(nir_format::ImageMenu::effect_assets)
    {
        consumers
            .entry(asset)
            .or_default()
            .insert("menu-effects".into());
    }
    let title_nodes = program
        .title_scene
        .as_ref()
        .and_then(|scene| program.scenes.get(scene))
        .or_else(|| program.scenes.values().next());
    if let Some(nodes) = title_nodes {
        for asset in nodes.iter().filter_map(|node| node.asset.as_ref()) {
            consumers
                .entry(asset.clone())
                .or_default()
                .insert("bootstrap".into());
        }
    }
    for (locale, plan) in &program.locale_config.ui {
        for font in &plan.fonts {
            consumers
                .entry(font.clone())
                .or_default()
                .insert(format!("locale:ui:{locale}"));
        }
    }
    for (locale, plan) in &program.locale_config.text {
        for font in &plan.fonts {
            consumers
                .entry(font.clone())
                .or_default()
                .insert(format!("locale:text:{locale}"));
        }
    }
    consumers
}

fn dependency_graph(
    program: &Program,
    function_owners: &BTreeMap<String, String>,
    modules: &BTreeSet<String>,
) -> Result<(BTreeMap<String, BTreeSet<String>>, String)> {
    let mut graph: BTreeMap<String, BTreeSet<String>> = modules
        .iter()
        .map(|module| (module.clone(), BTreeSet::new()))
        .collect();
    for (source, function) in &program.functions {
        let source_module = function_owners.get(source).ok_or_else(|| {
            anyhow::anyhow!("E_MODULE_OWNERSHIP: no module owns function {source}")
        })?;
        for block in function.blocks.values() {
            if let Terminator::Call {
                function: target, ..
            } = &block.terminator
            {
                let target_module = function_owners.get(target).ok_or_else(|| {
                    anyhow::anyhow!("E_MODULE_OWNERSHIP: no module owns function {target}")
                })?;
                if source_module != target_module {
                    graph
                        .get_mut(source_module)
                        .unwrap()
                        .insert(target_module.clone());
                }
            }
        }
    }
    let entry_module = function_owners
        .get(&program.entry)
        .cloned()
        .ok_or_else(|| {
            anyhow::anyhow!("E_MODULE_OWNERSHIP: no module owns entry {}", program.entry)
        })?;
    Ok((graph, entry_module))
}

/// Scene declarations are immutable templates. Reusing an identical template
/// retains every activation and its timing; each activation still captures a
/// fresh scene generation. Keep ownership boundaries intact when coalescing.
fn coalesce_scene_templates(source: &Program, modules: &BTreeSet<String>) -> Result<Program> {
    let mut program = source.clone();
    let mut templates: BTreeMap<(String, Vec<u8>), String> = BTreeMap::new();
    let mut aliases = BTreeMap::new();
    for (id, nodes) in &source.scenes {
        let key = (declaration_owner(id, modules)?, serde_json::to_vec(nodes)?);
        if let Some(canonical) = templates.get(&key) {
            aliases.insert(id.clone(), canonical.clone());
            program.scenes.remove(id);
        } else {
            templates.insert(key, id.clone());
        }
    }
    fn remap(effect: &mut Effect, aliases: &BTreeMap<String, String>) {
        match effect {
            Effect::StagePresent { scene, .. } => {
                if let Some(canonical) = aliases.get(scene) {
                    *scene = canonical.clone();
                }
            }
            Effect::Sequence { children } | Effect::ParallelAll { children } => {
                for child in children {
                    remap(&mut child.effect, aliases);
                }
            }
            _ => {}
        }
    }
    for cue in program.cues.values_mut() {
        for effect in &mut cue.effects {
            remap(&mut effect.effect, &aliases);
        }
    }
    if let Some(canonical) = program.title_scene.as_ref().and_then(|id| aliases.get(id)) {
        program.title_scene = Some(canonical.clone());
    }
    Ok(program)
}

/// Split immutable function bodies without renaming functions, task handles,
/// cues or variables. Shared scene/audio/text ownership stays in the author's
/// module, so page transitions keep the same running audio instances.
fn partition_large_code(
    source: &Program,
    modules: &mut BTreeMap<String, ModuleIndex>,
) -> Result<()> {
    const GROUP_BYTES: usize = 1024 * 1024;
    for module in modules.keys().cloned().collect::<Vec<_>>() {
        let functions = modules[&module].functions.clone();
        let sizes = functions
            .keys()
            .map(|id| {
                Ok((
                    id.clone(),
                    serde_json::to_vec(&source.functions[id])?.len() + id.len() + 4,
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        if functions.len() < 2 || sizes.values().sum::<usize>() <= 2 * GROUP_BYTES {
            continue;
        }
        // Each original module retains a real function. Prefer its entry,
        // ensuring startup also prepares its shared static/text packages.
        let retained = if functions.contains_key(&source.entry) {
            source.entry.clone()
        } else {
            functions.keys().next().unwrap().clone()
        };
        modules
            .get_mut(&module)
            .unwrap()
            .functions
            .retain(|id, _| *id == retained);
        let mut group = BTreeMap::new();
        let mut bytes = 0;
        let mut ordinal = 0;
        let flush = |group: &mut BTreeMap<String, FunctionSignature>,
                     ordinal: &mut usize,
                     modules: &mut BTreeMap<String, ModuleIndex>| {
            if group.is_empty() {
                return;
            }
            let id = loop {
                *ordinal += 1;
                let id = format!("{module}.code{ordinal:04}");
                if !modules.contains_key(&id) {
                    break id;
                }
            };
            modules.insert(
                id,
                ModuleIndex {
                    functions: std::mem::take(group),
                    texts: BTreeSet::new(),
                    code: String::new(),
                    static_content: String::new(),
                    locales: BTreeMap::new(),
                },
            );
        };
        for (id, signature) in functions {
            if id == retained {
                continue;
            }
            if bytes + sizes[&id] > GROUP_BYTES {
                flush(&mut group, &mut ordinal, modules);
                bytes = 0;
            }
            bytes += sizes[&id];
            group.insert(id, signature);
        }
        flush(&mut group, &mut ordinal, modules);
    }
    Ok(())
}

fn package_runtime(
    out: &Path,
    objects: &mut BTreeMap<String, Object>,
    source: &Program,
    reference: &Executable,
) -> Result<RuntimeBuild> {
    let mut modules = module_indexes(source)?;
    let author_function_owners: BTreeMap<String, String> = modules
        .iter()
        .flat_map(|(module, index)| {
            index
                .functions
                .keys()
                .map(|id| (id.clone(), module.clone()))
        })
        .collect();
    let author_module_ids: BTreeSet<String> = modules.keys().cloned().collect();
    let coalesced = coalesce_scene_templates(source, &author_module_ids)?;
    let source = &coalesced;
    partition_large_code(source, &mut modules)?;
    let module_ids: BTreeSet<String> = modules.keys().cloned().collect();

    let mut function_owners = BTreeMap::new();
    let mut text_owners = BTreeMap::new();
    for (module, index) in &modules {
        if module.is_empty() || index.functions.is_empty() {
            bail!("E_MODULE: {module} has an empty identity or function interface");
        }
        for (id, signature) in &index.functions {
            let function = source
                .functions
                .get(id)
                .ok_or_else(|| anyhow::anyhow!("E_MODULE: {module} owns missing function {id}"))?;
            if FunctionSignature::from(function) != *signature
                || function_owners.insert(id.clone(), module.clone()).is_some()
            {
                bail!("E_MODULE: invalid or duplicate function ownership for {id}");
            }
        }
        for id in &index.texts {
            if !source.texts.contains_key(id)
                || text_owners.insert(id.clone(), module.clone()).is_some()
            {
                bail!("E_MODULE: invalid or duplicate text ownership for {id}");
            }
        }
    }
    if function_owners.len() != source.functions.len()
        || source
            .functions
            .keys()
            .any(|id| !function_owners.contains_key(id))
        || text_owners.len() != source.texts.len()
        || source.texts.keys().any(|id| !text_owners.contains_key(id))
    {
        bail!("E_MODULE: module indexes do not own every function and text contract");
    }

    let scene_owners: BTreeMap<String, String> = source
        .scenes
        .keys()
        .map(|id| Ok((id.clone(), declaration_owner(id, &author_module_ids)?)))
        .collect::<Result<_>>()?;
    let timeline_owners: BTreeMap<String, String> = source
        .sprite_timelines
        .keys()
        .map(|id| Ok((id.clone(), declaration_owner(id, &author_module_ids)?)))
        .collect::<Result<_>>()?;
    let cue_owners: BTreeMap<String, String> = source
        .cues
        .keys()
        .map(|id| Ok((id.clone(), declaration_owner(id, &author_module_ids)?)))
        .collect::<Result<_>>()?;
    let choice_owners: BTreeMap<String, String> = source
        .choices
        .keys()
        .map(|id| Ok((id.clone(), declaration_owner(id, &author_module_ids)?)))
        .collect::<Result<_>>()?;
    let mut task_owners = BTreeMap::new();
    for (cue, definition) in &source.cues {
        let owner = cue_owners.get(cue).unwrap();
        for effect in &definition.effects {
            if task_owners
                .get(&effect.id)
                .is_some_and(|previous| previous != owner)
            {
                bail!("E_TASK_OWNER: duplicate task {}", effect.id);
            }
            task_owners.insert(effect.id.clone(), owner.clone());
        }
    }

    let consumers = asset_consumers(
        source,
        &scene_owners,
        &cue_owners,
        &text_owners,
        &choice_owners,
    );
    let roots: BTreeSet<String> = consumers.keys().cloned().collect();
    let mut catalog_groups: BTreeMap<BTreeSet<String>, BTreeMap<String, Asset>> = BTreeMap::new();
    for id in &roots {
        let asset = source
            .assets
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("E_ASSET_UNDECLARED: {id}"))?;
        catalog_groups
            .entry(consumers[id].clone())
            .or_default()
            .insert(id.clone(), asset.clone());
    }

    let mut catalog_assets = BTreeMap::new();
    let mut catalog_reports = BTreeMap::new();
    let mut catalogs = BTreeMap::new();
    let mut asset_index = BTreeMap::new();
    // A scene pins every catalog containing one of its assets. Keep catalogs
    // small so a handful of active images/audio cannot pin the metadata for
    // an entire large story alongside its code and text packages.
    let catalog_groups = catalog_groups.into_iter().flat_map(|(consumers, assets)| {
        const MAX_CATALOG_ASSETS: usize = 64;
        let split = assets.len() > MAX_CATALOG_ASSETS;
        let entries: Vec<_> = assets.into_iter().collect();
        let chunks: Vec<BTreeMap<_, _>> = entries
            .chunks(MAX_CATALOG_ASSETS)
            .map(|chunk| chunk.iter().cloned().collect())
            .collect();
        chunks
            .into_iter()
            .map(move |assets| (consumers.clone(), split, assets))
    });
    for (consumer_set, split, assets) in catalog_groups {
        let identity = if split {
            serde_json::to_vec(&(&consumer_set, assets.keys().collect::<Vec<_>>()))?
        } else {
            serde_json::to_vec(&consumer_set)?
        };
        let id = format!("catalog.{}", nir_content::digest(&identity));
        let package = AssetCatalog {
            format: CONTENT_PACKAGE_VERSION,
            catalog: id.clone(),
            assets: assets.clone(),
        };
        let bytes = serde_json::to_vec(&package)?;
        let hash = object(out, objects, &bytes, "json", "application/json")?;
        catalogs.insert(id.clone(), hash.clone());
        catalog_assets.insert(id.clone(), assets.keys().cloned().collect());
        for (asset_id, asset) in &assets {
            asset_index.insert(
                asset_id.clone(),
                AssetIndexEntry {
                    kind: asset.kind,
                    object: asset.object.clone(),
                    catalog: id.clone(),
                },
            );
        }
        catalog_reports.insert(
            id.clone(),
            AssetCatalogBuildReport {
                object: hash,
                bytes: bytes.len() as u64,
                consumers: consumer_set.iter().cloned().collect(),
                assets: assets.keys().cloned().collect(),
            },
        );
    }

    let mut module_reports = BTreeMap::new();
    let mut interned_packages = false;
    for (module, index) in &mut modules {
        let functions = index
            .functions
            .keys()
            .map(|id| Ok((id.clone(), source.functions[id].clone())))
            .collect::<Result<BTreeMap<_, _>>>()?;
        let code = ModuleCode {
            format: CONTENT_PACKAGE_VERSION,
            module: module.clone(),
            functions,
        };
        let (code_bytes, interned) = nir_content::interned::encode(&code)?;
        interned_packages |= interned;
        let code_hash = object(out, objects, &code_bytes, "json", "application/json")?;

        let scenes = source
            .scenes
            .iter()
            .filter(|(id, _)| scene_owners[*id] == *module)
            .map(|(id, value)| (id.clone(), value.clone()))
            .collect();
        let cues: BTreeMap<_, _> = source
            .cues
            .iter()
            .filter(|(id, _)| cue_owners[*id] == *module)
            .map(|(id, value)| (id.clone(), value.clone()))
            .collect();
        let choices = source
            .choices
            .iter()
            .filter(|(id, _)| choice_owners[*id] == *module)
            .map(|(id, value)| (id.clone(), value.clone()))
            .collect();
        let contracts = index
            .texts
            .iter()
            .map(|id| Ok((id.clone(), source.texts[id].clone())))
            .collect::<Result<BTreeMap<_, _>>>()?;
        let mut activation_recipes = BTreeMap::new();
        for cue in cues.keys() {
            let expected: BTreeSet<_> = reference.activation_recipes[cue]
                .iter()
                .filter(|asset| source.assets[asset.as_str()].kind != AssetKind::Font)
                .cloned()
                .collect();
            let actual = source.cue_media_assets(cue);
            if expected != actual {
                bail!("E_RECIPE: eager reference recipe differs for {cue}");
            }
            activation_recipes.insert(cue.clone(), actual);
        }
        let static_package = ModuleStatic {
            format: CONTENT_PACKAGE_VERSION,
            module: module.clone(),
            scenes,
            sprite_timelines: source
                .sprite_timelines
                .iter()
                .filter(|(id, _)| timeline_owners[*id] == *module)
                .map(|(id, value)| (id.clone(), value.clone()))
                .collect(),
            cues,
            choices,
            text_contracts: contracts,
            activation_recipes,
        };
        let (static_bytes, interned) = nir_content::interned::encode(&static_package)?;
        interned_packages |= interned;
        let static_hash = object(out, objects, &static_bytes, "json", "application/json")?;

        let mut text_reports = BTreeMap::new();
        let mut locale_objects = BTreeMap::new();
        for locale in source.locales.keys() {
            if index.texts.is_empty() {
                continue;
            }
            let available = &source.locales[locale];
            let texts = index
                .texts
                .iter()
                .map(|id| {
                    available
                        .get(id)
                        .cloned()
                        .map(|doc| (id.clone(), doc))
                        .ok_or_else(|| {
                            anyhow::anyhow!("E_MODULE_TEXT: {locale} is missing {module}.{id}")
                        })
                })
                .collect::<Result<BTreeMap<_, _>>>()?;
            let bundle = ModuleTexts {
                format: CONTENT_PACKAGE_VERSION,
                module: module.clone(),
                locale: locale.clone(),
                texts,
            };
            let (bytes, interned) = nir_content::interned::encode(&bundle)?;
            interned_packages |= interned;
            let hash = object(out, objects, &bytes, "json", "application/json")?;
            locale_objects.insert(locale.clone(), hash.clone());
            text_reports.insert(
                locale.clone(),
                TextBuildReport {
                    object: hash,
                    bytes: bytes.len() as u64,
                },
            );
        }
        index.code = code_hash.clone();
        index.static_content = static_hash.clone();
        index.locales = locale_objects;
        let largest_text = text_reports
            .values()
            .map(|text| text.bytes)
            .max()
            .unwrap_or(0);
        let demand_bytes = code_bytes.len() as u64 + static_bytes.len() as u64 + largest_text;
        if demand_bytes > nir_format::MAX_INPUT_BYTES as u64 {
            bail!(
                "E_CONTENT_LIMIT: module {module} requires {demand_bytes} encoded bytes; split its source into smaller modules to fit the {} byte content preparation budget",
                nir_format::MAX_INPUT_BYTES
            );
        }
        let owned_catalogs = catalog_reports
            .iter()
            .filter(|(_, report)| report.consumers.contains(&format!("module:{module}")))
            .map(|(id, _)| id.clone())
            .collect();
        module_reports.insert(
            module.clone(),
            ModuleBuildReport {
                code: code_hash,
                code_bytes: code_bytes.len() as u64,
                static_content: static_hash,
                static_bytes: static_bytes.len() as u64,
                asset_catalogs: owned_catalogs,
                locales: text_reports,
            },
        );
    }

    let (module_dependencies, entry_module) =
        dependency_graph(source, &function_owners, &module_ids)?;
    let function_index: BTreeMap<String, RuntimeFunctionIndex> = function_owners
        .iter()
        .map(|(id, module)| {
            (
                id.clone(),
                RuntimeFunctionIndex {
                    execution_module: (author_function_owners[id] != *module)
                        .then(|| author_function_owners[id].clone()),
                    module: module.clone(),
                    signature: modules[module].functions[id].clone(),
                },
            )
        })
        .collect();
    let runtime_text_contracts = text_owners
        .iter()
        .map(|(id, module)| {
            let contract = &source.texts[id];
            (
                id.clone(),
                RuntimeTextIdentity {
                    module: module.clone(),
                    source_revision: contract.source_revision,
                    contract_revision: contract.contract_revision,
                    meaning_revision: contract.meaning_revision,
                    contract_digest: contract.contract_digest.clone(),
                    images: contract.images.clone(),
                },
            )
        })
        .collect();
    let title_nodes = source
        .title_scene
        .as_ref()
        .and_then(|scene| source.scenes.get(scene))
        .or_else(|| source.scenes.values().next())
        .cloned()
        .unwrap_or_default();
    let mut requires = source.requires.clone();
    if interned_packages {
        requires.push(nir_content::interned::CAPABILITY.into());
    }
    if function_index
        .values()
        .any(|index| index.execution_module.is_some())
    {
        requires.push("story.code-packages.v1".into());
    }
    for (module, report) in &module_reports {
        let first = modules[module].functions.keys().next().unwrap();
        if let Some(scope) = &function_index[first].execution_module {
            let shared = &module_reports[scope];
            let bytes = report.code_bytes
                + report.static_bytes
                + shared.static_bytes
                + shared
                    .locales
                    .values()
                    .map(|text| text.bytes)
                    .max()
                    .unwrap_or(0);
            if bytes > nir_format::MAX_INPUT_BYTES as u64 {
                bail!("E_CONTENT_LIMIT: code package {module} and shared declaration scope {scope} require {bytes} bytes");
            }
        }
    }
    let runtime = RuntimeProgram {
        format: RUNTIME_FORMAT_VERSION,
        game_id: source.game_id.clone(),
        revision: source.revision.clone(),
        entry: source.entry.clone(),
        requires,
        stage: source.stage.clone(),
        variables: source.variables.clone(),
        function_index,
        modules,
        scene_owners,
        timeline_owners,
        cue_owners,
        choice_owners,
        text_owners,
        task_owners,
        text_contracts: runtime_text_contracts,
        locales: source.locales.keys().cloned().collect(),
        locale_config: source.locale_config.clone(),
        assets: asset_index,
        catalogs,
        default_locale: source.default_locale.clone(),
        title_scene: source
            .title_scene
            .clone()
            .or_else(|| source.scenes.keys().next().cloned()),
        title_nodes,
        theme: source.theme.clone(),
        player: source.player.clone(),
    };
    let executable = RuntimeExecutable {
        format: RUNTIME_FORMAT_VERSION,
        program: runtime,
    };
    Ok(RuntimeBuild {
        executable,
        modules: module_reports,
        catalogs: catalog_reports,
        module_dependencies,
        entry_module,
        catalog_assets,
    })
}

fn module_paths(
    start: &str,
    graph: &BTreeMap<String, BTreeSet<String>>,
) -> BTreeMap<String, String> {
    let mut paths = BTreeMap::from([(start.to_owned(), start.to_owned())]);
    let mut queue = vec![start.to_owned()];
    let mut cursor = 0;
    while cursor < queue.len() {
        let module = queue[cursor].clone();
        cursor += 1;
        let prefix = paths[&module].clone();
        for target in graph.get(&module).into_iter().flatten() {
            if paths.contains_key(target) {
                continue;
            }
            paths.insert(target.clone(), format!("{prefix} -> {target}"));
            queue.push(target.clone());
        }
    }
    paths
}

fn add_consumer_catalogs(
    closure: &mut ClosureBuilder,
    objects: &BTreeMap<String, Object>,
    runtime: &RuntimeBuild,
    consumer: &str,
) -> Result<()> {
    for (catalog, report) in &runtime.catalogs {
        if !report.consumers.iter().any(|actual| actual == consumer) {
            continue;
        }
        closure.add_object(
            objects,
            &report.object,
            "catalog",
            format!("catalog {catalog} selected by {consumer}"),
        )?;
        for asset_id in runtime.catalog_assets.get(catalog).into_iter().flatten() {
            let asset = runtime
                .executable
                .program
                .assets
                .get(asset_id)
                .ok_or_else(|| anyhow::anyhow!("E_DEPENDENCY_ASSET: {asset_id}"))?;
            let category = match asset.kind {
                AssetKind::Image => "media_image",
                AssetKind::Audio => "media_audio",
                AssetKind::Font => "media_font",
            };
            closure.add_object(
                objects,
                &asset.object,
                category,
                format!("asset {asset_id} selected by {consumer}"),
            )?;
        }
    }
    Ok(())
}

fn add_module_payload(
    closure: &mut ClosureBuilder,
    objects: &BTreeMap<String, Object>,
    runtime: &RuntimeBuild,
    module: &str,
    text_locale: &str,
    ui_locale: &str,
    reason: &str,
) -> Result<()> {
    let package = runtime
        .modules
        .get(module)
        .ok_or_else(|| anyhow::anyhow!("E_DEPENDENCY_MODULE: {module}"))?;
    closure.add_object(
        objects,
        &package.static_content,
        "static",
        format!("module {module} static declarations ({reason})"),
    )?;
    closure.add_object(
        objects,
        &package.code,
        "code",
        format!("module {module} code ({reason})"),
    )?;
    if let Some(text) = package.locales.get(text_locale) {
        closure.add_object(
            objects,
            &text.object,
            "text",
            format!("module {module} text locale {text_locale} ({reason})"),
        )?;
    }
    let root = &runtime.executable.program;
    if let Some(scope) = root
        .module_execution_scope(module)
        .filter(|scope| *scope != module)
    {
        let shared = &runtime.modules[scope];
        closure.add_object(
            objects,
            &shared.static_content,
            "static",
            format!("code package {module} shared declarations in {scope}"),
        )?;
        if let Some(text) = shared.locales.get(text_locale) {
            closure.add_object(
                objects,
                &text.object,
                "text",
                format!("code package {module} shared locale {text_locale} in {scope}"),
            )?;
        }
        add_consumer_catalogs(closure, objects, runtime, &format!("module:{scope}"))?;
    }
    add_consumer_catalogs(closure, objects, runtime, &format!("module:{module}"))?;
    add_consumer_catalogs(closure, objects, runtime, &format!("locale:ui:{ui_locale}"))?;
    add_consumer_catalogs(
        closure,
        objects,
        runtime,
        &format!("locale:text:{text_locale}"),
    )?;
    Ok(())
}

fn add_bootstrap_files(closure: &mut ClosureBuilder, out: &Path, release: &str) -> Result<()> {
    let channel = serde_json::to_vec(&serde_json::json!({"format":1,"release":release}))?;
    for (path, bytes, mime, reason) in [
        (
            "index.html".to_owned(),
            fs::read(out.join("index.html"))?,
            "text/html; charset=utf-8",
            "document bootstrap",
        ),
        (
            "bootstrap.js".to_owned(),
            fs::read(out.join("bootstrap.js"))?,
            "text/javascript",
            "release channel and runtime bootstrap",
        ),
        (
            "channels/stable.json".to_owned(),
            channel,
            "application/json",
            "selected release channel pointer",
        ),
        (
            format!("releases/{release}.json"),
            fs::read(out.join(format!("releases/{release}.json")))?,
            "application/json",
            "immutable release object index",
        ),
        (
            format!("releases/{release}/index.html"),
            fs::read(out.join(format!("releases/{release}/index.html")))?,
            "text/html; charset=utf-8",
            "fixed release document",
        ),
        (
            format!("releases/{release}/bootstrap.js"),
            fs::read(out.join(format!("releases/{release}/bootstrap.js")))?,
            "text/javascript",
            "fixed release bootstrap",
        ),
    ] {
        closure.add_file(&path, &bytes, mime, reason);
    }
    Ok(())
}

fn dependency_report(
    out: &Path,
    release: &str,
    program_hash: &str,
    engine: &EngineFiles,
    objects: &BTreeMap<String, Object>,
    runtime: &RuntimeBuild,
) -> Result<DependencyReport> {
    let mut graph: BTreeMap<String, Vec<String>> = runtime
        .module_dependencies
        .iter()
        .map(|(module, deps)| (module.clone(), deps.iter().cloned().collect()))
        .collect();
    // Include zero-outdegree modules explicitly for a useful, total adjacency map.
    for module in runtime.modules.keys() {
        graph.entry(module.clone()).or_default();
    }

    let ui_locales: Vec<_> = runtime
        .executable
        .program
        .locale_config
        .ui
        .keys()
        .cloned()
        .collect();
    let text_locales: Vec<_> = runtime
        .executable
        .program
        .locale_config
        .text
        .keys()
        .cloned()
        .collect();
    let mut boot = BTreeMap::new();
    let reachable = module_paths(&runtime.entry_module, &runtime.module_dependencies);
    let mut entry_reachable = BTreeMap::new();

    for ui_locale in &ui_locales {
        let mut boot_by_text = BTreeMap::new();
        let mut entries_by_ui = BTreeMap::new();
        for text_locale in &text_locales {
            let mut boot_builder = ClosureBuilder::default();
            boot_builder.add_object(
                objects,
                program_hash,
                "root",
                "runtime root indexes, configuration and minimal title scene".into(),
            )?;
            for (name, hash) in [
                ("engine glue", engine.js.as_str()),
                ("engine wasm", engine.wasm.as_str()),
                ("host adapter", engine.host.as_str()),
            ] {
                boot_builder.add_object(objects, hash, "engine", name.into())?;
            }
            for hash in [&engine.runtime_worker, &engine.asset_worker]
                .into_iter()
                .flatten()
            {
                boot_builder.add_object(objects, hash, "engine", "worker adapter".into())?;
            }
            for consumer in [
                "bootstrap".to_owned(),
                format!("locale:ui:{ui_locale}"),
                format!("locale:text:{text_locale}"),
            ] {
                add_consumer_catalogs(&mut boot_builder, objects, runtime, &consumer)?;
            }
            add_bootstrap_files(&mut boot_builder, out, release)?;

            let mut entry_builder = boot_builder.clone();
            for (module, path) in &reachable {
                add_module_payload(
                    &mut entry_builder,
                    objects,
                    runtime,
                    module,
                    text_locale,
                    ui_locale,
                    &format!("entry call closure {path}"),
                )?;
            }
            boot_by_text.insert(text_locale.clone(), boot_builder.finish());
            entries_by_ui.insert(text_locale.clone(), entry_builder.finish());
        }
        boot.insert(ui_locale.clone(), boot_by_text);
        entry_reachable.insert(ui_locale.clone(), entries_by_ui);
    }
    let mut modules = BTreeMap::new();
    for module in runtime.modules.keys() {
        let mut by_ui = BTreeMap::new();
        for ui_locale in &ui_locales {
            let mut by_text = BTreeMap::new();
            for text_locale in &text_locales {
                let mut builder = ClosureBuilder::default();
                add_module_payload(
                    &mut builder,
                    objects,
                    runtime,
                    module,
                    text_locale,
                    ui_locale,
                    "single-module closure",
                )?;
                by_text.insert(text_locale.clone(), builder.finish());
            }
            by_ui.insert(ui_locale.clone(), by_text);
        }
        modules.insert(module.clone(), by_ui);
    }

    Ok(DependencyReport {
        format: 1,
        game_id: runtime.executable.program.game_id.clone(),
        entry: runtime.executable.program.entry.clone(),
        module_dependencies: graph,
        boot,
        modules,
        entry_reachable,
        scope: "Source-byte counts; object hashes are deduplicated. Bootstrap files are counted by path. HTTP headers, TLS, compression and retry traffic are excluded. Entry closure conservatively includes every statically reachable Call edge and every asset/font referenced by its included packages.".into(),
    })
}

fn validate_runtime_packages(out: &Path, executable: &RuntimeExecutable) -> Result<()> {
    if executable.format != RUNTIME_FORMAT_VERSION
        || executable.program.format != RUNTIME_FORMAT_VERSION
    {
        bail!("E_RUNTIME_VERSION: compiler emitted an unsupported runtime root");
    }
    let root = &executable.program;
    let mut keys = Vec::new();
    for (module, index) in &root.modules {
        keys.push(ContentKey::Static {
            module: module.clone(),
        });
        keys.push(ContentKey::Code {
            module: module.clone(),
        });
        keys.extend(index.locales.keys().map(|locale| ContentKey::Text {
            module: module.clone(),
            locale: locale.clone(),
        }));
    }
    keys.extend(root.catalogs.keys().map(|catalog| ContentKey::Catalog {
        catalog: catalog.clone(),
    }));
    for key in keys {
        let requirement = root
            .content_requirement(&key)
            .ok_or_else(|| anyhow::anyhow!("E_RUNTIME_INDEX: {key:?}"))?;
        let bytes = fs::read(out.join(format!("objects/{}.json", requirement.digest)))?;
        nir_content::parse_runtime_object(root, &key, &bytes).map_err(anyhow::Error::new)?;
    }
    Ok(())
}

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
        resolve(root, sdk)?
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
    for (name, hash) in [("index.html", &html), ("bootstrap.js", &bootstrap)] {
        let source = out.join(&objects[hash].path);
        let target = fixed.join(name);
        if target.exists() {
            nir_content::verify(&fs::read(&target)?, hash)?;
        } else {
            fs::copy(source, target)?;
        }
    }
    for name in ["index.html", "bootstrap.js"] {
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
/// Platform parameters shared by the Windows and Linux native build paths.
struct NativeTarget {
    /// SDK-relative player path, e.g. `windows/player-windows.exe`.
    player: &'static str,
    /// Error code prefix, host name and executable format label.
    code: &'static str,
    host: &'static str,
    format: &'static str,
    /// Executable magic checked before packaging (PE `MZ`, ELF `\x7fELF`).
    magic: &'static [u8],
    /// Intermediate web build directory under the project's `.nir/`.
    intermediate: &'static str,
    /// Packaged binary name (`Game.exe` on Windows, `Game` on Linux).
    binary: &'static str,
    readme: String,
    /// Report file name under the project's `reports/`.
    report: &'static str,
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
fn collect_files(
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
        nir_apk::key::SigningIdentity::from_seed_phrase(&seed)
            .context("E_SIGNING_KEY: NIR_ANDROID_SIGNING_SEED is not usable")?
    } else if key_path.exists() {
        nir_apk::key::SigningIdentity::load(&key_path)
            .context("E_SIGNING_KEY: config/android-signing.pem is not a valid signing identity")?
    } else {
        let key = nir_apk::key::SigningIdentity::generate()?;
        fs::create_dir_all(key_path.parent().unwrap())?;
        // Unique temporary name plus rename, so a concurrent or interrupted
        // first save cannot leave a torn identity behind; re-loading after
        // the rename signs with whatever actually won on disk.
        let temporary = key_path.with_extension(format!("pem.{}.next", std::process::id()));
        key.save(&temporary)
            .context("E_SIGNING_KEY: could not write config/android-signing.pem")?;
        fs::rename(&temporary, &key_path)
            .context("E_SIGNING_KEY: could not write config/android-signing.pem")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600))
                .context("E_SIGNING_KEY: could not restrict config/android-signing.pem")?;
        }
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
    let manifest = nir_apk::ManifestSpec {
        package: format!(
            "one.nir.g{}",
            &nir_content::digest(bundle.manifest.game_id.as_bytes())[..15]
        ),
        version_code: 1,
        version_name: bundle.manifest.version.clone(),
        label: bundle.manifest.title.clone(),
        min_sdk: 26,
        target_sdk: 29,
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
struct NativeBundle {
    /// The platform's intermediate web-build directory under `.nir/`.
    intermediate: PathBuf,
    manifest: NativeRelease,
    report: BuildReport,
    player: Vec<u8>,
    /// Serialized `NativeRelease`; its digest becomes `report.release`.
    release_bytes: Vec<u8>,
}
fn native_release(
    root: &Path,
    sdk: &Path,
    profile: &str,
    locked: bool,
    target: &NativeTarget,
    optimize: &OptimizeOptions,
) -> Result<NativeBundle> {
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
    let intermediate = root.join(".nir").join(target.intermediate);
    let mut report = build_profile(root, sdk, &intermediate, profile, locked, true, optimize)?;
    let web: ReleaseManifest = nir_content::parse(
        &fs::read(intermediate.join(format!("releases/{}.json", report.release)))?,
        "release",
    )?;
    let mut objects = web.objects;
    for id in [
        &web.engine.js,
        &web.engine.host,
        &web.engine.wasm,
        &web.launch.html,
        &web.launch.bootstrap,
    ] {
        objects.remove(id);
    }
    for id in [&web.engine.runtime_worker, &web.engine.asset_worker]
        .into_iter()
        .flatten()
    {
        objects.remove(id);
    }
    let manifest = NativeRelease {
        format: 1,
        game_id: web.game_id,
        title: web.title,
        version: web.version,
        profile: web.profile,
        engine_build: web.engine_build,
        player: nir_content::digest(&player),
        program: web.program,
        objects,
    };
    let release_bytes = serde_json::to_vec(&manifest)?;
    report.release = nir_content::digest(&release_bytes);
    report.objects = manifest.objects.len();
    report.total_bytes =
        manifest.objects.values().map(|o| o.bytes).sum::<u64>() + player.len() as u64;
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
fn write_native_data(bundle: &NativeBundle, data: &Path) -> Result<()> {
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
fn build_native(
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

#[cfg(test)]
mod compression_tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn large_asset_catalogs_do_not_pin_the_whole_story_for_one_image() {
        let mut source: Program =
            serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        let mut nodes = vec![];
        for ordinal in 0..130 {
            let id = format!("catalog_image_{ordinal:03}");
            source
                .assets
                .insert(id.clone(), source.assets["bg.station"].clone());
            let mut node = source.scenes["station"][0].clone();
            node.id = id.clone();
            node.asset = Some(id);
            nodes.push(node);
        }
        source.scenes.insert("catalog_images".into(), nodes);
        for (id, asset) in &mut source.assets {
            asset.object = nir_content::digest(id.as_bytes());
            if asset.kind == AssetKind::Image {
                asset.decoded_bytes = asset.width as u64 * asset.height as u64 * 4;
            }
        }
        let media = source
            .assets
            .iter()
            .map(|(id, a)| (id.clone(), a.object.clone()))
            .collect();
        for plan in source
            .locale_config
            .ui
            .values_mut()
            .chain(source.locale_config.text.values_mut())
        {
            plan.digest = LocaleFontPlan::digest_for(&plan.fonts, &media);
        }
        let reference = crate::compile(&source).unwrap();
        let out = tempfile::tempdir().unwrap();
        fs::create_dir(out.path().join("objects")).unwrap();
        let mut objects = BTreeMap::new();
        let runtime = package_runtime(out.path(), &mut objects, &source, &reference).unwrap();
        let root = &runtime.executable.program;
        let chunks: BTreeSet<_> = (0..130)
            .map(|ordinal| {
                root.assets[&format!("catalog_image_{ordinal:03}")]
                    .catalog
                    .clone()
            })
            .collect();
        assert_eq!(chunks.len(), 3);
        let keys = root.asset_catalogs(["catalog_image_000"]);
        assert_eq!(keys.len(), 1);
        let key = keys.into_iter().next().unwrap();
        let digest = root.content_requirement(&key).unwrap().digest;
        let bytes = fs::read(out.path().join(format!("objects/{digest}.json"))).unwrap();
        let object = nir_content::parse_runtime_object(root, &key, &bytes).unwrap();
        let RuntimeObject::Catalog(catalog) = &object else {
            panic!("expected asset catalog")
        };
        assert!(catalog.assets.contains_key("catalog_image_000"));
        assert!(catalog.assets.len() <= 64);
        let view = nir_core::ValidatedProgram::from_runtime(root.clone())
            .unwrap()
            .install_batch(vec![(key, object, bytes.len() as u64)])
            .unwrap();
        assert_eq!(view.residency().resident_blocks, 1);
        for report in runtime.catalogs.values() {
            let bytes =
                fs::read(out.path().join(format!("objects/{}.json", report.object))).unwrap();
            let catalog: AssetCatalog = serde_json::from_slice(&bytes).unwrap();
            assert!(catalog.assets.len() <= 64);
            for id in catalog.assets.keys() {
                assert_eq!(root.assets[id].catalog, catalog.catalog);
            }
        }
    }

    #[test]
    fn large_code_packages_keep_shared_tasks_and_validate_each_runtime_module() {
        let mut source: Program =
            serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        for (id, asset) in &mut source.assets {
            asset.object = nir_content::digest(id.as_bytes());
        }
        let media = source
            .assets
            .iter()
            .map(|(id, asset)| (id.clone(), asset.object.clone()))
            .collect();
        for plan in source
            .locale_config
            .ui
            .values_mut()
            .chain(source.locale_config.text.values_mut())
        {
            plan.digest = LocaleFontPlan::digest_for(&plan.fonts, &media);
        }
        source
            .variables
            .insert("payload".into(), nir_format::Value::String(String::new()));
        for ordinal in 0..32 {
            let ops: Vec<_> = (0..100).map(|at| serde_json::json!({
                "id": format!("f{ordinal}_op{at}"),
                "operation":{"type":"assign","target":"payload","value":{"type":"const","value":{"type":"string","value":"x".repeat(1024)}}}
            })).collect();
            let mut function = source.functions[&source.entry].clone();
            for block in function.blocks.values_mut() {
                for op in &mut block.ops {
                    op.id = format!("f{ordinal}_{}", op.id);
                }
            }
            function
                .blocks
                .get_mut(&function.entry.clone())
                .unwrap()
                .ops
                .extend(
                    ops.into_iter()
                        .map(|op| serde_json::from_value(op).unwrap()),
                );
            source
                .functions
                .insert(format!("large_{ordinal}"), function);
        }
        let reference = crate::compile(&source).unwrap();
        let out = tempfile::tempdir().unwrap();
        fs::create_dir(out.path().join("objects")).unwrap();
        let mut objects = BTreeMap::new();
        for id in source.assets.keys() {
            object(
                out.path(),
                &mut objects,
                id.as_bytes(),
                "bin",
                "application/octet-stream",
            )
            .unwrap();
        }
        let runtime = package_runtime(out.path(), &mut objects, &source, &reference).unwrap();
        let root = &runtime.executable.program;
        assert!(root.modules.len() > 2);
        assert_eq!(root.function_index.len(), source.functions.len());
        assert_eq!(root.function_index[&source.entry].module, "_legacy");
        assert!(root.task_owners.values().all(|owner| owner == "_legacy"));
        assert!(root.text_owners.values().all(|owner| owner == "_legacy"));
        for (module, index) in &root.modules {
            if module != "_legacy" {
                assert!(index.texts.is_empty());
                assert!(runtime.modules[module].code_bytes < 2 * 1024 * 1024);
            }
        }
        let child = root
            .modules
            .keys()
            .find(|module| *module != "_legacy")
            .unwrap();
        let mut closure = ClosureBuilder::default();
        add_module_payload(
            &mut closure,
            &objects,
            &runtime,
            child,
            "en",
            "en",
            "cold code-package entry",
        )
        .unwrap();
        let closure = closure.finish();
        assert!(closure
            .objects
            .contains_key(&runtime.modules["_legacy"].static_content));
        assert!(closure
            .objects
            .contains_key(&runtime.modules["_legacy"].locales["en"].object));
        assert!(!closure
            .objects
            .contains_key(&runtime.modules["_legacy"].code));
        validate_runtime_packages(out.path(), &runtime.executable).unwrap();
        let root_view = nir_core::ValidatedProgram::from_runtime(root.clone()).unwrap();
        for (module, index) in &root.modules {
            let mut keys = std::collections::BTreeSet::from([ContentKey::Code {
                module: module.clone(),
            }]);
            keys.extend(root.content_prerequisites(&ContentKey::Code {
                module: module.clone(),
            }));
            let batch = keys
                .into_iter()
                .map(|key| {
                    let digest = root.content_requirement(&key).unwrap().digest;
                    let bytes =
                        fs::read(out.path().join(format!("objects/{digest}.json"))).unwrap();
                    let object = nir_content::parse_runtime_object(root, &key, &bytes).unwrap();
                    (key, object, bytes.len() as u64)
                })
                .collect();
            let view = root_view.install_batch(batch).unwrap();
            assert!(index
                .functions
                .keys()
                .all(|id| view.program().functions.contains_key(id)));
        }
    }

    #[test]
    fn identical_scene_templates_keep_activations_and_module_ownership() {
        let mut source: Program =
            serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        let nodes = source.scenes.values().next().unwrap().clone();
        source.scenes = BTreeMap::from([
            ("a.first".into(), nodes.clone()),
            ("a.repeat".into(), nodes.clone()),
            ("b.same".into(), nodes.clone()),
        ]);
        let mut changed = nodes;
        changed[0].x += 1.;
        source.scenes.insert("a.changed".into(), changed);
        source.title_scene = Some("a.repeat".into());
        let present = |scene: &str, duration| EffectDef {
            id: "stage".into(),
            scope: Scope::Session,
            effect: Effect::StagePresent {
                dialogue_visible: None,
                inherit_images: Vec::new(),
                inherit_image_geometry: Vec::new(),
                scene: scene.into(),
                transition: Default::default(),
                duration_us: Micros(duration),
            },
        };
        source.cues = BTreeMap::from([(
            "a.show".into(),
            Cue {
                effects: vec![EffectDef {
                    id: "sequence".into(),
                    scope: Scope::Session,
                    effect: Effect::Sequence {
                        children: vec![present("a.first", 100), present("a.repeat", 200)],
                    },
                }],
            },
        )]);
        let optimized =
            coalesce_scene_templates(&source, &BTreeSet::from(["a".into(), "b".into()])).unwrap();
        assert_eq!(optimized.scenes.len(), 3);
        assert!(optimized.scenes.contains_key("a.changed"));
        assert!(optimized.scenes.contains_key("b.same"));
        assert_eq!(optimized.title_scene.as_deref(), Some("a.first"));
        let children = optimized.cues["a.show"].effects[0]
            .effect
            .compose_children()
            .unwrap();
        assert_eq!(children.len(), 2);
        for (child, duration) in children.iter().zip([100, 200]) {
            assert!(
                matches!(&child.effect, Effect::StagePresent { scene, duration_us, .. } if scene == "a.first" && duration_us.0 == duration)
            );
        }
        assert!(
            source.scenes.contains_key("a.repeat"),
            "author declarations stay unchanged"
        );
    }

    #[test]
    fn inline_only_images_belong_to_the_text_module_catalog() {
        let mut program: Program =
            serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        program.texts.get_mut("intro").unwrap().images = vec![nir_format::TextImageContract {
            id: "inline".into(),
            asset: "inline-only".into(),
        }];
        let consumers = asset_consumers(
            &program,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::from([("intro".into(), "chapter".into())]),
            &BTreeMap::new(),
        );
        assert_eq!(
            consumers["inline-only"],
            BTreeSet::from(["module:chapter".into()])
        );
        assert!(!consumers["inline-only"].contains("bootstrap"));
    }

    #[test]
    fn gzip_is_reproducible_and_preserves_object_identity() {
        let out = tempfile::tempdir().unwrap();
        fs::create_dir(out.path().join("objects")).unwrap();
        let mut objects = BTreeMap::new();
        let bytes = b"compressible release content ".repeat(100);
        let hash = object(out.path(), &mut objects, &bytes, "json", "application/json").unwrap();
        let descriptor = &objects[&hash];
        assert_eq!(descriptor.bytes, bytes.len() as u64);
        let sidecar = out.path().join(format!("{}.gz", descriptor.path));
        let first = fs::read(&sidecar).unwrap();
        assert!(first.len() < bytes.len());
        assert_eq!(&first[4..8], &[0; 4]);
        let mut decoded = Vec::new();
        flate2::read::GzDecoder::new(first.as_slice())
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, bytes);
        object(out.path(), &mut objects, &bytes, "json", "application/json").unwrap();
        assert_eq!(fs::read(sidecar).unwrap(), first);
        let tiny = object(out.path(), &mut objects, b"{}", "json", "application/json").unwrap();
        assert!(!out
            .path()
            .join(format!("{}.gz", objects[&tiny].path))
            .exists());
        let media = object(out.path(), &mut objects, &bytes, "png", "image/png").unwrap();
        assert!(!out
            .path()
            .join(format!("{}.gz", objects[&media].path))
            .exists());
    }
}
