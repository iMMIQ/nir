#![forbid(unsafe_code)]
mod diagnostics;
pub use diagnostics::diagnostic;
mod project;
mod release;
mod scenario;
pub mod script;
pub use project::*;
pub use release::*;
pub use scenario::*;
pub use script::{lower_nirs, NirsOutput};

mod config;
pub use config::{
    LocaleManifest, PlayerConfig, ResolvedConfig, ResolvedField, ThemeManifest, ThemeTokens,
};

mod fonts;
pub use fonts::{FontMode, FontRecipe, FontReport, FONT_TOOL};

pub mod optimize;
pub use optimize::{
    AssetPolicy, AudioFormat, ImageFormat, OptimizeOptions, OptimizeReport, MP3_BITRATES,
    OPTIMIZE_TOOL,
};

mod texts;
pub use texts::{
    shape as shape_text, text_migrate, text_recover, text_review, text_status, text_update,
    AuthorTextContract, AuthorTextDoc, RevisionRecord, TextRevisions,
};
