use super::*;

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
