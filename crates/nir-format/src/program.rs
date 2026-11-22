use super::*;

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Program {
    pub format: u32,
    pub game_id: String,
    pub revision: String,
    pub entry: String,
    #[serde(default)]
    pub requires: Vec<String>,
    pub stage: Stage,
    #[serde(default)]
    pub variables: BTreeMap<String, Value>,
    pub functions: BTreeMap<String, Function>,
    /// Immutable module interfaces; bodies and text bundles may be absent until prepared.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub modules: BTreeMap<String, ModuleIndex>,
    #[serde(default)]
    pub scenes: BTreeMap<String, Vec<Node>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sprite_timelines: BTreeMap<String, SpriteTimeline>,
    #[serde(default)]
    pub cues: BTreeMap<String, Cue>,
    #[serde(default)]
    pub choices: BTreeMap<String, Choice>,
    #[serde(default)]
    pub texts: BTreeMap<String, TextContract>,
    #[serde(default)]
    pub locales: BTreeMap<String, BTreeMap<String, TextDoc>>,
    /// Explicit, per-surface language and ordered font plans. This is part of
    /// the executable identity; older executables without it are rejected.
    pub locale_config: LocaleConfig,
    #[serde(default)]
    pub assets: BTreeMap<String, Asset>,
    pub default_locale: String,
    #[serde(default)]
    pub title_scene: Option<String>,
    #[serde(default)]
    pub theme: Theme,
    #[serde(default)]
    pub player: PlayerDefaults,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FunctionSignature {
    pub params: BTreeMap<String, ValueType>,
    pub returns: Option<ValueType>,
    pub entry: String,
    pub entry_op: String,
}
impl From<&Function> for FunctionSignature {
    fn from(f: &Function) -> Self {
        Self {
            params: f.params.clone(),
            returns: f.returns,
            entry: f.entry.clone(),
            entry_op: f
                .blocks
                .get(&f.entry)
                .and_then(|b| b.ops.first())
                .map(|o| o.id.clone())
                .unwrap_or_else(|| "@terminator".into()),
        }
    }
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleIndex {
    pub functions: BTreeMap<String, FunctionSignature>,
    pub texts: BTreeSet<String>,
    pub code: String,
    /// Digest of the module's `ModuleStatic` package.
    #[serde(default)]
    pub static_content: String,
    pub locales: BTreeMap<String, String>,
}

/// An addressable runtime root. Unlike the authoring `Program`, this document
/// contains identities and ownership indexes, never module bodies or locale
/// text documents.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeProgram {
    pub format: u32,
    pub game_id: String,
    pub revision: String,
    pub entry: String,
    #[serde(default)]
    pub requires: Vec<String>,
    pub stage: Stage,
    #[serde(default)]
    pub variables: BTreeMap<String, Value>,
    /// Static function interfaces and the module responsible for each body.
    pub function_index: BTreeMap<String, RuntimeFunctionIndex>,
    /// Module content hashes and per-module declarations.
    #[serde(default)]
    pub modules: BTreeMap<String, ModuleIndex>,
    /// Lightweight owner indexes for static and text objects.
    #[serde(default)]
    pub scene_owners: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub timeline_owners: BTreeMap<String, String>,
    #[serde(default)]
    pub cue_owners: BTreeMap<String, String>,
    #[serde(default)]
    pub choice_owners: BTreeMap<String, String>,
    #[serde(default)]
    pub text_owners: BTreeMap<String, String>,
    /// Stable task ID (`cue/effect`) to owning module, used to resolve restored
    /// task references without loading every cue package.
    #[serde(default)]
    pub task_owners: BTreeMap<String, String>,
    /// Text identity needed to validate frozen dialogue and request its locale
    /// bundle while the actual text remains unloaded.
    #[serde(default)]
    pub text_contracts: BTreeMap<String, RuntimeTextIdentity>,
    /// Locale IDs only. Locale documents are held in independent Text blocks.
    #[serde(default)]
    pub locales: BTreeSet<String>,
    pub locale_config: LocaleConfig,
    /// Asset ID -> kind, immutable media identity and catalog locator.
    #[serde(default)]
    pub assets: BTreeMap<String, AssetIndexEntry>,
    /// Catalog ID -> immutable catalog package digest.
    #[serde(default)]
    pub catalogs: BTreeMap<String, String>,
    pub default_locale: String,
    #[serde(default)]
    pub title_scene: Option<String>,
    /// The title scene is the sole scene body embedded in the root.
    #[serde(default)]
    pub title_nodes: Vec<Node>,
    #[serde(default)]
    pub theme: Theme,
    #[serde(default)]
    pub player: PlayerDefaults,
}
impl RuntimeProgram {
    pub fn function_signature(&self, id: &str) -> Option<&FunctionSignature> {
        self.function_index.get(id).map(|entry| &entry.signature)
    }
    pub fn function_module(&self, id: &str) -> Option<&str> {
        self.function_index
            .get(id)
            .map(|entry| entry.module.as_str())
    }
    pub fn function_execution_module(&self, id: &str) -> Option<&str> {
        self.function_index
            .get(id)
            .map(|index| index.execution_module.as_deref().unwrap_or(&index.module))
    }
    /// Validated roots give every function in a code package the same scope.
    pub fn module_execution_scope(&self, module: &str) -> Option<&str> {
        let id = self.modules.get(module)?.functions.keys().next()?;
        self.function_execution_module(id)
    }
    pub fn content_prerequisites(&self, key: &ContentKey) -> BTreeSet<ContentKey> {
        let mut keys = BTreeSet::new();
        if let ContentKey::Code { module } | ContentKey::Text { module, .. } = key {
            keys.insert(ContentKey::Static {
                module: module.clone(),
            });
            if let ContentKey::Code { .. } = key {
                if let Some(scope) = self.module_execution_scope(module) {
                    keys.insert(ContentKey::Static {
                        module: scope.into(),
                    });
                }
            }
        }
        keys
    }
    pub fn text_module(&self, id: &str) -> Option<&str> {
        self.text_owners.get(id).map(String::as_str)
    }
    pub fn content_requirement(&self, key: &ContentKey) -> Option<ContentRequirement> {
        let (digest, reason) = match key {
            ContentKey::Static { module } => (
                &self.modules.get(module)?.static_content,
                "module declarations",
            ),
            ContentKey::Code { module } => (&self.modules.get(module)?.code, "function bodies"),
            ContentKey::Text { module, locale } => (
                self.modules.get(module)?.locales.get(locale)?,
                "localized text",
            ),
            ContentKey::Catalog { catalog } => {
                (self.catalogs.get(catalog)?, "resource descriptors")
            }
        };
        Some(ContentRequirement {
            key: key.clone(),
            digest: digest.clone(),
            reason: reason.into(),
        })
    }
    pub fn asset_catalogs<'a>(
        &'a self,
        assets: impl IntoIterator<Item = &'a str>,
    ) -> BTreeSet<ContentKey> {
        assets
            .into_iter()
            .filter_map(|id| self.assets.get(id))
            .map(|asset| ContentKey::Catalog {
                catalog: asset.catalog.clone(),
            })
            .collect()
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeFunctionIndex {
    /// Physical package holding this function body.
    pub module: String,
    /// Original declaration scope for compiler-generated code packages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_module: Option<String>,
    pub signature: FunctionSignature,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeTextIdentity {
    pub module: String,
    pub source_revision: u32,
    pub contract_revision: u32,
    pub meaning_revision: u32,
    pub contract_digest: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<TextImageContract>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AssetIndexEntry {
    pub kind: AssetKind,
    pub object: String,
    pub catalog: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleStatic {
    pub format: u32,
    pub module: String,
    #[serde(default)]
    pub scenes: BTreeMap<String, Vec<Node>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sprite_timelines: BTreeMap<String, SpriteTimeline>,
    #[serde(default)]
    pub cues: BTreeMap<String, Cue>,
    #[serde(default)]
    pub choices: BTreeMap<String, Choice>,
    #[serde(default)]
    pub text_contracts: BTreeMap<String, TextContract>,
    /// Cue asset recipes contain only cue audio and images from presented
    /// scenes. Font plans are separate catalog consumers.
    #[serde(default)]
    pub activation_recipes: BTreeMap<String, BTreeSet<String>>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetCatalog {
    pub format: u32,
    pub catalog: String,
    pub assets: BTreeMap<String, Asset>,
}

/// Content identities carried by runtime requests and resident-store APIs.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContentKey {
    Static { module: String },
    Code { module: String },
    Text { module: String, locale: String },
    Catalog { catalog: String },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentRequirement {
    pub key: ContentKey,
    pub digest: String,
    pub reason: String,
}

/// Read-only lease information exposed by runtime residency reports.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseInfo {
    pub id: u64,
    pub owner: String,
    pub keys: BTreeSet<ContentKey>,
    pub resident_bytes: u64,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResidencyBudget {
    pub resident_bytes: u64,
}

/// Parsed, digest-verified content. Parsing belongs to `nir-content`; semantic
/// validation and indexing belong to `nir-core`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone)]
pub enum RuntimeObject {
    Static(ModuleStatic),
    Code(ModuleCode),
    Text(ModuleTexts),
    Catalog(AssetCatalog),
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleCode {
    pub format: u32,
    pub module: String,
    pub functions: BTreeMap<String, Function>,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleTexts {
    pub format: u32,
    pub module: String,
    pub locale: String,
    pub texts: BTreeMap<String, TextDoc>,
}
impl Program {
    pub fn function_signature(&self, id: &str) -> Option<FunctionSignature> {
        self.functions
            .get(id)
            .map(FunctionSignature::from)
            .or_else(|| {
                self.modules
                    .values()
                    .find_map(|m| m.functions.get(id).cloned())
            })
    }
    pub fn function_module(&self, id: &str) -> Option<&str> {
        self.modules
            .iter()
            .find(|(_, m)| m.functions.contains_key(id))
            .map(|(id, _)| id.as_str())
    }
    pub fn text_module(&self, id: &str) -> Option<&str> {
        self.modules
            .iter()
            .find(|(_, m)| m.texts.contains(id))
            .map(|(id, _)| id.as_str())
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LocaleFontPlan {
    pub fonts: Vec<String>,
    pub digest: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LocaleConfig {
    pub default_ui: String,
    pub default_text: String,
    pub ui: BTreeMap<String, LocaleFontPlan>,
    pub text: BTreeMap<String, LocaleFontPlan>,
}

impl LocaleFontPlan {
    pub fn new(fonts: Vec<String>) -> Self {
        let digest = Self::digest_for(&fonts, &BTreeMap::new());
        Self { fonts, digest }
    }
    pub fn digest_for(fonts: &[String], objects: &BTreeMap<String, String>) -> String {
        use sha2::{Digest, Sha256};
        let identities: Vec<_> = fonts.iter().map(|font| (font, objects.get(font))).collect();
        let bytes = serde_json::to_vec(&(1u32, &identities)).expect("font plan is serializable");
        let digest = Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        digest
    }
}

impl Default for LocaleConfig {
    fn default() -> Self {
        let ui: BTreeMap<String, LocaleFontPlan> = ["zh-Hans", "en"]
            .into_iter()
            .map(|locale| (locale.into(), LocaleFontPlan::new(vec![])))
            .collect();
        let text = ui.clone();
        Self {
            default_ui: "zh-Hans".into(),
            default_text: "zh-Hans".into(),
            ui,
            text,
        }
    }
}
