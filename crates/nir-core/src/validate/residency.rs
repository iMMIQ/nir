use super::*;

#[derive(Debug, Clone)]
pub(crate) enum Instruction {
    Op(Op),
    Term(Terminator),
}

/// An indexed map whose declared keys can be known before their independent
/// content blocks are resident. Values themselves are shared per body.
#[derive(Debug, Clone)]
pub struct ContentTable<T> {
    pub(super) declared: Arc<BTreeSet<String>>,
    pub(super) values: Arc<BTreeMap<String, Arc<T>>>,
}
impl<T> Default for ContentTable<T> {
    fn default() -> Self {
        Self {
            declared: Arc::new(BTreeSet::new()),
            values: Arc::new(BTreeMap::new()),
        }
    }
}
impl<T> ContentTable<T> {
    pub(super) fn from_owned(values: BTreeMap<String, T>) -> Self {
        let declared = values.keys().cloned().collect();
        Self {
            declared: Arc::new(declared),
            values: Arc::new(values.into_iter().map(|(k, v)| (k, Arc::new(v))).collect()),
        }
    }
    pub(super) fn from_index(keys: impl IntoIterator<Item = String>) -> Self {
        Self {
            declared: Arc::new(keys.into_iter().collect()),
            values: Arc::new(BTreeMap::new()),
        }
    }
    pub(super) fn with_values(&self, values: impl IntoIterator<Item = (String, T)>) -> Self {
        let incoming: Vec<_> = values.into_iter().collect();
        let all_declared = incoming.iter().all(|(key, _)| self.declared.contains(key));
        let declared = if all_declared {
            self.declared.clone()
        } else {
            let mut next = (*self.declared).clone();
            for (key, _) in &incoming {
                next.insert(key.clone());
            }
            Arc::new(next)
        };
        let mut entries = (*self.values).clone();
        for (key, value) in incoming {
            entries.insert(key, Arc::new(value));
        }
        Self {
            declared,
            values: Arc::new(entries),
        }
    }
    pub(super) fn without_values(&self, removed: &BTreeSet<String>) -> Self {
        Self {
            declared: self.declared.clone(),
            values: Arc::new(
                self.values
                    .iter()
                    .filter(|(key, _)| !removed.contains(*key))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
        }
    }
    /// Share an immutable body while mutating a separate runtime state.
    pub fn shared<Q>(&self, key: &Q) -> Option<Arc<T>>
    where
        String: Borrow<Q>,
        Q: ?Sized + Ord,
    {
        self.values.get(key).cloned()
    }
    pub fn get<Q>(&self, key: &Q) -> Option<&T>
    where
        String: Borrow<Q>,
        Q: ?Sized + Ord,
    {
        self.values.get(key).map(AsRef::as_ref)
    }
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        String: Borrow<Q>,
        Q: ?Sized + Ord,
    {
        self.declared.contains(key)
    }
    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.declared.iter()
    }
    pub fn values(&self) -> impl Iterator<Item = &T> {
        self.values.values().map(AsRef::as_ref)
    }
    pub fn iter(&self) -> impl Iterator<Item = (&String, &T)> {
        self.values.iter().map(|(key, value)| (key, value.as_ref()))
    }
    pub fn len(&self) -> usize {
        self.declared.len()
    }
    pub fn is_empty(&self) -> bool {
        self.declared.is_empty()
    }
}
impl<T, Q> Index<&Q> for ContentTable<T>
where
    String: Borrow<Q>,
    Q: ?Sized + Ord,
{
    type Output = T;
    fn index(&self, key: &Q) -> &Self::Output {
        self.get(key)
            .expect("indexed runtime content is not resident")
    }
}

