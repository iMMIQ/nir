//! Versioned, platform-independent wire contracts. Unknown semantic fields fail closed.
#![forbid(unsafe_code)]
pub mod lame;
mod menu;
mod transition;
mod tween;
pub use menu::{
    MenuCondition, MenuContent, MenuEffects, MenuElement, MenuElementProperty, MenuElementTween,
    MenuImageStates, MenuLocal, MenuMusic, MenuPreference, MenuRangeBinding, MenuSlot,
    MenuToggleBinding, MenuTransition, MenuValue, MenuValueInput, MAX_MENU_PARENTS,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
pub use transition::{MaskChannel, StageTransition, WipeDirection};
pub use tween::{interpolate, DialogueAppearance, ScalarTween};

pub const FORMAT_VERSION: u32 = 1;
/// Wire version for the indexed, lazily loaded runtime root. Source `Program`
/// documents deliberately keep `FORMAT_VERSION` so authoring formats can evolve
/// independently from the runtime package layout.
pub const RUNTIME_FORMAT_VERSION: u32 = 2;
pub const CONTENT_PACKAGE_VERSION: u32 = 2;
pub const SNAPSHOT_VERSION: u32 = 2;
pub const CAPABILITIES: &[&str] = &[
    "module.lazy.v1",
    "control.v1",
    "stage.sprite.v1",
    "stage.dissolve.v1",
    "clip.scalar.v1",
    "tween.target.v1",
    "text.structured.v1",
    "text.revisions.v1",
    "text.gate.v1",
    "choice.v1",
    "audio.buffer.v1",
    "audio.gain.v1",
    "audio.stop.v1",
    "audio.gain-tween.v1",
    "audio.loop-region.v1",
    "ui.image-menu.v1",
    "text.visibility.v1",
    "text.window-transition.v1",
    "text.voice-binding.v1",
    "text.voice-timer.v1",
    "player.hide-policy.v1",
    "player.auto-delay-policy.v1",
    "text.shadow.v1",
    "stage.wipe.v1",
    "stage.mask.v1",
    "ui.menu-elements.v1",
    "ui.menu-state.v1",
    "ui.menu-services.v1",
    "ui.menu-navigation.v1",
    "ui.menu-chrome.v1",
    "ui.menu-history-availability.v1",
    "ui.menu-reading.v1",
    "ui.menu-story.v1",
    "ui.menu-stack.v1",
    "ui.menu-text-button.v1",
    "ui.menu-storage.v1",
    "ui.menu-history.v1",
    "ui.menu-history-flow.v1",
    "ui.menu-history-voice.v1",
    "ui.menu-history-scrollbar.v1",
    "ui.menu-values.v1",
    "ui.menu-effects.v1",
    "ui.menu-transition.v1",
    "ui.menu-element-tween.v1",
    "ui.replay.v1",
    "task.compose.v1",
    "story.typed-result.v1",
    "media.webp.v1",
    "media.mp3.v1",
];
/// Device observation, separate from the deterministic Story task clock.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioPosition {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope: Option<AudioEnvelopePosition>,
    pub task: u32,
    pub position_us: Micros,
}

/// Device envelope progress belongs to a concrete AudioStop task, not a bus.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AudioEnvelopePosition {
    pub owner: u32,
    pub elapsed_us: Micros,
}

pub const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_TASKS: usize = 256;
pub const MAX_FRAMES: usize = 64;
pub const MAX_NODES: usize = 1024;

/// Host clocks and pause routes; not an additional story execution context.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum TimeDomain {
    Story,
    ForegroundUi,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Micros(pub u64);
