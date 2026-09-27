//! Offline external-engine import. Only the author compiler depends on readers;
//! neither the player nor generated games require the original engine.
mod livenovel;
mod lower;
mod lsb;
mod media;

use anyhow::{ensure, Context, Result};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

pub use lower::{ImportDiagnostic, ImportReport, SourceLocation};

#[derive(Debug)]
pub struct ImportOptions {
    pub source: PathBuf,
    pub out: PathBuf,
    pub entry: Option<String>,
    pub line: u32,
    pub draft: bool,
    pub game_id: String,
    pub title: String,
    pub locale: String,
}

#[derive(Debug, Serialize)]
pub struct ScriptInventory {
    pub path: String,
    pub sha256: String,
    pub version: u32,
    pub commands: BTreeMap<String, usize>,
    pub glyphs: BTreeMap<String, usize>,
    pub events: BTreeMap<String, usize>,
}
#[derive(Debug, Serialize)]
pub struct Inspection {
    pub format: u32,
    pub engine: String,
    pub files_by_extension: BTreeMap<String, usize>,
    pub scripts: Vec<ScriptInventory>,
    pub errors: BTreeMap<String, String>,
}

fn read_binary(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let file = fs::File::open(path).context("E_IMPORT_FILE: cannot open source file")?;
    ensure!(
        file.metadata()?.len() <= 32 * 1024 * 1024,
        "E_IMPORT_LIMIT: source file exceeds 32 MiB"
    );
    let mut bytes = Vec::new();
    file.take(32 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 32 * 1024 * 1024,
        "E_IMPORT_LIMIT: source file exceeds 32 MiB"
    );
    Ok(bytes)
}

struct Source {
    root: PathBuf,
    scripts: BTreeMap<String, Arc<lsb::Script>>,
    bytes_read: usize,
}
impl Source {
    fn new(path: &Path) -> Result<Self> {
        let root = fs::canonicalize(path).context("E_IMPORT_SOURCE: source directory not found")?;
        ensure!(
            root.is_dir(),
            "E_IMPORT_SOURCE: expected an extracted game directory"
        );
        Ok(Self {
            root,
            scripts: BTreeMap::new(),
            bytes_read: 0,
        })
    }
    fn path(&self, name: &str) -> Result<PathBuf> {
        let normalized = name.replace('\\', "/");
        ensure!(
            !normalized.is_empty() && !normalized.contains(':') && !normalized.contains('\0'),
            "E_IMPORT_PATH: invalid source-relative path"
        );
        let relative = Path::new(&normalized);
        ensure!(
            relative
                .components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir)),
            "E_IMPORT_PATH: path must stay inside source directory"
        );
        let mut candidate = self.root.clone();
        for component in relative.components() {
            if let Component::Normal(part) = component {
                let exact = candidate.join(part);
                if exact.try_exists()? {
                    candidate = exact;
                    continue;
                }
                let name = part.to_str().context("E_IMPORT_PATH: non-Unicode path")?;
                let matches: Vec<_> = fs::read_dir(&candidate)?
                    .filter_map(|entry| entry.ok())
                    .filter(|entry| {
                        entry
                            .file_name()
                            .to_str()
                            .is_some_and(|s| s.eq_ignore_ascii_case(name))
                    })
                    .collect();
                ensure!(
                    matches.len() == 1,
                    "E_IMPORT_PATH: missing or ambiguous source file {name}"
                );
                candidate = matches[0].path();
            }
        }
        let full = fs::canonicalize(candidate)
            .context("E_IMPORT_PATH: referenced source file is missing")?;
        ensure!(
            full.starts_with(&self.root),
            "E_IMPORT_PATH: symlink escapes source directory"
        );
        Ok(full)
    }
    fn read(&mut self, name: &str) -> Result<(String, Arc<lsb::Script>)> {
        let path = self.path(name)?;
        let name = path
            .strip_prefix(&self.root)?
            .to_str()
            .context("E_IMPORT_PATH: filename is not Unicode")?
            .replace('\\', "/");
        if let Some(script) = self.scripts.get(&name) {
            return Ok((name, script.clone()));
        }
        ensure!(
            self.scripts.len() < 4096,
            "E_IMPORT_LIMIT: more than 4096 scripts"
        );
        let bytes = read_binary(&path)?;
        self.bytes_read += bytes.len();
        ensure!(
            self.bytes_read <= 128 * 1024 * 1024,
            "E_IMPORT_LIMIT: total script bytes exceed 128 MiB"
        );
        let script =
            Arc::new(lsb::parse(&bytes).with_context(|| format!("E_IMPORT_PARSE: {name}"))?);
        self.scripts.insert(name.clone(), script.clone());
        Ok((name, script))
    }
    fn entry(&self) -> Result<String> {
        lsb::startup(&read_binary(&self.path("live.lpb")?)?)
            .context("E_IMPORT_ENTRY: cannot read startup; specify --entry")
    }
}