/// Field-compatible view used by the VM. Root config and indexes are shared;
/// content values live in independently shared tables.
#[derive(Debug, Clone)]
pub struct RuntimeProgramView {
    pub format: u32,
    pub game_id: String,
    pub revision: String,
    pub entry: String,
    pub requires: Vec<String>,
    pub capabilities: CapabilitySet,
    pub stage: Stage,
    pub variables: Arc<BTreeMap<String, Value>>,
    pub functions: ContentTable<Function>,
    pub modules: Arc<BTreeMap<String, ModuleIndex>>,
    pub scenes: ContentTable<Vec<Node>>,
    pub sprite_timelines: ContentTable<SpriteTimeline>,
    pub cues: ContentTable<Cue>,
    pub choices: ContentTable<Choice>,
    pub texts: ContentTable<TextContract>,
    pub locales: Arc<BTreeMap<String, ContentTable<TextDoc>>>,
    pub locale_config: Arc<LocaleConfig>,
    pub assets: ContentTable<Asset>,
    pub default_locale: String,
    pub title_scene: Option<String>,
    pub theme: Arc<Theme>,
    pub player: Arc<PlayerDefaults>,
    pub(super) title_nodes: Arc<Vec<Node>>,
    pub(super) runtime_root: Option<Arc<RuntimeProgram>>,
    pub(super) function_index: Arc<BTreeMap<String, RuntimeFunctionIndex>>,
    pub(super) text_owners: Arc<BTreeMap<String, String>>,
    pub(super) text_identities: Arc<BTreeMap<String, RuntimeTextIdentity>>,
    pub(super) asset_index: Arc<BTreeMap<String, AssetIndexEntry>>,
    pub(super) cue_recipes: Arc<BTreeMap<String, Arc<BTreeSet<String>>>>,
    pub(super) task_definitions: Arc<BTreeMap<String, Vec<Arc<Effect>>>>,
    pub(super) op_owners: Arc<BTreeMap<String, String>>,
}
impl RuntimeProgramView {
    pub fn runtime_root(&self) -> Option<&RuntimeProgram> {
        self.runtime_root.as_deref()
    }
    pub fn title_nodes(&self) -> &[Node] {
        &self.title_nodes
    }
    pub fn asset(&self, id: &str) -> Option<&Asset> {
        self.assets.get(id)
    }
    pub fn asset_kind(&self, id: &str) -> Option<AssetKind> {
        self.asset_index.get(id).map(|asset| asset.kind)
    }
    pub fn text_identity(&self, id: &str) -> Option<&RuntimeTextIdentity> {
        self.text_identities.get(id)
    }
    pub fn function_signature(&self, id: &str) -> Option<FunctionSignature> {
        self.functions
            .get(id)
            .map(FunctionSignature::from)
            .or_else(|| self.function_index.get(id).map(|i| i.signature.clone()))
            .or_else(|| {
                self.modules
                    .values()
                    .find_map(|m| m.functions.get(id).cloned())
            })
    }
    pub fn function_module(&self, id: &str) -> Option<&str> {
        self.function_index
            .get(id)
            .filter(|i| i.module != "@root")
            .map(|i| i.module.as_str())
            .or_else(|| {
                self.modules
                    .iter()
                    .find(|(_, module)| module.functions.contains_key(id))
                    .map(|(id, _)| id.as_str())
            })
    }
    pub fn function_execution_module(&self, id: &str) -> Option<&str> {
        self.runtime_root()
            .and_then(|root| root.function_execution_module(id))
            .or_else(|| self.function_module(id))
    }
    pub fn text_module(&self, id: &str) -> Option<&str> {
        self.text_owners.get(id).map(String::as_str).or_else(|| {
            self.modules
                .iter()
                .find(|(_, module)| module.texts.contains(id))
                .map(|(id, _)| id.as_str())
        })
    }
    pub fn cue_assets(&self, cue: &str) -> BTreeSet<String> {
        if let Some(assets) = self.cue_recipes.get(cue) {
            return assets.as_ref().clone();
        }
        self.cues
            .get(cue)
            .map(|definition| {
                definition.media_assets(
                    &self.theme,
                    |id| self.scenes.get(id).map(Vec::as_slice),
                    |id| self.texts.get(id),
                )
            })
            .unwrap_or_default()
    }
    /// Materialize the legacy source representation on demand. Runtime roots
    /// cannot be converted because unloaded bodies are deliberately absent.
    pub fn to_source_program(&self) -> Option<Program> {
        if self.runtime_root.is_some() {
            return None;
        }
        Some(Program {
            format: self.format,
            game_id: self.game_id.clone(),
            revision: self.revision.clone(),
            entry: self.entry.clone(),
            requires: self.requires.clone(),
            stage: self.stage.clone(),
            variables: (*self.variables).clone(),
            functions: self
                .functions
                .iter()
                .map(|(id, value)| (id.clone(), value.clone()))
                .collect(),
            modules: (*self.modules).clone(),
            sprite_timelines: self
                .sprite_timelines
                .iter()
                .map(|(id, value)| (id.clone(), value.clone()))
                .collect(),
            scenes: self
                .scenes
                .iter()
                .map(|(id, value)| (id.clone(), value.clone()))
                .collect(),
            cues: self
                .cues
                .iter()
                .map(|(id, value)| (id.clone(), value.clone()))
                .collect(),
            choices: self
                .choices
                .iter()
                .map(|(id, value)| (id.clone(), value.clone()))
                .collect(),
            texts: self
                .texts
                .iter()
                .map(|(id, value)| (id.clone(), value.clone()))
                .collect(),
            locales: self
                .locales
                .iter()
                .map(|(locale, docs)| {
                    (
                        locale.clone(),
                        docs.iter()
                            .map(|(id, value)| (id.clone(), value.clone()))
                            .collect(),
                    )
                })
                .collect(),
            locale_config: (*self.locale_config).clone(),
            assets: self
                .assets
                .iter()
                .map(|(id, value)| (id.clone(), value.clone()))
                .collect(),
            default_locale: self.default_locale.clone(),
            title_scene: self.title_scene.clone(),
            theme: (*self.theme).clone(),
            player: (*self.player).clone(),
        })
    }
    pub(super) fn from_source(p: Program) -> Self {
        let function_index = p
            .functions
            .iter()
            .map(|(id, function)| {
                (
                    id.clone(),
                    RuntimeFunctionIndex {
                        execution_module: None,
                        module: p.function_module(id).unwrap_or("@root").to_string(),
                        signature: FunctionSignature::from(function),
                    },
                )
            })
            .collect();
        let text_owners: BTreeMap<String, String> = p
            .modules
            .iter()
            .flat_map(|(module, index)| {
                index
                    .texts
                    .iter()
                    .map(move |id| (id.clone(), module.clone()))
            })
            .collect();
        let text_identities = p
            .texts
            .iter()
            .map(|(id, contract)| {
                (
                    id.clone(),
                    RuntimeTextIdentity {
                        module: text_owners.get(id).cloned().unwrap_or_default(),
                        source_revision: contract.source_revision,
                        contract_revision: contract.contract_revision,
                        meaning_revision: contract.meaning_revision,
                        contract_digest: contract.contract_digest.clone(),
                        images: contract.images.clone(),
                    },
                )
            })
            .collect();
        let asset_index = p
            .assets
            .iter()
            .map(|(id, asset)| {
                (
                    id.clone(),
                    AssetIndexEntry {
                        kind: asset.kind,
                        object: asset.object.clone(),
                        catalog: String::new(),
                    },
                )
            })
            .collect();
        let cue_recipes = p
            .cues
            .keys()
            .map(|cue| (cue.clone(), Arc::new(p.cue_media_assets(cue))))
            .collect();
        let title_nodes = p
            .title_scene
            .as_ref()
            .and_then(|title| p.scenes.get(title))
            .cloned()
            .or_else(|| p.scenes.values().next().cloned())
            .unwrap_or_default();
        let mut task_definitions: BTreeMap<String, Vec<Arc<Effect>>> = BTreeMap::new();
        for effect in p.cues.values().flat_map(|cue| cue.effects.iter()) {
            task_definitions
                .entry(effect.id.clone())
                .or_default()
                .push(Arc::new(effect.effect.clone()));
        }
        let op_owners = p
            .functions
            .iter()
            .flat_map(|(function_id, function)| {
                function.blocks.values().flat_map(move |block| {
                    block
                        .ops
                        .iter()
                        .map(move |op| (op.id.clone(), function_id.clone()))
                })
            })
            .collect();
        Self {
            format: p.format,
            game_id: p.game_id,
            revision: p.revision,
            entry: p.entry,
            capabilities: CapabilitySet::from_names_known(p.requires.iter().map(String::as_str)),
            requires: p.requires,
            stage: p.stage,
            variables: Arc::new(p.variables),
            functions: ContentTable::from_owned(p.functions),
            modules: Arc::new(p.modules),
            scenes: ContentTable::from_owned(p.scenes),
            sprite_timelines: ContentTable::from_owned(p.sprite_timelines),
            cues: ContentTable::from_owned(p.cues),
            choices: ContentTable::from_owned(p.choices),
            texts: ContentTable::from_owned(p.texts),
            locales: Arc::new(
                p.locales
                    .into_iter()
                    .map(|(locale, docs)| (locale, ContentTable::from_owned(docs)))
                    .collect(),
            ),
            locale_config: Arc::new(p.locale_config),
            assets: ContentTable::from_owned(p.assets),
            default_locale: p.default_locale,
            title_scene: p.title_scene,
            theme: Arc::new(p.theme),
            player: Arc::new(p.player),
            title_nodes: Arc::new(title_nodes),
            runtime_root: None,
            function_index: Arc::new(function_index),
            text_owners: Arc::new(text_owners),
            text_identities: Arc::new(text_identities),
            asset_index: Arc::new(asset_index),
            cue_recipes: Arc::new(cue_recipes),
            task_definitions: Arc::new(task_definitions),
            op_owners: Arc::new(op_owners),
        }
    }
    pub(super) fn from_runtime(root: RuntimeProgram) -> Result<Self> {
        let functions = ContentTable::from_index(root.function_index.keys().cloned());
        let scenes = ContentTable::from_index(root.scene_owners.keys().cloned());
        let sprite_timelines = ContentTable::from_index(root.timeline_owners.keys().cloned());
        let cues = ContentTable::from_index(root.cue_owners.keys().cloned());
        let choices = ContentTable::from_index(root.choice_owners.keys().cloned());
        let texts = ContentTable::from_index(root.text_contracts.keys().cloned());
        let locales = root
            .locales
            .iter()
            .map(|locale| (locale.clone(), ContentTable::default()))
            .collect();
        let assets = ContentTable::from_index(root.assets.keys().cloned());
        let title_nodes = root.title_nodes.clone();
        let root = Arc::new(root);
        let view = Self {
            format: root.format,
            game_id: root.game_id.clone(),
            revision: root.revision.clone(),
            entry: root.entry.clone(),
            capabilities: CapabilitySet::from_names_known(root.requires.iter().map(String::as_str)),
            requires: root.requires.clone(),
            stage: root.stage.clone(),
            variables: Arc::new(root.variables.clone()),
            functions,
            modules: Arc::new(root.modules.clone()),
            scenes,
            sprite_timelines,
            cues,
            choices,
            texts,
            locales: Arc::new(locales),
            locale_config: Arc::new(root.locale_config.clone()),
            assets,
            default_locale: root.default_locale.clone(),
            title_scene: root.title_scene.clone(),
            theme: Arc::new(root.theme.clone()),
            player: Arc::new(root.player.clone()),
            title_nodes: Arc::new(title_nodes),
            function_index: Arc::new(root.function_index.clone()),
            text_owners: Arc::new(root.text_owners.clone()),
            text_identities: Arc::new(root.text_contracts.clone()),
            asset_index: Arc::new(root.assets.clone()),
            cue_recipes: Arc::new(BTreeMap::new()),
            task_definitions: Arc::new(BTreeMap::new()),
            op_owners: Arc::new(BTreeMap::new()),
            runtime_root: Some(root),
        };
        validate_runtime_root(view.runtime_root.as_ref().unwrap())?;
        Ok(view)
    }
    pub(super) fn with_runtime_objects(
        &self,
        objects: &[RuntimeObject],
        op_owners: &BTreeMap<String, String>,
    ) -> Result<Self> {
        let mut next = self.clone();
        let mut functions = Vec::new();
        let mut scenes = Vec::new();
        let mut sprite_timelines = Vec::new();
        let mut cues = Vec::new();
        let mut choices = Vec::new();
        let mut texts = Vec::new();
        let mut assets = Vec::new();
        let mut locale_tables = (*self.locales).clone();
        let mut recipes = (*self.cue_recipes).clone();
        let mut task_definitions = (*self.task_definitions).clone();
        let mut loaded_op_owners = (*self.op_owners).clone();
        let mut batch_op_ids = BTreeSet::new();
        for object in objects {
            match object {
                RuntimeObject::Static(package) => {
                    sprite_timelines.extend(
                        package
                            .sprite_timelines
                            .iter()
                            .map(|(id, value)| (id.clone(), value.clone())),
                    );
                    for (id, value) in &package.scenes {
                        scenes.push((id.clone(), value.clone()));
                    }
                    for (id, value) in &package.cues {
                        cues.push((id.clone(), value.clone()));
                    }
                    for (id, value) in &package.choices {
                        choices.push((id.clone(), value.clone()));
                    }
                    for (id, value) in &package.text_contracts {
                        texts.push((id.clone(), value.clone()));
                    }
                    recipes.extend(
                        package
                            .activation_recipes
                            .iter()
                            .map(|(cue, assets)| (cue.clone(), Arc::new(assets.clone()))),
                    );
                    for cue in package.cues.values() {
                        for effect in &cue.effects {
                            task_definitions
                                .entry(effect.id.clone())
                                .or_default()
                                .push(Arc::new(effect.effect.clone()));
                        }
                    }
                }
                RuntimeObject::Code(package) => {
                    for (id, value) in &package.functions {
                        for block in value.blocks.values() {
                            for op in &block.ops {
                                if !batch_op_ids.insert(op.id.clone())
                                    || op_owners.get(&op.id).is_some_and(|owner| owner != id)
                                {
                                    return Err(err(
                                        "E_DUPLICATE",
                                        &op.id,
                                        "duplicate operation identity",
                                    ));
                                }
                                loaded_op_owners.insert(op.id.clone(), id.clone());
                            }
                        }
                        functions.push((id.clone(), value.clone()));
                    }
                }
                RuntimeObject::Text(package) => {
                    let table = locale_tables
                        .get(&package.locale)
                        .ok_or_else(|| err("E_LOCALE", &package.locale, "unknown locale"))?;
                    locale_tables.insert(
                        package.locale.clone(),
                        table.with_values(package.texts.clone()),
                    );
                }
                RuntimeObject::Catalog(package) => {
                    for (id, value) in &package.assets {
                        assets.push((id.clone(), value.clone()));
                    }
                }
            }
        }
        next.functions = self.functions.with_values(functions);
        next.scenes = self.scenes.with_values(scenes);
        next.sprite_timelines = self.sprite_timelines.with_values(sprite_timelines);
        next.cues = self.cues.with_values(cues);
        next.choices = self.choices.with_values(choices);
        next.texts = self.texts.with_values(texts);
        next.assets = self.assets.with_values(assets);
        next.locales = Arc::new(locale_tables);
        next.cue_recipes = Arc::new(recipes);
        next.task_definitions = Arc::new(task_definitions);
        // Operation identities are root-scoped tombstones. Keep owners for
        // code that was resident in the past even when its body is evicted.
        let mut all_op_owners = op_owners.clone();
        for (id, owner) in loaded_op_owners {
            if all_op_owners
                .get(&id)
                .is_some_and(|expected| expected != &owner)
            {
                return Err(err("E_DUPLICATE", &id, "duplicate operation identity"));
            }
            all_op_owners.insert(id, owner);
        }
        next.op_owners = Arc::new(all_op_owners);
        Ok(next)
    }
    pub(super) fn without_runtime_objects(&self, keys: &BTreeSet<ContentKey>) -> Self {
        let Some(root) = self.runtime_root.as_deref() else {
            return self.clone();
        };
        let mut next = self.clone();
        let mut scenes = BTreeSet::new();
        let mut sprite_timelines = BTreeSet::new();
        let mut cues = BTreeSet::new();
        let mut choices = BTreeSet::new();
        let mut contracts = BTreeSet::new();
        let mut locale_texts = BTreeMap::<String, BTreeSet<String>>::new();
        let mut functions = BTreeSet::new();
        let mut assets = BTreeSet::new();
        let mut static_modules = BTreeSet::new();
        for key in keys {
            match key {
                ContentKey::Static { module } => {
                    static_modules.insert(module.clone());
                    sprite_timelines.extend(
                        root.timeline_owners
                            .iter()
                            .filter(|(_, owner)| *owner == module)
                            .map(|(id, _)| id.clone()),
                    );
                    scenes.extend(
                        root.scene_owners
                            .iter()
                            .filter(|(_, owner)| *owner == module)
                            .map(|(id, _)| id.clone()),
                    );
                    cues.extend(
                        root.cue_owners
                            .iter()
                            .filter(|(_, owner)| *owner == module)
                            .map(|(id, _)| id.clone()),
                    );
                    choices.extend(
                        root.choice_owners
                            .iter()
                            .filter(|(_, owner)| *owner == module)
                            .map(|(id, _)| id.clone()),
                    );
                    contracts.extend(
                        root.text_owners
                            .iter()
                            .filter(|(_, owner)| *owner == module)
                            .map(|(id, _)| id.clone()),
                    );
                }
                ContentKey::Code { module } => functions.extend(
                    root.function_index
                        .iter()
                        .filter(|(_, index)| index.module == *module)
                        .map(|(id, _)| id.clone()),
                ),
                ContentKey::Text { module, locale } => {
                    locale_texts.entry(locale.clone()).or_default().extend(
                        root.text_owners
                            .iter()
                            .filter(|(_, owner)| *owner == module)
                            .map(|(id, _)| id.clone()),
                    );
                }
                ContentKey::Catalog { catalog } => assets.extend(
                    root.assets
                        .iter()
                        .filter(|(_, asset)| asset.catalog == *catalog)
                        .map(|(id, _)| id.clone()),
                ),
            }
        }
        next.functions = self.functions.without_values(&functions);
        next.scenes = self.scenes.without_values(&scenes);
        next.sprite_timelines = self.sprite_timelines.without_values(&sprite_timelines);
        next.cues = self.cues.without_values(&cues);
        next.choices = self.choices.without_values(&choices);
        next.texts = self.texts.without_values(&contracts);
        next.assets = self.assets.without_values(&assets);
        let mut locales = (*self.locales).clone();
        for (locale, texts) in locale_texts {
            if let Some(docs) = locales.get_mut(&locale) {
                *docs = docs.without_values(&texts);
            }
        }
        next.locales = Arc::new(locales);
        let mut recipes = (*self.cue_recipes).clone();
        for id in &cues {
            recipes.remove(id);
        }
        next.cue_recipes = Arc::new(recipes);
        let mut task_definitions = (*self.task_definitions).clone();
        for (task, owner) in &root.task_owners {
            if static_modules.contains(owner) {
                task_definitions.remove(task);
            }
        }
        next.task_definitions = Arc::new(task_definitions);
        next
    }
}

