use super::*;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
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
