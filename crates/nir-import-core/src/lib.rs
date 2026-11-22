#![forbid(unsafe_code)]

//! Pluggable multi-engine Visual Novel importer framework (`nir-import-core`).
//!
//! Architecture:
//!   Source Game (dir / XP3 / LiveMaker VF / ...)
//!       ──▶ `ArchiveVfs` (case-insensitive layered VFS + auto CJK encoding)
//!       ──▶ `EngineImporter` plugin (LiveMaker, KiriKiri2/KAG3, ...)
//!       ──▶ `VnProject` (high-level, engine-agnostic VN-IR) + `ConversionReport`
//!       ──▶ `emit_nirs_project` (editable `.nirs` + `game.toml` + `assets/`)
//!       ──▶ `nir_build::compile_project` (auto-optimized `.nirpak`)

pub mod emit;
pub mod ir;
pub mod report;
pub mod vfs;

use anyhow::{bail, Result};
use std::path::Path;

pub use emit::{emit_nirs_project, sanitize_ident};
pub use ir::{VnCharacter, VnChoice, VnConst, VnProject, VnStmt};
pub use report::{ConversionReport, Diagnostic, Fidelity};
pub use vfs::{decode_text_bytes, ArchiveSource, CompositeVfs, DirArchive, MemoryArchive};

/// Options passed to an engine importer.
#[derive(Clone, Debug)]
pub struct ImportOptions {
    /// Fail conversion if any `Fidelity::Fallback` diagnostic is recorded.
    pub strict: bool,
    /// Optional target locale override (e.g. `"ja"`, `"zh-Hans"`, `"en"`).
    pub source_locale: Option<String>,
    /// Safety cap on scripts processed in one run (prevents pathological inputs).
    pub max_scripts: usize,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            strict: false,
            source_locale: None,
            max_scripts: 4096,
        }
    }
}

/// Detection result from probing a source directory or archive.
#[derive(Clone, Debug)]
pub struct EngineProbe {
    pub engine_id: String,
    pub display_name: String,
    /// Confidence score `1..=100` (higher wins during auto-detection).
    pub confidence: u8,
    pub summary: String,
}

/// Pluggable importer for a specific third-party visual novel engine.
pub trait EngineImporter: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn detect(&self, source: &Path) -> Option<EngineProbe>;
    fn import(&self, source: &Path, opts: &ImportOptions) -> Result<(VnProject, ConversionReport)>;
}

/// Registry of available engine importers with automatic engine detection.
#[derive(Default)]
pub struct ImporterRegistry {
    importers: Vec<Box<dyn EngineImporter>>,
}

impl ImporterRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<I: EngineImporter + 'static>(&mut self, importer: I) {
        self.importers.push(Box::new(importer));
    }

    pub fn ids(&self) -> Vec<&'static str> {
        self.importers.iter().map(|i| i.id()).collect()
    }

    pub fn by_id(&self, id: &str) -> Option<&dyn EngineImporter> {
        self.importers
            .iter()
            .find(|i| i.id().eq_ignore_ascii_case(id))
            .map(AsRef::as_ref)
    }

    /// Probes all registered importers and returns matches sorted by descending confidence.
    pub fn detect_all(&self, source: &Path) -> Vec<EngineProbe> {
        let mut matches: Vec<EngineProbe> = self
            .importers
            .iter()
            .filter_map(|i| i.detect(source))
            .collect();
        matches.sort_by_key(|a| std::cmp::Reverse(a.confidence));
        matches
    }

    /// Imports `source` using either an explicit `engine_id` or automatic detection.
    pub fn import_auto(
        &self,
        source: &Path,
        engine_id: Option<&str>,
        opts: &ImportOptions,
    ) -> Result<(VnProject, ConversionReport)> {
        let importer = if let Some(id) = engine_id.filter(|s| *s != "auto") {
            self.by_id(id).ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown engine `{id}`; available engines: {}",
                    self.ids().join(", ")
                )
            })?
        } else {
            let probes = self.detect_all(source);
            let best = probes.first().ok_or_else(|| {
                anyhow::anyhow!(
                    "could not detect a supported visual novel engine at `{}`; supported: {}",
                    source.display(),
                    self.ids().join(", ")
                )
            })?;
            self.by_id(&best.engine_id).expect("probed importer exists")
        };

        let (project, report) = importer.import(source, opts)?;
        if opts.strict && !report.is_strict_clean() {
            bail!(
                "strict import failed for {} ({}): {} fallback item(s) encountered\n{}",
                project.title,
                importer.id(),
                report.fallback_count,
                report.summary_text()
            );
        }
        Ok((project, report))
    }
}