fn files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut pending = vec![(root.to_path_buf(), 0)];
    let mut result = vec![];
    let mut entries = 0;
    while let Some((directory, depth)) = pending.pop() {
        ensure!(depth < 64, "E_IMPORT_LIMIT: directory nesting exceeds 64");
        for entry in fs::read_dir(directory)? {
            entries += 1;
            ensure!(
                entries <= 100_000,
                "E_IMPORT_LIMIT: too many directory entries"
            );
            let entry = entry?;
            let ty = entry.file_type()?;
            ensure!(
                !ty.is_symlink(),
                "E_IMPORT_PATH: symlinks are not permitted in source inventories"
            );
            if ty.is_dir() {
                pending.push((entry.path(), depth + 1));
            } else if ty.is_file() {
                result.push(entry.path());
            }
        }
    }
    result.sort();
    Ok(result)
}

/// Structural inventory without exporting dialogue, images or event arguments.
pub fn inspect(source: &Path) -> Result<Inspection> {
    let mut source = Source::new(source)?;
    let mut report = Inspection {
        format: 1,
        engine: "livemaker".into(),
        files_by_extension: BTreeMap::new(),
        scripts: vec![],
        errors: BTreeMap::new(),
    };
    let mut found = false;
    for path in files(&source.root)? {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        *report.files_by_extension.entry(ext.clone()).or_default() += 1;
        if ext != "lsb" {
            continue;
        }
        found = true;
        let name = path
            .strip_prefix(&source.root)?
            .to_str()
            .context("E_IMPORT_PATH: filename is not Unicode")?
            .replace('\\', "/");
        match source.read(&name) {
            Ok((_, script)) => {
                let mut item = ScriptInventory {
                    path: name,
                    sha256: nir_content::digest(&read_binary(&path)?),
                    version: script.version,
                    commands: BTreeMap::new(),
                    glyphs: BTreeMap::new(),
                    events: BTreeMap::new(),
                };
                for c in &script.commands {
                    *item.commands.entry(c.name().into()).or_default() += 1;
                    if let lsb::Body::Text { text, .. } = &c.body {
                        for (key, n) in &text.counts {
                            *item.glyphs.entry(key.clone()).or_default() += n;
                        }
                        for (key, n) in &text.events {
                            *item.events.entry(key.clone()).or_default() += n;
                        }
                    }
                }
                report.scripts.push(item);
            }
            Err(e) => {
                report.errors.insert(name, format!("{e:#}"));
            }
        }
    }
    ensure!(
        found,
        "E_IMPORT_ENGINE: no extracted LiveMaker LSB scripts found"
    );
    Ok(report)
}

fn destination(source: &Path, out: &Path) -> Result<PathBuf> {
    ensure!(!out.try_exists()?, "E_IMPORT_EXISTS: output already exists");
    let parent = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    // Requiring an existing parent avoids writes before path validation.
    let parent =
        fs::canonicalize(parent).context("E_IMPORT_OUTPUT: output parent must already exist")?;
    let name = out
        .file_name()
        .context("E_IMPORT_OUTPUT: output directory needs a name")?;
    let target = parent.join(name);
    ensure!(
        !target.starts_with(source) && !source.starts_with(&target),
        "E_IMPORT_OUTPUT: source and output must not overlap"
    );
    Ok(target)
}

/// Strict mode writes nothing on unsupported semantics. Draft mode emits explicit
/// runtime faults, an incomplete marker and a source map, never silent no-ops.
pub fn convert(options: &ImportOptions, sdk: &Path) -> Result<ImportReport> {
    ensure!(
        matches!(options.locale.as_str(), "ja" | "en" | "zh-Hans"),
        "E_IMPORT_LOCALE: expected ja, en or zh-Hans"
    );
    let source = Source::new(&options.source)?;
    let out = destination(&source.root, &options.out)?;
    let entry = options
        .entry
        .clone()
        .map(Ok)
        .unwrap_or_else(|| source.entry())?;
    if options.line == 0 && livenovel::recognizes(&source, &entry) {
        return livenovel::convert(source, &entry, options, sdk, &out);
    }
    let mut lowering = lower::Lowering::new(source);
    let story = lowering.run(&entry, options.line)?;
    let mut report = lowering.report();
    if report.errors > 0 && !options.draft {
        return Ok(report);
    }
    let staging = tempfile::Builder::new()
        .prefix(".nir-import-")
        .tempdir_in(out.parent().unwrap())?;
    let project = staging.path().join("project");
    crate::init(&project, sdk, &options.game_id)?;
    export(&project, options, &lowering.texts, &story, &report)?;
    // Use the same font, text-revision and executable validation as normal author
    // projects. Failed validation leaves no half-written destination.
    let loaded = crate::load_project(&project)?;
    crate::compile(&loaded.program)?;
    report.written = true;
    write_json(&project.join("import-report.json"), &report)?;
    ensure!(
        !out.try_exists()?,
        "E_IMPORT_EXISTS: output appeared during import"
    );
    fs::rename(&project, &out).context("E_IMPORT_OUTPUT: cannot publish generated project")?;
    Ok(report)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}
