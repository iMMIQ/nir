#![forbid(unsafe_code)]
mod validate;
mod vm;
pub use validate::{expr_type, ValidatedProgram};
pub use vm::*;
