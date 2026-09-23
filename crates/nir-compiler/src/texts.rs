//! Author-side revision records; never linked into the player.
use crate::{
    project::{json, toml_file, Module},
    relative, GameManifest,
};
use anyhow::{anyhow, bail, Context, Result};
use nir_format::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthorTextContract {
    pub source_revision: u32,
    pub contract_revision: u32,
    pub meaning_revision: u32,
    #[serde(default)]
    pub gates: Vec<String>,
    #[serde(default)]
    pub params: BTreeMap<String, ValueType>,
}
impl AuthorTextContract {
    fn runtime(&self) -> TextContract {
        let mut c = TextContract {
            source_revision: self.source_revision,
            contract_revision: self.contract_revision,
            meaning_revision: self.meaning_revision,
            contract_digest: String::new(),
            gates: self.gates.clone(),
            params: self.params.clone(),
        };
        c.contract_digest = text_contract_digest(&c);
        c
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthorTextDoc {
    pub source_revision: u32,
    pub contract_revision: u32,
    pub spans: Vec<Span>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TextRevisions {
    pub format: u32,
    pub source_locale: String,
    pub texts: BTreeMap<String, RevisionRecord>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RevisionRecord {
    pub source_revision: u32,
    pub contract_revision: u32,
    pub meaning_revision: u32,
    pub source_digest: String,
    pub contract_digest: String,
    pub shape_digest: String,
    pub reviewed: BTreeMap<String, String>,
}
#[derive(Debug, Serialize)]
pub struct TextIssue {
    pub code: String,
    pub text_id: String,
    pub locale: String,
    pub file: String,
    pub pointer: String,
    pub message: String,
}
#[derive(Debug, Serialize)]
pub struct TextStatus {
    pub format: u32,
    pub source_locale: String,
    pub texts: usize,
    pub locales: usize,
    pub ready: bool,
    pub issues: Vec<TextIssue>,
}
struct Sources {
    root: PathBuf,
    module_id: String,
    namespaced: bool,
    source: String,
    contract_path: PathBuf,
    paths: BTreeMap<String, PathBuf>,
    ledger_path: PathBuf,
    contracts: BTreeMap<String, AuthorTextContract>,
    docs: BTreeMap<String, BTreeMap<String, AuthorTextDoc>>,
    ledger: TextRevisions,
    originals: BTreeMap<PathBuf, Vec<u8>>,
}
fn hash<T: Serialize>(value: &T) -> Result<String> {
    Ok(nir_content::digest(&serde_json::to_vec(value)?))
}
fn shape(c: &AuthorTextContract) -> Result<String> {
    hash(&(&c.params, &c.gates))
}
fn modules(root: &Path) -> Result<(GameManifest, Vec<(PathBuf, Module)>)> {
    let manifest: GameManifest = toml_file(&root.join("game.toml"))?;
    if manifest.inputs.modules.is_empty() {
        bail!("E_MODULE: project must list at least one module");
    }
    let mut result = Vec::new();
    let mut ids = std::collections::BTreeSet::new();
    for name in &manifest.inputs.modules {
        let path = relative(root, root, name)?;
        let m: Module = toml_file(&path)?;
        if m.module_format != 1 || !crate::project::valid_module_id(&m.id) {
            bail!("E_MODULE: unsupported identity/format");
        }
        if !ids.insert(m.id.clone()) {
            bail!("E_MODULE: duplicate module id {}", m.id);
        }
        result.push((path, m));
    }
    let mut text_paths = std::collections::BTreeSet::new();
    for (module_path, module) in &result {
        let base = module_path.parent().unwrap();
        let mut names = vec![module.text_contracts.as_str()];
        if let Some(revisions) = &module.text_revisions {
            names.push(revisions);
        }
        names.extend(module.text_bundles.values().map(String::as_str));
        for name in names {
            let path = relative(root, base, name)?;
            if !text_paths.insert(path.clone()) {
                bail!(
                    "E_TEXT_PATH: modules must own distinct text files ({})",
                    path.file_name().unwrap_or_default().to_string_lossy()
                );
            }
        }
    }
    Ok((manifest, result))
}
fn module(root: &Path) -> Result<(GameManifest, PathBuf, Module)> {
    let (manifest, mut modules) = modules(root)?;
    if modules.len() != 1 {
        bail!("E_CAPABILITY: this operation requires a single module");
    }
    let (path, module) = modules.pop().unwrap();
    Ok((manifest, path, module))
}
fn journal_path(root: &Path) -> Result<PathBuf> {
    let dir = root.join(".nir");
    if fs::symlink_metadata(&dir).is_ok_and(|m| m.file_type().is_symlink() || !m.is_dir()) {
        bail!("E_PATH_ESCAPE: .nir must be a project directory");
    }
    Ok(dir.join("text-transaction.json"))
}
fn no_transaction(root: &Path) -> Result<()> {
    if journal_path(root)?.exists() {
        bail!("E_TEXT_TRANSACTION: interrupted edit; run novelc text recover before continuing");
    }
    Ok(())
}
impl Sources {
    fn load(root: &Path, requested_module: Option<&str>) -> Result<Self> {
        let root = fs::canonicalize(root)?;
        no_transaction(&root)?;
        let (manifest, module_entries) = modules(&root)?;
        let namespaced = module_entries.len() > 1;
        let (path, m) = if let Some(requested) = requested_module {
            module_entries
                .into_iter()
                .find(|(_, module)| module.id == requested)
                .ok_or_else(|| anyhow!("E_MODULE: unknown module {requested}"))?
        } else if module_entries.len() == 1 {
            module_entries.into_iter().next().unwrap()
        } else {
            bail!("E_MODULE: select a module for text revision operations");
        };
        let base = path.parent().unwrap();
        let ledger_name=m.text_revisions.as_ref().ok_or_else(||anyhow!("E_TEXT_MIGRATION: legacy text revisions; run novelc text migrate --out NEW_DIRECTORY"))?;
        let contract_path = relative(&root, base, &m.text_contracts)?;
        let ledger_path = relative(&root, base, ledger_name)?;
        let mut docs = BTreeMap::new();
        let mut paths = BTreeMap::new();
        for (locale, name) in &m.text_bundles {
            let p = relative(&root, base, name)?;
            docs.insert(locale.clone(), json(&p)?);
            paths.insert(locale.clone(), p);
        }
        if !paths.contains_key(&manifest.game.source_locale) {
            bail!("E_LOCALE: source locale bundle is missing");
        }
        let mut unique = std::collections::BTreeSet::new();
        for p in paths.values().chain([&contract_path, &ledger_path]) {
            if !unique.insert(p) {
                bail!("E_TEXT_PATH: contract, locale bundles and revision record must use distinct files");
            }
        }
        let ledger: TextRevisions = json(&ledger_path)?;
        if ledger.format != 1 || ledger.source_locale != manifest.game.source_locale {
            bail!("E_TEXT_LEDGER: unsupported record or source locale changed");
        }
        let contracts: BTreeMap<String, AuthorTextContract> = json(&contract_path)?;
        let mut originals = BTreeMap::new();
        for p in paths.values().chain([&contract_path, &ledger_path]) {
            originals.insert(p.clone(), fs::read(p)?);
        }
        // Verify that the captured bytes describe the same input we parsed.
        if hash(&contracts)?
            != hash(&serde_json::from_slice::<
                BTreeMap<String, AuthorTextContract>,
            >(&originals[&contract_path])?)?
            || hash(&ledger)?
                != hash(&serde_json::from_slice::<TextRevisions>(
                    &originals[&ledger_path],
                )?)?
        {
            bail!("E_TEXT_TRANSACTION: inputs changed while reading");
        }
        for (locale, p) in &paths {
            if hash(&docs[locale])?
                != hash(&serde_json::from_slice::<BTreeMap<String, AuthorTextDoc>>(
                    &originals[p],
                )?)?
            {
                bail!("E_TEXT_TRANSACTION: inputs changed while reading");
            }
        }
        Ok(Self {
            root,
            module_id: m.id.clone(),
            namespaced,
            source: manifest.game.source_locale,
            contract_path,
            paths,
            ledger_path,
            contracts,
            docs,
            ledger,
            originals,
        })
    }
    fn report(&self) -> Result<TextStatus> {
        let mut issues = Vec::new();
        let mut issue = |code: &str, id: &str, locale: &str, path: &Path, message: &str| {
            issues.push(TextIssue {
                code: code.into(),
                text_id: if self.namespaced {
                    format!("{}.{}", self.module_id, id)
                } else {
                    id.into()
                },
                locale: locale.into(),
                file: path
                    .strip_prefix(&self.root)
                    .unwrap()
                    .to_string_lossy()
                    .into(),
                pointer: format!("/{}", id.replace('~', "~0").replace('/', "~1")),
                message: message.into(),
            });
        };
        for (id, c) in &self.contracts {
            let compiled = c.runtime();
            let record = self.ledger.texts.get(id);
            let confirmed = record.is_some_and(|r| {
                r.source_revision == c.source_revision
                    && r.contract_revision == c.contract_revision
                    && r.meaning_revision == c.meaning_revision
                    && r.contract_digest == compiled.contract_digest
            });
            if !confirmed
                || c.source_revision == 0
                || c.contract_revision == 0
                || c.meaning_revision == 0
            {
                issue(
                    "E_TEXT_UNRECORDED",
                    id,
                    &self.source,
                    &self.contract_path,
                    "contract/revisions changed; run text update with an explicit meaning decision",
                );
            }
            for (locale, docs) in &self.docs {
                let Some(d) = docs.get(id) else {
                    issue(
                        "E_TRANSLATION_MISSING",
                        id,
                        locale,
                        &self.paths[locale],
                        "missing text",
                    );
                    continue;
                };
                if let Err(e) = validate_text_spans(id, &compiled, &d.spans) {
                    issue(&e.code, id, locale, &self.paths[locale], &e.message);
                }
                if d.source_revision != c.source_revision
                    || d.contract_revision != c.contract_revision
                {
                    issue(
                        "E_TRANSLATION_STALE",
                        id,
                        locale,
                        &self.paths[locale],
                        &format!(
                            "expected source {} / contract {}, found source {} / contract {}",
                            c.source_revision,
                            c.contract_revision,
                            d.source_revision,
                            d.contract_revision
                        ),
                    );
                }
                if locale == &self.source {
                    if !record.is_some_and(|r| hash(&d.spans).is_ok_and(|h| h == r.source_digest)) {
                        issue(
                            "E_TEXT_SOURCE_CHANGED",
                            id,
                            locale,
                            &self.paths[locale],
                            "source text changed without a recorded revision; run text update",
                        );
                    }
                } else if !confirmed
                    || !record
                        .is_some_and(|r| hash(d).is_ok_and(|h| r.reviewed.get(locale) == Some(&h)))
                {
                    issue(
                        "E_TRANSLATION_UNREVIEWED",
                        id,
                        locale,
                        &self.paths[locale],
                        "translation needs explicit text review against the current source",
                    );
                }
            }
        }
        for (locale, docs) in &self.docs {
            for id in docs.keys().filter(|id| !self.contracts.contains_key(*id)) {
                issue(
                    "E_TEXT_CONTRACT",
                    id,
                    locale,
                    &self.paths[locale],
                    "text has no contract",
                );
            }
        }
        Ok(TextStatus {
            format: 1,
            source_locale: self.source.clone(),
            texts: self.contracts.len(),
            locales: self.docs.len(),
            ready: issues.is_empty(),
            issues,
        })
    }
    fn edits(&self) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
        let mut edits = BTreeMap::from([
            (self.contract_path.clone(), pretty(&self.contracts)?),
            (self.ledger_path.clone(), pretty(&self.ledger)?),
        ]);
        for (locale, docs) in &self.docs {
            edits.insert(self.paths[locale].clone(), pretty(docs)?);
        }
        Ok(edits)
    }
}
fn pretty<T: Serialize>(v: &T) -> Result<Vec<u8>> {
    let mut b = serde_json::to_vec_pretty(v)?;
    b.push(b'\n');
    Ok(b)
}
pub fn text_status(root: &Path) -> Result<TextStatus> {
    let root = fs::canonicalize(root)?;
    let (manifest, entries) = modules(&root)?;
    let namespaced = entries.len() > 1;
    let mut issues = Vec::new();
    let mut texts = 0;
    let mut expected_locales: Option<std::collections::BTreeSet<String>> = None;
    for (_, module) in entries {
        let source = Sources::load(&root, Some(&module.id))?;
        let actual_locales: std::collections::BTreeSet<_> = source.docs.keys().cloned().collect();
        if expected_locales
            .as_ref()
            .is_some_and(|expected| expected != &actual_locales)
        {
            bail!("E_TRANSLATION: all modules must provide the same locale bundles");
        }
        expected_locales = Some(actual_locales);
        let report = source.report()?;
        texts += report.texts;
        issues.extend(report.issues);
    }
    if namespaced {
        for issue in &mut issues {
            // Reports from Sources already qualify each module's IDs.
            if !issue.text_id.contains('.') {
                bail!("E_TEXT_ID: internal module-qualified diagnostic was lost");
            }
        }
    }
    Ok(TextStatus {
        format: 1,
        source_locale: manifest.game.source_locale,
        texts,
        locales: expected_locales.map_or(0, |locales| locales.len()),
        ready: issues.is_empty(),
        issues,
    })
}
pub(crate) struct CompiledTexts {
    pub contracts: BTreeMap<String, TextContract>,
    pub locales: BTreeMap<String, BTreeMap<String, TextDoc>>,
    pub module_texts: BTreeMap<String, std::collections::BTreeSet<String>>,
}
pub(crate) fn compiled_texts(root: &Path, namespaced: bool) -> Result<CompiledTexts> {
    let root = fs::canonicalize(root)?;
    let (_, entries) = modules(&root)?;
    if namespaced != (entries.len() > 1) {
        bail!("E_MODULE: inconsistent namespace mode");
    }
    let mut contracts = BTreeMap::new();
    let mut locales: BTreeMap<String, BTreeMap<String, TextDoc>> = BTreeMap::new();
    let mut module_texts = BTreeMap::new();
    let mut expected_locales: Option<std::collections::BTreeSet<String>> = None;
    for (_, module) in entries {
        let s = Sources::load(&root, Some(&module.id))?;
        let module_locales: std::collections::BTreeSet<_> = s.docs.keys().cloned().collect();
        if expected_locales
            .as_ref()
            .is_some_and(|expected| expected != &module_locales)
        {
            bail!("E_TRANSLATION: all modules must provide the same locale bundles");
        }
        expected_locales = Some(module_locales);
        let report = s.report()?;
        if let Some(i) = report.issues.first() {
            let mut d = Diagnostic::new(&i.code, &i.text_id, &i.message).classified(
                ErrorDomain::Content,
                "text",
                "revision",
                vec![Recovery::FixContent],
            );
            if let Some(details) = &mut d.details {
                details.source = Some(crate::diagnostics::text_source(
                    &i.file,
                    &i.pointer,
                    &s.originals[&s.root.join(&i.file)],
                ));
                details.references = vec![i.text_id.clone(), i.locale.clone()];
                details.hint = Some("Run novelc text status --json; update the source revision or explicitly review the translation.".into());
            }
            return Err(d.into());
        }
        for (id, c) in &s.contracts {
            let key = if namespaced {
                format!("{}.{}", module.id, id)
            } else {
                id.clone()
            };
            if contracts.insert(key.clone(), c.runtime()).is_some() {
                bail!("E_DUPLICATE: text contract {key}");
            }
            module_texts
                .entry(module.id.clone())
                .or_insert_with(std::collections::BTreeSet::new)
                .insert(key);
        }
        for (locale, docs) in s.docs {
            let out = locales.entry(locale).or_default();
            for (id, d) in docs {
                let key = if namespaced {
                    format!("{}.{}", module.id, id)
                } else {
                    id
                };
                let contract = contracts
                    .get(&key)
                    .ok_or_else(|| anyhow!("E_TEXT_CONTRACT: {key}"))?;
                if out
                    .insert(
                        key.clone(),
                        TextDoc {
                            source_revision: d.source_revision,
                            contract_revision: d.contract_revision,
                            contract_digest: contract.contract_digest.clone(),
                            spans: d.spans,
                        },
                    )
                    .is_some()
                {
                    bail!("E_DUPLICATE: localized text {key}");
                }
            }
        }
    }
    Ok(CompiledTexts {
        contracts,
        locales,
        module_texts,
    })
}
fn next(n: u32) -> Result<u32> {
    n.checked_add(1)
        .ok_or_else(|| anyhow!("E_TEXT_REVISION: revision overflow"))
}
fn load_for_text_id(root: &Path, id: &str) -> Result<(Sources, String)> {
    let root = fs::canonicalize(root)?;
    let (_, entries) = modules(&root)?;
    if entries.len() == 1 {
        let module = &entries[0].1;
        return Ok((Sources::load(&root, Some(&module.id))?, id.to_owned()));
    }
    let (module_id, local_id) = id
        .split_once('.')
        .ok_or_else(|| anyhow!("E_TEXT_ID: multi-module text IDs use module.text form"))?;
    if !entries.iter().any(|(_, module)| module.id == module_id) || local_id.is_empty() {
        bail!("E_TEXT_ID: unknown module/text {id}");
    }
    Ok((Sources::load(&root, Some(module_id))?, local_id.to_owned()))
}
/// An explicit content decision; never infer meaning changes from text hashes.
pub fn text_update(root: &Path, id: &str, meaning_changed: bool) -> Result<()> {
    let (mut s, id) = load_for_text_id(root, id)?;
    let c = s
        .contracts
        .get_mut(&id)
        .ok_or_else(|| anyhow!("E_TEXT_ID: unknown contract {id}"))?;
    let d = s
        .docs
        .get_mut(&s.source)
        .unwrap()
        .get_mut(&id)
        .ok_or_else(|| anyhow!("E_TRANSLATION_MISSING: source {id}"))?;
    validate_text_spans(&id, &c.runtime(), &d.spans)?;
    let source_hash = hash(&d.spans)?;
    let shape_hash = shape(c)?;
    if let Some(old) = s.ledger.texts.get(&id) {
        if (c.source_revision, c.contract_revision, c.meaning_revision)
            != (
                old.source_revision,
                old.contract_revision,
                old.meaning_revision,
            )
            || (d.source_revision, d.contract_revision)
                != (old.source_revision, old.contract_revision)
        {
            bail!("E_TEXT_REVISION: {id}: leave recorded integers unchanged; text update increments them");
        }
        let contract_changed = old.shape_digest != shape_hash;
        if contract_changed && !meaning_changed {
            bail!("E_TEXT_MEANING: Gate/parameter contract changes require --meaning bump");
        }
        if source_hash == old.source_digest && !contract_changed && !meaning_changed {
            return Ok(());
        }
        c.source_revision = next(c.source_revision)?;
        if contract_changed || meaning_changed {
            c.contract_revision = next(c.contract_revision)?;
            c.meaning_revision = next(c.meaning_revision)?;
        }
    } else if (c.source_revision, c.contract_revision, c.meaning_revision) != (1, 1, 1)
        || (d.source_revision, d.contract_revision) != (1, 1)
    {
        bail!("E_TEXT_REVISION: new text must begin at revisions 1");
    }
    d.source_revision = c.source_revision;
    d.contract_revision = c.contract_revision;
    s.ledger.texts.insert(
        id.clone(),
        RevisionRecord {
            source_revision: c.source_revision,
            contract_revision: c.contract_revision,
            meaning_revision: c.meaning_revision,
            source_digest: source_hash,
            contract_digest: c.runtime().contract_digest,
            shape_digest: shape_hash,
            reviewed: BTreeMap::new(),
        },
    );
    commit(&s.root, s.edits()?, &s.originals)
}
pub fn text_review(root: &Path, id: &str, locale: &str) -> Result<()> {
    let (mut s, id) = load_for_text_id(root, id)?;
    if locale == s.source {
        bail!("E_TEXT_REVIEW: use text update for the source locale");
    }
    let c = s
        .contracts
        .get(&id)
        .ok_or_else(|| anyhow!("E_TEXT_ID: unknown contract {id}"))?;
    if s.report()?.issues.iter().any(|i| {
        (i.text_id == id || i.text_id.ends_with(&format!(".{id}"))) && i.locale == s.source
    }) {
        bail!("E_TEXT_SOURCE_CHANGED: record valid source {id} before reviewing translations");
    }
    let d = s
        .docs
        .get_mut(locale)
        .ok_or_else(|| anyhow!("E_LOCALE: unknown {locale}"))?
        .get_mut(&id)
        .ok_or_else(|| anyhow!("E_TRANSLATION_MISSING: {locale}/{id}"))?;
    validate_text_spans(&id, &c.runtime(), &d.spans)?;
    d.source_revision = c.source_revision;
    d.contract_revision = c.contract_revision;
    s.ledger
        .texts
        .get_mut(&id)
        .ok_or_else(|| anyhow!("E_TEXT_UNRECORDED: {id}"))?
        .reviewed
        .insert(locale.into(), hash(d)?);
    commit(&s.root, s.edits()?, &s.originals)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    path: String,
    before: String,
    after: String,
}
fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut tmp = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path)?;
    Ok(())
}
fn commit(
    root: &Path,
    edits: BTreeMap<PathBuf, Vec<u8>>,
    originals: &BTreeMap<PathBuf, Vec<u8>>,
) -> Result<()> {
    let journal = journal_path(root)?;
    no_transaction(root)?;
    let mut changes = Vec::new();
    for (path, after) in edits {
        let before = originals[&path].clone();
        if fs::read(&path)? != before {
            bail!("E_TEXT_TRANSACTION: inputs changed before commit");
        }
        if serde_json::from_slice::<serde_json::Value>(&before)?
            != serde_json::from_slice::<serde_json::Value>(&after)?
        {
            changes.push(Edit {
                path: path.strip_prefix(root)?.to_string_lossy().into(),
                before: String::from_utf8(before)?,
                after: String::from_utf8(after)?,
            });
        }
    }
    if changes.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(journal.parent().unwrap())?;
    // Exclusive creation serializes author-tool writers. An interrupted commit
    // fails closed until recover restores the complete original file set.
    let payload = pretty(&changes)?;
    if payload.len() > 64 * 1024 * 1024 {
        bail!("E_LIMIT: text transaction exceeds 64 MiB; split text bundles before updating");
    }
    let mut f = tempfile::NamedTempFile::new_in(journal.parent().unwrap())?;
    f.write_all(&payload)?;
    f.as_file().sync_all()?;
    f.persist_noclobber(&journal)
        .context("E_TEXT_TRANSACTION: another text edit is active")?;
    for e in &changes {
        let path = relative(root, root, &e.path)?;
        if fs::read(&path)? != e.before.as_bytes() {
            bail!("E_TEXT_TRANSACTION: concurrent edit; inspect journal and run text recover");
        }
        atomic(&path, e.after.as_bytes())?;
    }
    fs::remove_file(journal)?;
    Ok(())
}
pub fn text_recover(root: &Path) -> Result<()> {
    let root = fs::canonicalize(root)?;
    let journal = journal_path(&root)?;
    if !journal.exists() {
        return Ok(());
    }
    let path = relative(&root, &root, ".nir/text-transaction.json")?;
    if fs::metadata(&path)?.len() > 64 * 1024 * 1024 {
        bail!("E_LIMIT: text transaction exceeds 64 MiB");
    }
    let edits: Vec<Edit> = serde_json::from_slice(&fs::read(path)?)?;
    for e in &edits {
        let p = relative(&root, &root, &e.path)?;
        let b = fs::read(p)?;
        if b != e.before.as_bytes() && b != e.after.as_bytes() {
            bail!(
                "E_TEXT_TRANSACTION: {} has additional edits; preserve them before recovery",
                e.path
            );
        }
    }
    for e in edits {
        atomic(&relative(&root, &root, &e.path)?, e.before.as_bytes())?;
    }
    fs::remove_file(journal)?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OldContract {
    revision: u32,
    #[serde(default)]
    gates: Vec<String>,
    #[serde(default)]
    params: BTreeMap<String, ValueType>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OldDoc {
    revision: u32,
    spans: Vec<Span>,
}
/// Migrate into a separate directory, preserving source and the initial review
/// baseline. Never rewrite legacy saves or touch the original project.
pub fn text_migrate(root: &Path, out: &Path) -> Result<()> {
    let root = fs::canonicalize(root)?;
    no_transaction(&root)?;
    let (manifest, module_path, m) = module(&root)?;
    if m.text_revisions.is_some() {
        bail!("E_TEXT_MIGRATION: project already has revision records");
    }
    if out.exists() {
        bail!("E_EXISTS: migration destination exists");
    }
    let parent = fs::canonicalize(
        out.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?;
    if parent.starts_with(&root) {
        bail!("E_PATH: migration destination must be outside the source project");
    }
    let target = parent.join(
        out.file_name()
            .ok_or_else(|| anyhow!("E_PATH: destination name"))?,
    );
    let base = module_path.parent().unwrap();
    let contract_path = relative(&root, base, &m.text_contracts)?;
    let old: BTreeMap<String, OldContract> = json(&contract_path)?;
    let contracts: BTreeMap<_, _> = old
        .iter()
        .map(|(id, c)| {
            (
                id.clone(),
                AuthorTextContract {
                    source_revision: c.revision,
                    contract_revision: c.revision,
                    meaning_revision: c.revision,
                    gates: c.gates.clone(),
                    params: c.params.clone(),
                },
            )
        })
        .collect();
    let mut docs = BTreeMap::new();
    let mut paths = BTreeMap::new();
    for (locale, name) in &m.text_bundles {
        let path = relative(&root, base, name)?;
        let old_docs: BTreeMap<String, OldDoc> = json(&path)?;
        if old_docs.len() != contracts.len() {
            bail!("E_TEXT_MIGRATION: incomplete/extra legacy translations in {locale}");
        }
        let mut converted = BTreeMap::new();
        for (id, c) in &contracts {
            let d = old_docs
                .get(id)
                .ok_or_else(|| anyhow!("E_TRANSLATION_MISSING: {locale}/{id}"))?;
            if c.source_revision == 0 || d.revision != c.source_revision {
                bail!("E_TEXT_REVISION: legacy {locale}/{id}");
            }
            validate_text_spans(id, &c.runtime(), &d.spans)?;
            converted.insert(
                id.clone(),
                AuthorTextDoc {
                    source_revision: d.revision,
                    contract_revision: d.revision,
                    spans: d.spans.clone(),
                },
            );
        }
        docs.insert(locale.clone(), converted);
        paths.insert(locale.clone(), path);
    }
    let source = docs
        .get(&manifest.game.source_locale)
        .ok_or_else(|| anyhow!("E_LOCALE: missing legacy source"))?;
    let mut ledger = TextRevisions {
        format: 1,
        source_locale: manifest.game.source_locale.clone(),
        texts: BTreeMap::new(),
    };
    for (id, c) in &contracts {
        let reviewed = docs
            .iter()
            .filter(|(locale, _)| *locale != &manifest.game.source_locale)
            .map(|(locale, d)| Ok((locale.clone(), hash(&d[id])?)))
            .collect::<Result<_>>()?;
        ledger.texts.insert(
            id.clone(),
            RevisionRecord {
                source_revision: c.source_revision,
                contract_revision: c.contract_revision,
                meaning_revision: c.meaning_revision,
                source_digest: hash(&source[id].spans)?,
                contract_digest: c.runtime().contract_digest,
                shape_digest: shape(c)?,
                reviewed,
            },
        );
    }
    let ledger_path = contract_path.parent().unwrap().join("revisions.json");
    if ledger_path.exists() {
        bail!("E_EXISTS: texts/revisions.json would be overwritten");
    }
    let mut module_value: toml::Value = toml_file(&module_path)?;
    module_value.as_table_mut().unwrap().insert(
        "text_revisions".into(),
        toml::Value::String(ledger_path.strip_prefix(base)?.to_string_lossy().into()),
    );
    let stage = tempfile::tempdir_in(&parent)?;
    crate::copy_tree(&root, stage.path())?;
    for (path, bytes) in [
        (contract_path, pretty(&contracts)?),
        (ledger_path, pretty(&ledger)?),
        (
            module_path,
            toml::to_string_pretty(&module_value)?.into_bytes(),
        ),
    ] {
        fs::write(stage.path().join(path.strip_prefix(&root)?), bytes)?;
    }
    for (locale, path) in paths {
        fs::write(
            stage.path().join(path.strip_prefix(&root)?),
            pretty(&docs[&locale])?,
        )?;
    }
    let status = text_status(stage.path())?;
    if !status.ready {
        bail!("E_TEXT_MIGRATION: candidate failed text validation");
    }
    crate::write_schemas(&stage.path().join("schemas"))?;
    fs::rename(stage.path(), target)?;
    Ok(())
}
