//! NIR v2 compact binary IR, bytecode instructions, persistent scene graph
//! primitives, declarative UI schema, and `.nirpak` container format.
//!
//! Zero legacy capability bloat, zero software Float80, zero JSON-in-JSON pools.
#![forbid(unsafe_code)]

pub mod bytecode;
pub mod lame;
pub mod pack;
pub mod scene;
pub mod text;
pub mod ui;
pub mod value;

pub use bytecode::*;
pub use pack::*;
pub use scene::*;
pub use text::*;
pub use ui::*;
pub use value::*;

pub mod ui_messages {
    pub const EN_FTL: &str = include_str!("../messages/en.ftl");
    pub const ZH_HANS_FTL: &str = include_str!("../messages/zh-Hans.ftl");
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{code} ({location}): {message}")]
pub struct NirError {
    pub code: &'static str,
    pub location: String,
    pub message: String,
}

impl NirError {
    pub fn new(code: &'static str, location: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code,
            location: location.into(),
            message: message.into(),
        }
    }
}

pub type Result<T> = std::result::Result<T, NirError>;
