//! Versioned, platform-independent wire contracts. Unknown semantic fields fail closed.
#![forbid(unsafe_code)]
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const FORMAT_VERSION: u32 = 1;
pub const SNAPSHOT_VERSION: u32 = 1;
pub const CAPABILITIES: &[&str] = &[
    "control.v1",
    "stage.sprite.v1",
    "stage.dissolve.v1",
    "clip.scalar.v1",
    "text.structured.v1",
    "text.revisions.v1",
    "text.gate.v1",
    "choice.v1",
    "audio.buffer.v1",
];
pub const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_TASKS: usize = 256;
pub const MAX_FRAMES: usize = 64;
pub const MAX_NODES: usize = 1024;

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
    ProfileMerge {
        key: String,
    },
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
    Dialogue {
        text: String,
        #[serde(default)]
        speaker: String,
        reveal_us: Micros,
    },
    Audio {
        asset: String,
        bus: AudioBus,
        #[serde(default)]
        looped: bool,
    },
    Delay {
        duration_us: Micros,
    },
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum AudioBus {
    Bgm,
    Voice,
    Sfx,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
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
        }
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DialogueProps {
    pub height: f32,
    pub padding: f32,
    pub font_size: f32,
}
impl Default for DialogueProps {
    fn default() -> Self {
        Self {
            height: 220.,
            padding: 24.,
            font_size: 23.,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PlayerDefaults {
    pub font_scale: f32,
    pub bgm_volume: f32,
    pub voice_volume: f32,
    pub sfx_volume: f32,
    pub reduced_motion: bool,
    pub auto_delay_us: Micros,
}
impl Default for PlayerDefaults {
    fn default() -> Self {
        Self {
            font_scale: 1.,
            bgm_volume: 0.3,
            voice_volume: 0.8,
            sfx_volume: 0.5,
            reduced_motion: false,
            auto_delay_us: Micros(1_200_000),
        }
    }
}
impl PlayerDefaults {
    pub fn preferences(&self, ui_locale: String, text_locale: String) -> Preferences {
        Preferences {
            ui_locale,
            text_locale,
            font_scale: self.font_scale,
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
        ("dialogue.height", theme.dialogue.height, 220., 320.),
        ("dialogue.padding", theme.dialogue.padding, 12., 32.),
        ("dialogue.font_size", theme.dialogue.font_size, 18., 28.),
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
    if !(100_000..=30_000_000).contains(&player.auto_delay_us.0) {
        return Err(Diagnostic::new(
            "E_PLAYER_CONFIG",
            "player.auto_delay_us",
            "expected 100000..30000000 microseconds",
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
    pub game_id: String,
    pub title: String,
    pub version: String,
    pub engine_build: String,
    pub program: String,
    pub objects: BTreeMap<String, Object>,
    pub engine: EngineFiles,
    #[serde(default)]
    pub notices: Vec<String>,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineFiles {
    pub js: String,
    pub wasm: String,
    pub host: String,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub ui_locale: String,
    pub text_locale: String,
    pub font_scale: f32,
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
            bgm_volume: 0.3,
            voice_volume: 0.8,
            sfx_volume: 0.5,
            reduced_motion: false,
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum UiAction {
    NewGame,
    Continue,
    Advance,
    Choose { option: String },
    Menu,
    Close,
    Settings,
    History,
    Saves,
    Save { slot: u32 },
    Load { slot: u32 },
    Rollback,
    Title,
    ToggleAuto,
    ToggleSkip,
    UiLocale { locale: String },
    TextLocale { locale: String },
    LocaleRetry,
    LocaleCancel,
    FontSize { delta: f32 },
    Volume { bus: AudioBus, delta: f32 },
    ReducedMotion,
    HistoryPage { delta: i32 },
    Scroll { region: ScrollRegion, delta: i32 },
    Export,
    Import,
    Retry,
}

/// Viewport navigation, never a VM instruction or a snapshot cursor.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollRegion {
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
