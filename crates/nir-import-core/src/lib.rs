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
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

pub use emit::{emit_nirs_project, sanitize_ident};
pub use ir::{VnCharacter, VnChoice, VnConst, VnProject, VnStmt};
pub use report::{ConversionReport, Diagnostic, Fidelity};
pub use vfs::{decode_text_bytes, ArchiveSource, CompositeVfs, DirArchive, MemoryArchive};

/// Direct uncompressed raster pixels passed from an importer straight into `nir-build`,
/// eliminating intermediate PNG encode/decode round-trips during conversion!
#[derive(Clone)]
pub enum DecodedPixels {
    Rgb {
        width: u32,
        height: u32,
        data: Vec<u8>,
    },
    Rgba {
        width: u32,
        height: u32,
        data: Vec<u8>,
    },
}

impl std::fmt::Debug for DecodedPixels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rgb { width, height, .. } => write!(f, "Rgb({width}x{height})"),
            Self::Rgba { width, height, .. } => write!(f, "Rgba({width}x{height})"),
        }
    }
}

/// Lazy or in-memory raw media payload supplied by a direct-to-IR engine importer.
/// Using `Lazy` / `LazyImage` avoids holding thousands of decoded rasters in RAM before
/// tree-shaking and lets `nir-build` optimize assets in parallel across CPU cores.
#[derive(Clone)]
pub enum RawAssetData {
    Bytes(Vec<u8>),
    Lazy(Arc<dyn Fn() -> Result<Vec<u8>> + Send + Sync>),
    /// Direct pixel decoder bypassing intermediate PNG encoding/decoding.
    LazyImage(Arc<dyn Fn() -> Result<DecodedPixels> + Send + Sync>),
}

impl std::fmt::Debug for RawAssetData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bytes(b) => f.debug_tuple("Bytes").field(&b.len()).finish(),
            Self::Lazy(_) => f.write_str("Lazy(<fn>)"),
            Self::LazyImage(_) => f.write_str("LazyImage(<fn>)"),
        }
    }
}

impl RawAssetData {
    pub fn load(&self) -> Result<Vec<u8>> {
        match self {
            Self::Bytes(b) => Ok(b.clone()),
            Self::Lazy(func) => func(),
            Self::LazyImage(_) => bail!("LazyImage payload must be consumed via load_pixels()"),
        }
    }

    pub fn load_pixels(&self) -> Option<Result<DecodedPixels>> {
        match self {
            Self::LazyImage(func) => Some(func()),
            _ => None,
        }
    }
}

/// Returns a fast `(byte_len, mtime_nanos)` fingerprint for a source file on disk so
/// incremental rebuilds can hit disk cache BEFORE decoding `.gal` pixels or hashing large `.wav`s.
pub fn file_fingerprint(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let len = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    Some((len, mtime))
}

/// Asset record matching `program.assets[i]` produced by a direct-to-IR engine importer.
#[derive(Clone, Debug)]
pub struct RawAssetInput {
    pub name: String,
    pub kind: nir_ir::AssetKind,
    pub codec: nir_ir::MediaCodec,
    pub data: RawAssetData,
    /// Optional `(byte_len, mtime_nanos)` of the original source file for fast pre-decode cache hits.
    pub source_fingerprint: Option<(u64, u64)>,
}

/// Complete direct-to-IR conversion output ready for `nir-build`'s parallel optimizer & packager.
pub struct DirectImport {
    pub program: nir_ir::NirProgram,
    pub raw_assets: Vec<RawAssetInput>,
    /// Extra `AssetId`s that must survive tree-shaking (e.g., runtime `asset_lookup` targets).
    pub keep_assets: BTreeSet<nir_ir::AssetId>,
}

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

    /// Optional direct-to-`NirProgram` compilation path for full-fidelity conversion
    /// (preserving persistent layers, dynamic expressions, menus, timelines, and replays).
    fn import_direct(
        &self,
        _source: &Path,
        _opts: &ImportOptions,
    ) -> Option<Result<(DirectImport, ConversionReport)>> {
        None
    }
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

    fn resolve_importer(
        &self,
        source: &Path,
        engine_id: Option<&str>,
    ) -> Result<&dyn EngineImporter> {
        if let Some(id) = engine_id.filter(|s| *s != "auto") {
            self.by_id(id).ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown engine `{id}`; available engines: {}",
                    self.ids().join(", ")
                )
            })
        } else {
            let probes = self.detect_all(source);
            let best = probes.first().ok_or_else(|| {
                anyhow::anyhow!(
                    "could not detect a supported visual novel engine at `{}`; supported: {}",
                    source.display(),
                    self.ids().join(", ")
                )
            })?;
            Ok(self.by_id(&best.engine_id).expect("probed importer exists"))
        }
    }

    /// Imports `source` using either an explicit `engine_id` or automatic detection.
    pub fn import_auto(
        &self,
        source: &Path,
        engine_id: Option<&str>,
        opts: &ImportOptions,
    ) -> Result<(VnProject, ConversionReport)> {
        let importer = self.resolve_importer(source, engine_id)?;
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

    /// Attempts direct-to-`NirProgram` import if supported by the detected engine plugin.
    pub fn import_direct_auto(
        &self,
        source: &Path,
        engine_id: Option<&str>,
        opts: &ImportOptions,
    ) -> Result<Option<(DirectImport, ConversionReport)>> {
        let importer = self.resolve_importer(source, engine_id)?;
        let Some(res) = importer.import_direct(source, opts) else {
            return Ok(None);
        };
        let (direct, report) = res?;
        if opts.strict && !report.is_strict_clean() {
            bail!(
                "strict direct import failed for {} ({}): {} fallback item(s) encountered\n{}",
                direct.program.meta.title,
                importer.id(),
                report.fallback_count,
                report.summary_text()
            );
        }
        Ok(Some((direct, report)))
    }
}