#[derive(Debug, Clone)]
pub(super) struct FunctionInstructions {
    pub(super) blocks: BTreeMap<String, Vec<Instruction>>,
}
pub(super) fn function_instructions(function: &Function) -> FunctionInstructions {
    FunctionInstructions {
        blocks: function
            .blocks
            .iter()
            .map(|(id, block)| {
                let mut instructions = block
                    .ops
                    .iter()
                    .cloned()
                    .map(Instruction::Op)
                    .collect::<Vec<_>>();
                instructions.push(Instruction::Term(block.terminator.clone()));
                (id.clone(), instructions)
            })
            .collect(),
    }
}
pub(super) fn build_instruction_map(
    functions: &ContentTable<Function>,
) -> BTreeMap<String, Arc<FunctionInstructions>> {
    functions
        .iter()
        .map(|(id, function)| (id.clone(), Arc::new(function_instructions(function))))
        .collect()
}
pub(super) fn add_instructions(
    instructions: &mut BTreeMap<String, Arc<FunctionInstructions>>,
    objects: &[RuntimeObject],
) {
    for object in objects {
        if let RuntimeObject::Code(package) = object {
            for (id, function) in &package.functions {
                instructions
                    .entry(id.clone())
                    .or_insert_with(|| Arc::new(function_instructions(function)));
            }
        }
    }
}
pub(super) fn remove_instructions(
    instructions: &mut BTreeMap<String, Arc<FunctionInstructions>>,
    root: &RuntimeProgram,
    keys: &BTreeSet<ContentKey>,
) {
    let removed_modules: BTreeSet<_> = keys
        .iter()
        .filter_map(|key| match key {
            ContentKey::Code { module } => Some(module.as_str()),
            _ => None,
        })
        .collect();
    instructions.retain(|id, _| {
        !root
            .function_index
            .get(id)
            .is_some_and(|index| removed_modules.contains(index.module.as_str()))
    });
}

