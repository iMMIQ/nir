use crate::{AssetId, VarId};
use serde::{Deserialize, Serialize};

/// Rich-text span inside a localized dialogue or UI text entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RichSpan {
    Text(String),
    Emphasis(String),
    Color {
        text: String,
        rgba: [f32; 4],
    },
    Ruby {
        base: String,
        ruby: String,
    },
    /// Timed or click-to-continue inline pause.
    Pause {
        duration_us: u64,
        click: bool,
    },
    InlineImage {
        asset: AssetId,
        width: u32,
        height: u32,
    },
    /// Interpolates a global variable's display string at dialogue commit time.
    Var(VarId),
    /// Adjusts reveal speed multiplier for subsequent spans.
    Speed(f32),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TextEntry {
    pub spans: Vec<RichSpan>,
}

impl TextEntry {
    pub fn plain(s: impl Into<String>) -> Self {
        Self {
            spans: vec![RichSpan::Text(s.into())],
        }
    }

    /// Flattens static text spans into a plain string (used for bilingual overlay & search).
    pub fn to_plain_text(&self) -> String {
        let mut out = String::new();
        for span in &self.spans {
            match span {
                RichSpan::Text(s) | RichSpan::Emphasis(s) | RichSpan::Color { text: s, .. } => {
                    out.push_str(s);
                }
                RichSpan::Ruby { base, .. } => out.push_str(base),
                RichSpan::Pause { .. }
                | RichSpan::InlineImage { .. }
                | RichSpan::Var(_)
                | RichSpan::Speed(_) => {}
            }
        }
        out
    }

    /// Collects all static characters for fast compiler font subsetting.
    pub fn collect_chars(&self, out: &mut std::collections::BTreeSet<char>) {
        for span in &self.spans {
            match span {
                RichSpan::Text(s) | RichSpan::Emphasis(s) | RichSpan::Color { text: s, .. } => {
                    out.extend(s.chars());
                }
                RichSpan::Ruby { base, ruby } => {
                    out.extend(base.chars());
                    out.extend(ruby.chars());
                }
                RichSpan::Pause { .. }
                | RichSpan::InlineImage { .. }
                | RichSpan::Var(_)
                | RichSpan::Speed(_) => {}
            }
        }
    }
}
