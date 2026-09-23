use nir_format::*;
use std::borrow::Borrow;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Index;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub(crate) enum Instruction {
    Op(Op),
    Term(Terminator),
}

/// An indexed map whose declared keys can be known before their independent
/// content blocks are resident. Values themselves are shared per body.
#[derive(Debug, Clone)]
pub struct ContentTable<T> {
    declared: Arc<BTreeSet<String>>,
    values: Arc<BTreeMap<String, Arc<T>>>,
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
    fn from_owned(values: BTreeMap<String, T>) -> Self {
        let declared = values.keys().cloned().collect();
        Self {
            declared: Arc::new(declared),
            values: Arc::new(values.into_iter().map(|(k, v)| (k, Arc::new(v))).collect()),
        }
    }
    fn from_index(keys: impl IntoIterator<Item = String>) -> Self {
        Self {
            declared: Arc::new(keys.into_iter().collect()),
            values: Arc::new(BTreeMap::new()),
        }
    }
    fn with_values(&self, values: impl IntoIterator<Item = (String, T)>) -> Self {
        let mut declared = (*self.declared).clone();
        let mut entries = (*self.values).clone();
        for (key, value) in values {
            declared.insert(key.clone());
            entries.insert(key, Arc::new(value));
        }
        Self {
            declared: Arc::new(declared),
            values: Arc::new(entries),
        }
    }
    fn without_values(&self, removed: &BTreeSet<String>) -> Self {
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
    pub stage: Stage,
    pub variables: Arc<BTreeMap<String, Value>>,
    pub functions: ContentTable<Function>,
    pub modules: Arc<BTreeMap<String, ModuleIndex>>,
    pub scenes: ContentTable<Vec<Node>>,
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
    title_nodes: Arc<Vec<Node>>,
    runtime_root: Option<Arc<RuntimeProgram>>,
    function_index: Arc<BTreeMap<String, RuntimeFunctionIndex>>,
    text_owners: Arc<BTreeMap<String, String>>,
    text_identities: Arc<BTreeMap<String, RuntimeTextIdentity>>,
    asset_index: Arc<BTreeMap<String, AssetIndexEntry>>,
    cue_recipes: Arc<BTreeMap<String, Arc<BTreeSet<String>>>>,
    task_definitions: Arc<BTreeMap<String, Vec<Arc<Effect>>>>,
    op_owners: Arc<BTreeMap<String, String>>,
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
        let mut set = BTreeSet::new();
        if let Some(definition) = self.cues.get(cue) {
            for effect in &definition.effects {
                match &effect.effect {
                    Effect::StagePresent { scene, .. } => {
                        if let Some(nodes) = self.scenes.get(scene) {
                            set.extend(nodes.iter().filter_map(|node| node.asset.clone()));
                        }
                    }
                    Effect::Audio { asset, .. } => {
                        set.insert(asset.clone());
                    }
                    _ => {}
                }
            }
        }
        set
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
    fn from_source(p: Program) -> Self {
        let function_index = p
            .functions
            .iter()
            .map(|(id, function)| {
                (
                    id.clone(),
                    RuntimeFunctionIndex {
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
        let mut cue_recipes = BTreeMap::new();
        for cue in p.cues.keys() {
            let mut assets = BTreeSet::new();
            if let Some(definition) = p.cues.get(cue) {
                for effect in &definition.effects {
                    match &effect.effect {
                        Effect::StagePresent { scene, .. } => {
                            if let Some(nodes) = p.scenes.get(scene) {
                                assets.extend(nodes.iter().filter_map(|node| node.asset.clone()));
                            }
                        }
                        Effect::Audio { asset, .. } => {
                            assets.insert(asset.clone());
                        }
                        _ => {}
                    }
                }
            }
            cue_recipes.insert(cue.clone(), Arc::new(assets));
        }
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
            requires: p.requires,
            stage: p.stage,
            variables: Arc::new(p.variables),
            functions: ContentTable::from_owned(p.functions),
            modules: Arc::new(p.modules),
            scenes: ContentTable::from_owned(p.scenes),
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
    fn from_runtime(root: RuntimeProgram) -> Result<Self> {
        let functions = ContentTable::from_index(root.function_index.keys().cloned());
        let scenes = ContentTable::from_index(root.scene_owners.keys().cloned());
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
            requires: root.requires.clone(),
            stage: root.stage.clone(),
            variables: Arc::new(root.variables.clone()),
            functions,
            modules: Arc::new(root.modules.clone()),
            scenes,
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
    fn with_runtime_objects(
        &self,
        objects: &[RuntimeObject],
        op_owners: &BTreeMap<String, String>,
    ) -> Result<Self> {
        let mut next = self.clone();
        let mut functions = Vec::new();
        let mut scenes = Vec::new();
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
    fn without_runtime_objects(&self, keys: &BTreeSet<ContentKey>) -> Self {
        let Some(root) = self.runtime_root.as_deref() else {
            return self.clone();
        };
        let mut next = self.clone();
        let mut scenes = BTreeSet::new();
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
struct FunctionInstructions {
    blocks: BTreeMap<String, Vec<Instruction>>,
}
fn function_instructions(function: &Function) -> FunctionInstructions {
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
fn build_instruction_map(
    functions: &ContentTable<Function>,
) -> BTreeMap<String, Arc<FunctionInstructions>> {
    functions
        .iter()
        .map(|(id, function)| (id.clone(), Arc::new(function_instructions(function))))
        .collect()
}
fn add_instructions(
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
fn remove_instructions(
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
struct ResidentBlock {
    digest: String,
    encoded_bytes: u64,
    generation: u64,
    speculative: bool,
}
impl ResidentBlock {
    fn metadata(&self) -> (String, u64) {
        (self.digest.clone(), self.encoded_bytes)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct OpTombstone {
    function: String,
    code_digest: String,
}
#[derive(Debug, Clone)]
struct ActiveLease {
    info: LeaseInfo,
    generations: BTreeMap<ContentKey, u64>,
}
#[derive(Debug, Clone, Copy)]
struct UsageStamp {
    last_used: Option<u64>,
    speculative: bool,
    admitted: u64,
}
#[derive(Debug)]
struct LedgerState {
    leases: BTreeMap<u64, ActiveLease>,
    retired: BTreeSet<u64>,
    usage: BTreeMap<u64, UsageStamp>,
    op_tombstones: BTreeMap<String, OpTombstone>,
    next_generation: u64,
    next_lease: u64,
    clock: u64,
    revision: u64,
}
impl LedgerState {
    fn empty() -> Self {
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
struct ResidencyLedger {
    state: Mutex<LedgerState>,
}
impl ResidencyLedger {
    fn new() -> Self {
        Self {
            state: Mutex::new(LedgerState::empty()),
        }
    }
    fn with_tombstones(tombstones: BTreeMap<String, OpTombstone>) -> Self {
        let ledger = Self::new();
        ledger
            .state
            .lock()
            .expect("residency mutex poisoned")
            .op_tombstones = tombstones;
        ledger
    }
    fn report(
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
fn unique_bytes<'a>(items: impl IntoIterator<Item = &'a (String, u64)>) -> u64 {
    let mut hashes = BTreeMap::<&str, u64>::new();
    for (digest, bytes) in items {
        hashes.entry(digest.as_str()).or_insert(*bytes);
    }
    hashes.values().copied().sum()
}
fn record_bytes(records: &BTreeMap<ContentKey, ResidentBlock>) -> u64 {
    let values: Vec<_> = records.values().map(ResidentBlock::metadata).collect();
    unique_bytes(values.iter())
}
fn key_priority(key: &ContentKey) -> u8 {
    match key {
        ContentKey::Static { .. } => 0,
        ContentKey::Catalog { .. } => 1,
        ContentKey::Code { .. } => 2,
        ContentKey::Text { .. } => 3,
    }
}
fn block_lru_key(
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
fn metadata_map(
    records: &BTreeMap<ContentKey, ResidentBlock>,
) -> BTreeMap<ContentKey, (String, u64)> {
    records
        .iter()
        .map(|(key, block)| (key.clone(), block.metadata()))
        .collect()
}
fn generation_map(records: &BTreeMap<ContentKey, ResidentBlock>) -> BTreeMap<ContentKey, u64> {
    records
        .iter()
        .map(|(key, block)| (key.clone(), block.generation))
        .collect()
}
fn op_owner_map(tombstones: &BTreeMap<String, OpTombstone>) -> BTreeMap<String, String> {
    tombstones
        .iter()
        .map(|(id, tombstone)| (id.clone(), tombstone.function.clone()))
        .collect()
}

#[derive(Debug, Clone)]
pub struct ValidatedProgram {
    program: Arc<RuntimeProgramView>,
    instructions: Arc<BTreeMap<String, Arc<FunctionInstructions>>>,
    runtime_objects: Arc<BTreeMap<ContentKey, (String, u64)>>,
    runtime_generations: Arc<BTreeMap<ContentKey, u64>>,
    op_tombstones: Arc<BTreeMap<String, OpTombstone>>,
    residency: Arc<ResidencyLedger>,
    budget: ResidencyBudget,
    projected: bool,
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
    id: u64,
    ledger: Arc<ResidencyLedger>,
    info: LeaseInfo,
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
    ledger: Arc<ResidencyLedger>,
    base_revision: u64,
    next_generation: u64,
    clock: u64,
    candidate: ValidatedProgram,
    evicted: BTreeSet<ContentKey>,
    evicted_generations: BTreeMap<ContentKey, u64>,
    admitted: BTreeMap<u64, bool>,
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
    fn commit_inner(
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
    fn new_view(
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
    fn record_snapshot(&self) -> BTreeMap<ContentKey, ResidentBlock> {
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
    fn prepare_eviction_inner(&self, keys: BTreeSet<ContentKey>) -> Result<ContentAdmission> {
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
    fn prepare_batch(
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
            if let ContentKey::Code { module } | ContentKey::Text { module, .. } = &key {
                let static_key = ContentKey::Static {
                    module: module.clone(),
                };
                if !records.contains_key(&static_key) {
                    return Err(err(
                        "E_CONTENT_MISSING",
                        module,
                        "static module package must be resident before code or text",
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
    fn materialize_records(
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
}

fn bad_runtime(at: &str, message: &str) -> Diagnostic {
    err("E_RUNTIME_ROOT", at, message)
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn validate_runtime_root(root: &RuntimeProgram) -> Result<()> {
    validate_ui_config(&root.theme, &root.player)?;
    let fail = || bad_runtime("program", "invalid runtime root index or configuration");
    if root.format != RUNTIME_FORMAT_VERSION
        || root.game_id.is_empty()
        || root.function_index.len() > 4096
        || root.text_contracts.len() > 100_000
        || root.assets.len() > 10_000
        || root.stage.width == 0
        || root.stage.height == 0
        || root.stage.width > 8192
        || root.stage.height > 8192
        || !root.locales.contains(&root.default_locale)
        || root.locales.is_empty()
        || root
            .locales
            .iter()
            .any(|locale| locale != "zh-Hans" && locale != "en")
    {
        return Err(fail());
    }
    for capability in &root.requires {
        if !CAPABILITIES.contains(&capability.as_str()) {
            return Err(err("E_CAPABILITY", "requires", capability));
        }
    }
    let entry = root.function_signature(&root.entry).ok_or_else(fail)?;
    if !entry.params.is_empty() {
        return Err(err("E_CALL", "entry", "entry requires arguments"));
    }
    if root.modules.len() > 4096 {
        return Err(err("E_LIMIT", "modules", "too many modules"));
    }
    for (id, module) in &root.modules {
        if id.is_empty()
            || module.functions.is_empty()
            || !valid_hash(&module.code)
            || !valid_hash(&module.static_content)
            || module
                .locales
                .keys()
                .any(|locale| !root.locales.contains(locale))
            || module.locales.values().any(|digest| !valid_hash(digest))
            || (!module.texts.is_empty()
                && module.locales.keys().cloned().collect::<BTreeSet<_>>() != root.locales)
        {
            return Err(err("E_MODULE", id, "invalid module root index"));
        }
        for (function, signature) in &module.functions {
            let Some(index) = root.function_index.get(function) else {
                return Err(err("E_MODULE", function, "function index missing"));
            };
            if index.module != *id
                || index.signature != *signature
                || signature.entry.is_empty()
                || signature.entry_op.is_empty()
            {
                return Err(err(
                    "E_MODULE",
                    function,
                    "function interface ownership mismatch",
                ));
            }
        }
        let text_ids: BTreeSet<_> = root
            .text_owners
            .iter()
            .filter(|(_, owner)| *owner == id)
            .map(|(text, _)| text)
            .collect();
        if text_ids != module.texts.iter().collect() {
            return Err(err("E_MODULE", id, "text ownership index mismatch"));
        }
    }
    if root.function_index.iter().any(|(id, index)| {
        root.modules
            .get(&index.module)
            .and_then(|module| module.functions.get(id))
            != Some(&index.signature)
    }) {
        return Err(err(
            "E_MODULE",
            "function_index",
            "orphan or mismatched function",
        ));
    }
    let owner_maps = [
        &root.scene_owners,
        &root.cue_owners,
        &root.choice_owners,
        &root.text_owners,
    ];
    if owner_maps
        .into_iter()
        .flat_map(|owners| owners.values())
        .any(|module| !root.modules.contains_key(module))
        || root
            .task_owners
            .values()
            .any(|module| !root.modules.contains_key(module))
        || root.text_contracts.iter().any(|(id, identity)| {
            root.text_owners.get(id) != Some(&identity.module)
                || !root.modules.contains_key(&identity.module)
                || identity.source_revision == 0
                || identity.contract_revision == 0
                || identity.meaning_revision == 0
                || !valid_hash(&identity.contract_digest)
        })
        || root
            .text_owners
            .keys()
            .any(|id| !root.text_contracts.contains_key(id))
    {
        return Err(err(
            "E_MODULE",
            "ownership",
            "invalid runtime ownership index",
        ));
    }
    let hash_map: BTreeMap<_, _> = root
        .assets
        .iter()
        .map(|(id, asset)| (id.clone(), asset.object.clone()))
        .collect();
    if root.locale_config.default_ui != "zh-Hans" && root.locale_config.default_ui != "en"
        || root.locale_config.default_text != root.default_locale
        || root
            .locale_config
            .text
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>()
            != root.locales
        || !root
            .locale_config
            .ui
            .contains_key(&root.locale_config.default_ui)
        || root.locale_config.ui.is_empty()
        || root
            .locale_config
            .ui
            .keys()
            .any(|locale| locale != "zh-Hans" && locale != "en")
    {
        return Err(err(
            "E_LOCALE_CONFIG",
            "program.locale_config",
            "invalid runtime locale set",
        ));
    }
    for (surface, plans) in [
        ("ui", &root.locale_config.ui),
        ("text", &root.locale_config.text),
    ] {
        for (locale, plan) in plans {
            if plan.fonts.is_empty()
                || plan.digest != LocaleFontPlan::digest_for(&plan.fonts, &hash_map)
                || plan.fonts.iter().collect::<BTreeSet<_>>().len() != plan.fonts.len()
                || plan
                    .fonts
                    .iter()
                    .any(|font| root.assets.get(font).map(|a| a.kind) != Some(AssetKind::Font))
            {
                return Err(err(
                    "E_FONT_PLAN",
                    &format!("{surface}.{locale}"),
                    "invalid font plan index",
                ));
            }
        }
    }
    for (catalog, digest) in &root.catalogs {
        if catalog.is_empty() || !valid_hash(digest) {
            return Err(err("E_CATALOG", catalog, "invalid catalog identity"));
        }
    }
    for (id, asset) in &root.assets {
        if id.is_empty()
            || !valid_hash(&asset.object)
            || !root.catalogs.contains_key(&asset.catalog)
        {
            return Err(err(
                "E_ASSET",
                id,
                "asset index references an unknown object or catalog",
            ));
        }
    }
    for (task, owner) in &root.task_owners {
        if task.is_empty() || owner.is_empty() {
            return Err(err("E_TASK", task, "invalid task owner"));
        }
    }
    if let Some(title) = &root.title_scene {
        if !root.scene_owners.contains_key(title) || root.title_nodes.len() > MAX_NODES {
            return Err(err("E_SCENE", title, "invalid root title scene"));
        }
        validate_scene_nodes(&root.title_nodes, title, &root.assets)?;
    } else if !root.title_nodes.is_empty() {
        return Err(err(
            "E_SCENE",
            "title_nodes",
            "title nodes without a title scene",
        ));
    }
    Ok(())
}

fn validate_object_key(
    root: &RuntimeProgram,
    key: &ContentKey,
    object: &RuntimeObject,
) -> Result<()> {
    let fail = || {
        err(
            "E_CONTENT_KEY",
            &format!("{key:?}"),
            "package does not match typed root identity",
        )
    };
    match (key, object) {
        (ContentKey::Static { module }, RuntimeObject::Static(package)) => {
            let Some(index) = root.modules.get(module) else {
                return Err(fail());
            };
            if package.format != RUNTIME_FORMAT_VERSION
                || package.module != *module
                || package
                    .scenes
                    .keys()
                    .any(|id| root.scene_owners.get(id) != Some(module))
                || package
                    .cues
                    .keys()
                    .any(|id| root.cue_owners.get(id) != Some(module))
                || package
                    .choices
                    .keys()
                    .any(|id| root.choice_owners.get(id) != Some(module))
                || package
                    .text_contracts
                    .keys()
                    .any(|id| root.text_owners.get(id) != Some(module))
                || package.scenes.len()
                    != root
                        .scene_owners
                        .values()
                        .filter(|owner| *owner == module)
                        .count()
                || package.cues.len()
                    != root
                        .cue_owners
                        .values()
                        .filter(|owner| *owner == module)
                        .count()
                || package.choices.len()
                    != root
                        .choice_owners
                        .values()
                        .filter(|owner| *owner == module)
                        .count()
                || package
                    .text_contracts
                    .keys()
                    .cloned()
                    .collect::<BTreeSet<_>>()
                    != index.texts
            {
                return Err(fail());
            }
            for cue in package.cues.values() {
                for effect in &cue.effects {
                    if root.task_owners.get(&effect.id) != Some(module) {
                        return Err(err(
                            "E_TASK",
                            &effect.id,
                            "task ownership differs from root",
                        ));
                    }
                }
            }
            Ok(())
        }
        (ContentKey::Code { module }, RuntimeObject::Code(package)) => {
            let Some(index) = root.modules.get(module) else {
                return Err(fail());
            };
            if package.format != RUNTIME_FORMAT_VERSION
                || package.module != *module
                || package.functions.len() != index.functions.len()
                || package.functions.iter().any(|(id, function)| {
                    index.functions.get(id) != Some(&FunctionSignature::from(function))
                        || root.function_module(id) != Some(module.as_str())
                })
            {
                return Err(fail());
            }
            Ok(())
        }
        (ContentKey::Text { module, locale }, RuntimeObject::Text(package)) => {
            let Some(index) = root.modules.get(module) else {
                return Err(fail());
            };
            if package.format != RUNTIME_FORMAT_VERSION
                || package.module != *module
                || package.locale != *locale
                || !root.locales.contains(locale)
                || package.texts.keys().cloned().collect::<BTreeSet<_>>() != index.texts
            {
                return Err(fail());
            }
            Ok(())
        }
        (ContentKey::Catalog { catalog }, RuntimeObject::Catalog(package)) => {
            let expected: BTreeSet<_> = root
                .assets
                .iter()
                .filter(|(_, a)| a.catalog == *catalog)
                .map(|(id, _)| id.clone())
                .collect();
            if package.format != RUNTIME_FORMAT_VERSION
                || package.catalog != *catalog
                || package.assets.keys().cloned().collect::<BTreeSet<_>>() != expected
                || package.assets.iter().any(|(id, asset)| {
                    root.assets
                        .get(id)
                        .is_none_or(|i| i.kind != asset.kind || i.object != asset.object)
                })
            {
                return Err(fail());
            }
            Ok(())
        }
        _ => Err(fail()),
    }
}

fn validate_runtime_objects(view: &RuntimeProgramView, objects: &[RuntimeObject]) -> Result<()> {
    for object in objects {
        match object {
            RuntimeObject::Static(package) => validate_static_package(view, package)?,
            RuntimeObject::Code(package) => {
                for (id, function) in &package.functions {
                    validate_runtime_function(view, id, function)?;
                }
            }
            RuntimeObject::Text(package) => {
                let contracts = &view.texts;
                for (id, document) in &package.texts {
                    let contract = contracts.get(id).ok_or_else(|| {
                        err(
                            "E_TEXT_CONTRACT",
                            id,
                            "static text contract is not resident",
                        )
                    })?;
                    if document.source_revision != contract.source_revision
                        || document.contract_revision != contract.contract_revision
                        || document.contract_digest != contract.contract_digest
                    {
                        return Err(err("E_TEXT_REVISION", id, &package.locale));
                    }
                    validate_text_spans(id, contract, &document.spans)?;
                }
            }
            RuntimeObject::Catalog(package) => {
                for (id, asset) in &package.assets {
                    if asset.bytes > MAX_INPUT_BYTES as u64 * 16
                        || asset.width > 8192
                        || asset.height > 8192
                        || (asset.kind == AssetKind::Image
                            && (asset.width == 0 || asset.height == 0))
                        || (asset.kind == AssetKind::Image
                            && asset.decoded_bytes != asset.width as u64 * asset.height as u64 * 4)
                        || (asset.kind == AssetKind::Audio
                            && (asset.duration_us.0 == 0 || asset.decoded_bytes == 0))
                        || (asset.kind == AssetKind::Font && asset.decoded_bytes == 0)
                    {
                        return Err(err("E_ASSET", id, "invalid asset descriptor"));
                    }
                }
            }
        }
    }
    Ok(())
}

fn validate_scene_nodes(
    nodes: &[Node],
    id: &str,
    assets: &BTreeMap<String, AssetIndexEntry>,
) -> Result<()> {
    if nodes.len() > MAX_NODES {
        return Err(err("E_LIMIT", id, "too many nodes"));
    }
    let mut ids = BTreeSet::new();
    for node in nodes {
        if !ids.insert(&node.id) {
            return Err(err("E_DUPLICATE", id, &node.id));
        }
        if ![
            node.x,
            node.y,
            node.width,
            node.height,
            node.scale,
            node.opacity,
        ]
        .iter()
        .chain(node.color.iter())
        .all(|v| v.is_finite())
            || node.width < 0.
            || node.height < 0.
            || node.scale < 0.
            || !(0.0..=1.0).contains(&node.opacity)
        {
            return Err(err("E_VISUAL", id, &node.id));
        }
        if node
            .clip
            .is_some_and(|clip| !clip.iter().all(|v| v.is_finite()) || clip[2] < 0. || clip[3] < 0.)
        {
            return Err(err("E_VISUAL", id, &node.id));
        }
        if let Some(asset) = &node.asset {
            if assets.get(asset).map(|a| a.kind) != Some(AssetKind::Image) {
                return Err(err("E_ASSET_TYPE", id, asset));
            }
        }
    }
    for node in nodes {
        let mut seen = BTreeSet::new();
        let mut parent = node.parent.as_ref();
        while let Some(key) = parent {
            if key == &node.id || !seen.insert(key) {
                return Err(err("E_SCENE_CYCLE", id, key));
            }
            parent = nodes
                .iter()
                .find(|candidate| &candidate.id == key)
                .ok_or_else(|| err("E_NODE", id, key))?
                .parent
                .as_ref();
        }
    }
    Ok(())
}

fn validate_static_package(view: &RuntimeProgramView, package: &ModuleStatic) -> Result<()> {
    let root = view.runtime_root().unwrap();
    if package
        .activation_recipes
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>()
        != package.cues.keys().cloned().collect::<BTreeSet<_>>()
    {
        return Err(err(
            "E_RECIPE",
            &package.module,
            "activation recipe set differs from cues",
        ));
    }
    for (id, nodes) in &package.scenes {
        validate_scene_nodes(nodes, id, &root.assets)?;
    }
    if view
        .title_scene
        .as_deref()
        .is_some_and(|title| root.scene_owners.get(title) == Some(&package.module))
        && package
            .scenes
            .get(view.title_scene.as_ref().unwrap())
            .is_some_and(|nodes| nodes.as_slice() != view.title_nodes.as_slice())
    {
        return Err(err(
            "E_SCENE",
            "title_scene",
            "root title nodes differ from static scene",
        ));
    }
    for (id, cue) in &package.cues {
        if cue.effects.is_empty() || cue.effects.len() > MAX_TASKS {
            return Err(err("E_LIMIT", id, "invalid cue size"));
        }
        let mut names = BTreeSet::new();
        let mut writers = BTreeSet::new();
        let mut stages = 0;
        let mut dialogues = 0;
        for def in &cue.effects {
            if !names.insert(&def.id) {
                return Err(err("E_DUPLICATE", id, &def.id));
            }
            match &def.effect {
                Effect::StagePresent { scene, .. } => {
                    stages += 1;
                    if root.scene_owners.get(scene) != Some(&package.module)
                        || view.scenes.get(scene).is_none()
                    {
                        return Err(err("E_SCENE", id, scene));
                    }
                }
                Effect::Dialogue { text, speaker, .. } => {
                    dialogues += 1;
                    if root.text_owners.get(text) != Some(&package.module)
                        || view.texts.get(text).is_none()
                        || (!speaker.is_empty()
                            && (root.text_owners.get(speaker) != Some(&package.module)
                                || view.texts.get(speaker).is_none()))
                    {
                        return Err(err("E_TEXT", id, text));
                    }
                }
                Effect::Audio { asset, .. } if view.asset_kind(asset) != Some(AssetKind::Audio) => {
                    return Err(err("E_ASSET_TYPE", id, asset));
                }
                Effect::Clip {
                    node, property, to, ..
                } if !to.is_finite()
                    || (*property == Property::Opacity && !(0.0..=1.0).contains(to))
                    || (*property == Property::Scale && *to < 0.)
                    || !writers.insert((node, property)) =>
                {
                    return Err(err("E_VISUAL", id, node));
                }
                Effect::Clip { node, property, .. } => {
                    writers.insert((node, property));
                }
                _ => {}
            }
        }
        if stages > 1 || dialogues > 1 {
            return Err(err("E_CUE", id, "at most one stage and dialogue per cue"));
        }
        let recipe = &package.activation_recipes[id];
        if recipe.iter().any(|asset| {
            view.asset_kind(asset)
                .is_none_or(|kind| kind == AssetKind::Font)
        }) {
            return Err(err(
                "E_RECIPE",
                id,
                "recipe references a missing or non-runtime asset",
            ));
        }
        let mut expected = BTreeSet::new();
        let mut all_scenes_known = true;
        for effect in &cue.effects {
            if let Effect::Audio { asset, .. } = &effect.effect {
                expected.insert(asset.clone());
                if !recipe.contains(asset) {
                    return Err(err(
                        "E_RECIPE",
                        id,
                        "audio asset missing from activation recipe",
                    ));
                }
            }
            if let Effect::StagePresent { scene, .. } = &effect.effect {
                let nodes = view.scenes.get(scene).map(Vec::as_slice).or_else(|| {
                    (view.title_scene.as_deref() == Some(scene))
                        .then_some(view.title_nodes.as_slice())
                });
                if let Some(nodes) = nodes {
                    for asset in nodes.iter().filter_map(|node| node.asset.as_ref()) {
                        expected.insert(asset.clone());
                    }
                    if nodes
                        .iter()
                        .filter_map(|node| node.asset.as_ref())
                        .any(|asset| !recipe.contains(asset))
                    {
                        return Err(err(
                            "E_RECIPE",
                            id,
                            "scene image missing from activation recipe",
                        ));
                    }
                } else {
                    all_scenes_known = false;
                }
            }
        }
        if (all_scenes_known && recipe != &expected)
            || (!all_scenes_known && !expected.is_subset(recipe))
        {
            return Err(err(
                "E_RECIPE",
                id,
                "activation recipe differs from cue consumers",
            ));
        }
    }
    for (id, choice) in &package.choices {
        let mut ids = BTreeSet::new();
        for option in &choice.options {
            if !ids.insert(&option.id) {
                return Err(err("E_DUPLICATE", id, &option.id));
            }
            if root.text_owners.get(&option.text) != Some(&package.module)
                || view.texts.get(&option.text).is_none()
            {
                return Err(err("E_TEXT", id, &option.text));
            }
            let vars = view
                .variables
                .iter()
                .map(|(key, value)| (key.clone(), value.ty()))
                .collect();
            for expression in [&option.visible, &option.enabled].into_iter().flatten() {
                if expr_type(expression, &vars, id)? != ValueType::Bool {
                    return Err(err("E_TYPE", id, "choice predicate must be Bool"));
                }
            }
        }
        if choice.timeout_us.is_some()
            && !choice
                .default
                .as_ref()
                .is_some_and(|default| ids.contains(default))
        {
            return Err(err(
                "E_CHOICE_DEFAULT",
                id,
                "timeout requires a valid default",
            ));
        }
    }
    for (id, contract) in &package.text_contracts {
        let Some(identity) = root.text_contracts.get(id) else {
            return Err(err("E_TEXT_CONTRACT", id, "text identity missing"));
        };
        if contract.source_revision != identity.source_revision
            || contract.contract_revision != identity.contract_revision
            || contract.meaning_revision != identity.meaning_revision
            || contract.contract_digest != identity.contract_digest
            || contract.contract_digest != text_contract_digest(contract)
            || contract
                .params
                .iter()
                .any(|(param, ty)| view.variables.get(param).map(Value::ty) != Some(*ty))
        {
            return Err(err(
                "E_TEXT_REVISION",
                id,
                "contract differs from root identity",
            ));
        }
    }
    Ok(())
}

fn validate_runtime_function(
    view: &RuntimeProgramView,
    fid: &str,
    function: &Function,
) -> Result<()> {
    if !function.blocks.contains_key(&function.entry) || function.blocks.len() > 100_000 {
        return Err(err("E_BLOCK", fid, "invalid entry/block limit"));
    }
    let mut vars: BTreeMap<_, _> = view
        .variables
        .iter()
        .map(|(key, value)| (key.clone(), value.ty()))
        .collect();
    for (key, ty) in function.params.iter().chain(function.locals.iter()) {
        if vars.insert(key.clone(), *ty).is_some() {
            return Err(err("E_DUPLICATE", fid, key));
        }
    }
    let mut op_ids = BTreeSet::new();
    for (bid, block) in &function.blocks {
        let at = format!("{fid}/{bid}");
        for target in outgoing(&block.terminator) {
            if !function.blocks.contains_key(target) {
                return Err(err("E_BLOCK", &at, target));
            }
        }
        for op in &block.ops {
            if !op_ids.insert(&op.id) {
                return Err(err("E_DUPLICATE", &at, &op.id));
            }
            match &op.operation {
                Operation::Assign { target, value }
                    if vars.get(target).copied() != Some(expr_type(value, &vars, &op.id)?) =>
                {
                    return Err(err("E_TYPE", &op.id, target));
                }
                Operation::Random { target, min, max }
                    if vars.get(target) != Some(&ValueType::I32) || min > max =>
                {
                    return Err(err("E_RANDOM", &op.id, "invalid bounds/target"));
                }
                Operation::DraftPatch { value, .. } if !value.is_finite() => {
                    return Err(err("E_VISUAL", &op.id, "non-finite patch"));
                }
                _ => {}
            }
        }
        match &block.terminator {
            Terminator::Branch { condition, .. }
                if expr_type(condition, &vars, &at)? != ValueType::Bool =>
            {
                return Err(err("E_TYPE", &at, "branch requires Bool"));
            }
            Terminator::Switch { value, cases, .. } => {
                let ty = expr_type(value, &vars, &at)?;
                if ty == ValueType::Bool
                    || (ty == ValueType::I32 && cases.keys().any(|key| key.parse::<i32>().is_err()))
                {
                    return Err(err("E_TYPE", &at, "switch requires I32 or String keys"));
                }
            }
            Terminator::Call {
                function,
                args,
                result,
                ..
            } => {
                let callee = view
                    .function_signature(function)
                    .ok_or_else(|| err("E_FUNCTION", &at, function))?;
                if args.len() != callee.params.len() {
                    return Err(err("E_CALL", &at, "argument count"));
                }
                for (key, ty) in &callee.params {
                    let expr = args.get(key).ok_or_else(|| err("E_CALL", &at, key))?;
                    if expr_type(expr, &vars, &at)? != *ty {
                        return Err(err("E_TYPE", &at, key));
                    }
                }
                if result
                    .as_ref()
                    .is_some_and(|target| vars.get(target).copied() != callee.returns)
                {
                    return Err(err("E_TYPE", &at, "return target"));
                }
            }
            Terminator::Return { value }
                if value
                    .as_ref()
                    .map(|e| expr_type(e, &vars, &at))
                    .transpose()?
                    != function.returns =>
            {
                return Err(err("E_TYPE", &at, "return type"));
            }
            Terminator::Activate { cue, .. }
                if view
                    .runtime_root()
                    .unwrap()
                    .cue_owners
                    .get(cue)
                    .map(String::as_str)
                    != view.function_module(fid)
                    || view.cues.get(cue).is_none() =>
            {
                return Err(err("E_CUE", &at, cue));
            }
            Terminator::Await { conditions, .. }
                if conditions.is_empty()
                    || conditions.iter().any(|condition| {
                        view.runtime_root()
                            .unwrap()
                            .task_owners
                            .get(&condition.task)
                            .map(String::as_str)
                            != view.function_module(fid)
                            || !view.task_definitions.contains_key(&condition.task)
                    }) =>
            {
                return Err(err(
                    "E_TASK",
                    &at,
                    "unknown, unloaded or cross-module task wait",
                ));
            }
            Terminator::Interact { choice, .. }
                if view
                    .runtime_root()
                    .unwrap()
                    .choice_owners
                    .get(choice)
                    .map(String::as_str)
                    != view.function_module(fid)
                    || view.choices.get(choice).is_none() =>
            {
                return Err(err("E_CHOICE", &at, choice));
            }
            _ => {}
        }
        if let Terminator::Await { conditions, .. } = &block.terminator {
            for condition in conditions {
                if let Some(definitions) = view.task_definitions.get(&condition.task) {
                    if condition.milestone == Milestone::Finished
                        && definitions.iter().all(|effect| {
                            matches!(effect.as_ref(), Effect::Audio { looped: true, .. })
                        })
                    {
                        return Err(err("E_INFINITE_WAIT", &at, &condition.task));
                    }
                    if let Milestone::Marker(marker) = &condition.milestone {
                        let exists = definitions.iter().any(|effect| match effect.as_ref() {
                            Effect::Dialogue { text, .. } => view
                                .texts
                                .get(text)
                                .is_some_and(|contract| contract.gates.contains(marker)),
                            _ => false,
                        });
                        if !exists {
                            return Err(err("E_MILESTONE", &at, marker));
                        }
                    }
                }
            }
        }
        if let Terminator::Interact {
            choice, branches, ..
        } = &block.terminator
        {
            if let Some(definition) = view.choices.get(choice) {
                if definition.options.len() != branches.len()
                    || definition
                        .options
                        .iter()
                        .any(|option| !branches.contains_key(&option.id))
                {
                    return Err(err("E_CHOICE", &at, "branch coverage"));
                }
            }
        }
        for expr in term_exprs(&block.terminator) {
            expr_type(expr, &vars, &at)?;
        }
    }
    let initial: BTreeSet<_> = view
        .variables
        .keys()
        .chain(function.params.keys())
        .cloned()
        .collect();
    let mut incoming = BTreeMap::from([(function.entry.clone(), initial)]);
    let mut changed = true;
    while changed {
        changed = false;
        for (bid, block) in &function.blocks {
            let Some(mut assigned) = incoming.get(bid).cloned() else {
                continue;
            };
            for op in &block.ops {
                if let Operation::Assign { target, .. } | Operation::Random { target, .. } =
                    &op.operation
                {
                    assigned.insert(target.clone());
                }
            }
            if let Terminator::Call {
                result: Some(result),
                ..
            } = &block.terminator
            {
                assigned.insert(result.clone());
            }
            for target in outgoing(&block.terminator) {
                let merged = match incoming.get(target) {
                    Some(old) => old.intersection(&assigned).cloned().collect(),
                    None => assigned.clone(),
                };
                if incoming.get(target) != Some(&merged) {
                    incoming.insert(target.to_owned(), merged);
                    changed = true;
                }
            }
        }
    }
    for (bid, block) in &function.blocks {
        if let Some(mut assigned) = incoming.get(bid).cloned() {
            for op in &block.ops {
                if let Operation::Assign { target, value } = &op.operation {
                    check_reads(value, &assigned, &op.id)?;
                    assigned.insert(target.clone());
                } else if let Operation::Random { target, .. } = &op.operation {
                    assigned.insert(target.clone());
                }
            }
            for expr in term_exprs(&block.terminator) {
                check_reads(expr, &assigned, &format!("{fid}/{bid}"))?;
            }
        }
    }
    Ok(())
}

fn err(code: &str, at: &str, message: &str) -> Diagnostic {
    Diagnostic::new(code, at, message)
}
const MAX_CONTENT_BATCH_ITEMS: usize = 128;
pub fn expr_type(e: &Expr, vars: &BTreeMap<String, ValueType>, at: &str) -> Result<ValueType> {
    use BinaryOp::*;
    match e {
        Expr::Const { value } => Ok(value.ty()),
        Expr::Var { name } => vars
            .get(name)
            .copied()
            .ok_or_else(|| err("E_VARIABLE", at, name)),
        Expr::Not { value } => {
            if expr_type(value, vars, at)? != ValueType::Bool {
                return Err(err("E_TYPE", at, "not requires Bool"));
            }
            Ok(ValueType::Bool)
        }
        Expr::Binary { op, left, right } => {
            let l = expr_type(left, vars, at)?;
            let r = expr_type(right, vars, at)?;
            let result = match op {
                Add | Sub | Mul | Div | Rem if l == ValueType::I32 && r == l => ValueType::I32,
                Lt | Le | Gt | Ge if l == ValueType::I32 && r == l => ValueType::Bool,
                Eq | Ne if l == r => ValueType::Bool,
                And | Or if l == ValueType::Bool && r == l => ValueType::Bool,
                Concat if l == ValueType::String && r == l => ValueType::String,
                _ => return Err(err("E_TYPE", at, "binary operand types disagree")),
            };
            Ok(result)
        }
    }
}
fn reads(e: &Expr, out: &mut BTreeSet<String>) {
    match e {
        Expr::Var { name } => {
            out.insert(name.clone());
        }
        Expr::Not { value } => reads(value, out),
        Expr::Binary { left, right, .. } => {
            reads(left, out);
            reads(right, out);
        }
        _ => {}
    }
}
fn check_reads(e: &Expr, assigned: &BTreeSet<String>, at: &str) -> Result<()> {
    let mut r = BTreeSet::new();
    reads(e, &mut r);
    if let Some(v) = r.difference(assigned).next() {
        return Err(err("E_UNINITIALIZED", at, v));
    }
    Ok(())
}
fn outgoing(t: &Terminator) -> Vec<&str> {
    match t {
        Terminator::Goto { target } => vec![target],
        Terminator::Branch { yes, no, .. } => vec![yes, no],
        Terminator::Switch { cases, default, .. } => cases
            .values()
            .map(String::as_str)
            .chain(std::iter::once(default.as_str()))
            .collect(),
        Terminator::Call { next, .. } | Terminator::Activate { next, .. } => vec![next],
        Terminator::Await {
            next,
            on_cancelled,
            on_failed,
            ..
        } => vec![next, on_cancelled, on_failed],
        Terminator::Interact {
            branches, on_empty, ..
        } => branches
            .values()
            .map(String::as_str)
            .chain(std::iter::once(on_empty.as_str()))
            .collect(),
        _ => vec![],
    }
}
fn term_exprs(t: &Terminator) -> Vec<&Expr> {
    match t {
        Terminator::Branch { condition, .. } => vec![condition],
        Terminator::Switch { value, .. } => vec![value],
        Terminator::Call { args, .. } => args.values().collect(),
        Terminator::Return { value } => value.iter().collect(),
        _ => vec![],
    }
}
fn validate(p: &RuntimeProgramView) -> Result<()> {
    if let Some(root) = p.runtime_root() {
        return validate_runtime_root(root);
    }
    validate_ui_config(&p.theme, &p.player)?;
    if p.format != FORMAT_VERSION {
        return Err(err("E_VERSION", "program", "unsupported semantic version"));
    }
    if p.functions.len() > 4096 || p.texts.len() > 100_000 || p.assets.len() > 10_000 {
        return Err(err("E_LIMIT", "program", "table limit"));
    }
    for cap in &p.requires {
        if !CAPABILITIES.contains(&cap.as_str()) {
            return Err(err("E_CAPABILITY", "requires", cap));
        }
    }
    if p.game_id.is_empty()
        || p.function_signature(&p.entry).is_none()
        || p.stage.width == 0
        || p.stage.height == 0
        || p.stage.width > 8192
        || p.stage.height > 8192
    {
        return Err(err("E_PROGRAM", "program", "invalid game, entry or stage"));
    }
    if !p.function_signature(&p.entry).unwrap().params.is_empty() {
        return Err(err("E_CALL", "entry", "entry requires arguments"));
    }
    if !p.locales.contains_key(&p.default_locale) {
        return Err(err("E_LOCALE", "program", "missing default locale"));
    }
    let config = &p.locale_config;
    let asset_objects: BTreeMap<_, _> = p
        .asset_index
        .iter()
        .map(|(id, asset)| (id.clone(), asset.object.clone()))
        .collect();
    if config.default_ui != "zh-Hans" && config.default_ui != "en"
        || config.default_text != p.default_locale
        || !config.ui.contains_key(&config.default_ui)
        || !config.text.contains_key(&config.default_text)
        || config.ui.is_empty()
        || config.text.keys().collect::<BTreeSet<_>>() != p.locales.keys().collect::<BTreeSet<_>>()
        || config
            .ui
            .keys()
            .any(|locale| locale != "zh-Hans" && locale != "en")
    {
        return Err(err(
            "E_LOCALE_CONFIG",
            "program.locale_config",
            "invalid defaults or supported locale sets",
        ));
    }
    for (surface, plans) in [("ui", &config.ui), ("text", &config.text)] {
        for (locale, plan) in plans {
            if plan.fonts.is_empty()
                || plan.digest != LocaleFontPlan::digest_for(&plan.fonts, &asset_objects)
                || plan.fonts.iter().collect::<BTreeSet<_>>().len() != plan.fonts.len()
            {
                return Err(err(
                    "E_FONT_PLAN",
                    locale,
                    "font plans must be nonempty, unique and have a valid digest",
                ));
            }
            for font in &plan.fonts {
                if p.assets.get(font).map(|asset| asset.kind) != Some(AssetKind::Font) {
                    return Err(err(
                        "E_FONT_PLAN",
                        &format!("{surface}.{locale}"),
                        &format!("unknown font asset {font}"),
                    ));
                }
            }
        }
    }
    let mut declared_functions = BTreeSet::new();
    let mut declared_texts = BTreeSet::new();
    if p.modules.len() > 4096 {
        return Err(err("E_LIMIT", "modules", "too many modules"));
    }
    for (id, module) in p.modules.iter() {
        if id.is_empty() || module.functions.is_empty() {
            return Err(err("E_MODULE", id, "empty module identity or interface"));
        }
        for (fid, signature) in &module.functions {
            if !declared_functions.insert(fid)
                || signature.entry.is_empty()
                || signature.entry_op.is_empty()
                || p.functions
                    .get(fid)
                    .is_some_and(|f| FunctionSignature::from(f) != *signature)
            {
                return Err(err(
                    "E_MODULE",
                    fid,
                    "duplicate or mismatched function interface",
                ));
            }
        }
        for tid in &module.texts {
            if !declared_texts.insert(tid) || !p.texts.contains_key(tid) {
                return Err(err("E_MODULE", tid, "duplicate or unknown text ownership"));
            }
        }
    }
    if !p.modules.is_empty()
        && (p
            .functions
            .keys()
            .any(|id| !declared_functions.contains(id))
            || p.texts.keys().any(|id| !declared_texts.contains(id))
            || declared_functions.len() > 4096)
    {
        return Err(err(
            "E_MODULE",
            "modules",
            "incomplete ownership or function limit",
        ));
    }
    for (locale, texts) in p.locales.iter() {
        if locale != "zh-Hans" && locale != "en" {
            return Err(err(
                "E_CAPABILITY",
                locale,
                "locale is not in the tested language profile",
            ));
        }
        for (id, c) in p.texts.iter() {
            if c.source_revision == 0
                || c.contract_revision == 0
                || c.meaning_revision == 0
                || c.contract_digest != text_contract_digest(c)
            {
                return Err(err("E_TEXT_REVISION", id, locale));
            }
            for (param, ty) in &c.params {
                if p.variables.get(param).map(Value::ty) != Some(*ty) {
                    return Err(err("E_TEXT_PARAM", id, param));
                }
            }
            let Some(d) = texts.get(id) else {
                if p.text_module(id).is_some() {
                    continue;
                }
                return Err(err("E_TRANSLATION", locale, id));
            };
            if d.source_revision != c.source_revision
                || d.contract_revision != c.contract_revision
                || d.contract_digest != c.contract_digest
            {
                return Err(err("E_TEXT_REVISION", id, locale));
            }
            validate_text_spans(id, c, &d.spans)?;
        }
        if texts.keys().any(|id| !p.texts.contains_key(id)) {
            return Err(err("E_TEXT_CONTRACT", locale, "unexpected text"));
        }
    }
    for (id, a) in p.assets.iter() {
        if a.bytes > MAX_INPUT_BYTES as u64 * 16 || a.width > 8192 || a.height > 8192 {
            return Err(err("E_LIMIT", id, "asset too large"));
        }
    }
    for (id, nodes) in p.scenes.iter() {
        let err = |code: &str, at: &str, message: &str| {
            err(code, at, message).classified(
                ErrorDomain::Content,
                "scenes",
                "validate",
                vec![Recovery::FixContent],
            )
        };
        if nodes.len() > MAX_NODES {
            return Err(err("E_LIMIT", id, "too many nodes"));
        }
        let mut ids = BTreeSet::new();
        for n in nodes {
            if !ids.insert(&n.id) {
                return Err(err("E_DUPLICATE", id, &n.id));
            }
            if ![n.x, n.y, n.width, n.height, n.scale, n.opacity]
                .iter()
                .chain(n.color.iter())
                .all(|v| v.is_finite())
                || n.width < 0.
                || n.height < 0.
                || n.scale < 0.
                || !(0.0..=1.0).contains(&n.opacity)
            {
                return Err(err("E_VISUAL", id, &n.id));
            }
            if let Some(a) = &n.asset {
                if p.assets.get(a).map(|a| a.kind) != Some(AssetKind::Image) {
                    return Err(err("E_ASSET_TYPE", id, a));
                }
            }
        }
        for n in nodes {
            let mut seen = BTreeSet::new();
            let mut parent = n.parent.as_ref();
            while let Some(k) = parent {
                if k == &n.id || !seen.insert(k) {
                    return Err(err("E_SCENE_CYCLE", id, k));
                }
                parent = nodes
                    .iter()
                    .find(|v| &v.id == k)
                    .ok_or_else(|| err("E_NODE", id, k))?
                    .parent
                    .as_ref();
            }
        }
    }
    let mut task_defs: BTreeMap<&str, Vec<&Effect>> = BTreeMap::new();
    for (id, cue) in p.cues.iter() {
        let err = |code: &str, at: &str, message: &str| {
            err(code, at, message).classified(
                ErrorDomain::Content,
                "cues",
                "validate",
                vec![Recovery::FixContent],
            )
        };
        if cue.effects.is_empty() || cue.effects.len() > MAX_TASKS {
            return Err(err("E_LIMIT", id, "invalid cue size"));
        }
        let mut names = BTreeSet::new();
        let mut writers = BTreeSet::new();
        let mut stage_count = 0;
        let mut dialogue_count = 0;
        for def in &cue.effects {
            if !names.insert(&def.id) {
                return Err(err("E_DUPLICATE", id, &def.id));
            }
            task_defs.entry(&def.id).or_default().push(&def.effect);
            match &def.effect {
                Effect::StagePresent { scene, .. } => {
                    stage_count += 1;
                    if !p.scenes.contains_key(scene) {
                        return Err(err("E_SCENE", id, scene));
                    }
                }
                Effect::Dialogue { text, speaker, .. } => {
                    dialogue_count += 1;
                    if !p.texts.contains_key(text)
                        || (!speaker.is_empty() && !p.texts.contains_key(speaker))
                    {
                        return Err(err("E_TEXT", id, text));
                    }
                }
                Effect::Audio { asset, .. }
                    if p.assets.get(asset).map(|a| a.kind) != Some(AssetKind::Audio) =>
                {
                    return Err(err("E_ASSET_TYPE", id, asset));
                }
                Effect::Clip {
                    node, property, to, ..
                } if !to.is_finite()
                    || (*property == Property::Opacity && !(0.0..=1.0).contains(to))
                    || (*property == Property::Scale && *to < 0.) =>
                {
                    return Err(err("E_VISUAL", id, node));
                }
                Effect::Clip { node, property, .. } if !writers.insert((node, property)) => {
                    return Err(err("E_OWNERSHIP", id, node));
                }
                Effect::Clip { node, property, .. } => {
                    writers.insert((node, property));
                }
                _ => {}
            }
        }
        if stage_count > 1 || dialogue_count > 1 {
            return Err(err("E_CUE", id, "at most one stage and dialogue per cue"));
        }
    }
    for (id, c) in p.choices.iter() {
        let mut ids = BTreeSet::new();
        for o in &c.options {
            if !ids.insert(&o.id) {
                return Err(err("E_DUPLICATE", id, &o.id));
            }
            if !p.texts.contains_key(&o.text) {
                return Err(err("E_TEXT", id, &o.text));
            }
            let vars = p
                .variables
                .iter()
                .map(|(k, v)| (k.clone(), v.ty()))
                .collect();
            for e in [&o.visible, &o.enabled].into_iter().flatten() {
                if expr_type(e, &vars, id)? != ValueType::Bool {
                    return Err(err("E_TYPE", id, "choice predicate must be Bool"));
                }
            }
        }
        if c.timeout_us.is_some() && !c.default.as_ref().is_some_and(|d| ids.contains(d)) {
            return Err(err(
                "E_CHOICE_DEFAULT",
                id,
                "timeout requires a valid default",
            ));
        }
    }
    let mut op_ids = BTreeSet::new();
    for (fid, f) in p.functions.iter() {
        if !f.blocks.contains_key(&f.entry) || f.blocks.len() > 100_000 {
            return Err(err("E_BLOCK", fid, "invalid entry/block limit"));
        }
        let mut vars: BTreeMap<_, _> = p
            .variables
            .iter()
            .map(|(k, v)| (k.clone(), v.ty()))
            .collect();
        for (k, v) in f.params.iter().chain(f.locals.iter()) {
            if vars.insert(k.clone(), *v).is_some() {
                return Err(err("E_DUPLICATE", fid, k));
            }
        }
        for (bid, b) in &f.blocks {
            let at = format!("{fid}/{bid}");
            for next in outgoing(&b.terminator) {
                if !f.blocks.contains_key(next) {
                    return Err(err("E_BLOCK", &at, next));
                }
            }
            for op in &b.ops {
                if !op_ids.insert(&op.id) {
                    return Err(err("E_DUPLICATE", &at, &op.id));
                }
                match &op.operation {
                    Operation::Assign { target, value }
                        if vars.get(target).copied() != Some(expr_type(value, &vars, &op.id)?) =>
                    {
                        return Err(err("E_TYPE", &op.id, target));
                    }
                    Operation::Random { target, min, max }
                        if (vars.get(target) != Some(&ValueType::I32) || min > max) =>
                    {
                        return Err(err("E_RANDOM", &op.id, "invalid bounds/target"));
                    }
                    Operation::DraftPatch { value, .. } if !value.is_finite() => {
                        return Err(err("E_VISUAL", &op.id, "non-finite patch"));
                    }
                    _ => {}
                }
            }
            match &b.terminator {
                Terminator::Branch { condition, .. }
                    if expr_type(condition, &vars, &at)? != ValueType::Bool =>
                {
                    return Err(err("E_TYPE", &at, "branch requires Bool"));
                }
                Terminator::Switch { value, cases, .. } => {
                    let ty = expr_type(value, &vars, &at)?;
                    if ty == ValueType::Bool
                        || (ty == ValueType::I32 && cases.keys().any(|s| s.parse::<i32>().is_err()))
                    {
                        return Err(err("E_TYPE", &at, "switch requires I32 or String keys"));
                    }
                }
                Terminator::Call {
                    function,
                    args,
                    result,
                    ..
                } => {
                    let callee = p
                        .function_signature(function)
                        .ok_or_else(|| err("E_FUNCTION", &at, function))?;
                    if args.len() != callee.params.len() {
                        return Err(err("E_CALL", &at, "argument count"));
                    }
                    for (k, ty) in &callee.params {
                        let e = args.get(k).ok_or_else(|| err("E_CALL", &at, k))?;
                        if expr_type(e, &vars, &at)? != *ty {
                            return Err(err("E_TYPE", &at, k));
                        }
                    }
                    if let Some(r) = result {
                        if vars.get(r).copied() != callee.returns {
                            return Err(err("E_TYPE", &at, "return target"));
                        }
                    }
                }
                Terminator::Return { value }
                    if value
                        .as_ref()
                        .map(|e| expr_type(e, &vars, &at))
                        .transpose()?
                        != f.returns =>
                {
                    return Err(err("E_TYPE", &at, "return type"));
                }
                Terminator::Activate { cue, .. } if !p.cues.contains_key(cue) => {
                    return Err(err("E_CUE", &at, cue));
                }
                Terminator::Await { conditions, .. } => {
                    if conditions.is_empty() {
                        return Err(err("E_WAIT", &at, "empty All"));
                    }
                    for c in conditions {
                        let defs = task_defs
                            .get(c.task.as_str())
                            .ok_or_else(|| err("E_TASK", &at, &c.task))?;
                        if c.milestone == Milestone::Finished
                            && defs
                                .iter()
                                .all(|e| matches!(e, Effect::Audio { looped: true, .. }))
                        {
                            return Err(err("E_INFINITE_WAIT", &at, &c.task));
                        }
                        if let Milestone::Marker(g) = &c.milestone {
                            if !defs.iter().any(|e| match e {
                                Effect::Dialogue { text, .. } => p.texts[text].gates.contains(g),
                                _ => false,
                            }) {
                                return Err(err("E_MILESTONE", &at, g));
                            }
                        }
                    }
                }
                Terminator::Interact {
                    choice, branches, ..
                } => {
                    let c = p
                        .choices
                        .get(choice)
                        .ok_or_else(|| err("E_CHOICE", &at, choice))?;
                    if c.options.len() != branches.len()
                        || c.options.iter().any(|o| !branches.contains_key(&o.id))
                    {
                        return Err(err("E_CHOICE", &at, "branch coverage"));
                    }
                }
                _ => {}
            }
        }
        // Forward must-analysis: entry facts flow until loop joins reach a fixed point.
        let initial: BTreeSet<_> = p.variables.keys().chain(f.params.keys()).cloned().collect();
        let mut incoming = BTreeMap::from([(f.entry.clone(), initial)]);
        let mut changed = true;
        while changed {
            changed = false;
            for (bid, b) in &f.blocks {
                let Some(mut assigned) = incoming.get(bid).cloned() else {
                    continue;
                };
                for op in &b.ops {
                    match &op.operation {
                        Operation::Assign { target, .. } | Operation::Random { target, .. } => {
                            assigned.insert(target.clone());
                        }
                        _ => {}
                    }
                }
                if let Terminator::Call {
                    result: Some(r), ..
                } = &b.terminator
                {
                    assigned.insert(r.clone());
                }
                for next in outgoing(&b.terminator) {
                    let merged = match incoming.get(next) {
                        Some(old) => old.intersection(&assigned).cloned().collect(),
                        None => assigned.clone(),
                    };
                    if incoming.get(next) != Some(&merged) {
                        incoming.insert(next.to_owned(), merged);
                        changed = true;
                    }
                }
            }
        }
        for (bid, b) in &f.blocks {
            if let Some(mut assigned) = incoming.get(bid).cloned() {
                for op in &b.ops {
                    match &op.operation {
                        Operation::Assign { target, value } => {
                            check_reads(value, &assigned, &op.id)?;
                            assigned.insert(target.clone());
                        }
                        Operation::Random { target, .. } => {
                            assigned.insert(target.clone());
                        }
                        _ => {}
                    }
                }
                for e in term_exprs(&b.terminator) {
                    check_reads(e, &assigned, &format!("{fid}/{bid}"))?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod runtime_tests {
    use super::*;

    fn block(terminator: Terminator, ops: Vec<Op>) -> Block {
        Block { ops, terminator }
    }
    fn simple_function() -> Function {
        Function {
            params: BTreeMap::new(),
            locals: BTreeMap::new(),
            returns: None,
            entry: "end".into(),
            blocks: BTreeMap::from([(
                "end".into(),
                block(
                    Terminator::End {
                        outcome: "done".into(),
                    },
                    vec![],
                ),
            )]),
        }
    }
    fn runtime_fixture(
        functions: BTreeMap<String, Function>,
        mut static_package: ModuleStatic,
    ) -> (ValidatedProgram, Vec<(ContentKey, RuntimeObject, u64)>) {
        static_package.format = RUNTIME_FORMAT_VERSION;
        static_package.module = "m".into();
        let signatures: BTreeMap<_, _> = functions
            .iter()
            .map(|(id, function)| (id.clone(), FunctionSignature::from(function)))
            .collect();
        let code = ModuleCode {
            format: RUNTIME_FORMAT_VERSION,
            module: "m".into(),
            functions,
        };
        let mut assets: BTreeMap<String, AssetIndexEntry> = BTreeMap::new();
        assets.insert(
            "font".into(),
            AssetIndexEntry {
                kind: AssetKind::Font,
                object: "f".repeat(64),
                catalog: "resources".into(),
            },
        );
        assets.insert(
            "image".into(),
            AssetIndexEntry {
                kind: AssetKind::Image,
                object: "1".repeat(64),
                catalog: "resources".into(),
            },
        );
        let objects: BTreeMap<_, _> = assets
            .iter()
            .map(|(id, asset)| (id.clone(), asset.object.clone()))
            .collect();
        let plan = LocaleFontPlan {
            fonts: vec!["font".into()],
            digest: LocaleFontPlan::digest_for(&["font".into()], &objects),
        };
        let locale_config = LocaleConfig {
            default_ui: "en".into(),
            default_text: "en".into(),
            ui: BTreeMap::from([("en".into(), plan.clone())]),
            text: BTreeMap::from([("en".into(), plan.clone()), ("zh-Hans".into(), plan)]),
        };
        let module_texts: BTreeSet<_> = static_package.text_contracts.keys().cloned().collect();
        let locales = if module_texts.is_empty() {
            BTreeMap::new()
        } else {
            BTreeMap::from([
                ("en".into(), "b".repeat(64)),
                ("zh-Hans".into(), "e".repeat(64)),
            ])
        };
        let module = ModuleIndex {
            functions: signatures.clone(),
            texts: module_texts,
            code: "c".repeat(64),
            static_content: "a".repeat(64),
            locales,
        };
        let function_index = signatures
            .iter()
            .map(|(id, signature)| {
                (
                    id.clone(),
                    RuntimeFunctionIndex {
                        module: "m".into(),
                        signature: signature.clone(),
                    },
                )
            })
            .collect();
        let scene_owners = static_package
            .scenes
            .keys()
            .map(|id| (id.clone(), "m".into()))
            .collect();
        let cue_owners = static_package
            .cues
            .keys()
            .map(|id| (id.clone(), "m".into()))
            .collect();
        let choice_owners = static_package
            .choices
            .keys()
            .map(|id| (id.clone(), "m".into()))
            .collect();
        let text_owners = static_package
            .text_contracts
            .keys()
            .map(|id| (id.clone(), "m".into()))
            .collect();
        let mut task_owners = BTreeMap::new();
        for cue in static_package.cues.values() {
            for effect in &cue.effects {
                task_owners.insert(effect.id.clone(), "m".into());
            }
        }
        let text_contracts = static_package
            .text_contracts
            .iter()
            .map(|(id, contract)| {
                (
                    id.clone(),
                    RuntimeTextIdentity {
                        module: "m".into(),
                        source_revision: contract.source_revision,
                        contract_revision: contract.contract_revision,
                        meaning_revision: contract.meaning_revision,
                        contract_digest: contract.contract_digest.clone(),
                    },
                )
            })
            .collect();
        let root = RuntimeProgram {
            format: RUNTIME_FORMAT_VERSION,
            game_id: "runtime-test".into(),
            revision: "r1".into(),
            entry: "m.main".into(),
            requires: vec![],
            stage: Stage {
                width: 640,
                height: 480,
            },
            variables: BTreeMap::new(),
            function_index,
            modules: BTreeMap::from([("m".into(), module)]),
            scene_owners,
            cue_owners,
            choice_owners,
            text_owners,
            task_owners,
            text_contracts,
            locales: BTreeSet::from(["en".into(), "zh-Hans".into()]),
            locale_config,
            assets,
            catalogs: BTreeMap::from([("resources".into(), "d".repeat(64))]),
            default_locale: "en".into(),
            title_scene: None,
            title_nodes: vec![],
            theme: Theme::default(),
            player: PlayerDefaults::default(),
        };
        let module = &root.modules["m"];
        assert!(
            !module.functions.is_empty(),
            "test module function index is empty"
        );
        assert!(
            valid_hash(&module.code),
            "invalid test code hash {:?}",
            module.code
        );
        assert!(
            valid_hash(&module.static_content),
            "invalid test static hash {:?}",
            module.static_content
        );
        assert!(
            module
                .locales
                .keys()
                .all(|locale| root.locales.contains(locale)),
            "invalid locale hashes/index"
        );
        let static_size = serde_json::to_vec(&static_package).unwrap().len() as u64;
        let code_size = serde_json::to_vec(&code).unwrap().len() as u64;
        let view = ValidatedProgram::from_runtime(root).unwrap();
        let batch = vec![
            (
                ContentKey::Static { module: "m".into() },
                RuntimeObject::Static(static_package),
                static_size,
            ),
            (
                ContentKey::Code { module: "m".into() },
                RuntimeObject::Code(code),
                code_size,
            ),
        ];
        (view, batch)
    }
    fn empty_static() -> ModuleStatic {
        ModuleStatic {
            format: RUNTIME_FORMAT_VERSION,
            module: "m".into(),
            scenes: BTreeMap::new(),
            cues: BTreeMap::new(),
            choices: BTreeMap::new(),
            text_contracts: BTreeMap::new(),
            activation_recipes: BTreeMap::new(),
        }
    }
    fn contract() -> TextContract {
        let mut contract = TextContract {
            source_revision: 1,
            contract_revision: 1,
            meaning_revision: 1,
            contract_digest: String::new(),
            gates: vec![],
            params: BTreeMap::new(),
        };
        contract.contract_digest = text_contract_digest(&contract);
        contract
    }
    #[test]
    fn runtime_content_installs_valid_static_and_code() {
        let (view, batch) = runtime_fixture(
            BTreeMap::from([("m.main".into(), simple_function())]),
            empty_static(),
        );
        let next = view.install_batch(batch).unwrap();
        assert!(next.is_resident(&ContentKey::Static { module: "m".into() }));
        assert!(next.is_resident(&ContentKey::Code { module: "m".into() }));
        assert!(view.program().functions.get("m.main").is_none());
        assert!(next.program().functions.get("m.main").is_some());
    }

    fn text_block(contract: &TextContract) -> RuntimeObject {
        text_block_locale("en", contract)
    }

    fn text_block_locale(locale: &str, contract: &TextContract) -> RuntimeObject {
        RuntimeObject::Text(ModuleTexts {
            format: RUNTIME_FORMAT_VERSION,
            module: "m".into(),
            locale: locale.into(),
            texts: BTreeMap::from([(
                "m.label".into(),
                TextDoc {
                    source_revision: contract.source_revision,
                    contract_revision: contract.contract_revision,
                    contract_digest: contract.contract_digest.clone(),
                    spans: vec![],
                },
            )]),
        })
    }

    fn catalog_block() -> RuntimeObject {
        let font = Asset {
            kind: AssetKind::Font,
            object: "f".repeat(64),
            bytes: 1,
            width: 0,
            height: 0,
            duration_us: Micros(0),
            decoded_bytes: 1,
        };
        let image = Asset {
            kind: AssetKind::Image,
            object: "1".repeat(64),
            bytes: 4,
            width: 1,
            height: 1,
            duration_us: Micros(0),
            decoded_bytes: 4,
        };
        RuntimeObject::Catalog(AssetCatalog {
            format: RUNTIME_FORMAT_VERSION,
            catalog: "resources".into(),
            assets: BTreeMap::from([("font".into(), font), ("image".into(), image)]),
        })
    }

    #[test]
    fn runtime_content_eviction_is_independent_and_reloadable() {
        let mut static_package = empty_static();
        let contract = contract();
        static_package
            .text_contracts
            .insert("m.label".into(), contract.clone());
        static_package.cues.insert(
            "m.cue".into(),
            Cue {
                effects: vec![EffectDef {
                    id: "m.wait".into(),
                    scope: Scope::Session,
                    effect: Effect::Delay {
                        duration_us: Micros(1),
                    },
                }],
            },
        );
        static_package
            .activation_recipes
            .insert("m.cue".into(), BTreeSet::new());
        let (base, batch) = runtime_fixture(
            BTreeMap::from([("m.main".into(), simple_function())]),
            static_package,
        );
        let static_object = batch
            .iter()
            .find_map(|(key, object, _)| {
                matches!(key, ContentKey::Static { .. }).then(|| object.clone())
            })
            .unwrap();
        let static_bytes = batch
            .iter()
            .find_map(|(key, _, bytes)| matches!(key, ContentKey::Static { .. }).then_some(*bytes))
            .unwrap();
        let mut resident = base.install_batch(batch).unwrap();
        resident = resident
            .install_batch(vec![
                (
                    ContentKey::Text {
                        module: "m".into(),
                        locale: "en".into(),
                    },
                    text_block(&contract),
                    100,
                ),
                (
                    ContentKey::Text {
                        module: "m".into(),
                        locale: "zh-Hans".into(),
                    },
                    text_block_locale("zh-Hans", &contract),
                    100,
                ),
                (
                    ContentKey::Catalog {
                        catalog: "resources".into(),
                    },
                    catalog_block(),
                    100,
                ),
            ])
            .unwrap();
        assert!(resident.program().cues.get("m.cue").is_some());
        assert!(resident.program().functions.get("m.main").is_some());
        assert!(resident.program().locales["en"].get("m.label").is_some());
        assert!(resident.program().locales["zh-Hans"]
            .get("m.label")
            .is_some());
        assert!(resident.asset("image").is_some());
        let cue = resident.program.cues.values.get("m.cue").unwrap();
        let weak_cue = Arc::downgrade(cue);

        let without_static = resident
            .evict(BTreeSet::from([ContentKey::Static { module: "m".into() }]))
            .unwrap();
        assert!(!without_static.is_resident(&ContentKey::Static { module: "m".into() }));
        assert!(without_static.is_resident(&ContentKey::Code { module: "m".into() }));
        assert!(without_static.is_resident(&ContentKey::Text {
            module: "m".into(),
            locale: "en".into(),
        }));
        assert!(without_static.is_resident(&ContentKey::Catalog {
            catalog: "resources".into(),
        }));
        assert!(without_static.program().functions.get("m.main").is_some());
        assert!(without_static.program().locales["en"]
            .get("m.label")
            .is_some());
        assert!(without_static.program().locales["zh-Hans"]
            .get("m.label")
            .is_some());
        assert!(without_static.program().texts.get("m.label").is_none());
        assert!(weak_cue.upgrade().is_some());
        drop(resident);
        assert!(weak_cue.upgrade().is_none());

        let restored_static = without_static
            .install_batch(vec![(
                ContentKey::Static { module: "m".into() },
                static_object,
                static_bytes,
            )])
            .unwrap();
        assert!(restored_static.program().cues.get("m.cue").is_some());
        assert!(restored_static.program().texts.get("m.label").is_some());
        assert!(restored_static.program().locales["en"]
            .get("m.label")
            .is_some());
        assert!(restored_static.program().locales["zh-Hans"]
            .get("m.label")
            .is_some());

        let without_english = restored_static
            .evict(BTreeSet::from([ContentKey::Text {
                module: "m".into(),
                locale: "en".into(),
            }]))
            .unwrap();
        assert!(without_english.program().texts.get("m.label").is_some());
        assert!(without_english.program().locales["en"]
            .get("m.label")
            .is_none());
        assert!(without_english.program().locales["zh-Hans"]
            .get("m.label")
            .is_some());
        assert!(without_english.program().functions.get("m.main").is_some());
        let without_chinese = without_english
            .evict(BTreeSet::from([ContentKey::Text {
                module: "m".into(),
                locale: "zh-Hans".into(),
            }]))
            .unwrap();
        assert!(without_chinese.program().locales["zh-Hans"]
            .get("m.label")
            .is_none());
        let without_catalog = without_chinese
            .evict(BTreeSet::from([ContentKey::Catalog {
                catalog: "resources".into(),
            }]))
            .unwrap();
        assert!(without_catalog.asset("image").is_none());
        assert!(without_catalog.program().functions.get("m.main").is_some());
        assert!(without_catalog.program().cues.get("m.cue").is_some());
    }

    #[test]
    fn leased_content_cannot_be_evicted_and_stale_views_cannot_repin_it() {
        let (base, batch) = runtime_fixture(
            BTreeMap::from([("m.main".into(), simple_function())]),
            empty_static(),
        );
        let resident = base.install_batch(batch).unwrap();
        let code = ContentKey::Code { module: "m".into() };
        let lease = resident
            .lease(BTreeSet::from([code.clone()]), "active frame".into())
            .unwrap();
        assert_eq!(
            resident
                .evict(BTreeSet::from([code.clone()]))
                .unwrap_err()
                .code,
            "E_CONTENT_PINNED"
        );
        assert!(resident.is_resident(&code));
        drop(lease);
        let retired = resident.evict(BTreeSet::from([code.clone()])).unwrap();
        assert!(!retired.is_resident(&code));
        assert_eq!(
            resident
                .lease(BTreeSet::from([code]), "stale view".into())
                .unwrap_err()
                .code,
            "E_CONTENT_RETIRED"
        );
    }

    #[test]
    fn speculative_blocks_are_first_lru_victims_and_admission_is_atomic() {
        let mut static_package = empty_static();
        let contract = contract();
        static_package
            .text_contracts
            .insert("m.label".into(), contract.clone());
        let (base, batch) = runtime_fixture(
            BTreeMap::from([("m.main".into(), simple_function())]),
            static_package,
        );
        let mut resident = base.install_batch(batch).unwrap();
        let static_key = ContentKey::Static { module: "m".into() };
        let code_key = ContentKey::Code { module: "m".into() };
        let catalog_key = ContentKey::Catalog {
            catalog: "resources".into(),
        };
        let prefetch = resident
            .prepare_prefetch_batch(vec![(catalog_key.clone(), catalog_block(), 100)])
            .unwrap();
        assert_eq!(resident.residency().resident_blocks, 2);
        resident = prefetch.commit().unwrap();
        let budget = resident.residency().resident_bytes;
        resident = resident
            .set_residency_budget(ResidencyBudget {
                resident_bytes: budget,
            })
            .unwrap();
        let transaction = resident
            .prepare_install_batch(vec![(
                ContentKey::Text {
                    module: "m".into(),
                    locale: "en".into(),
                },
                text_block(&contract),
                100,
            )])
            .unwrap();
        assert_eq!(
            transaction.evicted_keys(),
            &BTreeSet::from([catalog_key.clone()])
        );
        assert!(resident.is_resident(&catalog_key));
        assert!(transaction.view().is_resident(&ContentKey::Text {
            module: "m".into(),
            locale: "en".into(),
        }));
        resident = transaction.commit().unwrap();
        assert!(!resident.is_resident(&catalog_key));
        assert!(resident.is_resident(&static_key));
        assert!(resident.is_resident(&code_key));
        assert_eq!(resident.residency().resident_bytes, budget);
    }

    #[test]
    fn op_id_owner_tombstones_survive_code_eviction_and_stale_views() {
        let with_ops = |op_ids: &[&str]| Function {
            params: BTreeMap::new(),
            locals: BTreeMap::new(),
            returns: None,
            entry: "end".into(),
            blocks: BTreeMap::from([(
                "end".into(),
                block(
                    Terminator::End {
                        outcome: "done".into(),
                    },
                    op_ids
                        .iter()
                        .map(|id| Op {
                            id: (*id).into(),
                            operation: Operation::ProfileMerge { key: "seen".into() },
                        })
                        .collect(),
                ),
            )]),
        };
        let functions = BTreeMap::from([
            ("m.main".into(), with_ops(&["m.main-entry", "m.shared"])),
            ("m.other".into(), with_ops(&["m.other-entry"])),
        ]);
        let (base, batch) = runtime_fixture(functions.clone(), empty_static());
        let static_object = batch
            .iter()
            .find_map(|(key, object, _)| {
                matches!(key, ContentKey::Static { .. }).then(|| object.clone())
            })
            .unwrap();
        let static_bytes = batch
            .iter()
            .find_map(|(key, _, bytes)| matches!(key, ContentKey::Static { .. }).then_some(*bytes))
            .unwrap();
        let stale_root = base.clone();
        let resident = base.install_batch(batch).unwrap();
        let code_key = ContentKey::Code { module: "m".into() };
        let without_code = resident.evict(BTreeSet::from([code_key.clone()])).unwrap();
        assert!(!without_code.program().functions.values().any(|_| true));
        let valid_reload = functions.clone();
        let reloaded = without_code
            .install_batch(vec![(
                code_key.clone(),
                RuntimeObject::Code(ModuleCode {
                    format: RUNTIME_FORMAT_VERSION,
                    module: "m".into(),
                    functions: valid_reload,
                }),
                100,
            )])
            .unwrap();
        assert!(reloaded.program().functions.get("m.main").is_some());
        let without_code = reloaded.evict(BTreeSet::from([code_key])).unwrap();
        let conflicting = BTreeMap::from([
            ("m.main".into(), with_ops(&["m.main-entry"])),
            ("m.other".into(), with_ops(&["m.other-entry", "m.shared"])),
        ]);
        let bad = vec![
            (
                ContentKey::Static { module: "m".into() },
                static_object,
                static_bytes,
            ),
            (
                ContentKey::Code { module: "m".into() },
                RuntimeObject::Code(ModuleCode {
                    format: RUNTIME_FORMAT_VERSION,
                    module: "m".into(),
                    functions: conflicting,
                }),
                100,
            ),
        ];
        assert_eq!(
            stale_root.install_batch(bad).unwrap_err().code,
            "E_DUPLICATE"
        );
        assert!(!without_code.program().functions.values().any(|_| true));
    }

    #[test]
    fn runtime_code_rejects_unknown_task_and_invalid_dialogue_gate() {
        let mut static_package = empty_static();
        static_package.cues.insert(
            "m.cue".into(),
            Cue {
                effects: vec![EffectDef {
                    id: "m.work".into(),
                    scope: Scope::Session,
                    effect: Effect::Delay {
                        duration_us: Micros(1),
                    },
                }],
            },
        );
        static_package
            .activation_recipes
            .insert("m.cue".into(), BTreeSet::new());
        let function = Function {
            params: BTreeMap::new(),
            locals: BTreeMap::new(),
            returns: None,
            entry: "wait".into(),
            blocks: BTreeMap::from([
                (
                    "wait".into(),
                    block(
                        Terminator::Await {
                            conditions: vec![WaitCondition {
                                task: "m.work".into(),
                                milestone: Milestone::Marker("missing".into()),
                            }],
                            next: "done".into(),
                            on_cancelled: "done".into(),
                            on_failed: "done".into(),
                        },
                        vec![],
                    ),
                ),
                (
                    "done".into(),
                    block(
                        Terminator::End {
                            outcome: "done".into(),
                        },
                        vec![],
                    ),
                ),
            ]),
        };
        let (view, batch) = runtime_fixture(
            BTreeMap::from([("m.main".into(), function)]),
            static_package,
        );
        assert_eq!(view.install_batch(batch).unwrap_err().code, "E_MILESTONE");
    }

    #[test]
    fn runtime_code_rejects_interaction_branch_mismatch() {
        let mut static_package = empty_static();
        static_package
            .text_contracts
            .insert("m.label".into(), contract());
        static_package.choices.insert(
            "m.choice".into(),
            Choice {
                options: vec![ChoiceOption {
                    id: "yes".into(),
                    text: "m.label".into(),
                    visible: None,
                    enabled: None,
                }],
                timeout_us: None,
                default: None,
            },
        );
        let function = Function {
            params: BTreeMap::new(),
            locals: BTreeMap::new(),
            returns: None,
            entry: "choice".into(),
            blocks: BTreeMap::from([
                (
                    "choice".into(),
                    block(
                        Terminator::Interact {
                            choice: "m.choice".into(),
                            branches: BTreeMap::from([("no".into(), "done".into())]),
                            on_empty: "done".into(),
                        },
                        vec![],
                    ),
                ),
                (
                    "done".into(),
                    block(
                        Terminator::End {
                            outcome: "done".into(),
                        },
                        vec![],
                    ),
                ),
            ]),
        };
        let (view, batch) = runtime_fixture(
            BTreeMap::from([("m.main".into(), function)]),
            static_package,
        );
        assert_eq!(view.install_batch(batch).unwrap_err().code, "E_CHOICE");
    }

    #[test]
    fn runtime_code_rejects_duplicate_op_ids_across_functions() {
        let op = Op {
            id: "m.shared".into(),
            operation: Operation::ProfileMerge { key: "seen".into() },
        };
        let with_op = || Function {
            params: BTreeMap::new(),
            locals: BTreeMap::new(),
            returns: None,
            entry: "end".into(),
            blocks: BTreeMap::from([(
                "end".into(),
                block(
                    Terminator::End {
                        outcome: "done".into(),
                    },
                    vec![op.clone()],
                ),
            )]),
        };
        let functions =
            BTreeMap::from([("m.main".into(), with_op()), ("m.other".into(), with_op())]);
        let (view, batch) = runtime_fixture(functions, empty_static());
        assert_eq!(view.install_batch(batch).unwrap_err().code, "E_DUPLICATE");
    }

    #[test]
    fn runtime_static_rejects_invalid_clip_values_and_incomplete_recipes() {
        let clip = EffectDef {
            id: "m.clip".into(),
            scope: Scope::Scene,
            effect: Effect::Clip {
                node: "art".into(),
                property: Property::X,
                to: f32::NAN,
                duration_us: Micros(1),
                replace: false,
                easing: Easing::Linear,
                finish: FinishPolicy::CommitEnd,
                cancel: CancelPolicy::CommitCurrent,
            },
        };
        let mut clip_package = empty_static();
        clip_package.cues.insert(
            "m.cue".into(),
            Cue {
                effects: vec![clip],
            },
        );
        clip_package
            .activation_recipes
            .insert("m.cue".into(), BTreeSet::new());
        let (view, batch) = runtime_fixture(
            BTreeMap::from([("m.main".into(), simple_function())]),
            clip_package,
        );
        assert_eq!(view.install_batch(batch).unwrap_err().code, "E_VISUAL");

        let mut recipe_package = empty_static();
        recipe_package.scenes.insert(
            "m.scene".into(),
            vec![Node {
                id: "art".into(),
                parent: None,
                asset: Some("image".into()),
                x: 0.,
                y: 0.,
                width: 1.,
                height: 1.,
                scale: 1.,
                opacity: 1.,
                color: [1.; 4],
                order: 0,
                clip: None,
            }],
        );
        recipe_package.cues.insert(
            "m.cue".into(),
            Cue {
                effects: vec![EffectDef {
                    id: "m.present".into(),
                    scope: Scope::Scene,
                    effect: Effect::StagePresent {
                        scene: "m.scene".into(),
                        duration_us: Micros(0),
                    },
                }],
            },
        );
        recipe_package
            .activation_recipes
            .insert("m.cue".into(), BTreeSet::new());
        let (view, batch) = runtime_fixture(
            BTreeMap::from([("m.main".into(), simple_function())]),
            recipe_package,
        );
        assert_eq!(view.install_batch(batch).unwrap_err().code, "E_RECIPE");
    }
}
