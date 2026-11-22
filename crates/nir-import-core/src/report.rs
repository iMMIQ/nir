//! Playable-First conversion report tracking Exact / Adapted / Fallback fidelity.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fidelity {
    Exact,
    Adapted,
    Fallback,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Diagnostic {
    pub fidelity: Fidelity,
    pub source: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ConversionReport {
    pub engine: String,
    pub exact_count: usize,
    pub adapted_count: usize,
    pub fallback_count: usize,
    pub scripts_converted: usize,
    pub assets_converted: usize,
    pub statements_emitted: usize,
    pub diagnostics: Vec<Diagnostic>,
}

impl ConversionReport {
    pub fn new(engine: impl Into<String>) -> Self {
        Self {
            engine: engine.into(),
            ..Self::default()
        }
    }

    pub fn record_exact(&mut self) {
        self.exact_count += 1;
    }

    pub fn record_adapted(&mut self, source: impl Into<String>, message: impl Into<String>) {
        self.adapted_count += 1;
        if self.diagnostics.len() < 512 {
            self.diagnostics.push(Diagnostic {
                fidelity: Fidelity::Adapted,
                source: source.into(),
                message: message.into(),
            });
        }
    }

    pub fn record_fallback(&mut self, source: impl Into<String>, message: impl Into<String>) {
        self.fallback_count += 1;
        if self.diagnostics.len() < 512 {
            self.diagnostics.push(Diagnostic {
                fidelity: Fidelity::Fallback,
                source: source.into(),
                message: message.into(),
            });
        }
    }

    pub fn is_strict_clean(&self) -> bool {
        self.fallback_count == 0
    }

    pub fn fidelity_score(&self) -> f32 {
        let total = self.exact_count + self.adapted_count + self.fallback_count;
        if total == 0 {
            return 100.0;
        }
        let weighted = (self.exact_count as f32) + (self.adapted_count as f32) * 0.85;
        (weighted / total as f32) * 100.0
    }

    pub fn summary_text(&self) -> String {
        let mut out = format!(
            "Engine: {} | Scripts: {} | Assets: {} | Stmts: {}\nFidelity: {:.1}% (exact={}, adapted={}, fallback={})",
            self.engine,
            self.scripts_converted,
            self.assets_converted,
            self.statements_emitted,
            self.fidelity_score(),
            self.exact_count,
            self.adapted_count,
            self.fallback_count,
        );
        for d in self.diagnostics.iter().take(20) {
            let tag = match d.fidelity {
                Fidelity::Exact => "EXACT",
                Fidelity::Adapted => "ADAPT",
                Fidelity::Fallback => "FALLBACK",
            };
            out.push_str(&format!("\n  [{tag}] {}: {}", d.source, d.message));
        }
        if self.diagnostics.len() > 20 {
            out.push_str(&format!("\n  ... and {} more", self.diagnostics.len() - 20));
        }
        out
    }
}
