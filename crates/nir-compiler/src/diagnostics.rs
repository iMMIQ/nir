//! Author-only source index. It never enters Program, snapshots or release objects.
use nir_format::*;
use std::{collections::BTreeMap, path::Path};

#[derive(Default)]
pub(crate) struct SourceIndex {
    declarations: BTreeMap<String, SourceRef>,
    references: BTreeMap<(String, String), SourceRef>,
}
impl SourceIndex {
    pub fn fragment(&mut self, root: &Path, path: &Path, bytes: &[u8]) {
        // Called only after strict parsing: duplicates, depth and syntax are checked there.
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
            return;
        };
        let mut offsets = BTreeMap::new();
        scan(bytes, &mut 0, String::new(), &mut offsets);
        let file = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
        let source = |pointer: String| {
            let offset = offsets.get(&pointer).copied().unwrap_or(0);
            let prefix = &bytes[..offset];
            SourceRef {
                file: file.clone(),
                line: 1 + prefix.iter().filter(|b| **b == b'\n').count(),
                column: offset
                    - prefix
                        .iter()
                        .rposition(|b| *b == b'\n')
                        .map_or(0, |p| p + 1)
                    + 1,
                pointer,
            }
        };
        for table in ["variables", "functions", "scenes", "cues", "choices"] {
            if let Some(items) = value[table].as_object() {
                for (id, item) in items {
                    let pointer = format!("/{table}/{}", escape(id));
                    self.declarations
                        .insert(format!("{table}:{id}"), source(pointer.clone()));
                    self.declarations
                        .entry(id.clone())
                        .or_insert_with(|| source(pointer.clone()));
                    let prefix = format!("{pointer}/");
                    for (path, _) in offsets
                        .range(prefix.clone()..)
                        .take_while(|(path, _)| path.starts_with(&prefix))
                    {
                        if [
                            "asset", "scene", "node", "parent", "text", "speaker", "cue", "target",
                            "function", "choice",
                        ]
                        .contains(&path.rsplit('/').next().unwrap_or_default())
                        {
                            if let Some(reference) = value.pointer(path).and_then(|v| v.as_str()) {
                                self.references
                                    .entry((format!("{table}:{id}"), reference.into()))
                                    .or_insert_with(|| source(path.clone()));
                            }
                        }
                    }
                    if table == "functions" {
                        if let Some(blocks) = item["blocks"].as_object() {
                            for (bid, block) in blocks {
                                let bp = format!("{pointer}/blocks/{}", escape(bid));
                                self.declarations
                                    .insert(format!("{id}/{bid}"), source(bp.clone()));
                                if let Some(ops) = block["ops"].as_array() {
                                    for (i, op) in ops.iter().enumerate() {
                                        if let Some(oid) = op["id"].as_str() {
                                            self.declarations.insert(
                                                oid.into(),
                                                source(format!("{bp}/ops/{i}")),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    pub fn annotate(&self, mut d: Diagnostic) -> Diagnostic {
        if d.details.is_none() {
            d = d.classified(
                ErrorDomain::Content,
                "check",
                "validate",
                vec![Recovery::FixContent],
            );
        }
        let details = d.details.as_mut().unwrap();
        let key = format!("{}:{}", details.operation, d.location);
        details.source = self
            .references
            .get(&(key.clone(), d.message.clone()))
            .or_else(|| self.declarations.get(&key))
            .or_else(|| self.declarations.get(&d.location))
            .cloned();
        details.references = vec![d.location.clone(), d.message.clone()];
        details.hint = Some(match d.code.as_str() {
            "E_ASSET_TYPE" => "Register the referenced resource in the asset catalog with the required type, or correct the reference.",
            "E_BLOCK" => "Declare the target block in this function or correct its logical ID.",
            "E_TRANSLATION" | "E_TEXT_CONTRACT" | "E_GATE" => "Check the locale bundle against the text contract and preserve Gate order.",
            "E_UNINITIALIZED" => "Assign the local on every incoming control-flow path before reading it.",
            _ => "Check this logical declaration and its references against the project schema.",
        }.into());
        d
    }
}
fn escape(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}
fn whitespace(b: &[u8], p: &mut usize) {
    while *p < b.len() && b[*p].is_ascii_whitespace() {
        *p += 1;
    }
}
fn string(b: &[u8], p: &mut usize) -> String {
    let start = *p;
    *p += 1;
    while *p < b.len() {
        match b[*p] {
            b'\\' => *p += 2,
            b'"' => {
                *p += 1;
                break;
            }
            _ => *p += 1,
        }
    }
    serde_json::from_slice(&b[start..*p]).unwrap_or_default()
}
fn scan(b: &[u8], p: &mut usize, pointer: String, out: &mut BTreeMap<String, usize>) {
    whitespace(b, p);
    out.insert(pointer.clone(), *p);
    match b.get(*p) {
        Some(b'{') => {
            *p += 1;
            whitespace(b, p);
            while b.get(*p) != Some(&b'}') {
                let key = string(b, p);
                whitespace(b, p);
                *p += 1;
                scan(b, p, format!("{pointer}/{}", escape(&key)), out);
                whitespace(b, p);
                if b.get(*p) != Some(&b',') {
                    break;
                }
                *p += 1;
                whitespace(b, p);
            }
            *p += 1;
        }
        Some(b'[') => {
            *p += 1;
            whitespace(b, p);
            let mut i = 0;
            while b.get(*p) != Some(&b']') {
                scan(b, p, format!("{pointer}/{i}"), out);
                i += 1;
                whitespace(b, p);
                if b.get(*p) != Some(&b',') {
                    break;
                }
                *p += 1;
            }
            *p += 1;
        }
        Some(b'"') => {
            string(b, p);
        }
        _ => {
            while *p < b.len() && !b[*p].is_ascii_whitespace() && !b",]}".contains(&b[*p]) {
                *p += 1;
            }
        }
    }
}

/// Preserve typed diagnostics; older tool boundaries still receive a stable envelope.
pub fn diagnostic(error: &anyhow::Error) -> Diagnostic {
    if let Some(d) = error.downcast_ref::<Diagnostic>() {
        return d.clone();
    }
    let message = format!("{error:#}");
    let code = message
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .find(|s| s.starts_with("E_"))
        .unwrap_or("E_TOOL");
    Diagnostic::new(code, "project", &message).classified(
        ErrorDomain::Content,
        "project",
        "tool",
        vec![Recovery::FixContent],
    )
}