#[derive(Debug, Clone)]
pub(super) struct ResidentBlock {
    pub(super) digest: String,
    pub(super) encoded_bytes: u64,
    pub(super) generation: u64,
    pub(super) speculative: bool,
}
impl ResidentBlock {
    pub(super) fn metadata(&self) -> (String, u64) {
        (self.digest.clone(), self.encoded_bytes)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OpTombstone {
    pub(super) function: String,
    pub(super) code_digest: String,
}
#[derive(Debug, Clone)]
pub(super) struct ActiveLease {
    pub(super) info: LeaseInfo,
    pub(super) generations: BTreeMap<ContentKey, u64>,
}
#[derive(Debug, Clone, Copy)]
pub(super) struct UsageStamp {
    pub(super) last_used: Option<u64>,
    pub(super) speculative: bool,
    pub(super) admitted: u64,
}
#[derive(Debug)]
pub(super) struct LedgerState {
    pub(super) leases: BTreeMap<u64, ActiveLease>,
    pub(super) retired: BTreeSet<u64>,
    pub(super) usage: BTreeMap<u64, UsageStamp>,
    pub(super) op_tombstones: BTreeMap<String, OpTombstone>,
    pub(super) next_generation: u64,
    pub(super) next_lease: u64,
    pub(super) clock: u64,
    pub(super) revision: u64,
}
impl LedgerState {
    pub(super) fn empty() -> Self {
        Self {
            leases: BTreeMap::new(),
            retired: BTreeSet::new(),
            usage: BTreeMap::new(),
            op_tombstones: BTreeMap::new(),
            next_generation: 1,
            next_lease: 1,
            clock: 1,
            revision: 0,
        }
    }
}
#[derive(Debug)]
pub(super) struct ResidencyLedger {
    pub(super) state: Mutex<LedgerState>,
}
impl ResidencyLedger {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(LedgerState::empty()),
        }
    }
    pub(super) fn with_tombstones(tombstones: BTreeMap<String, OpTombstone>) -> Self {
        let ledger = Self::new();
        ledger
            .state
            .lock()
            .expect("residency mutex poisoned")
            .op_tombstones = tombstones;
        ledger
    }
    pub(super) fn report(
        &self,
        resident: &BTreeMap<ContentKey, (String, u64, u64)>,
        budget: &ResidencyBudget,
    ) -> ResidencyReport {
        let state = self.state.lock().expect("residency mutex poisoned");
        let current: BTreeMap<_, _> = resident
            .iter()
            .filter(|(_, (_, _, generation))| !state.retired.contains(generation))
            .map(|(key, (digest, bytes, generation))| {
                (key.clone(), (digest.clone(), *bytes, *generation))
            })
            .collect();
        let active_leases: Vec<_> = state
            .leases
            .values()
            .filter(|lease| {
                lease.generations.iter().any(|(key, generation)| {
                    current.get(key).is_some_and(|(_, _, resident_generation)| {
                        resident_generation == generation
                    })
                })
            })
            .collect();
        let pinned = |key: &ContentKey, generation: u64| {
            active_leases
                .iter()
                .any(|lease| lease.generations.get(key) == Some(&generation))
        };
        let resident_values: Vec<_> = current
            .values()
            .map(|(digest, bytes, _)| (digest.clone(), *bytes))
            .collect();
        let resident_bytes = unique_bytes(resident_values.iter());
        let pinned_records: Vec<_> = current
            .iter()
            .filter(|(key, (_, _, generation))| pinned(key, *generation))
            .map(|(_, (digest, bytes, _))| (digest.clone(), *bytes))
            .collect();
        let pinned_bytes = unique_bytes(pinned_records.iter());
        let blocks = current
            .iter()
            .map(
                |(key, (digest, encoded_bytes, generation))| ResidencyEntry {
                    key: key.clone(),
                    digest: digest.clone(),
                    encoded_bytes: *encoded_bytes,
                    pinned_by: active_leases
                        .iter()
                        .filter(|lease| lease.generations.get(key) == Some(generation))
                        .map(|lease| lease.info.owner.clone())
                        .collect(),
                },
            )
            .collect();
        ResidencyReport {
            resident_blocks: current.len(),
            pinned_blocks: current
                .iter()
                .filter(|(key, (_, _, generation))| pinned(key, *generation))
                .count(),
            lease_count: active_leases.len(),
            resident_bytes,
            pinned_bytes,
            unpinned_bytes: resident_bytes.saturating_sub(pinned_bytes),
            budget_bytes: budget.resident_bytes,
            blocks,
            leases: active_leases
                .iter()
                .map(|lease| lease.info.clone())
                .collect(),
        }
    }
}
pub(super) fn unique_bytes<'a>(items: impl IntoIterator<Item = &'a (String, u64)>) -> u64 {
    let mut hashes = BTreeMap::<&str, u64>::new();
    for (digest, bytes) in items {
        hashes.entry(digest.as_str()).or_insert(*bytes);
    }
    hashes.values().copied().sum()
}
pub(super) fn record_bytes(records: &BTreeMap<ContentKey, ResidentBlock>) -> u64 {
    let values: Vec<_> = records.values().map(ResidentBlock::metadata).collect();
    unique_bytes(values.iter())
}
pub(super) fn key_priority(key: &ContentKey) -> u8 {
    match key {
        ContentKey::Static { .. } => 0,
        ContentKey::Catalog { .. } => 1,
        ContentKey::Code { .. } => 2,
        ContentKey::Text { .. } => 3,
    }
}
pub(super) fn block_lru_key(
    key: &ContentKey,
    block: &ResidentBlock,
    usage: &BTreeMap<u64, UsageStamp>,
) -> (u8, u64, ContentKey) {
    let stamp = usage.get(&block.generation).copied().unwrap_or(UsageStamp {
        last_used: Some(0),
        speculative: block.speculative,
        admitted: 0,
    });
    let speculative_rank = u8::from(!(stamp.speculative && stamp.last_used.is_none()));
    let age = stamp.last_used.unwrap_or(stamp.admitted);
    (speculative_rank, age, key.clone())
}
pub(super) fn metadata_map(
    records: &BTreeMap<ContentKey, ResidentBlock>,
) -> BTreeMap<ContentKey, (String, u64)> {
    records
        .iter()
        .map(|(key, block)| (key.clone(), block.metadata()))
        .collect()
}
pub(super) fn generation_map(
    records: &BTreeMap<ContentKey, ResidentBlock>,
) -> BTreeMap<ContentKey, u64> {
    records
        .iter()
        .map(|(key, block)| (key.clone(), block.generation))
        .collect()
}
pub(super) fn op_owner_map(tombstones: &BTreeMap<String, OpTombstone>) -> BTreeMap<String, String> {
    tombstones
        .iter()
        .map(|(id, tombstone)| (id.clone(), tombstone.function.clone()))
        .collect()
}

