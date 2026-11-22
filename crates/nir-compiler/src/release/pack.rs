use super::*;

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
pub(super) struct DependencyReport {
    pub(super) format: u32,
    pub(super) game_id: String,
    pub(super) entry: String,
    pub(super) module_dependencies: BTreeMap<String, Vec<String>>,
    pub(super) boot: BTreeMap<String, BTreeMap<String, DependencyClosure>>,
    pub(super) modules: BTreeMap<String, BTreeMap<String, BTreeMap<String, DependencyClosure>>>,
    pub(super) entry_reachable: BTreeMap<String, BTreeMap<String, DependencyClosure>>,
    pub(super) scope: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(super) struct DependencyClosure {
    pub(super) object_count: usize,
    pub(super) object_bytes: u64,
    pub(super) file_count: usize,
    pub(super) file_bytes: u64,
    pub(super) total_bytes: u64,
    pub(super) category_bytes: BTreeMap<String, u64>,
    pub(super) objects: BTreeMap<String, DependencyObject>,
    pub(super) files: BTreeMap<String, DependencyFile>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct DependencyObject {
    pub(super) bytes: u64,
    pub(super) media_type: String,
    pub(super) categories: Vec<String>,
    pub(super) reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct DependencyFile {
    pub(super) sha256: String,
    pub(super) bytes: u64,
    pub(super) media_type: String,
    pub(super) reason: String,
}

#[derive(Clone, Default)]
pub(super) struct ClosureBuilder {
    pub(super) objects: BTreeMap<String, (u64, String, BTreeSet<String>, BTreeSet<String>)>,
    pub(super) files: BTreeMap<String, DependencyFile>,
}
impl ClosureBuilder {
    pub(super) fn add_object(
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

    pub(super) fn add_file(&mut self, path: &str, bytes: &[u8], media_type: &str, reason: &str) {
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

    pub(super) fn finish(self) -> DependencyClosure {
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

pub(super) struct RuntimeBuild {
    pub(super) executable: RuntimeExecutable,
    pub(super) modules: BTreeMap<String, ModuleBuildReport>,
    pub(super) catalogs: BTreeMap<String, AssetCatalogBuildReport>,
    pub(super) module_dependencies: BTreeMap<String, BTreeSet<String>>,
    pub(super) entry_module: String,
    pub(super) catalog_assets: BTreeMap<String, BTreeSet<String>>,
}

pub(super) fn module_indexes(program: &Program) -> Result<BTreeMap<String, ModuleIndex>> {
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

pub(super) fn declaration_owner(id: &str, modules: &BTreeSet<String>) -> Result<String> {
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

pub(super) fn asset_consumers(
    program: &Program,
    function_owners: &BTreeMap<String, String>,
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
                let mut assets = BTreeSet::new();
                if let Effect::StagePresent { transition, .. } = &effect.effect {
                    assets.extend(transition.asset().map(str::to_owned));
                }
                effect.effect.collect_audio_assets(&mut assets);
                effect
                    .effect
                    .collect_text_image_assets(&program.texts, &mut assets);
                effect
                    .effect
                    .collect_dialogue_style_assets(&program.theme, &mut assets);
                for asset in assets {
                    consumers
                        .entry(asset)
                        .or_default()
                        .insert(format!("module:{module}"));
                }
            }
        }
    }
    // Mid-block reveals have no cue recipe. Their catalogs follow the authored
    // function scope even when its code is split into a generated package.
    for (function, definition) in &program.functions {
        if let Some(module) = function_owners.get(function) {
            for op in definition.blocks.values().flat_map(|block| &block.ops) {
                if let Operation::DialogueVisibility {
                    transition: Some(transition),
                    ..
                } = &op.operation
                {
                    if let Some(asset) = transition.asset() {
                        consumers
                            .entry(asset.to_owned())
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

pub(super) fn dependency_graph(
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
pub(super) fn coalesce_scene_templates(
    source: &Program,
    modules: &BTreeSet<String>,
) -> Result<Program> {
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
pub(super) fn partition_large_code(
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

pub(super) fn package_runtime(
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
        &author_function_owners,
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

pub(super) fn module_paths(
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

pub(super) fn add_consumer_catalogs(
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

pub(super) fn add_module_payload(
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

pub(super) fn add_bootstrap_files(
    closure: &mut ClosureBuilder,
    out: &Path,
    release: &str,
) -> Result<()> {
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
            "style.css".to_owned(),
            fs::read(out.join("style.css"))?,
            "text/css; charset=utf-8",
            "release shell and overlay stylesheet",
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
        (
            format!("releases/{release}/style.css"),
            fs::read(out.join(format!("releases/{release}/style.css")))?,
            "text/css; charset=utf-8",
            "fixed release stylesheet",
        ),
    ] {
        closure.add_file(&path, &bytes, mime, reason);
    }
    Ok(())
}

pub(super) fn dependency_report(
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

pub(super) fn validate_runtime_packages(out: &Path, executable: &RuntimeExecutable) -> Result<()> {
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