fn export(
    root: &Path,
    options: &ImportOptions,
    imported_texts: &BTreeMap<String, crate::AuthorTextDoc>,
    story: &serde_json::Value,
    report: &ImportReport,
) -> Result<()> {
    use crate::texts::{AuthorTextContract, RevisionRecord, TextRevisions};
    use serde_json::json;
    let path = root.join("game.toml");
    let mut manifest: crate::GameManifest = toml::from_str(&fs::read_to_string(&path)?)?;
    manifest.game.title = options.title.clone();
    manifest.game.slug = "imported-story".into();
    manifest.game.source_locale = options.locale.clone();
    manifest.inputs.scenarios.clear();
    fs::write(path, toml::to_string_pretty(&manifest)?)?;
    fs::remove_dir_all(root.join("tests"))?;
    let texts = root.join("content/main/texts");
    fs::remove_dir_all(&texts)?;
    fs::create_dir(&texts)?;
    write_json(&root.join("content/main/story.nir.json"), story)?;
    write_json(&texts.join("source.json"), imported_texts)?;
    let mut contracts = BTreeMap::new();
    let mut ledger = TextRevisions {
        format: 1,
        source_locale: options.locale.clone(),
        texts: BTreeMap::new(),
    };
    for (id, doc) in imported_texts {
        let c = AuthorTextContract {
            source_revision: 1,
            contract_revision: 1,
            meaning_revision: 1,
            gates: doc
                .spans
                .iter()
                .filter_map(|s| {
                    if let nir_format::Span::Gate { id } = s {
                        Some(id.clone())
                    } else {
                        None
                    }
                })
                .collect(),
            params: BTreeMap::new(),
        };
        let runtime = nir_format::TextContract {
            source_revision: 1,
            contract_revision: 1,
            meaning_revision: 1,
            contract_digest: String::new(),
            gates: c.gates.clone(),
            params: BTreeMap::new(),
        };
        ledger.texts.insert(
            id.clone(),
            RevisionRecord {
                source_revision: 1,
                contract_revision: 1,
                meaning_revision: 1,
                source_digest: nir_content::digest(&serde_json::to_vec(&doc.spans)?),
                contract_digest: nir_format::text_contract_digest(&runtime),
                shape_digest: nir_content::digest(&serde_json::to_vec(&(
                    c.params.clone(),
                    c.gates.clone(),
                ))?),
                reviewed: BTreeMap::new(),
            },
        );
        contracts.insert(id.clone(), c);
    }
    write_json(&texts.join("contracts.json"), &contracts)?;
    write_json(&texts.join("revisions.json"), &ledger)?;
    let module = json!({"id":"main", "module_format":1, "sources":["story.nir.json"], "text_contracts":"texts/contracts.json",
        "text_revisions":"texts/revisions.json", "exports":{"start":"main"}, "text_bundles":{options.locale.clone():"texts/source.json"}});
    fs::write(
        root.join("content/main/module.toml"),
        toml::to_string_pretty(&module)?,
    )?;
    let locales = json!({"format":1,"default_ui":"en","default_text":options.locale,"ui":{"en":["font.reader"]},"text":{options.locale.clone():["font.reader"]}});
    fs::write(
        root.join("config/locales.toml"),
        toml::to_string_pretty(&locales)?,
    )?;
    fs::write(root.join("README.md"), "Generated LiveMaker migration project. Inspect import-report.json before editing.\nSource assets are not included. Imported dialogue retains its original rights.\n")?;
    let notice_path = root.join("NOTICE.md");
    let mut notice = fs::read_to_string(&notice_path)?;
    notice.push_str("\nImported dialogue retains its original rights. The template license does not cover imported game content.\n");
    fs::write(notice_path, notice)?;
    if report.errors > 0 {
        fs::write(root.join("MIGRATION-INCOMPLETE.txt"), "INCOMPLETE: unsupported instructions stop with E_IMPORT_UNSUPPORTED.\nThis draft is not a playable conversion of the source game. See import-report.json.\n")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