#[derive(Debug, Clone)]
pub struct ValidatedProgram {
    pub(super) program: Arc<RuntimeProgramView>,
    pub(super) instructions: Arc<BTreeMap<String, Arc<FunctionInstructions>>>,
    pub(super) runtime_objects: Arc<BTreeMap<ContentKey, (String, u64)>>,
    pub(super) runtime_generations: Arc<BTreeMap<ContentKey, u64>>,
    pub(super) op_tombstones: Arc<BTreeMap<String, OpTombstone>>,
    pub(super) residency: Arc<ResidencyLedger>,
    pub(super) budget: ResidencyBudget,
    pub(super) projected: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResidencyReport {
    pub resident_blocks: usize,
    pub pinned_blocks: usize,
    pub lease_count: usize,
    pub resident_bytes: u64,
    pub pinned_bytes: u64,
    pub unpinned_bytes: u64,
    pub budget_bytes: u64,
    pub blocks: Vec<ResidencyEntry>,
    pub leases: Vec<LeaseInfo>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResidencyEntry {
    pub key: ContentKey,
    pub digest: String,
    pub encoded_bytes: u64,
    pub pinned_by: BTreeSet<String>,
}

#[derive(Debug)]
pub struct ContentLease {
    pub(super) id: u64,
    pub(super) ledger: Arc<ResidencyLedger>,
    pub(super) info: LeaseInfo,
}
impl ContentLease {
    pub fn info(&self) -> &LeaseInfo {
        &self.info
    }
}
impl Drop for ContentLease {
    fn drop(&mut self) {
        let mut state = self.ledger.state.lock().expect("residency mutex poisoned");
        if state.leases.remove(&self.id).is_some() {
            state.revision = state.revision.wrapping_add(1);
        }
    }
}

#[derive(Debug)]
pub struct ContentAdmission {
    pub(super) ledger: Arc<ResidencyLedger>,
    pub(super) base_revision: u64,
    pub(super) next_generation: u64,
    pub(super) clock: u64,
    pub(super) candidate: ValidatedProgram,
    pub(super) evicted: BTreeSet<ContentKey>,
    pub(super) evicted_generations: BTreeMap<ContentKey, u64>,
    pub(super) admitted: BTreeMap<u64, bool>,
}
impl ContentAdmission {
    /// Validated projected view. New keys become leaseable only after commit.
    pub fn view(&self) -> &ValidatedProgram {
        &self.candidate
    }
    pub fn evicted_keys(&self) -> &BTreeSet<ContentKey> {
        &self.evicted
    }
    pub fn commit(self) -> Result<ValidatedProgram> {
        self.commit_inner(None).map(|(program, _)| program)
    }
    /// Commit and register the replacement active reference while holding the
    /// same mutex, so no concurrent admission can retire it between the two.
    pub fn commit_with_lease(
        self,
        keys: BTreeSet<ContentKey>,
        owner: String,
    ) -> Result<(ValidatedProgram, ContentLease)> {
        let (program, lease) = self.commit_inner(Some((keys, owner)))?;
        Ok((program, lease.expect("lease requested")))
    }
    pub(super) fn commit_inner(
        self,
        lease: Option<(BTreeSet<ContentKey>, String)>,
    ) -> Result<(ValidatedProgram, Option<ContentLease>)> {
        let mut state = self.ledger.state.lock().expect("residency mutex poisoned");
        if state.revision != self.base_revision {
            return Err(err(
                "E_RESIDENCY_STALE",
                "runtime",
                "content admission changed while the transaction was prepared",
            ));
        }
        for (key, generation) in &self.evicted_generations {
            if state
                .leases
                .values()
                .any(|lease| lease.generations.get(key) == Some(generation))
            {
                return Err(err(
                    "E_CONTENT_PINNED",
                    &format!("{key:?}"),
                    "content is protected by an active lease",
                ));
            }
        }
        let bytes: Vec<_> = self
            .candidate
            .runtime_objects
            .values()
            .map(|(digest, bytes)| (digest.clone(), *bytes))
            .collect();
        if unique_bytes(bytes.iter()) > self.candidate.budget.resident_bytes {
            return Err(err(
                "E_RESIDENCY_BUDGET",
                "runtime",
                "content exceeds residency budget",
            ));
        }
        if let Some((keys, _)) = &lease {
            if let Some(key) = keys.iter().find(|key| {
                !self.candidate.runtime_generations.contains_key(*key)
                    || state
                        .retired
                        .contains(&self.candidate.runtime_generations[*key])
            }) {
                return Err(err(
                    "E_CONTENT_MISSING",
                    &format!("{key:?}"),
                    "cannot lease a missing content block",
                ));
            }
        }
        for generation in self.evicted_generations.values() {
            state.retired.insert(*generation);
            state.usage.remove(generation);
        }
        state.op_tombstones = (*self.candidate.op_tombstones).clone();
        state.next_generation = self.next_generation;
        state.clock = self.clock;
        for (generation, speculative) in &self.admitted {
            let admitted = state.clock;
            state.usage.insert(
                *generation,
                UsageStamp {
                    last_used: if *speculative { None } else { Some(admitted) },
                    speculative: *speculative,
                    admitted,
                },
            );
            if !speculative {
                state.clock = state.clock.wrapping_add(1);
            }
        }
        state.revision = state.revision.wrapping_add(1);
        let mut lease_value = None;
        if let Some((keys, owner)) = lease {
            let id = state.next_lease;
            state.next_lease = state.next_lease.wrapping_add(1).max(1);
            let generations: BTreeMap<_, _> = keys
                .iter()
                .filter_map(|key| {
                    self.candidate
                        .runtime_generations
                        .get(key)
                        .map(|generation| (key.clone(), *generation))
                })
                .collect();
            let resident_bytes = unique_bytes(
                keys.iter()
                    .filter_map(|key| self.candidate.runtime_objects.get(key))
                    .map(|(digest, bytes)| (digest.clone(), *bytes))
                    .collect::<Vec<_>>()
                    .iter(),
            );
            let info = LeaseInfo {
                id,
                owner,
                keys,
                resident_bytes,
            };
            state.leases.insert(
                id,
                ActiveLease {
                    info: info.clone(),
                    generations,
                },
            );
            state.revision = state.revision.wrapping_add(1);
            lease_value = Some(ContentLease {
                id,
                ledger: self.ledger.clone(),
                info,
            });
        }
        let mut candidate = self.candidate;
        candidate.projected = false;
        Ok((candidate, lease_value))
    }
}

impl ValidatedProgram {
    pub fn new(p: Program) -> Result<Self> {
        let program = RuntimeProgramView::from_source(p);
        validate(&program)?;
        let instructions = Arc::new(build_instruction_map(&program.functions));
        Ok(Self::new_view(
            program,
            instructions,
            BTreeMap::new(),
            Arc::new(ResidencyLedger::new()),
            BTreeMap::new(),
            ResidencyBudget {
                resident_bytes: MAX_INPUT_BYTES as u64,
            },
            false,
        ))
    }
    pub fn from_runtime(root: RuntimeProgram) -> Result<Self> {
        let program = RuntimeProgramView::from_runtime(root)?;
        Ok(Self::new_view(
            program,
            Arc::new(BTreeMap::new()),
            BTreeMap::new(),
            Arc::new(ResidencyLedger::new()),
            BTreeMap::new(),
            ResidencyBudget {
                resident_bytes: MAX_INPUT_BYTES as u64,
            },
            false,
        ))
    }
    pub(super) fn new_view(
        program: RuntimeProgramView,
        instructions: Arc<BTreeMap<String, Arc<FunctionInstructions>>>,
        records: BTreeMap<ContentKey, ResidentBlock>,
        residency: Arc<ResidencyLedger>,
        op_tombstones: BTreeMap<String, OpTombstone>,
        budget: ResidencyBudget,
        projected: bool,
    ) -> Self {
        Self {
            program: Arc::new(program),
            instructions,
            runtime_objects: Arc::new(metadata_map(&records)),
            runtime_generations: Arc::new(generation_map(&records)),
            op_tombstones: Arc::new(op_tombstones),
            residency,
            budget,
            projected,
        }
    }
    pub(super) fn record_snapshot(&self) -> BTreeMap<ContentKey, ResidentBlock> {
        self.runtime_objects
            .iter()
            .filter_map(|(key, (digest, encoded_bytes))| {
                Some((
                    key.clone(),
                    ResidentBlock {
                        digest: digest.clone(),
                        encoded_bytes: *encoded_bytes,
                        generation: *self.runtime_generations.get(key)?,
                        speculative: false,
                    },
                ))
            })
            .collect()
    }
    pub fn program(&self) -> &RuntimeProgramView {
        &self.program
    }
    pub fn legacy_program(&self) -> Option<Program> {
        self.program.to_source_program()
    }
    pub fn runtime_root(&self) -> Option<&RuntimeProgram> {
        self.program.runtime_root()
    }
    pub(crate) fn runtime_root_arc(&self) -> Option<Arc<RuntimeProgram>> {
        self.program.runtime_root.clone()
    }
    pub(crate) fn contains_content_body(&self, key: &ContentKey) -> bool {
        self.runtime_objects.contains_key(key)
    }
    pub fn is_resident(&self, key: &ContentKey) -> bool {
        let Some(generation) = self.runtime_generations.get(key) else {
            return false;
        };
        if self.projected {
            return true;
        }
        !self
            .residency
            .state
            .lock()
            .expect("residency mutex poisoned")
            .retired
            .contains(generation)
    }
    /// Pure additive compatibility API: it preserves every currently resident
    /// block and fails if the candidate cannot fit without eviction.
    pub fn install_batch(&self, batch: Vec<(ContentKey, RuntimeObject, u64)>) -> Result<Self> {
        self.prepare_batch(batch, false, false)?.commit()
    }
    /// Prepare a demand admission. The transaction may evict unused speculative
    /// blocks first, then least recently used unleased blocks.
    pub fn prepare_install_batch(
        &self,
        batch: Vec<(ContentKey, RuntimeObject, u64)>,
    ) -> Result<ContentAdmission> {
        self.prepare_batch(batch, true, false)
    }
    /// Prepare speculative content without evicting anything. These blocks are
    /// the first eligible LRU victims if a later demand needs their space.
    pub fn prepare_prefetch_batch(
        &self,
        batch: Vec<(ContentKey, RuntimeObject, u64)>,
    ) -> Result<ContentAdmission> {
        self.prepare_batch(batch, false, true)
    }
    pub fn empty_content_view(&self) -> Result<Self> {
        self.runtime_root()
            .ok_or_else(|| err("E_CONTENT_KEY", "runtime", "not a runtime program"))?;
        let mut tombstones = (*self.op_tombstones).clone();
        {
            let state = self
                .residency
                .state
                .lock()
                .expect("residency mutex poisoned");
            for (id, tombstone) in &state.op_tombstones {
                if tombstones.get(id).is_some_and(|known| known != tombstone) {
                    return Err(err(
                        "E_DUPLICATE",
                        id,
                        "operation identity tombstone differs",
                    ));
                }
                tombstones.insert(id.clone(), tombstone.clone());
            }
        }
        let resident_keys = self.runtime_generations.keys().cloned().collect();
        let mut program = self.program.without_runtime_objects(&resident_keys);
        program.op_owners = Arc::new(op_owner_map(&tombstones));
        Ok(Self::new_view(
            program,
            Arc::new(BTreeMap::new()),
            BTreeMap::new(),
            Arc::new(ResidencyLedger::with_tombstones(tombstones.clone())),
            tombstones,
            ResidencyBudget {
                resident_bytes: MAX_INPUT_BYTES as u64,
            },
            false,
        ))
    }
    pub fn prepare_eviction(&self, keys: BTreeSet<ContentKey>) -> Result<ContentAdmission> {
        self.prepare_eviction_inner(keys)
    }
    pub fn evict(&self, keys: BTreeSet<ContentKey>) -> Result<Self> {
        self.prepare_eviction(keys)?.commit()
    }
    pub(super) fn prepare_eviction_inner(
        &self,
        keys: BTreeSet<ContentKey>,
    ) -> Result<ContentAdmission> {
        let root = self
            .runtime_root()
            .ok_or_else(|| err("E_CONTENT_KEY", "runtime", "not a runtime program"))?;
        let state = self
            .residency
            .state
            .lock()
            .expect("residency mutex poisoned");
        let source_records = self.record_snapshot();
        if source_records
            .values()
            .any(|record| state.retired.contains(&record.generation))
        {
            return Err(err(
                "E_RESIDENCY_STALE",
                "runtime",
                "cannot evict from a view with retired content",
            ));
        }
        for key in &keys {
            let Some(record) = source_records.get(key) else {
                return Err(err(
                    "E_CONTENT_MISSING",
                    &format!("{key:?}"),
                    "cannot evict a missing content block",
                ));
            };
            if state.retired.contains(&record.generation) {
                return Err(err(
                    "E_CONTENT_RETIRED",
                    &format!("{key:?}"),
                    "content generation has been retired",
                ));
            }
            if state
                .leases
                .values()
                .any(|lease| lease.generations.get(key) == Some(&record.generation))
            {
                return Err(err(
                    "E_CONTENT_PINNED",
                    &format!("{key:?}"),
                    "content is protected by an active lease",
                ));
            }
        }
        let base_revision = state.revision;
        let mut tombstones = state.op_tombstones.clone();
        for (id, tombstone) in self.op_tombstones.iter() {
            if tombstones.get(id).is_some_and(|known| known != tombstone) {
                return Err(err(
                    "E_DUPLICATE",
                    id,
                    "operation identity tombstone differs",
                ));
            }
            tombstones.insert(id.clone(), tombstone.clone());
        }
        let next_generation = state.next_generation;
        let clock = state.clock;
        let evicted_generations = keys
            .iter()
            .filter_map(|key| {
                source_records
                    .get(key)
                    .map(|record| (key.clone(), record.generation))
            })
            .collect();
        drop(state);
        let mut records = source_records;
        for key in &keys {
            records.remove(key);
        }
        let tombstone_owners = op_owner_map(&tombstones);
        let candidate_parts = self.materialize_records(
            root,
            &self.record_snapshot(),
            &records,
            &tombstone_owners,
            &[],
        )?;
        let mut program = candidate_parts.0;
        let mut instructions = candidate_parts.1;
        program = program.without_runtime_objects(&keys);
        remove_instructions(&mut instructions, root, &keys);
        let candidate = Self::new_view(
            program,
            Arc::new(instructions),
            records,
            self.residency.clone(),
            tombstones,
            self.budget.clone(),
            true,
        );
        Ok(ContentAdmission {
            ledger: self.residency.clone(),
            base_revision,
            next_generation,
            clock,
            candidate,
            evicted: keys,
            evicted_generations,
            admitted: BTreeMap::new(),
        })
    }
    pub(super) fn prepare_batch(
        &self,
        batch: Vec<(ContentKey, RuntimeObject, u64)>,
        allow_eviction: bool,
        speculative: bool,
    ) -> Result<ContentAdmission> {
        let root = self
            .runtime_root()
            .ok_or_else(|| err("E_CONTENT_KEY", "runtime", "not a runtime program"))?;
        if batch.len() > MAX_CONTENT_BATCH_ITEMS {
            return Err(err(
                "E_LIMIT",
                "content",
                "content batch has more than 128 blocks",
            ));
        }
        let mut payload_bytes = 0u64;
        let mut seen = BTreeSet::new();
        for (key, _, bytes) in &batch {
            if !seen.insert(key.clone()) {
                return Err(err(
                    "E_DUPLICATE",
                    &format!("{key:?}"),
                    "duplicate content key",
                ));
            }
            if *bytes == 0 || *bytes > MAX_INPUT_BYTES as u64 {
                return Err(err(
                    "E_LIMIT",
                    "content",
                    "content block size is outside 1..16 MiB",
                ));
            }
            payload_bytes = payload_bytes
                .checked_add(*bytes)
                .ok_or_else(|| err("E_LIMIT", "content", "content batch byte count overflow"))?;
            if payload_bytes > MAX_INPUT_BYTES as u64 {
                return Err(err("E_LIMIT", "content", "content batch exceeds 16 MiB"));
            }
        }
        let mut records = self.record_snapshot();
        let state = self
            .residency
            .state
            .lock()
            .expect("residency mutex poisoned");
        if records
            .values()
            .any(|record| state.retired.contains(&record.generation))
        {
            return Err(err(
                "E_RESIDENCY_STALE",
                "runtime",
                "cannot admit from a view with retired content",
            ));
        }
        let base_revision = state.revision;
        let mut op_tombstones = state.op_tombstones.clone();
        for (id, tombstone) in self.op_tombstones.iter() {
            if op_tombstones
                .get(id)
                .is_some_and(|known| known != tombstone)
            {
                return Err(err(
                    "E_DUPLICATE",
                    id,
                    "operation identity tombstone differs",
                ));
            }
            op_tombstones.insert(id.clone(), tombstone.clone());
        }
        let mut next_generation = state.next_generation;
        let clock = state.clock;
        let usage = state.usage.clone();
        let pinned: BTreeSet<_> = state
            .leases
            .values()
            .flat_map(|lease| lease.generations.values().copied())
            .collect();
        drop(state);

        let mut batch = batch;
        batch.sort_by_key(|(key, _, _)| (key_priority(key), key.clone()));
        let mut added_objects = Vec::<RuntimeObject>::new();
        let mut incoming = BTreeSet::new();
        let mut admitted = BTreeMap::new();
        let mut seen_ops = BTreeSet::new();
        for (key, object, encoded_bytes) in batch {
            let requirement = root
                .content_requirement(&key)
                .ok_or_else(|| err("E_CONTENT_KEY", &format!("{key:?}"), "undeclared content"))?;
            validate_object_key(root, &key, &object)?;
            incoming.insert(key.clone());
            if let Some(existing) = records.get(&key) {
                if existing.digest != requirement.digest {
                    return Err(err(
                        "E_CONTENT_KEY",
                        &format!("{key:?}"),
                        "resident digest differs",
                    ));
                }
                if !speculative {
                    admitted.insert(existing.generation, false);
                }
                continue;
            }
            for prerequisite in root.content_prerequisites(&key) {
                if !records.contains_key(&prerequisite) {
                    return Err(err(
                        "E_CONTENT_MISSING",
                        &format!("{key:?}"),
                        "declaration scope must be resident before code or text",
                    ));
                }
            }
            if let RuntimeObject::Code(package) = &object {
                for (function_id, function) in &package.functions {
                    for op in function.blocks.values().flat_map(|block| &block.ops) {
                        if !seen_ops.insert(op.id.clone()) {
                            return Err(err("E_DUPLICATE", &op.id, "duplicate operation identity"));
                        }
                        let tombstone = OpTombstone {
                            function: function_id.clone(),
                            code_digest: requirement.digest.clone(),
                        };
                        if op_tombstones
                            .get(&op.id)
                            .is_some_and(|known| known != &tombstone)
                        {
                            return Err(err(
                                "E_DUPLICATE",
                                &op.id,
                                "operation identity belongs to different code",
                            ));
                        }
                        op_tombstones.insert(op.id.clone(), tombstone);
                    }
                }
            }
            records.insert(
                key.clone(),
                ResidentBlock {
                    digest: requirement.digest,
                    encoded_bytes,
                    generation: next_generation,
                    speculative,
                },
            );
            admitted.insert(next_generation, speculative);
            next_generation = next_generation.wrapping_add(1).max(1);
            added_objects.push(object);
        }
        let tombstone_owners = op_owner_map(&op_tombstones);
        let mut candidate_program = self.materialize_records(
            root,
            &self.record_snapshot(),
            &records,
            &tombstone_owners,
            &added_objects,
        )?;
        if !added_objects.is_empty() {
            validate_runtime_objects(&candidate_program.0, &added_objects)?;
        }

        let mut evicted = BTreeSet::new();
        let mut evicted_generations = BTreeMap::new();
        if record_bytes(&records) > self.budget.resident_bytes {
            if !allow_eviction {
                return Err(err(
                    "E_RESIDENCY_BUDGET",
                    "runtime",
                    "content exceeds residency budget",
                ));
            }
            let mut victims: Vec<_> = records
                .iter()
                .filter(|(key, block)| {
                    !incoming.contains(*key) && !pinned.contains(&block.generation)
                })
                .map(|(key, block)| (block_lru_key(key, block, &usage), key.clone()))
                .collect();
            victims.sort_by(|a, b| a.0.cmp(&b.0));
            for (_, key) in victims {
                if record_bytes(&records) <= self.budget.resident_bytes {
                    break;
                }
                if let Some(record) = records.remove(&key) {
                    evicted_generations.insert(key.clone(), record.generation);
                    evicted.insert(key);
                }
            }
            if record_bytes(&records) > self.budget.resident_bytes {
                return Err(err(
                    "E_RESIDENCY_BUDGET",
                    "runtime",
                    "resident and pinned content exceeds budget",
                ));
            }
        }
        if !evicted.is_empty() {
            candidate_program.0 = candidate_program.0.without_runtime_objects(&evicted);
            remove_instructions(&mut candidate_program.1, root, &evicted);
        }
        let final_candidate = ValidatedProgram::new_view(
            candidate_program.0,
            Arc::new(candidate_program.1),
            records.clone(),
            self.residency.clone(),
            op_tombstones,
            self.budget.clone(),
            true,
        );
        Ok(ContentAdmission {
            ledger: self.residency.clone(),
            base_revision,
            next_generation,
            clock,
            candidate: final_candidate,
            evicted,
            evicted_generations,
            admitted,
        })
    }
    pub(super) fn materialize_records(
        &self,
        root: &RuntimeProgram,
        old_records: &BTreeMap<ContentKey, ResidentBlock>,
        new_records: &BTreeMap<ContentKey, ResidentBlock>,
        op_owners: &BTreeMap<String, String>,
        newly_added: &[RuntimeObject],
    ) -> Result<(
        RuntimeProgramView,
        BTreeMap<String, Arc<FunctionInstructions>>,
    )> {
        let mut stale = BTreeSet::new();
        for (key, old) in self.runtime_generations.iter() {
            if old_records
                .get(key)
                .is_none_or(|record| record.generation != *old)
            {
                stale.insert(key.clone());
            }
        }
        let mut program = self.program.without_runtime_objects(&stale);
        // `prepare_batch` orders incoming objects by dependency; borrow them
        // directly so installation does not clone already parsed bodies.
        if !newly_added.is_empty() {
            program = program.with_runtime_objects(newly_added, op_owners)?;
        } else if !program.op_owners.as_ref().eq(op_owners) {
            program.op_owners = Arc::new(op_owners.clone());
        }
        let mut instructions = (*self.instructions).clone();
        remove_instructions(&mut instructions, root, &stale);
        add_instructions(&mut instructions, newly_added);
        let _ = new_records;
        Ok((program, instructions))
    }
    pub fn lease(&self, keys: BTreeSet<ContentKey>, owner: String) -> Result<ContentLease> {
        if self.projected {
            return Err(err(
                "E_CONTENT_PENDING",
                "runtime",
                "projected content cannot be leased before admission commits",
            ));
        }
        let mut state = self
            .residency
            .state
            .lock()
            .expect("residency mutex poisoned");
        let mut generations = BTreeMap::new();
        for key in &keys {
            let Some(generation) = self.runtime_generations.get(key) else {
                return Err(err(
                    "E_CONTENT_MISSING",
                    &format!("{key:?}"),
                    "cannot lease a missing content block",
                ));
            };
            if state.retired.contains(generation) {
                return Err(err(
                    "E_CONTENT_RETIRED",
                    &format!("{key:?}"),
                    "content generation has been retired",
                ));
            }
            generations.insert(key.clone(), *generation);
        }
        let id = state.next_lease;
        state.next_lease = state.next_lease.wrapping_add(1).max(1);
        let values: Vec<_> = keys
            .iter()
            .filter_map(|key| self.runtime_objects.get(key))
            .map(|(digest, bytes)| (digest.clone(), *bytes))
            .collect();
        let resident_bytes = unique_bytes(values.iter());
        let info = LeaseInfo {
            id,
            owner,
            keys,
            resident_bytes,
        };
        state.leases.insert(
            id,
            ActiveLease {
                info: info.clone(),
                generations,
            },
        );
        state.revision = state.revision.wrapping_add(1);
        Ok(ContentLease {
            id,
            ledger: self.residency.clone(),
            info,
        })
    }
    /// Mark resident keys as used without extending their lifetime with a pin.
    pub fn touch_content(&self, keys: &BTreeSet<ContentKey>) -> Result<()> {
        if self.projected {
            return Err(err(
                "E_CONTENT_PENDING",
                "runtime",
                "projected content is not committed",
            ));
        }
        let mut state = self
            .residency
            .state
            .lock()
            .expect("residency mutex poisoned");
        for key in keys {
            let Some(generation) = self.runtime_generations.get(key) else {
                return Err(err(
                    "E_CONTENT_MISSING",
                    &format!("{key:?}"),
                    "content is not resident",
                ));
            };
            if state.retired.contains(generation) {
                return Err(err(
                    "E_CONTENT_RETIRED",
                    &format!("{key:?}"),
                    "content generation has been retired",
                ));
            }
        }
        for key in keys {
            if let Some(generation) = self.runtime_generations.get(key) {
                let admitted = state
                    .usage
                    .get(generation)
                    .map(|stamp| stamp.admitted)
                    .unwrap_or(state.clock);
                let used = state.clock;
                state.usage.insert(
                    *generation,
                    UsageStamp {
                        last_used: Some(used),
                        speculative: false,
                        admitted,
                    },
                );
            }
        }
        state.clock = state.clock.wrapping_add(1);
        state.revision = state.revision.wrapping_add(1);
        Ok(())
    }
    pub fn residency(&self) -> ResidencyReport {
        let generations = &self.runtime_generations;
        let resident: BTreeMap<_, _> = self
            .runtime_objects
            .iter()
            .filter_map(|(key, (digest, bytes))| {
                generations
                    .get(key)
                    .map(|generation| (key.clone(), (digest.clone(), *bytes, *generation)))
            })
            .collect();
        self.residency.report(&resident, &self.budget)
    }
    pub fn set_residency_budget(&self, budget: ResidencyBudget) -> Result<Self> {
        if self.residency().resident_bytes > budget.resident_bytes {
            return Err(err(
                "E_RESIDENCY_BUDGET",
                "runtime",
                "resident content exceeds budget",
            ));
        }
        let mut next = self.clone();
        next.budget = budget;
        Ok(next)
    }
    pub(crate) fn instruction(&self, function: &str, block: &str, op: usize) -> &Instruction {
        &self.instructions[function].blocks[block][op]
    }
    pub fn asset(&self, id: &str) -> Option<&Asset> {
        self.program.asset(id)
    }
    pub fn title_nodes(&self) -> &[Node] {
        self.program.title_nodes()
    }
    pub fn cue_assets(&self, cue: &str) -> BTreeSet<String> {
        self.program.cue_assets(cue)
    }
    /// Mask identities referenced by `DialogueVisibility` reveal styles in
    /// every loaded function body. Runtime roots only see admitted modules;
    /// the in-flight reveal carries its own mask for the first encounter.
    pub fn window_transition_assets(&self) -> BTreeSet<String> {
        let mut assets: BTreeSet<_> = self
            .program
            .functions
            .values()
            .flat_map(|function| function.blocks.values())
            .flat_map(|block| block.ops.iter())
            .filter_map(|op| match &op.operation {
                Operation::DialogueVisibility {
                    transition: Some(style),
                    ..
                } => style.asset().map(str::to_owned),
                _ => None,
            })
            .collect();
        assets.extend(
            self.program
                .cues
                .values()
                .flat_map(|cue| &cue.effects)
                .filter_map(|def| match &def.effect {
                    Effect::StagePresent {
                        dialogue_visible: Some(_),
                        transition,
                        ..
                    } => transition.asset().map(str::to_owned),
                    _ => None,
                }),
        );
        assets
    }
}
