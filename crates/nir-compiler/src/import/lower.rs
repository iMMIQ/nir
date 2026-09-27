use super::{
    lsb::{Body, Command, Glyph, Literal, Reference, Script},
    Source,
};
use anyhow::{bail, ensure, Result};
use nir_format::Span;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

type Point = (String, usize);
fn id(point: &Point) -> String {
    format!(
        "s{}_c{}",
        &nir_content::digest(point.0.as_bytes())[..16],
        point.1
    )
}
fn block(term: Value) -> Value {
    json!({"ops":[], "terminator":term})
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportDiagnostic {
    pub severity: String,
    pub source: String,
    pub index: usize,
    pub line: u32,
    pub byte: usize,
    pub command: String,
    pub message: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct SourceLocation {
    pub source: String,
    pub index: usize,
    pub line: u32,
    pub byte: usize,
    pub command: String,
}
#[derive(Debug, Serialize)]
pub struct ImportReport {
    pub format: u32,
    pub engine: String,
    pub status: String,
    pub written: bool,
    pub errors: usize,
    pub text_pages: usize,
    pub functions: usize,
    pub coverage: String,
    pub diagnostics: Vec<ImportDiagnostic>,
    pub source_map: BTreeMap<String, SourceLocation>,
}

pub(super) struct Lowering {
    source: Source,
    functions: BTreeMap<String, Value>,
    cues: BTreeMap<String, Value>,
    pub texts: BTreeMap<String, crate::AuthorTextDoc>,
    locations: BTreeMap<String, SourceLocation>,
    diagnostics: Vec<ImportDiagnostic>,
    diagnostic_keys: BTreeSet<(Point, String)>,
    pending: VecDeque<Point>,
    scheduled: BTreeSet<Point>,
    blocks: usize,
}
impl Lowering {
    pub fn new(source: Source) -> Self {
        Self {
            source,
            functions: BTreeMap::new(),
            cues: BTreeMap::new(),
            texts: BTreeMap::new(),
            locations: BTreeMap::new(),
            diagnostics: vec![],
            diagnostic_keys: BTreeSet::new(),
            pending: VecDeque::new(),
            scheduled: BTreeSet::new(),
            blocks: 0,
        }
    }
    fn schedule(&mut self, point: Point) -> Result<String> {
        if self.scheduled.insert(point.clone()) {
            ensure!(
                self.scheduled.len() <= 1024,
                "E_IMPORT_LIMIT: too many call entries"
            );
            self.pending.push_back(point.clone());
        }
        Ok(format!("f_{}", id(&point)))
    }
    fn diagnostic(&mut self, point: &Point, c: &Command, message: String, severity: &str) {
        if self
            .diagnostic_keys
            .insert((point.clone(), message.clone()))
        {
            self.diagnostics.push(ImportDiagnostic {
                severity: severity.into(),
                source: point.0.clone(),
                index: point.1,
                line: c.line,
                byte: c.offset,
                command: c.name().into(),
                message,
            });
        }
    }
    fn target(&mut self, page: &str, reference: &Reference) -> Result<Point> {
        let name = if reference.page.is_empty() {
            page
        } else {
            &reference.page
        };
        let name = if name.to_ascii_lowercase().ends_with(".lsc") {
            format!("{}.lsb", &name[..name.len() - 4])
        } else {
            name.into()
        };
        let (page, script) = self.source.read(&name)?;
        Ok((page, label_index(&script, reference.line)?))
    }
    pub fn run(&mut self, entry: &str, line: u32) -> Result<Value> {
        let (page, script) = self.source.read(entry)?;
        let function = self.schedule((page, label_index(&script, line)?))?;
        while let Some(start) = self.pending.pop_front() {
            self.function(start)?;
        }
        self.functions.insert(
            "main".into(),
            json!({"entry":"start", "blocks":{
            "start":block(json!({"type":"call","function":function,"next":"end"})),
            "end":block(json!({"type":"end","outcome":"completed"}))}}),
        );
        Ok(json!({"fragment_format":1,"functions":self.functions,"cues":self.cues}))
    }
    pub fn report(&self) -> ImportReport {
        let errors = self
            .diagnostics
            .iter()
            .filter(|d| d.severity == "error")
            .count();
        ImportReport { format: 1, engine: "livemaker".into(), status: if errors > 0 { "blocked" } else { "converted" }.into(),
            written: false, errors, text_pages: self.texts.len(), functions: self.functions.len(),
            coverage: "Reachable control flow only. Unsupported instructions terminate analysis on their path. NIR reader presentation replaces source typography; no source media are copied.".into(),
            diagnostics: self.diagnostics.clone(), source_map: self.locations.clone() }
    }
    fn function(&mut self, start: Point) -> Result<()> {
        let mut blocks = BTreeMap::new();
        let mut queue = VecDeque::from([start.clone()]);
        let mut visited = BTreeSet::new();
        while let Some(point) = queue.pop_front() {
            if !visited.insert(point.clone()) {
                continue;
            }
            self.blocks += 1;
            ensure!(
                self.blocks <= 100_000,
                "E_IMPORT_LIMIT: too many generated blocks"
            );
            let (_, script) = self.source.read(&point.0)?;
            if point.1 == script.commands.len() {
                blocks.insert(id(&point), block(json!({"type":"return"})));
                continue;
            }
            let c = &script.commands[point.1];
            self.locations.insert(
                id(&point),
                SourceLocation {
                    source: point.0.clone(),
                    index: point.1,
                    line: c.line,
                    byte: c.offset,
                    command: c.name().into(),
                },
            );
            let term = match self.command(&point, c, &mut queue, &mut blocks) {
                Ok(term) => term,
                Err(error) => {
                    let message = format!("{error:#}");
                    self.diagnostic(&point, c, message.clone(), "error");
                    json!({"type":"fault","code":"E_IMPORT_UNSUPPORTED","message":format!("{}:{} ({}): {}", point.0, c.line, c.name(), message)})
                }
            };
            blocks.insert(id(&point), block(term));
        }
        blocks.insert(
            "cancelled".into(),
            block(json!({"type":"end","outcome":"cancelled"})),
        );
        blocks.insert("failed".into(), block(json!({"type":"fault","code":"E_IMPORT_TASK","message":"Imported dialogue task failed"})));
        self.functions.insert(
            format!("f_{}", id(&start)),
            json!({"entry":id(&start),"blocks":blocks}),
        );
        Ok(())
    }
    fn command(
        &mut self,
        point: &Point,
        c: &Command,
        queue: &mut VecDeque<Point>,
        blocks: &mut BTreeMap<String, Value>,
    ) -> Result<Value> {
        let next = (point.0.clone(), point.1 + 1);
        if c.muted || matches!(c.kind, 3 | 27) {
            return Ok(goto(next, queue));
        }
        ensure!(
            c.indent == 0,
            "E_IMPORT_SCOPE: structured command scope requires adaptation"
        );
        match &c.body {
            Body::Jump(target, condition) => {
                let target = if condition.flag()? {
                    self.target(&point.0, target)?
                } else {
                    next
                };
                Ok(goto(target, queue))
            }
            Body::Call {
                target,
                condition,
                has_params,
                ..
            } => {
                if !condition.flag()? {
                    return Ok(goto(next, queue));
                }
                ensure!(
                    !has_params,
                    "E_IMPORT_CALL: parameters/results require adaptation"
                );
                let target = self.target(&point.0, target)?;
                let function = self.schedule(target)?;
                queue.push_back(next.clone());
                Ok(json!({"type":"call","function":function,"next":id(&next)}))
            }
            Body::Exit(condition) => Ok(if condition.flag()? {
                json!({"type":"return"})
            } else {
                goto(next, queue)
            }),
            Body::Text {
                text,
                target,
                history,
                wait,
                stop,
            } => {
                ensure!(
                    wait.flag()? && stop.flag()?,
                    "E_IMPORT_ASYNC_TEXT: asynchronous TextIns requires event-system adaptation"
                );
                ensure!(
                    history.flag()?,
                    "E_IMPORT_HISTORY: hidden-history dialogue requires adaptation"
                );
                ensure!(
                    matches!(&target.literal, Some(Literal::String(name)) if !name.is_empty()),
                    "E_IMPORT_TEXT_TARGET: expected a literal message box name"
                );
                ensure!(
                    !text.has_conditions_or_links,
                    "E_IMPORT_TEXT_LINK: conditional/interactive text requires adaptation"
                );
                ensure!(
                    !text.has_ruby,
                    "E_IMPORT_RUBY: ruby text requires adaptation"
                );
                let mut pages = vec![];
                let mut spans = vec![];
                let mut buffer = String::new();
                for glyph in &text.glyphs {
                    match glyph {
                        Glyph::Char(s) => buffer.push_str(s),
                        Glyph::Break(0) => { flush(&mut buffer, &mut spans); spans.push(Span::Break { id: format!("span{}", spans.len()) }); }
                        Glyph::Break(1) => {
                            flush(&mut buffer, &mut spans);
                            ensure!(!spans.is_empty(), "E_IMPORT_EMPTY_PAGE: empty page wait requires adaptation");
                            pages.push(std::mem::take(&mut spans));
                        }
                        _ => bail!("E_IMPORT_GLYPH: event, pause, clear, variable, layout or image glyph requires adaptation"),
                    }
                }
                flush(&mut buffer, &mut spans);
                if !spans.is_empty() {
                    pages.push(spans);
                }
                self.diagnostic(
                    point,
                    c,
                    "NIR reader replaces source message-box layout, styles and reveal speed".into(),
                    "warning",
                );
                let count = pages.len();
                for (number, spans) in pages.into_iter().enumerate() {
                    let tid = format!("{}_p{number}", id(point));
                    self.locations
                        .insert(tid.clone(), self.locations[&id(point)].clone());
                    self.texts.insert(
                        tid.clone(),
                        crate::AuthorTextDoc {
                            source_revision: 1,
                            contract_revision: 1,
                            spans,
                        },
                    );
                    self.cues.insert(tid.clone(), json!({"effects":[{"id":"line","scope":"interaction","effect":{"type":"dialogue","text":tid,"speaker":"","reveal_us":"32000"}}]}));
                    let wait = format!("{tid}_wait");
                    blocks.insert(
                        tid.clone(),
                        block(json!({"type":"activate","cue":tid,"next":wait})),
                    );
                    let next = if number + 1 < count {
                        format!("{}_p{}", id(point), number + 1)
                    } else {
                        id(&next)
                    };
                    blocks.insert(wait, block(json!({"type":"await","conditions":[{"task":"line","milestone":{"type":"finished"}}],"next":next,"on_cancelled":"cancelled","on_failed":"failed"})));
                }
                queue.push_back(next.clone());
                Ok(
                    json!({"type":"goto","target":if count > 0 { format!("{}_p0", id(point)) } else { id(&next) }}),
                )
            }
            _ if c.kind == 45 => Ok(json!({"type":"end","outcome":"completed"})),
            _ => bail!("E_IMPORT_COMMAND: unsupported {}", c.name()),
        }
    }
}
fn label_index(script: &Script, line: u32) -> Result<usize> {
    if line == 0 {
        return Ok(0);
    }
    let labels: Vec<_> = script
        .commands
        .iter()
        .enumerate()
        .filter(|(_, c)| c.kind == 3 && c.line == line)
        .collect();
    ensure!(
        labels.len() == 1,
        "E_IMPORT_LABEL: expected one Label with source LineNo {line}, found {}",
        labels.len()
    );
    Ok(labels[0].0)
}
fn goto(point: Point, queue: &mut VecDeque<Point>) -> Value {
    let target = id(&point);
    queue.push_back(point);
    json!({"type":"goto","target":target})
}
fn flush(buffer: &mut String, spans: &mut Vec<Span>) {
    if !buffer.is_empty() {
        spans.push(Span::Text {
            id: format!("span{}", spans.len()),
            text: std::mem::take(buffer),
            emphasis: false,
        });
    }
}
