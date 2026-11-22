use super::*;

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ValueType {
    Bool,
    I32,
    F80,
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
    F80(Float80),
    String(String),
}
impl Value {
    pub fn ty(&self) -> ValueType {
        match self {
            Self::Bool(_) => ValueType::Bool,
            Self::I32(_) => ValueType::I32,
            Self::F80(_) => ValueType::F80,
            Self::String(_) => ValueType::String,
        }
    }
}
/// Bounds for independent typed progress, shared by Core and native storage.
pub fn valid_profile_values(values: &BTreeMap<String, Value>) -> bool {
    values.len() <= 4096
        && values.iter().all(|(key, value)| {
            !key.is_empty()
                && key.len() <= 1024
                && !matches!(value, Value::String(s) if s.len() > 65536)
        })
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
    ToF80 {
        value: Box<Expr>,
    },
    ToI32 {
        value: Box<Expr>,
    },
}
impl Expr {
    pub fn uses_float80(&self) -> bool {
        match self {
            Self::Const { value } => value.ty() == ValueType::F80,
            Self::ToF80 { .. } | Self::ToI32 { .. } => true,
            Self::Not { value } => value.uses_float80(),
            Self::Binary { left, right, .. } => left.uses_float80() || right.uses_float80(),
            Self::Var { .. } => false,
        }
    }
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