impl TryFrom<String> for Micros {
    type Error = String;
    fn try_from(s: String) -> std::result::Result<Self, Self::Error> {
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err("E_TIME: expected unsigned decimal string".into());
        }
        s.parse()
            .map(Self)
            .map_err(|_| "E_TIME: u64 overflow".into())
    }
}
impl From<Micros> for String {
    fn from(v: Micros) -> Self {
        v.0.to_string()
    }
}
impl Micros {
    pub fn is_zero(&self) -> bool {
        self.0 == 0
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, thiserror::Error)]
#[error("{code} at {location}: {message}")]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    pub code: String,
    pub location: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Box<DiagnosticDetails>>,
}
impl Diagnostic {
    pub fn new(code: &str, location: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            location: location.into(),
            message: message.into(),
            details: None,
        }
    }
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorDomain {
    Content,
    Core,
    Prepare,
    Render,
    Storage,
    Host,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Recovery {
    Retry,
    KeepCurrent,
    Reload,
    Exit,
    FixContent,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceRef {
    pub file: String,
    pub line: usize,
    /// One-based UTF-8 byte column, matching serde_json diagnostics.
    pub column: usize,
    pub pointer: String,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticDetails {
    pub domain: ErrorDomain,
    pub operation: String,
    pub stage: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceRef>,
    #[serde(default)]
    pub references: Vec<String>,
    #[serde(default)]
    pub recovery: Vec<Recovery>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<u32>,
}
impl Diagnostic {
    pub fn classified(
        mut self,
        domain: ErrorDomain,
        operation: &str,
        stage: &str,
        recovery: Vec<Recovery>,
    ) -> Self {
        self.details = Some(Box::new(DiagnosticDetails {
            domain,
            operation: operation.into(),
            stage: stage.into(),
            source: None,
            references: vec![],
            recovery,
            hint: None,
            release: None,
            session: None,
            device: None,
            request: None,
            task: None,
        }));
        self
    }
}
pub type Result<T> = std::result::Result<T, Diagnostic>;

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ValueType {
    Bool,
    I32,
    String,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Value {
    Bool(bool),
    I32(i32),
    String(String),
}
impl Value {
    pub fn ty(&self) -> ValueType {
        match self {
            Self::Bool(_) => ValueType::Bool,
            Self::I32(_) => ValueType::I32,
            Self::String(_) => ValueType::String,
        }
    }
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Expr {
    Const {
        value: Value,
    },
    Var {
        name: String,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Not {
        value: Box<Expr>,
    },
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Concat,
}

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
    pub module: String,
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stage {
    pub width: u32,
    pub height: u32,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Function {
    #[serde(default)]
    pub params: BTreeMap<String, ValueType>,
    #[serde(default)]
    pub locals: BTreeMap<String, ValueType>,
    #[serde(default)]
    pub returns: Option<ValueType>,
    pub entry: String,
    pub blocks: BTreeMap<String, Block>,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Block {
    #[serde(default)]
    pub ops: Vec<Op>,
    pub terminator: Terminator,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Op {
    pub id: String,
    pub operation: Operation,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    DialogueVisibility {
        visible: bool,
        /// Styled reveal; absent or zero-duration commits flip instantly.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transition: Option<StageTransition>,
        #[serde(default, skip_serializing_if = "Micros::is_zero")]
        duration_us: Micros,
    },
    Assign {
        target: String,
        value: Expr,
    },
    Random {
        target: String,
        min: i32,
        max: i32,
    },
    DraftPatch {
        node: String,
        property: Property,
        value: f32,
    },
    TaskControl {
        task: String,
        action: TaskAction,
    },
    DialogueContinue {
        task: String,
    },
    DialogueVoice {
        task: String,
        voice: Option<String>,
        wait: VoiceWaitPolicy,
    },
    ProfileMerge {
        key: String,
    },
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VoiceWaitPolicy {
    /// Freeze remaining audible voice duration at the beginning of the Auto cycle.
    SampledRemaining,
    /// Reading delay starts once the bound voice is no longer running.
    AfterVoice,
    /// Reading delay and bound voice run concurrently; both must complete.
    Parallel,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskAction {
    Cancel,
    Finish,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Terminator {
    Goto {
        target: String,
    },
    Branch {
        condition: Expr,
        yes: String,
        no: String,
    },
    Switch {
        value: Expr,
        cases: BTreeMap<String, String>,
        default: String,
    },
    Call {
        function: String,
        #[serde(default)]
        args: BTreeMap<String, Expr>,
        next: String,
        #[serde(default)]
        result: Option<String>,
    },
    Return {
        #[serde(default)]
        value: Option<Expr>,
    },
    Activate {
        cue: String,
        next: String,
    },
    Await {
        conditions: Vec<WaitCondition>,
        next: String,
        on_cancelled: String,
        on_failed: String,
    },
    Interact {
        choice: String,
        branches: BTreeMap<String, String>,
        on_empty: String,
        /// Typed-result mode: the chosen option's declared value is written to
        /// this variable by the VM before the branch. The host never writes.
        #[serde(default)]
        result: Option<String>,
        /// Explicit cancel target; absent means the interaction is modal.
        #[serde(default)]
        on_cancel: Option<String>,
    },
    End {
        outcome: String,
    },
    Fault {
        code: String,
        message: String,
    },
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitCondition {
    pub task: String,
    pub milestone: Milestone,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(
    tag = "type",
    content = "id",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Milestone {
    Started,
    Finished,
    Marker(String),
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cue {
    pub effects: Vec<EffectDef>,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectDef {
    pub id: String,
    pub scope: Scope,
    pub effect: Effect,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Session,
    Frame,
    Scene,
    Interaction,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Effect {
    StagePresent {
        scene: String,
        #[serde(default, skip_serializing_if = "StageTransition::is_default")]
        transition: StageTransition,
        #[serde(default)]
        duration_us: Micros,
    },
    Clip {
        node: String,
        property: Property,
        to: f32,
        duration_us: Micros,
        #[serde(default)]
        replace: bool,
        #[serde(default)]
        easing: Easing,
        #[serde(default)]
        finish: FinishPolicy,
        #[serde(default)]
        cancel: CancelPolicy,
    },
    Tween {
        target: TweenTarget,
        to: f32,
        duration_us: Micros,
        #[serde(default)]
        replace: bool,
        #[serde(default)]
        easing: Easing,
        #[serde(default)]
        finish: FinishPolicy,
        #[serde(default)]
        cancel: CancelPolicy,
    },
    Dialogue {
        text: String,
        #[serde(default)]
        speaker: String,
        reveal_us: Micros,
    },
    Audio {
        asset: String,
        bus: AudioBus,
        #[serde(default = "unit_gain", skip_serializing_if = "is_unit_gain")]
        gain: f32,
        #[serde(default)]
        looped: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        loop_region: Option<AudioLoopRegion>,
    },
    /// A finite stop operation; completion is separate from natural playback end.
    AudioStop {
        target: String,
        duration_us: Micros,
    },
    Delay {
        duration_us: Micros,
    },
    /// Children run one after another: the next child starts only after the
    /// previous one completed, capturing the current property values at its
    /// own start. The composition is itself a task; children are tasks with
    /// the composition's scope.
    Sequence {
        children: Vec<EffectDef>,
    },
    /// Children all start together; the composition finishes when every child
    /// finished, and fails or cancels as soon as any child does.
    ParallelAll {
        children: Vec<EffectDef>,
    },
}
impl Effect {
    /// Children of a composition, for tree walks shared by validation,
    /// compilation and the runtime.
    pub fn compose_children(&self) -> Option<&[EffectDef]> {
        match self {
            Self::Sequence { children } | Self::ParallelAll { children } => Some(children),
            _ => None,
        }
    }
    /// Whether this subtree contains any composition node. Children exist
    /// only inside compositions, so the root being one is the whole answer.
    pub fn uses_compose(&self) -> bool {
        self.compose_children().is_some()
    }
    /// Total effect definitions in this subtree, including composites.
    pub fn compose_leaves(&self) -> usize {
        1 + self
            .compose_children()
            .map(|children| children.iter().map(|def| def.effect.compose_leaves()).sum())
            .unwrap_or(0)
    }
    /// Composition nesting depth; a leaf has depth 0.
    pub fn compose_depth(&self) -> usize {
        self.compose_children()
            .map(|children| {
                1 + children
                    .iter()
                    .map(|def| def.effect.compose_depth())
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0)
    }
    /// Depth-first test over this effect and every composition child; cue
    /// walkers that gate capabilities or media on effect kind must see the
    /// whole tree, not only top-level definitions.
    pub fn effect_tree_any(&self, predicate: &impl Fn(&Self) -> bool) -> bool {
        predicate(self)
            || self.compose_children().is_some_and(|children| {
                children
                    .iter()
                    .any(|def| def.effect.effect_tree_any(predicate))
            })
    }
    /// Every audio asset this effect subtree starts, compositions included.
    /// Stage present transitions are handled by the caller (scene nodes).
    pub fn collect_audio_assets(&self, out: &mut BTreeSet<String>) {
        if let Self::Audio { asset, .. } = self {
            out.insert(asset.clone());
        }
        for def in self.compose_children().unwrap_or(&[]) {
            def.effect.collect_audio_assets(out);
        }
    }
}
/// Typed property addresses. UI-owned objects are deliberately not addressable by Story.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TweenTarget {
    SceneNode {
        node: String,
        property: Property,
    },
    DialogueRoot {
        property: DialogueProperty,
    },
    /// A live audio instance's envelope, by task handle. The envelope is the
    /// 0..1 multiplier on top of the authored event gain; one envelope owner
    /// (a gain tween or a timed stop) may target an instance at a time.
    AudioInstance {
        task: String,
        property: AudioProperty,
    },
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum DialogueProperty {
    Opacity,
    BackgroundOpacity,
    TextOpacity,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum AudioProperty {
    Gain,
}
impl TweenTarget {
    pub fn accepts(&self, value: f32) -> bool {
        value.is_finite()
            && match self {
                Self::SceneNode {
                    property: Property::X | Property::Y,
                    ..
                } => true,
                Self::SceneNode {
                    property: Property::Scale,
                    ..
                } => value >= 0.,
                _ => (0.0..=1.0).contains(&value),
            }
    }
    /// The device renders one linear ramp per envelope owner, so envelope
    /// tracks must delegate linearly like timed stops do.
    pub fn requires_linear_easing(&self) -> bool {
        matches!(self, Self::AudioInstance { .. })
    }
}
impl Effect {
    /// Legacy Clip and typed Tween use one evaluator and one writer identity.
    pub fn scalar_track(&self, from: f32, base: f32) -> Option<(TweenTarget, ScalarTween, bool)> {
        let (target, to, duration_us, replace, easing, finish, cancel) = match self {
            Self::Clip {
                node,
                property,
                to,
                duration_us,
                replace,
                easing,
                finish,
                cancel,
            } => (
                TweenTarget::SceneNode {
                    node: node.clone(),
                    property: *property,
                },
                to,
                duration_us,
                replace,
                easing,
                finish,
                cancel,
            ),
            Self::Tween {
                target,
                to,
                duration_us,
                replace,
                easing,
                finish,
                cancel,
            } => (
                target.clone(),
                to,
                duration_us,
                replace,
                easing,
                finish,
                cancel,
            ),
            _ => return None,
        };
        Some((
            target,
            ScalarTween {
                from,
                base,
                to: *to,
                duration_us: *duration_us,
                easing: *easing,
                finish: *finish,
                cancel: *cancel,
            },
            *replace,
        ))
    }
}
fn unit_gain() -> f32 {
    1.0
}
fn is_unit_gain(value: &f32) -> bool {
    *value == 1.0
}
/// Play from the beginning to `end_us` once, then repeat [start_us, end_us).
/// Each backend resolves boundaries at its decoded buffer's sample rate,
/// choosing the nearest sample frame without trimming unrelated samples.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AudioLoopRegion {
    pub start_us: Micros,
    pub end_us: Micros,
}
impl AudioLoopRegion {
    pub fn valid(&self, duration_us: u64) -> bool {
        self.start_us.0 < self.end_us.0 && self.end_us.0 <= duration_us
    }
    pub fn frame_bounds(&self, rate: u32, frames: u64) -> Option<(u64, u64)> {
        if rate == 0 || self.start_us.0 >= self.end_us.0 {
            return None;
        }
        let frame = |us: u64| (us as u128 * rate as u128 + 500_000) / 1_000_000;
        let start = frame(self.start_us.0);
        let end = frame(self.end_us.0);
        (start < end && end <= frames as u128).then_some((start as u64, end as u64))
    }
    /// A cumulative device position includes the one-time intro. Wrap only
    /// after the first end boundary; restoration must not replay that intro.
    pub fn playback_frame(&self, position_us: u64, rate: u32, frames: u64) -> Option<u64> {
        let (start, end) = self.frame_bounds(rate, frames)?;
        let position = (position_us as u128 * rate as u128 + 500_000) / 1_000_000;
        Some(if position < end as u128 {
            position as u64
        } else {
            start + ((position - end as u128) % (end - start) as u128) as u64
        })
    }
}

/// Event gain is separate from the player's mixer preferences.
pub fn valid_audio_gain(value: f32) -> bool {
    value.is_finite() && (0.0..=4.0).contains(&value)
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum AudioBus {
    #[default]
    Bgm,
    Voice,
    Sfx,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Easing {
    #[default]
    Linear,
    Smooth,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishPolicy {
    #[default]
    CommitEnd,
    RemoveEffect,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelPolicy {
    #[default]
    CommitCurrent,
    SettleEnd,
    RestoreBase,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Property {
    X,
    Y,
    Scale,
    Opacity,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub id: String,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub asset: Option<String>,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default = "one")]
    pub scale: f32,
    #[serde(default = "one")]
    pub opacity: f32,
    #[serde(default = "white")]
    pub color: [f32; 4],
    #[serde(default)]
    pub order: i32,
    #[serde(default)]
    pub clip: Option<[f32; 4]>,
}
fn one() -> f32 {
    1.0
}
fn white() -> [f32; 4] {
    [1.; 4]
}
impl Node {
    pub fn get(&self, p: Property) -> f32 {
        match p {
            Property::X => self.x,
            Property::Y => self.y,
            Property::Scale => self.scale,
            Property::Opacity => self.opacity,
        }
    }
    pub fn set(&mut self, p: Property, v: f32) {
        match p {
            Property::X => self.x = v,
            Property::Y => self.y = v,
            Property::Scale => self.scale = v,
            Property::Opacity => self.opacity = v,
        }
    }
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub options: Vec<ChoiceOption>,
    #[serde(default)]
    pub timeout_us: Option<Micros>,
    #[serde(default)]
    pub default: Option<String>,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChoiceOption {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub visible: Option<Expr>,
    #[serde(default)]
    pub enabled: Option<Expr>,
    /// Typed result committed by the VM when an Interact declares `result`.
    /// Every option must carry one of the target variable's type.
    #[serde(default)]
    pub value: Option<Value>,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextContract {
    pub source_revision: u32,
    pub contract_revision: u32,
    pub meaning_revision: u32,
    pub contract_digest: String,
    #[serde(default)]
    pub gates: Vec<String>,
    #[serde(default)]
    pub params: BTreeMap<String, ValueType>,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextDoc {
    pub source_revision: u32,
    pub contract_revision: u32,
    pub contract_digest: String,
    pub spans: Vec<Span>,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Span {
    Text {
        id: String,
        text: String,
        #[serde(default)]
        emphasis: bool,
    },
    Break {
        id: String,
    },
    Param {
        id: String,
        name: String,
    },
    Gate {
        id: String,
    },
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub kind: AssetKind,
    pub object: String,
    pub bytes: u64,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default)]
    pub duration_us: Micros,
    #[serde(default)]
    pub decoded_bytes: u64,
}
impl Asset {
    /// Conservative stereo f32 payload at a platform decoder's sample rate.
    /// Authored duration is rounded down to microseconds; reserve one extra
    /// output frame for that loss of precision and resampling rounding. The
    /// source payload remains a floor, and impossible costs saturate instead
    /// of wrapping into an apparently small admission request.
    pub fn resampled_pcm_budget(&self, sample_rate: u32) -> u64 {
        let frames = (self.duration_us.0 as u128 * sample_rate as u128).div_ceil(1_000_000);
        let bytes = ((frames + 1) * 2 * 4).min(u64::MAX as u128) as u64;
        self.decoded_bytes.max(bytes)
    }
}
#[cfg(test)]
mod audio_budget_tests {
    use super::*;
    fn asset(duration: u64, source_bytes: u64) -> Asset {
        Asset {
            kind: AssetKind::Audio,
            object: String::new(),
            bytes: 0,
            width: 0,
            height: 0,
            duration_us: Micros(duration),
            decoded_bytes: source_bytes,
        }
    }
    #[test]
    fn context_rate_stereo_budget_includes_resampling_and_rounding() {
        assert_eq!(
            asset(8_000_000, 768_000).resampled_pcm_budget(48_000),
            3_072_008
        );
        assert_eq!(asset(1, 4).resampled_pcm_budget(48_000), 16);
        assert_eq!(asset(22, 4).resampled_pcm_budget(48_000), 24);
    }
    #[test]
    fn source_payload_remains_a_floor_and_overflow_cannot_wrap() {
        assert_eq!(
            asset(1_000_000, 768_000).resampled_pcm_budget(24_000),
            768_000
        );
        assert_eq!(asset(u64::MAX, 4).resampled_pcm_budget(u32::MAX), u64::MAX);
    }
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Image,
    Audio,
    Font,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub menu_overlay: Option<String>,
    pub background: [f32; 4],
    pub panel: [f32; 4],
    pub accent: [f32; 4],
    pub text: [f32; 4],
    pub muted: [f32; 4],
    #[serde(default)]
    pub slots: ThemeSlots,
    #[serde(default)]
    pub dialogue: DialogueProps,
    #[serde(default)]
    pub choice: ChoiceProps,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub image_menus: BTreeMap<String, ImageMenu>,
    #[serde(default)]
    pub return_to_title: bool,
}
impl Default for Theme {
    fn default() -> Self {
        Self {
            background: [0.04, 0.075, 0.09, 1.],
            panel: [0.06, 0.105, 0.12, 0.97],
            accent: [0.83, 0.73, 0.48, 1.],
            text: [0.93, 0.94, 0.88, 1.],
            muted: [0.58, 0.69, 0.69, 1.],
            slots: ThemeSlots::default(),
            dialogue: DialogueProps::default(),
            choice: ChoiceProps::default(),
            menu_overlay: None,
            image_menus: BTreeMap::new(),
            return_to_title: false,
        }
    }
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageMenu {
    /// Authors can reserve the complete page for source controls. Shared
    /// Escape/right-click and preparation-failure exits remain available.
    #[serde(
        default = "builtin_navigation_default",
        skip_serializing_if = "is_builtin_navigation_default"
    )]
    pub builtin_navigation: bool,
    /// Explicit read-only aliases of bounded story scalars. Never UI write targets.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub story_exports: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub locals: BTreeMap<String, MenuLocal>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub elements: Vec<MenuElement>,
    pub background: String,
    pub buttons: Vec<ImageButton>,
    /// Finite page presentation effects; absent keeps legacy behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effects: Option<MenuEffects>,
}
fn builtin_navigation_default() -> bool {
    true
}
fn is_builtin_navigation_default(value: &bool) -> bool {
    *value
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageButton {
    pub id: String,
    pub label: String,
    pub asset: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hover_asset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked_asset: Option<String>,
    pub rect: [f32; 4],
    pub action: ImageMenuAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<String>,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ImageMenuAction {
    PushMenu {
        menu: String,
    },
    Back,
    Reading {
        mode: MenuReadingMode,
    },
    HistoryPage {
        window: String,
        delta: i32,
    },
    SaveSlot {
        slot: MenuSlot,
    },
    LoadSlot {
        slot: MenuSlot,
    },
    Close,
    AdjustPreference {
        field: MenuPreference,
        delta: f32,
    },
    ToggleReducedMotion,
    SetLocal {
        local: String,
        value: MenuValue,
    },
    NewGame,
    Saves,
    Settings,
    Title,
    Menu {
        menu: String,
    },
    Entry {
        function: String,
    },
    /// Isolated replay: freeze the current session, run the function as a
    /// temporary one, and return to the frozen session afterwards. Unlike
    /// Entry this never destroys the launching session.
    Replay {
        function: String,
    },
    /// Manual return from an active replay to the frozen session. Only
    /// enabled while a replay session is live.
    ExitReplay,
}
impl ImageMenuAction {
    pub fn ui_action(&self) -> Option<UiAction> {
        Some(match self {
            Self::PushMenu { .. }
            | Self::Back
            | Self::SetLocal { .. }
            | Self::Reading { .. }
            | Self::SaveSlot { .. }
            | Self::LoadSlot { .. }
            | Self::HistoryPage { .. } => return None,
            Self::Close => UiAction::Close,
            Self::AdjustPreference { field, delta } => field.adjust(*delta),
            Self::ToggleReducedMotion => UiAction::ReducedMotion,
            Self::NewGame => UiAction::NewGame,
            Self::Saves => UiAction::Saves,
            Self::Settings => UiAction::Settings,
            Self::Title => UiAction::Title,
            Self::Menu { menu } => UiAction::ImageMenu { menu: menu.clone() },
            Self::Entry { function } => UiAction::ImageMenuEntry {
                function: function.clone(),
            },
            Self::Replay { function } => UiAction::ImageMenuReplay {
                function: function.clone(),
            },
            Self::ExitReplay => UiAction::ExitReplay,
        })
    }
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MenuReadingMode {
    Auto,
    SkipRead,
    PeekStory,
}
impl Theme {
    pub fn image_assets(&self) -> BTreeSet<String> {
        let mut assets = BTreeSet::new();
        if let Some(asset) = &self.dialogue.background {
            assets.insert(asset.clone());
        }
        for menu in self.image_menus.values() {
            assets.extend(menu.image_assets());
        }
        assets
    }
}
/// Closed component registry implemented by the locked player SDK. Components
/// only project state; their actions and focus semantics remain player-owned.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeSlots {
    #[serde(default, rename = "dialogue.main")]
    pub dialogue: DialogueComponent,
    #[serde(default, rename = "choice.main")]
    pub choice: ChoiceComponent,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DialogueComponent {
    #[default]
    #[serde(rename = "builtin.dialogue")]
    Bottom,
    #[serde(rename = "builtin.dialogue.top")]
    Top,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChoiceComponent {
    #[default]
    #[serde(rename = "builtin.choice")]
    Standard,
    #[serde(rename = "builtin.choice.compact")]
    Compact,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextShadow {
    pub offset: [f32; 2],
    pub color: [f32; 4],
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DialogueProps {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadow: Option<TextShadow>,
    pub height: f32,
    pub padding: f32,
    pub font_size: f32,
    pub line_height: f32,
    pub opacity: f32,
    pub background: Option<String>,
    pub rect: Option<[f32; 4]>,
}
impl Default for DialogueProps {
    fn default() -> Self {
        Self {
            shadow: None,
            height: 220.,
            padding: 24.,
            font_size: 23.,
            line_height: 1.5,
            opacity: 1.,
            background: None,
            rect: None,
        }
    }
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ChoiceProps {
    pub width: f32,
    pub item_height: f32,
}
impl Default for ChoiceProps {
    fn default() -> Self {
        Self {
            width: 520.,
            item_height: 58.,
        }
    }
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HidePolicy {
    #[default]
    ContinueStory,
    PauseStory,
}
fn default_hide_policy(value: &HidePolicy) -> bool {
    *value == HidePolicy::ContinueStory
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoDelayPolicy {
    #[default]
    LengthScaled,
    Fixed,
}
fn default_auto_delay_policy(value: &AutoDelayPolicy) -> bool {
    *value == AutoDelayPolicy::LengthScaled
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PlayerDefaults {
    #[serde(skip_serializing_if = "default_auto_delay_policy")]
    pub auto_delay_policy: AutoDelayPolicy,
    #[serde(skip_serializing_if = "default_hide_policy")]
    pub hide_policy: HidePolicy,
    pub font_scale: f32,
    pub bgm_volume: f32,
    pub voice_volume: f32,
    pub sfx_volume: f32,
    pub reduced_motion: bool,
    pub auto_delay_us: Micros,
    pub prefetch_content: bool,
    pub prefetch_media: bool,
}
impl Default for PlayerDefaults {
    fn default() -> Self {
        Self {
            auto_delay_policy: AutoDelayPolicy::LengthScaled,
            hide_policy: HidePolicy::ContinueStory,
            font_scale: 1.,
            bgm_volume: 0.3,
            voice_volume: 0.8,
            sfx_volume: 0.5,
            reduced_motion: false,
            auto_delay_us: Micros(1_200_000),
            prefetch_content: true,
            prefetch_media: false,
        }
    }
}
impl PlayerDefaults {
    pub fn auto_delay(&self, character_count: usize, scale: f32) -> u64 {
        let extra = if self.auto_delay_policy == AutoDelayPolicy::LengthScaled {
            (character_count as u64).saturating_mul(20_000)
        } else {
            0
        };
        let base = self.auto_delay_us.0.saturating_add(extra);
        (base as f64 * scale as f64).round() as u64
    }
    pub fn preferences(&self, ui_locale: String, text_locale: String) -> Preferences {
        Preferences {
            ui_locale,
            text_locale,
            font_scale: self.font_scale,
            text_speed: 1.,
            auto_wait_scale: 1.,
            auto_wait_voice: true,
            voice_continue: true,
            character_voices: BTreeMap::new(),
            bgm_volume: self.bgm_volume,
            voice_volume: self.voice_volume,
            sfx_volume: self.sfx_volume,
            reduced_motion: self.reduced_motion,
        }
    }
}
/// Also enforced at runtime: a hand-written executable cannot bypass author checks.
pub fn validate_ui_config(theme: &Theme, player: &PlayerDefaults) -> Result<()> {
    let range = |value: f32, lo: f32, hi: f32| value.is_finite() && (lo..=hi).contains(&value);
    for (name, color) in [
        ("background", theme.background),
        ("panel", theme.panel),
        ("accent", theme.accent),
        ("text", theme.text),
        ("muted", theme.muted),
    ] {
        if !color.iter().all(|v| range(*v, 0., 1.)) || color[3] < 0.5 {
            return Err(Diagnostic::new(
                "E_THEME_PROPS",
                format!("theme.{name}"),
                "RGBA must be finite in 0..1; alpha must be at least 0.5",
            ));
        }
    }
    let rect_ok = |r: &[f32; 4]| {
        r.iter().all(|v| v.is_finite() && v.abs() <= 8192.) && r[2] > 0. && r[3] > 0.
    };
    if theme.dialogue.rect.as_ref().is_some_and(|r| !rect_ok(r))
        || theme.image_menus.len() > 64
        || (!theme.image_menus.is_empty()
            && !theme.image_menus.contains_key("title")
            && theme.menu_overlay.is_none())
    {
        return Err(Diagnostic::new(
            "E_THEME_PROPS",
            "theme",
            "invalid image menu or dialogue rectangle",
        ));
    }
    if theme
        .menu_overlay
        .as_ref()
        .is_some_and(|id| !theme.image_menus.contains_key(id))
    {
        return Err(Diagnostic::new(
            "E_VIEW",
            "theme.menu_overlay",
            "unknown overlay menu",
        ));
    }
    for (id, menu) in &theme.image_menus {
        menu.validate_elements()?;
        if menu.controls().any(|(_,action,_)|matches!(action,ImageMenuAction::Menu {menu} | ImageMenuAction::PushMenu {menu} if !theme.image_menus.contains_key(menu))) {return Err(Diagnostic::new("E_VIEW","theme.image_menus","unknown menu target"));}
        let mut ids = BTreeSet::new();
        if id.is_empty()
            || (menu.uses_navigation() && id.len() > 128)
            || menu.background.is_empty()
            || menu.buttons.len() > 256
        {
            return Err(Diagnostic::new(
                "E_THEME_PROPS",
                "theme.image_menus",
                "invalid menu",
            ));
        }
        for button in &menu.buttons {
            if button.id.is_empty()
                || !ids.insert(&button.id)
                || button.label.is_empty()
                || button.asset.is_empty()
                || !rect_ok(&button.rect)
                || matches!(&button.action, ImageMenuAction::Menu { menu } | ImageMenuAction::PushMenu {menu} if !theme.image_menus.contains_key(menu))
            {
                return Err(Diagnostic::new(
                    "E_THEME_PROPS",
                    "theme.image_menus",
                    "invalid button or menu target",
                ));
            }
        }
    }
    // System overlays cannot launch an unisolated title-only replay entry.
    if let Some(root) = &theme.menu_overlay {
        let mut pending = vec![root.as_str()];
        let mut visited = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if !visited.insert(id) {
                continue;
            }
            for (_, action, _) in theme.image_menus[id].controls() {
                match action {
                    ImageMenuAction::Entry { .. } => return Err(Diagnostic::new(
                        "E_VIEW_SERVICE",
                        "theme.menu_overlay",
                        "entry requires the title context; isolated replay is not available here",
                    )),
                    ImageMenuAction::Menu { menu } | ImageMenuAction::PushMenu { menu } => {
                        pending.push(menu.as_str())
                    }
                    _ => {}
                }
            }
        }
    }
    let luminance = |c: [f32; 4]| {
        let channel = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        channel(c[0]) * 0.2126 + channel(c[1]) * 0.7152 + channel(c[2]) * 0.0722
    };
    for (name, foreground, background) in [
        ("text/panel", theme.text, theme.panel),
        ("accent/background", theme.accent, theme.background),
    ] {
        let a = luminance(foreground);
        let b = luminance(background);
        if (a.max(b) + 0.05) / (a.min(b) + 0.05) < 4.5 {
            return Err(Diagnostic::new(
                "E_THEME_CONTRAST",
                format!("theme.{name}"),
                "base color contrast must be at least 4.5:1",
            ));
        }
    }
    for (name, value, lo, hi) in [
        (
            "dialogue.height",
            theme.dialogue.height,
            if theme.dialogue.rect.is_some() {
                32.
            } else {
                220.
            },
            if theme.dialogue.rect.is_some() {
                8192.
            } else {
                320.
            },
        ),
        (
            "dialogue.padding",
            theme.dialogue.padding,
            if theme.dialogue.rect.is_some() {
                0.
            } else {
                12.
            },
            32.,
        ),
        ("dialogue.line_height", theme.dialogue.line_height, 1., 2.),
        ("dialogue.opacity", theme.dialogue.opacity, 0., 1.),
        (
            "dialogue.font_size",
            theme.dialogue.font_size,
            18.,
            if theme.dialogue.rect.is_some() {
                64.
            } else {
                28.
            },
        ),
        ("choice.width", theme.choice.width, 360., 680.),
        ("choice.item_height", theme.choice.item_height, 48., 72.),
    ] {
        if !range(value, lo, hi) {
            return Err(Diagnostic::new(
                "E_THEME_PROPS",
                format!("theme.{name}"),
                format!("expected {lo}..{hi}"),
            ));
        }
    }
    if let Some(shadow) = theme.dialogue.shadow {
        if shadow.offset.iter().any(|v| !range(*v, -16., 16.))
            || shadow.color.iter().any(|v| !range(*v, 0., 1.))
        {
            return Err(Diagnostic::new(
                "E_THEME_PROPS",
                "theme.dialogue.shadow",
                "expected finite offset -16..16 and RGBA 0..1",
            ));
        }
    }
    for (name, value, lo, hi) in [
        ("font_scale", player.font_scale, 0.8, 1.5),
        ("bgm_volume", player.bgm_volume, 0., 1.),
        ("voice_volume", player.voice_volume, 0., 1.),
        ("sfx_volume", player.sfx_volume, 0., 1.),
    ] {
        if !range(value, lo, hi) {
            return Err(Diagnostic::new(
                "E_PLAYER_CONFIG",
                format!("player.{name}"),
                format!("expected {lo}..{hi}"),
            ));
        }
    }
    let minimum = if player.auto_delay_policy == AutoDelayPolicy::Fixed {
        0
    } else {
        100_000
    };
    if !(minimum..=30_000_000).contains(&player.auto_delay_us.0) {
        return Err(Diagnostic::new(
            "E_PLAYER_CONFIG",
            "player.auto_delay_us",
            format!("expected {minimum}..30000000 microseconds"),
        ));
    }
    Ok(())
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Executable {
    pub format: u32,
    pub program: Program,
    pub addresses: Vec<Address>,
    pub resume_map: BTreeMap<String, u32>,
    pub semantic_cost_map: Vec<u32>,
    pub activation_recipes: BTreeMap<String, BTreeSet<String>>,
}

/// Runtime wire artifact. The source-facing `Executable` stays format 1 for
/// compiler validation and compatibility; release packaging emits this small
/// format 2 root and its separately addressed content blocks.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeExecutable {
    pub format: u32,
    pub program: RuntimeProgram,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Address {
    pub function: String,
    pub block: String,
    pub op: usize,
    pub stable_id: String,
}
impl Address {
    pub fn key(&self) -> String {
        format!("{}/{}/{}", self.function, self.block, self.stable_id)
    }
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseManifest {
    pub format: u32,
    pub profile: String,
    pub game_id: String,
    pub title: String,
    pub version: String,
    pub engine_build: String,
    pub program: String,
    pub objects: BTreeMap<String, Object>,
    pub engine: EngineFiles,
    pub launch: LaunchFiles,
    #[serde(default)]
    pub notices: Vec<String>,
}
/// Native desktop content graph. Its serialized digest is the save identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeRelease {
    pub format: u32,
    pub game_id: String,
    pub title: String,
    pub version: String,
    pub profile: String,
    pub engine_build: String,
    pub player: String,
    pub program: String,
    pub objects: BTreeMap<String, Object>,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchFiles {
    pub html: String,
    pub bootstrap: String,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineFiles {
    pub js: String,
    pub wasm: String,
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_worker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_worker: Option<String>,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Object {
    pub path: String,
    pub bytes: u64,
    pub media_type: String,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub ui_locale: String,
    pub text_locale: String,
    pub font_scale: f32,
    #[serde(default = "one", skip_serializing_if = "is_unit_gain")]
    pub text_speed: f32,
    #[serde(default = "one", skip_serializing_if = "is_unit_gain")]
    pub auto_wait_scale: f32,
    #[serde(default = "wait_voice_by_default")]
    pub auto_wait_voice: bool,
    /// Allow authored voice lifetimes across a completed reading interaction.
    #[serde(default = "wait_voice_by_default")]
    pub voice_continue: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub character_voices: BTreeMap<String, CharacterVoicePreference>,
    pub bgm_volume: f32,
    pub voice_volume: f32,
    pub sfx_volume: f32,
    pub reduced_motion: bool,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            ui_locale: "zh-Hans".into(),
            text_locale: "zh-Hans".into(),
            font_scale: 1.,
            text_speed: 1.,
            auto_wait_scale: 1.,
            auto_wait_voice: true,
            voice_continue: true,
            character_voices: BTreeMap::new(),
            bgm_volume: 0.3,
            voice_volume: 0.8,
            sfx_volume: 0.5,
            reduced_motion: false,
        }
    }
}
fn wait_voice_by_default() -> bool {
    true
}

pub const MAX_CHARACTER_VOICES: usize = 128;
pub const MAX_CHARACTER_ID_BYTES: usize = 256;
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CharacterVoicePreference {
    pub volume: f32,
    pub muted: bool,
}
impl Default for CharacterVoicePreference {
    fn default() -> Self {
        Self {
            volume: 1.,
            muted: false,
        }
    }
}
impl Preferences {
    pub fn character_voice_gain(&self, character: &str) -> f32 {
        self.character_voices.get(character).map_or(1., |voice| {
            if voice.muted {
                0.
            } else if voice.volume.is_finite() {
                voice.volume.clamp(0., 1.)
            } else {
                1.
            }
        })
    }
    pub fn normalize_character_voices(&mut self) {
        self.character_voices.retain(|id, value| {
            !id.is_empty() && id.len() <= MAX_CHARACTER_ID_BYTES && value.volume.is_finite()
        });
        for value in self.character_voices.values_mut() {
            value.volume = value.volume.clamp(0., 1.);
        }
        while self.character_voices.len() > MAX_CHARACTER_VOICES {
            self.character_voices.pop_last();
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum UiAction {
    MenuHistoryVoice {
        instance: u32,
        revision: u32,
        window: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        layout: Option<u32>,
        entry: usize,
        #[serde(default)]
        stop: bool,
    },
    MenuHistoryScroll {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        control: Option<String>,
        instance: u32,
        revision: u32,
        window: String,
        layout: u32,
        input: HistoryScrollInput,
    },
    MenuValue {
        instance: u32,
        revision: u32,
        control: String,
        value: MenuValueInput,
    },
    ConfirmSave {
        token: u32,
    },
    CancelSave {
        token: u32,
    },
    MenuControl {
        instance: u32,
        revision: u32,
        control: String,
    },
    NewGame,
    ImageMenu {
        menu: String,
    },
    ImageMenuEntry {
        function: String,
    },
    ImageMenuReplay {
        function: String,
    },
    ExitReplay,
    HoverImage {
        id: Option<String>,
    },
    Continue,
    Advance,
    Choose {
        option: String,
    },
    /// Move the semantic selection cursor of the pending typed-result
    /// interaction. An observation: no input identity, no story progress.
    SelectChoice {
        option: String,
    },
    /// Cancel the pending typed-result interaction through its declared
    /// cancel target. Hosts only offer this while a cancel affordance exists.
    CancelChoice,
    Menu,
    Close,
    Settings,
    History,
    Saves,
    Save {
        slot: u32,
    },
    Load {
        slot: u32,
    },
    Rollback,
    Title,
    ToggleAuto,
    ToggleSkip,
    ToggleInterface,
    RestoreInterface,
    HoldSkip {
        pressed: bool,
    },
    UiLocale {
        locale: String,
    },
    TextLocale {
        locale: String,
    },
    LocaleRetry,
    LocaleCancel,
    FontSize {
        delta: f32,
    },
    TextSpeed {
        delta: f32,
    },
    AutoWait {
        delta: f32,
    },
    AutoWaitVoice {
        enabled: bool,
    },
    VoiceContinue {
        enabled: bool,
    },
    CharacterVolume {
        character: String,
        delta: f32,
    },
    CharacterMute {
        character: String,
        muted: bool,
    },
    Volume {
        bus: AudioBus,
        delta: f32,
    },
    ReducedMotion,
    HistoryPage {
        delta: i32,
    },
    Scroll {
        region: ScrollRegion,
        delta: i32,
    },
    Export,
    Import,
    Retry,
    HistoryVoice {
        entry: usize,
    },
    StopHistoryVoice,
}

impl UiAction {
    /// Only focus retention may ignore a menu model revision. Dispatch must not.
    pub fn same_pointer_target(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::MenuValue {
                    instance: a,
                    revision: b,
                    control: c,
                    ..
                },
                Self::MenuValue {
                    instance: d,
                    revision: e,
                    control: f,
                    ..
                },
            ) => a == d && b == e && c == f,
            _ => self == other,
        }
    }
    pub fn same_focus_target(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::MenuHistoryVoice {
                    instance: a,
                    window: b,
                    entry: c,
                    ..
                },
                Self::MenuHistoryVoice {
                    instance: d,
                    window: e,
                    entry: f,
                    ..
                },
            ) => a == d && b == e && c == f,
            (
                Self::MenuHistoryScroll {
                    instance: a,
                    window: wa,
                    control: Some(b),
                    input: c,
                    ..
                },
                Self::MenuHistoryScroll {
                    instance: d,
                    window: wd,
                    control: Some(e),
                    input: f,
                    ..
                },
            ) => {
                a == d
                    && wa == wd
                    && b == e
                    && match (c, f) {
                        (
                            HistoryScrollInput::Position { .. },
                            HistoryScrollInput::Position { .. },
                        ) => true,
                        (
                            HistoryScrollInput::Line { delta: x },
                            HistoryScrollInput::Line { delta: y },
                        ) => x == y,
                        _ => false,
                    }
            }
            (
                Self::MenuValue {
                    instance: a,
                    control: b,
                    ..
                },
                Self::MenuValue {
                    instance: c,
                    control: d,
                    ..
                },
            ) => a == c && b == d,
            (
                Self::MenuControl {
                    instance: a,
                    control: b,
                    ..
                },
                Self::MenuControl {
                    instance: c,
                    control: d,
                    ..
                },
            ) => a == c && b == d,
            _ => self == other,
        }
    }
}

/// View-local history navigation; never a story or saved-state mutation.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum HistoryScrollInput {
    Line { delta: i32 },
    Step { delta: i32 },
    Page { delta: i32 },
    Position { ratio: f32 },
}

/// Viewport navigation, never a VM instruction or a snapshot cursor.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollRegion {
    Menu,
    Saves,
    Settings,
    Dialogue,
    Choices,
    History,
}

/// Canonical semantic text contract identity. Source copy edits do not change it.
pub fn text_contract_digest(c: &TextContract) -> String {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(&(
        1u32,
        c.contract_revision,
        c.meaning_revision,
        &c.params,
        &c.gates,
    ))
    .expect("text contract serialization");
    format!("{:x}", Sha256::digest(bytes))
}
/// Shape rules shared by author checks and the runtime loader.
pub fn validate_text_spans(id: &str, c: &TextContract, spans: &[Span]) -> Result<()> {
    let mut ids = BTreeSet::new();
    let mut gates = Vec::new();
    let mut params = BTreeSet::new();
    for span in spans {
        let sid = match span {
            Span::Text { id, text, .. } => {
                if text.len() > 128 * 1024 {
                    return Err(Diagnostic::new("E_LIMIT", id, "text too long"));
                }
                id
            }
            Span::Break { id } => id,
            Span::Gate { id } => {
                gates.push(id.clone());
                id
            }
            Span::Param { id, name } => {
                if !c.params.contains_key(name) {
                    return Err(Diagnostic::new("E_TEXT_PARAM", id, name));
                }
                params.insert(name.clone());
                id
            }
        };
        if sid.is_empty() || !ids.insert(sid) {
            return Err(Diagnostic::new(
                "E_DUPLICATE",
                id,
                "empty or duplicate span identity",
            ));
        }
    }
    if gates != c.gates {
        return Err(Diagnostic::new("E_GATE", id, "gate order/count mismatch"));
    }
    if params != c.params.keys().cloned().collect() {
        return Err(Diagnostic::new(
            "E_TEXT_PARAM",
            id,
            "parameter coverage mismatch",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// text.window-transition.v1: the styled reveal fields are optional and
    /// omit cleanly; the legacy two-field op still deserializes unchanged.
    #[test]
    fn dialogue_visibility_transition_fields_roundtrip_and_default_away() {
        let legacy: Operation = serde_json::from_value(serde_json::json!({
            "type":"dialogue_visibility","visible":false
        }))
        .unwrap();
        match &legacy {
            Operation::DialogueVisibility {
                visible,
                transition,
                duration_us,
            } => {
                assert!(!*visible);
                assert_eq!(transition, &None);
                assert_eq!(*duration_us, Micros(0));
            }
            _ => panic!("wrong variant"),
        }
        assert_eq!(
            serde_json::to_value(&legacy).unwrap(),
            serde_json::json!({"type":"dialogue_visibility","visible":false})
        );
        let styled: Operation = serde_json::from_value(serde_json::json!({
            "type":"dialogue_visibility","visible":true,
            "transition":{"type":"dissolve"},
            "duration_us":"1000000"
        }))
        .unwrap();
        match &styled {
            Operation::DialogueVisibility {
                visible,
                transition,
                duration_us,
            } => {
                assert!(*visible);
                assert_eq!(transition, &Some(StageTransition::Dissolve));
                assert_eq!(*duration_us, Micros(1_000_000));
            }
            _ => panic!("wrong variant"),
        }
        assert_eq!(
            serde_json::to_value(&styled).unwrap(),
            serde_json::json!({
                "type":"dialogue_visibility","visible":true,
                "transition":{"type":"dissolve"},
                "duration_us":"1000000"
            })
        );
    }
}
