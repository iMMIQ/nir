//! Declarative narrative script front-end (`.nirs`).
//!
//! Compiles human-friendly `.nirs` source files statically at compile time into
//! standard NIR IR (`Fragment` + `TextContract`/`TextDoc`s) with zero runtime
//! overhead and deterministic IDs.
//!
//! Supported directives:
//! - `# comment` and blank lines
//! - `@var <name> = <literal>` (`true`, `false`, `i32`, or `"string"`)
//! - `@scene <scene_id>` followed by:
//!   - `@bg <asset_id> [x y w h]`
//!   - `@sprite <node_id> <asset_id> [x y w h] [order=<i32>] [opacity=<f32>] [scale=<f32>]`
//! - `@function <name>` (defaults to `main`)
//! - `@block <name>` (defaults to `start`)
//! - `@show <scene_id> [cut|dissolve [duration_ms]]`
//! - `@bgm <asset_id> [gain=<f32>]`
//! - `@sfx <asset_id> [gain=<f32>]`
//! - `@stop_bgm [fade_ms]` / `@stop_audio <task_id> [fade_ms]`
//! - `@wait <duration_ms>`
//! - `@set <var> = <expr>`
//! - `@unlock <profile_key>`
//! - `@if <expr> -> <yes_block> else <no_block>`
//! - `@jump <block>`
//! - `@call <function> [-> <next_block>]`
//! - `@return [<expr>]`
//! - `@choice <choice_id>` followed by `- <label> -> <target_block> [if <expr>]`
//! - `@end <outcome>`
//! - `Speaker: Dialogue line` (ASCII `:` or full-width `：`) or `"Speaker Name": Dialogue line`
//! - `"Narration text"` or `> Narration text`

use crate::Fragment;
use anyhow::{anyhow, bail, Result};
use nir_format::*;
use std::{collections::BTreeMap, path::Path};

#[derive(Debug)]
pub struct NirsOutput {
    pub fragment: Fragment,
    pub text_contracts: BTreeMap<String, TextContract>,
    pub text_docs: BTreeMap<String, TextDoc>,
}

/// Lower a `.nirs` script into a NIR `Fragment` and its generated text
/// contracts/documents. Text IDs are emitted unqualified; `project.rs`
/// qualifies them when compiling multi-module projects.
pub fn lower_nirs(path: &Path, source: &str, stage: &Stage) -> Result<NirsOutput> {
    let stem = sanitize_stem(
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("script"),
    );
    let mut ctx = Lowerer::new(path, stem, stage.clone());
    ctx.parse_all(source)?;
    ctx.finish()
}

fn sanitize_stem(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "script".into()
    } else {
        out
    }
}

fn valid_ident(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

fn make_text(text: &str) -> (TextContract, TextDoc) {
    let mut contract = TextContract {
        source_revision: 1,
        contract_revision: 1,
        meaning_revision: 1,
        contract_digest: String::new(),
        gates: vec![],
        pauses: vec![],
        images: vec![],
        params: BTreeMap::new(),
    };
    contract.contract_digest = text_contract_digest(&contract);
    let doc = TextDoc {
        source_revision: 1,
        contract_revision: 1,
        contract_digest: contract.contract_digest.clone(),
        spans: vec![Span::Text {
            id: "s0".into(),
            text: text.to_owned(),
            emphasis: false,
        }],
    };
    (contract, doc)
}

#[derive(Debug)]
struct OpenBlock {
    name: String,
    ops: Vec<Op>,
}

#[derive(Debug)]
struct FunctionBuilder {
    name: String,
    entry: Option<String>,
    blocks: BTreeMap<String, Block>,
    current: Option<OpenBlock>,
    needs_fault_blocks: bool,
    step_seq: usize,
}

impl FunctionBuilder {
    fn new(name: String) -> Self {
        Self {
            name,
            entry: None,
            blocks: BTreeMap::new(),
            current: None,
            needs_fault_blocks: false,
            step_seq: 0,
        }
    }

    fn ensure_open(&mut self) {
        if self.current.is_none() {
            let name = if self.entry.is_none() {
                self.entry = Some("start".into());
                "start".to_owned()
            } else {
                self.fresh_block("cont")
            };
            self.current = Some(OpenBlock {
                name,
                ops: Vec::new(),
            });
        }
    }

    fn fresh_block(&mut self, prefix: &str) -> String {
        self.step_seq += 1;
        format!("__nirs_{prefix}_{}", self.step_seq)
    }

    fn seal_current(&mut self, terminator: Terminator, path: &Path, line: usize) -> Result<()> {
        let block = self.current.take().ok_or_else(|| {
            anyhow!(
                "E_NIRS_SEMANTIC: {}:{}: statement outside any open block",
                path.display(),
                line
            )
        })?;
        if self
            .blocks
            .insert(
                block.name.clone(),
                Block {
                    ops: block.ops,
                    terminator,
                },
            )
            .is_some()
        {
            bail!(
                "E_NIRS_DUPLICATE: {}:{}: duplicate block `{}` in function `{}`",
                path.display(),
                line,
                block.name,
                self.name
            );
        }
        Ok(())
    }

    fn start_named_block(&mut self, name: String, path: &Path, line: usize) -> Result<()> {
        if let Some(open) = self.current.take() {
            let terminator = Terminator::Goto {
                target: name.clone(),
            };
            if self
                .blocks
                .insert(
                    open.name.clone(),
                    Block {
                        ops: open.ops,
                        terminator,
                    },
                )
                .is_some()
            {
                bail!(
                    "E_NIRS_DUPLICATE: {}:{}: duplicate block `{}` in function `{}`",
                    path.display(),
                    line,
                    open.name,
                    self.name
                );
            }
        }
        if self.entry.is_none() {
            self.entry = Some(name.clone());
        }
        if self.blocks.contains_key(&name) {
            bail!(
                "E_NIRS_DUPLICATE: {}:{}: block `{}` already defined in function `{}`",
                path.display(),
                line,
                name,
                self.name
            );
        }
        self.current = Some(OpenBlock {
            name,
            ops: Vec::new(),
        });
        Ok(())
    }

    fn into_function(mut self, path: &Path) -> Result<Function> {
        if let Some(open) = self.current.take() {
            bail!(
                "E_NIRS_SEMANTIC: {}: function `{}` block `{}` ends without a terminator (@end, @jump, @if, @choice, or @return)",
                path.display(),
                self.name,
                open.name
            );
        }
        let entry = self.entry.ok_or_else(|| {
            anyhow!(
                "E_NIRS_SEMANTIC: {}: function `{}` has no blocks",
                path.display(),
                self.name
            )
        })?;
        if self.needs_fault_blocks {
            self.blocks
                .entry("__nirs_cancelled".into())
                .or_insert(Block {
                    ops: vec![],
                    terminator: Terminator::Fault {
                        code: "E_CANCELLED".into(),
                        message: "A required performance was cancelled.".into(),
                    },
                });
            self.blocks.entry("__nirs_failed".into()).or_insert(Block {
                ops: vec![],
                terminator: Terminator::Fault {
                    code: "E_PERFORMANCE".into(),
                    message: "A required performance failed.".into(),
                },
            });
        }
        Ok(Function {
            params: BTreeMap::new(),
            locals: BTreeMap::new(),
            returns: None,
            entry,
            blocks: self.blocks,
        })
    }
}

struct PendingChoice {
    id: String,
    line: usize,
    options: Vec<ChoiceOption>,
    branches: BTreeMap<String, String>,
    first_target: Option<String>,
}

struct Lowerer<'a> {
    path: &'a Path,
    stem: String,
    stage: Stage,
    fragment: Fragment,
    text_contracts: BTreeMap<String, TextContract>,
    text_docs: BTreeMap<String, TextDoc>,
    speakers: BTreeMap<String, String>,
    active_scene: Option<String>,
    active_fn: Option<FunctionBuilder>,
    pending_choice: Option<PendingChoice>,
    last_bgm_task: Option<String>,
    cue_seq: usize,
    op_seq: usize,
}

impl<'a> Lowerer<'a> {
    fn new(path: &'a Path, stem: String, stage: Stage) -> Self {
        Self {
            path,
            stem,
            stage,
            fragment: Fragment {
                fragment_format: 1,
                variables: BTreeMap::new(),
                functions: BTreeMap::new(),
                scenes: BTreeMap::new(),
                sprite_timelines: BTreeMap::new(),
                cues: BTreeMap::new(),
                choices: BTreeMap::new(),
            },
            text_contracts: BTreeMap::new(),
            text_docs: BTreeMap::new(),
            speakers: BTreeMap::new(),
            active_scene: None,
            active_fn: None,
            pending_choice: None,
            last_bgm_task: None,
            cue_seq: 0,
            op_seq: 0,
        }
    }

    fn err(&self, line: usize, code: &str, msg: impl std::fmt::Display) -> anyhow::Error {
        anyhow!("{code}: {}:{line}: {msg}", self.path.display())
    }

    fn intern_text(&mut self, id: String, content: &str, line: usize) -> Result<String> {
        let (contract, doc) = make_text(content);
        if self.text_contracts.insert(id.clone(), contract).is_some() {
            return Err(self.err(line, "E_NIRS_DUPLICATE", format!("duplicate text `{id}`")));
        }
        self.text_docs.insert(id.clone(), doc);
        Ok(id)
    }

    fn intern_speaker(&mut self, name: &str, line: usize) -> Result<String> {
        if let Some(id) = self.speakers.get(name) {
            return Ok(id.clone());
        }
        let id = format!("nirs.{}.speaker.{}", self.stem, self.speakers.len());
        self.intern_text(id.clone(), name, line)?;
        self.speakers.insert(name.to_owned(), id.clone());
        Ok(id)
    }

    fn flush_choice(&mut self) -> Result<()> {
        let Some(choice) = self.pending_choice.take() else {
            return Ok(());
        };
        if choice.options.is_empty() {
            return Err(self.err(
                choice.line,
                "E_NIRS_SYNTAX",
                format!(
                    "@choice `{}` must have at least one `- label -> target` option",
                    choice.id
                ),
            ));
        }
        let choice_id = choice.id.clone();
        if self
            .fragment
            .choices
            .insert(
                choice_id.clone(),
                Choice {
                    options: choice.options,
                    timeout_us: None,
                    default: None,
                },
            )
            .is_some()
        {
            return Err(self.err(
                choice.line,
                "E_NIRS_DUPLICATE",
                format!("duplicate choice `{choice_id}`"),
            ));
        }
        let path = self.path;
        let f = self.ensure_fn();
        f.ensure_open();
        f.needs_fault_blocks = true;
        let on_empty = choice
            .first_target
            .unwrap_or_else(|| "__nirs_failed".into());
        f.seal_current(
            Terminator::Interact {
                choice: choice_id,
                branches: choice.branches,
                on_empty,
                result: None,
                on_cancel: None,
            },
            path,
            choice.line,
        )?;
        Ok(())
    }

    fn flush_fn(&mut self) -> Result<()> {
        self.flush_choice()?;
        if let Some(f) = self.active_fn.take() {
            let name = f.name.clone();
            let built = f.into_function(self.path)?;
            if self
                .fragment
                .functions
                .insert(name.clone(), built)
                .is_some()
            {
                bail!(
                    "E_NIRS_DUPLICATE: {}: duplicate function `{name}`",
                    self.path.display()
                );
            }
        }
        Ok(())
    }

    fn ensure_fn(&mut self) -> &mut FunctionBuilder {
        self.active_scene = None;
        self.active_fn
            .get_or_insert_with(|| FunctionBuilder::new("main".into()))
    }

    fn next_cue_id(&mut self, kind: &str) -> String {
        self.cue_seq += 1;
        format!("nirs_{}_{}_{}", self.stem, kind, self.cue_seq)
    }

    fn next_op_id(&mut self, kind: &str) -> String {
        self.op_seq += 1;
        format!("nirs_{}_{}_{}", self.stem, kind, self.op_seq)
    }

    fn emit_activate_and_continue(
        &mut self,
        cue_id: String,
        cue: Cue,
        await_task: Option<String>,
        line: usize,
    ) -> Result<()> {
        self.fragment.cues.insert(cue_id.clone(), cue);
        let path = self.path;
        let f = self.ensure_fn();
        f.ensure_open();
        if let Some(task) = await_task {
            let wait_block = f.fresh_block("wait");
            let next_block = f.fresh_block("cont");
            f.needs_fault_blocks = true;
            f.seal_current(
                Terminator::Activate {
                    cue: cue_id,
                    next: wait_block.clone(),
                },
                path,
                line,
            )?;
            f.blocks.insert(
                wait_block,
                Block {
                    ops: vec![],
                    terminator: Terminator::Await {
                        conditions: vec![WaitCondition {
                            task,
                            milestone: Milestone::Finished,
                        }],
                        next: next_block.clone(),
                        on_cancelled: "__nirs_cancelled".into(),
                        on_failed: "__nirs_failed".into(),
                        on_advance: None,
                    },
                },
            );
            f.current = Some(OpenBlock {
                name: next_block,
                ops: Vec::new(),
            });
        } else {
            let next_block = f.fresh_block("cont");
            f.seal_current(
                Terminator::Activate {
                    cue: cue_id,
                    next: next_block.clone(),
                },
                path,
                line,
            )?;
            f.current = Some(OpenBlock {
                name: next_block,
                ops: Vec::new(),
            });
        }
        Ok(())
    }

    fn parse_all(&mut self, source: &str) -> Result<()> {
        for (idx, raw_line) in source.lines().enumerate() {
            let line_no = idx + 1;
            let trimmed = raw_line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if self.pending_choice.is_some() {
                if let Some(rest) = trimmed.strip_prefix('-') {
                    self.parse_choice_option(rest.trim(), line_no)?;
                    continue;
                } else {
                    self.flush_choice()?;
                }
            }
            if let Some(directive) = trimmed.strip_prefix('@') {
                self.parse_directive(directive, line_no)?;
            } else {
                self.parse_dialogue_line(trimmed, line_no)?;
            }
        }
        self.flush_fn()?;
        Ok(())
    }

    fn parse_directive(&mut self, directive: &str, line: usize) -> Result<()> {
        let (cmd, rest) = split_head(directive);
        match cmd {
            "var" => {
                self.active_scene = None;
                let (name, expr_str) = rest.split_once('=').ok_or_else(|| {
                    self.err(line, "E_NIRS_SYNTAX", "expected `@var <name> = <value>`")
                })?;
                let name = name.trim();
                if !valid_ident(name) {
                    return Err(self.err(
                        line,
                        "E_NIRS_SYNTAX",
                        format!("invalid variable name `{name}`"),
                    ));
                }
                let val = parse_literal(expr_str.trim())
                    .map_err(|e| self.err(line, "E_NIRS_SYNTAX", e))?;
                if self.fragment.variables.insert(name.into(), val).is_some() {
                    return Err(self.err(
                        line,
                        "E_NIRS_DUPLICATE",
                        format!("duplicate variable `{name}`"),
                    ));
                }
            }
            "scene" => {
                let id = rest.trim();
                if !valid_ident(id) {
                    return Err(self.err(line, "E_NIRS_SYNTAX", "expected `@scene <scene_id>`"));
                }
                if self.fragment.scenes.contains_key(id) {
                    return Err(self.err(
                        line,
                        "E_NIRS_DUPLICATE",
                        format!("duplicate scene `{id}`"),
                    ));
                }
                self.fragment.scenes.insert(id.into(), Vec::new());
                self.active_scene = Some(id.into());
            }
            "bg" => {
                let scene_id = self.active_scene.clone().ok_or_else(|| {
                    self.err(
                        line,
                        "E_NIRS_SEMANTIC",
                        "`@bg` must appear inside a `@scene`",
                    )
                })?;
                let parts: Vec<_> = rest.split_whitespace().collect();
                if parts.is_empty() || !valid_ident(parts[0]) {
                    return Err(self.err(
                        line,
                        "E_NIRS_SYNTAX",
                        "expected `@bg <asset_id> [x y w h]`",
                    ));
                }
                let (x, y, w, h) = if parts.len() == 1 {
                    (0., 0., self.stage.width as f32, self.stage.height as f32)
                } else if parts.len() == 5 {
                    (
                        parse_f32(parts[1], line, self.path)?,
                        parse_f32(parts[2], line, self.path)?,
                        parse_f32(parts[3], line, self.path)?,
                        parse_f32(parts[4], line, self.path)?,
                    )
                } else {
                    return Err(self.err(
                        line,
                        "E_NIRS_SYNTAX",
                        "expected `@bg <asset_id> [x y w h]`",
                    ));
                };
                self.fragment
                    .scenes
                    .get_mut(&scene_id)
                    .unwrap()
                    .push(make_node(
                        "background".into(),
                        parts[0].into(),
                        x,
                        y,
                        w,
                        h,
                        0,
                        1.,
                        1.,
                    ));
            }
            "sprite" => {
                let scene_id = self.active_scene.clone().ok_or_else(|| {
                    self.err(
                        line,
                        "E_NIRS_SEMANTIC",
                        "`@sprite` must appear inside a `@scene`",
                    )
                })?;
                let parts: Vec<_> = rest.split_whitespace().collect();
                if parts.len() < 2 || !valid_ident(parts[0]) || !valid_ident(parts[1]) {
                    return Err(self.err(
                        line,
                        "E_NIRS_SYNTAX",
                        "expected `@sprite <node_id> <asset_id> [x y w h] [order=N] [opacity=F] [scale=F]`",
                    ));
                }
                let mut pos = Vec::new();
                let mut order = 1i32;
                let mut opacity = 1.0f32;
                let mut scale = 1.0f32;
                for &tok in &parts[2..] {
                    if let Some((k, v)) = tok.split_once('=') {
                        match k {
                            "order" => {
                                order = v.parse::<i32>().map_err(|_| {
                                    self.err(line, "E_NIRS_SYNTAX", format!("invalid order `{v}`"))
                                })?;
                            }
                            "opacity" => opacity = parse_f32(v, line, self.path)?,
                            "scale" => scale = parse_f32(v, line, self.path)?,
                            _ => {
                                return Err(self.err(
                                    line,
                                    "E_NIRS_SYNTAX",
                                    format!("unknown sprite option `{k}`"),
                                ))
                            }
                        }
                    } else {
                        pos.push(parse_f32(tok, line, self.path)?);
                    }
                }
                let (x, y, w, h) = match pos.as_slice() {
                    [] => (0., 0., self.stage.width as f32, self.stage.height as f32),
                    [x, y, w, h] => (*x, *y, *w, *h),
                    _ => {
                        return Err(self.err(
                            line,
                            "E_NIRS_SYNTAX",
                            "sprite geometry expects 4 numbers: x y w h",
                        ))
                    }
                };
                self.fragment
                    .scenes
                    .get_mut(&scene_id)
                    .unwrap()
                    .push(make_node(
                        parts[0].into(),
                        parts[1].into(),
                        x,
                        y,
                        w,
                        h,
                        order,
                        opacity,
                        scale,
                    ));
            }
            "function" => {
                self.active_scene = None;
                let name = rest.trim();
                if !valid_ident(name) {
                    return Err(self.err(line, "E_NIRS_SYNTAX", "expected `@function <name>`"));
                }
                self.flush_fn()?;
                self.active_fn = Some(FunctionBuilder::new(name.into()));
            }
            "block" => {
                self.active_scene = None;
                let name = rest.trim();
                if !valid_ident(name) {
                    return Err(self.err(line, "E_NIRS_SYNTAX", "expected `@block <name>`"));
                }
                let path = self.path;
                let f = self.ensure_fn();
                f.start_named_block(name.into(), path, line)?;
            }
            "show" => {
                let parts: Vec<_> = rest.split_whitespace().collect();
                if parts.is_empty() || !valid_ident(parts[0]) {
                    return Err(self.err(
                        line,
                        "E_NIRS_SYNTAX",
                        "expected `@show <scene_id> [cut|dissolve [duration_ms]]`",
                    ));
                }
                let scene = parts[0].to_owned();
                let (transition, duration_ms) = match parts.get(1).copied() {
                    None => (StageTransition::Dissolve, 0u64),
                    Some("cut") => (StageTransition::Dissolve, 0u64),
                    Some("dissolve") => {
                        let ms = match parts.get(2) {
                            Some(v) => v.parse::<u64>().map_err(|_| {
                                self.err(line, "E_NIRS_SYNTAX", format!("invalid duration `{v}`"))
                            })?,
                            None => 300,
                        };
                        (StageTransition::Dissolve, ms)
                    }
                    Some(other) => {
                        return Err(self.err(
                            line,
                            "E_NIRS_SYNTAX",
                            format!("expected `cut` or `dissolve`, got `{other}`"),
                        ))
                    }
                };
                let duration_us = Micros(duration_ms.saturating_mul(1_000));
                let cue_id = self.next_cue_id("stage");
                let cue = Cue {
                    effects: vec![EffectDef {
                        id: "stage".into(),
                        scope: Scope::Scene,
                        effect: Effect::StagePresent {
                            scene,
                            inherit_images: vec![],
                            inherit_image_geometry: vec![],
                            dialogue_visible: None,
                            transition,
                            duration_us,
                        },
                    }],
                };
                let await_task = (duration_ms > 0).then(|| "stage".into());
                self.emit_activate_and_continue(cue_id, cue, await_task, line)?;
            }
            "bgm" | "sfx" => {
                let parts: Vec<_> = rest.split_whitespace().collect();
                if parts.is_empty() || !valid_ident(parts[0]) {
                    return Err(self.err(
                        line,
                        "E_NIRS_SYNTAX",
                        format!("expected `@{cmd} <asset_id> [gain=<f32>]`"),
                    ));
                }
                let mut gain = 1.0f32;
                for &opt in &parts[1..] {
                    if let Some(v) = opt.strip_prefix("gain=") {
                        gain = parse_f32(v, line, self.path)?;
                    } else {
                        return Err(self.err(
                            line,
                            "E_NIRS_SYNTAX",
                            format!("unexpected option `{opt}`"),
                        ));
                    }
                }
                let is_bgm = cmd == "bgm";
                let cue_id = self.next_cue_id(cmd);
                let task_id = if is_bgm {
                    format!("bgm_{}", self.cue_seq)
                } else {
                    format!("sfx_{}", self.cue_seq)
                };
                if is_bgm {
                    self.last_bgm_task = Some(task_id.clone());
                }
                let cue = Cue {
                    effects: vec![EffectDef {
                        id: task_id,
                        scope: if is_bgm { Scope::Session } else { Scope::Scene },
                        effect: Effect::Audio {
                            asset: parts[0].into(),
                            bus: if is_bgm { AudioBus::Bgm } else { AudioBus::Sfx },
                            gain,
                            looped: is_bgm,
                            loop_region: None,
                        },
                    }],
                };
                self.emit_activate_and_continue(cue_id, cue, None, line)?;
            }
            "stop_bgm" => {
                let target = self.last_bgm_task.clone().ok_or_else(|| {
                    self.err(
                        line,
                        "E_NIRS_SEMANTIC",
                        "`@stop_bgm` requires a prior `@bgm` in this script",
                    )
                })?;
                let ms = if rest.trim().is_empty() {
                    0u64
                } else {
                    rest.trim().parse::<u64>().map_err(|_| {
                        self.err(
                            line,
                            "E_NIRS_SYNTAX",
                            format!("invalid fade duration `{rest}`"),
                        )
                    })?
                };
                let cue_id = self.next_cue_id("stop_bgm");
                let cue = Cue {
                    effects: vec![EffectDef {
                        id: "stop_bgm".into(),
                        scope: Scope::Scene,
                        effect: Effect::AudioStop {
                            target,
                            duration_us: Micros(ms.saturating_mul(1_000)),
                        },
                    }],
                };
                self.emit_activate_and_continue(cue_id, cue, None, line)?;
            }
            "stop_audio" => {
                let parts: Vec<_> = rest.split_whitespace().collect();
                if parts.is_empty() || !valid_ident(parts[0]) {
                    return Err(self.err(
                        line,
                        "E_NIRS_SYNTAX",
                        "expected `@stop_audio <task_id> [fade_ms]`",
                    ));
                }
                let ms = match parts.get(1) {
                    Some(v) => v.parse::<u64>().map_err(|_| {
                        self.err(
                            line,
                            "E_NIRS_SYNTAX",
                            format!("invalid fade duration `{v}`"),
                        )
                    })?,
                    None => 0,
                };
                let cue_id = self.next_cue_id("stop_audio");
                let cue = Cue {
                    effects: vec![EffectDef {
                        id: "stop_audio".into(),
                        scope: Scope::Scene,
                        effect: Effect::AudioStop {
                            target: parts[0].into(),
                            duration_us: Micros(ms.saturating_mul(1_000)),
                        },
                    }],
                };
                self.emit_activate_and_continue(cue_id, cue, None, line)?;
            }
            "wait" => {
                let ms = rest.trim().parse::<u64>().map_err(|_| {
                    self.err(line, "E_NIRS_SYNTAX", "expected `@wait <duration_ms>`")
                })?;
                let cue_id = self.next_cue_id("delay");
                let cue = Cue {
                    effects: vec![EffectDef {
                        id: "delay".into(),
                        scope: Scope::Scene,
                        effect: Effect::Delay {
                            duration_us: Micros(ms.saturating_mul(1_000)),
                        },
                    }],
                };
                self.emit_activate_and_continue(cue_id, cue, Some("delay".into()), line)?;
            }
            "set" => {
                let (target, expr_str) = rest.split_once('=').ok_or_else(|| {
                    self.err(line, "E_NIRS_SYNTAX", "expected `@set <var> = <expr>`")
                })?;
                let target = target.trim();
                if !valid_ident(target) {
                    return Err(self.err(
                        line,
                        "E_NIRS_SYNTAX",
                        format!("invalid variable `{target}`"),
                    ));
                }
                let expr =
                    parse_expr(expr_str.trim()).map_err(|e| self.err(line, "E_NIRS_SYNTAX", e))?;
                let op_id = self.next_op_id("set");
                let f = self.ensure_fn();
                f.ensure_open();
                f.current.as_mut().unwrap().ops.push(Op {
                    id: op_id,
                    operation: Operation::Assign {
                        target: target.into(),
                        value: expr,
                    },
                });
            }
            "unlock" => {
                let key = rest.trim();
                if !valid_ident(key) {
                    return Err(self.err(
                        line,
                        "E_NIRS_SYNTAX",
                        "expected `@unlock <profile_key>`",
                    ));
                }
                let op_id = self.next_op_id("unlock");
                let f = self.ensure_fn();
                f.ensure_open();
                f.current.as_mut().unwrap().ops.push(Op {
                    id: op_id,
                    operation: Operation::ProfileMerge { key: key.into() },
                });
            }
            "jump" => {
                let target = rest.trim();
                if !valid_ident(target) {
                    return Err(self.err(line, "E_NIRS_SYNTAX", "expected `@jump <block>`"));
                }
                let path = self.path;
                let f = self.ensure_fn();
                f.ensure_open();
                f.seal_current(
                    Terminator::Goto {
                        target: target.into(),
                    },
                    path,
                    line,
                )?;
            }
            "if" => {
                let (cond_str, targets) = rest.split_once("->").ok_or_else(|| {
                    self.err(
                        line,
                        "E_NIRS_SYNTAX",
                        "expected `@if <expr> -> <yes_block> else <no_block>`",
                    )
                })?;
                let (yes, no) = targets.split_once("else").ok_or_else(|| {
                    self.err(
                        line,
                        "E_NIRS_SYNTAX",
                        "expected `@if <expr> -> <yes_block> else <no_block>`",
                    )
                })?;
                let yes = yes.trim();
                let no = no.trim();
                if !valid_ident(yes) || !valid_ident(no) {
                    return Err(self.err(line, "E_NIRS_SYNTAX", "invalid branch block name"));
                }
                let condition =
                    parse_expr(cond_str.trim()).map_err(|e| self.err(line, "E_NIRS_SYNTAX", e))?;
                let path = self.path;
                let f = self.ensure_fn();
                f.ensure_open();
                f.seal_current(
                    Terminator::Branch {
                        condition,
                        yes: yes.into(),
                        no: no.into(),
                    },
                    path,
                    line,
                )?;
            }
            "choice" => {
                let id = rest.trim();
                if !valid_ident(id) {
                    return Err(self.err(line, "E_NIRS_SYNTAX", "expected `@choice <choice_id>`"));
                }
                self.pending_choice = Some(PendingChoice {
                    id: id.into(),
                    line,
                    options: Vec::new(),
                    branches: BTreeMap::new(),
                    first_target: None,
                });
            }
            "call" => {
                let (func, explicit_next) = match rest.split_once("->") {
                    Some((f_part, n_part)) => (f_part.trim(), Some(n_part.trim())),
                    None => (rest.trim(), None),
                };
                if !valid_ident(func) {
                    return Err(self.err(
                        line,
                        "E_NIRS_SYNTAX",
                        "expected `@call <function> [-> <next_block>]`",
                    ));
                }
                let path = self.path;
                let f = self.ensure_fn();
                f.ensure_open();
                if let Some(next_name) = explicit_next {
                    if !valid_ident(next_name) {
                        return Err(self.err(
                            line,
                            "E_NIRS_SYNTAX",
                            "invalid call continuation block",
                        ));
                    }
                    f.seal_current(
                        Terminator::Call {
                            function: func.into(),
                            args: BTreeMap::new(),
                            next: next_name.into(),
                            result: None,
                        },
                        path,
                        line,
                    )?;
                } else {
                    let next_block = f.fresh_block("after_call");
                    f.seal_current(
                        Terminator::Call {
                            function: func.into(),
                            args: BTreeMap::new(),
                            next: next_block.clone(),
                            result: None,
                        },
                        path,
                        line,
                    )?;
                    f.current = Some(OpenBlock {
                        name: next_block,
                        ops: Vec::new(),
                    });
                }
            }
            "return" => {
                let value = if rest.trim().is_empty() {
                    None
                } else {
                    Some(parse_expr(rest.trim()).map_err(|e| self.err(line, "E_NIRS_SYNTAX", e))?)
                };
                let path = self.path;
                let f = self.ensure_fn();
                f.ensure_open();
                f.seal_current(Terminator::Return { value }, path, line)?;
            }
            "end" => {
                let outcome = rest.trim();
                if !valid_ident(outcome) {
                    return Err(self.err(line, "E_NIRS_SYNTAX", "expected `@end <outcome>`"));
                }
                let path = self.path;
                let f = self.ensure_fn();
                f.ensure_open();
                f.seal_current(
                    Terminator::End {
                        outcome: outcome.into(),
                    },
                    path,
                    line,
                )?;
            }
            _ => {
                return Err(self.err(line, "E_NIRS_SYNTAX", format!("unknown directive `@{cmd}`")));
            }
        }
        Ok(())
    }

    fn parse_choice_option(&mut self, line_text: &str, line: usize) -> Result<()> {
        let (label_part, rhs) = line_text.split_once("->").ok_or_else(|| {
            self.err(
                line,
                "E_NIRS_SYNTAX",
                "expected `- <label> -> <target_block> [if <expr>]`",
            )
        })?;
        let label = strip_optional_quotes(label_part.trim(), line, self.path)?;
        if label.is_empty() {
            return Err(self.err(line, "E_NIRS_SYNTAX", "choice option label cannot be empty"));
        }
        let (target_part, visible) = if let Some((tgt, cond)) = split_trailing_if(rhs) {
            (
                tgt.trim(),
                Some(parse_expr(cond.trim()).map_err(|e| self.err(line, "E_NIRS_SYNTAX", e))?),
            )
        } else {
            (rhs.trim(), None)
        };
        if !valid_ident(target_part) {
            return Err(self.err(
                line,
                "E_NIRS_SYNTAX",
                format!("invalid choice target block `{target_part}`"),
            ));
        }
        let (choice_id, opt_index) = {
            let pending = self.pending_choice.as_ref().unwrap();
            (pending.id.clone(), pending.options.len())
        };
        let opt_id = format!("opt_{opt_index}");
        let text_id = format!("nirs.{}.{}.{}", self.stem, choice_id, opt_id);
        self.intern_text(text_id.clone(), &label, line)?;
        let pending = self.pending_choice.as_mut().unwrap();
        if pending.first_target.is_none() {
            pending.first_target = Some(target_part.into());
        }
        pending.branches.insert(opt_id.clone(), target_part.into());
        pending.options.push(ChoiceOption {
            id: opt_id,
            text: text_id,
            visible,
            enabled: None,
            value: None,
            image: None,
        });
        Ok(())
    }

    fn parse_dialogue_line(&mut self, trimmed: &str, line: usize) -> Result<()> {
        let (speaker_opt, text) = if let Some(rest) = trimmed.strip_prefix('>') {
            (None, rest.trim().to_owned())
        } else if trimmed.starts_with('"') {
            if let Some((quoted, after)) = parse_leading_quoted(trimmed) {
                let after = after.trim_start();
                if let Some(body) = after.strip_prefix(':').or_else(|| after.strip_prefix('：')) {
                    (Some(quoted), body.trim().to_owned())
                } else if after.is_empty() {
                    (None, quoted)
                } else {
                    return Err(self.err(
                        line,
                        "E_NIRS_SYNTAX",
                        "unexpected tokens after quoted dialogue line",
                    ));
                }
            } else {
                return Err(self.err(line, "E_NIRS_SYNTAX", "unterminated string literal"));
            }
        } else if let Some((spk, body)) = split_speaker(trimmed) {
            (Some(spk.to_owned()), body.to_owned())
        } else {
            return Err(self.err(
                line,
                "E_NIRS_SYNTAX",
                "expected directive (`@...`), `Speaker: text`, `\"narration\"`, or `> narration`",
            ));
        };
        if text.is_empty() {
            return Err(self.err(line, "E_NIRS_SYNTAX", "dialogue text cannot be empty"));
        }
        let speaker_id = match speaker_opt {
            Some(spk) if !spk.is_empty() => self.intern_speaker(&spk, line)?,
            _ => String::new(),
        };
        let text_id = format!("nirs.{}.line.{}", self.stem, line);
        self.intern_text(text_id.clone(), &text, line)?;
        let reveal_us = ((text.chars().count().max(1) as u64) * 35_000).clamp(200_000, 3_000_000);
        let cue_id = self.next_cue_id("line");
        let cue = Cue {
            effects: vec![EffectDef {
                id: "line".into(),
                scope: Scope::Scene,
                effect: Effect::Dialogue {
                    text: text_id,
                    speaker: speaker_id,
                    reveal_us: Micros(reveal_us),
                },
            }],
        };
        self.emit_activate_and_continue(cue_id, cue, Some("line".into()), line)
    }

    fn finish(self) -> Result<NirsOutput> {
        Ok(NirsOutput {
            fragment: self.fragment,
            text_contracts: self.text_contracts,
            text_docs: self.text_docs,
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn make_node(
    id: String,
    asset: String,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    order: i32,
    opacity: f32,
    scale: f32,
) -> Node {
    Node {
        id,
        parent: None,
        asset: Some(asset),
        x,
        y,
        width,
        height,
        offset: [0., 0.],
        sprite_transform: None,
        timeline_binding: None,
        inherit_existence: false,
        scale,
        opacity,
        color: [1., 1., 1., 1.],
        order,
        clip: None,
        bitmap_text: None,
        preserve_pose: vec![],
    }
}

fn split_head(s: &str) -> (&str, &str) {
    match s.find(char::is_whitespace) {
        Some(idx) => (&s[..idx], s[idx..].trim_start()),
        None => (s, ""),
    }
}

fn split_speaker(s: &str) -> Option<(&str, &str)> {
    let sep = s.find([':', '：'])?;
    let speaker = s[..sep].trim();
    if speaker.is_empty() || speaker.chars().any(char::is_whitespace) {
        return None;
    }
    let sep_len = s[sep..].chars().next()?.len_utf8();
    Some((speaker, s[sep + sep_len..].trim()))
}

fn split_trailing_if(s: &str) -> Option<(&str, &str)> {
    let bytes = s.as_bytes();
    let mut in_str = false;
    let mut escape = false;
    for i in 0..bytes.len() {
        let b = bytes[i];
        if in_str {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        if b == b'"' {
            in_str = true;
            continue;
        }
        if s[i..].starts_with(" if ") {
            return Some((&s[..i], &s[i + 4..]));
        }
    }
    None
}

fn parse_f32(tok: &str, line: usize, path: &Path) -> Result<f32> {
    let v: f32 = tok.parse().map_err(|_| {
        anyhow!(
            "E_NIRS_SYNTAX: {}:{line}: invalid number `{tok}`",
            path.display()
        )
    })?;
    if !v.is_finite() {
        bail!(
            "E_NIRS_SYNTAX: {}:{line}: non-finite number `{tok}`",
            path.display()
        );
    }
    Ok(v)
}

fn parse_leading_quoted(s: &str) -> Option<(String, &str)> {
    let mut chars = s.char_indices();
    if chars.next()?.1 != '"' {
        return None;
    }
    let mut out = String::new();
    while let Some((idx, ch)) = chars.next() {
        match ch {
            '"' => return Some((out, &s[idx + 1..])),
            '\\' => {
                let (_, esc) = chars.next()?;
                match esc {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    other => out.push(other),
                }
            }
            other => out.push(other),
        }
    }
    None
}

fn strip_optional_quotes(s: &str, line: usize, path: &Path) -> Result<String> {
    if s.starts_with('"') {
        let (parsed, rest) = parse_leading_quoted(s).ok_or_else(|| {
            anyhow!(
                "E_NIRS_SYNTAX: {}:{line}: unterminated quoted string",
                path.display()
            )
        })?;
        if !rest.trim().is_empty() {
            bail!(
                "E_NIRS_SYNTAX: {}:{line}: unexpected trailing text after string",
                path.display()
            );
        }
        Ok(parsed)
    } else {
        Ok(s.to_owned())
    }
}

fn parse_literal(s: &str) -> std::result::Result<Value, String> {
    if s == "true" {
        return Ok(Value::Bool(true));
    }
    if s == "false" {
        return Ok(Value::Bool(false));
    }
    if s.starts_with('"') {
        let (parsed, rest) =
            parse_leading_quoted(s).ok_or_else(|| "unterminated string literal".to_owned())?;
        if !rest.trim().is_empty() {
            return Err("unexpected tokens after string literal".into());
        }
        return Ok(Value::String(parsed));
    }
    if let Ok(n) = s.parse::<i32>() {
        return Ok(Value::I32(n));
    }
    Err(format!("expected bool, i32, or string literal; got `{s}`"))
}

// ── Recursive-descent expression parser ──────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Bool(bool),
    Int(i32),
    Str(String),
    Ident(String),
    LParen,
    RParen,
    Bang,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    DotDot,
    EqEq,
    BangEq,
    Lt,
    Le,
    Gt,
    Ge,
    AndAnd,
    OrOr,
}

fn tokenize(input: &str) -> std::result::Result<Vec<Tok>, String> {
    let mut toks = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        match b {
            b'(' => {
                toks.push(Tok::LParen);
                i += 1;
            }
            b')' => {
                toks.push(Tok::RParen);
                i += 1;
            }
            b'+' => {
                toks.push(Tok::Plus);
                i += 1;
            }
            b'-' => {
                toks.push(Tok::Minus);
                i += 1;
            }
            b'*' => {
                toks.push(Tok::Star);
                i += 1;
            }
            b'/' => {
                toks.push(Tok::Slash);
                i += 1;
            }
            b'%' => {
                toks.push(Tok::Percent);
                i += 1;
            }
            b'!' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    toks.push(Tok::BangEq);
                    i += 2;
                } else {
                    toks.push(Tok::Bang);
                    i += 1;
                }
            }
            b'=' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    toks.push(Tok::EqEq);
                    i += 2;
                } else {
                    return Err("expected `==` in expression".into());
                }
            }
            b'<' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    toks.push(Tok::Le);
                    i += 2;
                } else {
                    toks.push(Tok::Lt);
                    i += 1;
                }
            }
            b'>' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    toks.push(Tok::Ge);
                    i += 2;
                } else {
                    toks.push(Tok::Gt);
                    i += 1;
                }
            }
            b'&' if bytes.get(i + 1) == Some(&b'&') => {
                toks.push(Tok::AndAnd);
                i += 2;
            }
            b'|' if bytes.get(i + 1) == Some(&b'|') => {
                toks.push(Tok::OrOr);
                i += 2;
            }
            b'.' if bytes.get(i + 1) == Some(&b'.') => {
                toks.push(Tok::DotDot);
                i += 2;
            }
            b'"' => {
                let (s, rest) = parse_leading_quoted(&input[i..])
                    .ok_or_else(|| "unterminated string literal in expression".to_owned())?;
                toks.push(Tok::Str(s));
                i = input.len() - rest.len();
            }
            b'0'..=b'9' => {
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let n = input[start..i]
                    .parse::<i32>()
                    .map_err(|_| format!("integer out of i32 range: `{}`", &input[start..i]))?;
                toks.push(Tok::Int(n));
            }
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                let start = i;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'_' | b'.'))
                {
                    if bytes[i] == b'.' && bytes.get(i + 1) == Some(&b'.') {
                        break;
                    }
                    i += 1;
                }
                match &input[start..i] {
                    "true" => toks.push(Tok::Bool(true)),
                    "false" => toks.push(Tok::Bool(false)),
                    ident => toks.push(Tok::Ident(ident.to_owned())),
                }
            }
            _ => {
                return Err(format!(
                    "unexpected character `{}` in expression",
                    b as char
                ))
            }
        }
    }
    Ok(toks)
}

struct ExprParser {
    toks: Vec<Tok>,
    pos: usize,
}

impl ExprParser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos)?.clone();
        self.pos += 1;
        Some(t)
    }
    fn parse_or(&mut self) -> std::result::Result<Expr, String> {
        let mut lhs = self.parse_and()?;
        while matches!(self.peek(), Some(Tok::OrOr)) {
            self.next();
            let rhs = self.parse_and()?;
            lhs = Expr::Binary {
                op: BinaryOp::Or,
                left: Box::new(lhs),
                right: Box::new(rhs),
            };
        }
        Ok(lhs)
    }
    fn parse_and(&mut self) -> std::result::Result<Expr, String> {
        let mut lhs = self.parse_eq()?;
        while matches!(self.peek(), Some(Tok::AndAnd)) {
            self.next();
            let rhs = self.parse_eq()?;
            lhs = Expr::Binary {
                op: BinaryOp::And,
                left: Box::new(lhs),
                right: Box::new(rhs),
            };
        }
        Ok(lhs)
    }
    fn parse_eq(&mut self) -> std::result::Result<Expr, String> {
        let mut lhs = self.parse_cmp()?;
        loop {
            let op = match self.peek() {
                Some(Tok::EqEq) => BinaryOp::Eq,
                Some(Tok::BangEq) => BinaryOp::Ne,
                _ => break,
            };
            self.next();
            let rhs = self.parse_cmp()?;
            lhs = Expr::Binary {
                op,
                left: Box::new(lhs),
                right: Box::new(rhs),
            };
        }
        Ok(lhs)
    }
    fn parse_cmp(&mut self) -> std::result::Result<Expr, String> {
        let mut lhs = self.parse_add()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Lt) => BinaryOp::Lt,
                Some(Tok::Le) => BinaryOp::Le,
                Some(Tok::Gt) => BinaryOp::Gt,
                Some(Tok::Ge) => BinaryOp::Ge,
                _ => break,
            };
            self.next();
            let rhs = self.parse_add()?;
            lhs = Expr::Binary {
                op,
                left: Box::new(lhs),
                right: Box::new(rhs),
            };
        }
        Ok(lhs)
    }
    fn parse_add(&mut self) -> std::result::Result<Expr, String> {
        let mut lhs = self.parse_mul()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Plus) => BinaryOp::Add,
                Some(Tok::Minus) => BinaryOp::Sub,
                Some(Tok::DotDot) => BinaryOp::Concat,
                _ => break,
            };
            self.next();
            let rhs = self.parse_mul()?;
            lhs = Expr::Binary {
                op,
                left: Box::new(lhs),
                right: Box::new(rhs),
            };
        }
        Ok(lhs)
    }
    fn parse_mul(&mut self) -> std::result::Result<Expr, String> {
        let mut lhs = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Star) => BinaryOp::Mul,
                Some(Tok::Slash) => BinaryOp::Div,
                Some(Tok::Percent) => BinaryOp::Rem,
                _ => break,
            };
            self.next();
            let rhs = self.parse_unary()?;
            lhs = Expr::Binary {
                op,
                left: Box::new(lhs),
                right: Box::new(rhs),
            };
        }
        Ok(lhs)
    }
    fn parse_unary(&mut self) -> std::result::Result<Expr, String> {
        match self.peek() {
            Some(Tok::Bang) => {
                self.next();
                Ok(Expr::Not {
                    value: Box::new(self.parse_unary()?),
                })
            }
            Some(Tok::Minus) => {
                self.next();
                match self.peek() {
                    Some(Tok::Int(n)) => {
                        let n = *n;
                        self.next();
                        Ok(Expr::Const {
                            value: Value::I32(-n),
                        })
                    }
                    _ => Ok(Expr::Binary {
                        op: BinaryOp::Sub,
                        left: Box::new(Expr::Const {
                            value: Value::I32(0),
                        }),
                        right: Box::new(self.parse_unary()?),
                    }),
                }
            }
            _ => self.parse_primary(),
        }
    }
    fn parse_primary(&mut self) -> std::result::Result<Expr, String> {
        match self.next() {
            Some(Tok::Bool(b)) => Ok(Expr::Const {
                value: Value::Bool(b),
            }),
            Some(Tok::Int(n)) => Ok(Expr::Const {
                value: Value::I32(n),
            }),
            Some(Tok::Str(s)) => Ok(Expr::Const {
                value: Value::String(s),
            }),
            Some(Tok::Ident(name)) => Ok(Expr::Var { name }),
            Some(Tok::LParen) => {
                let inner = self.parse_or()?;
                if !matches!(self.next(), Some(Tok::RParen)) {
                    return Err("expected `)` in expression".into());
                }
                Ok(inner)
            }
            other => Err(format!("unexpected token in expression: {other:?}")),
        }
    }
}

fn parse_expr(input: &str) -> std::result::Result<Expr, String> {
    let toks = tokenize(input)?;
    if toks.is_empty() {
        return Err("empty expression".into());
    }
    let mut parser = ExprParser { toks, pos: 0 };
    let expr = parser.parse_or()?;
    if parser.pos != parser.toks.len() {
        return Err("unexpected trailing tokens in expression".into());
    }
    Ok(expr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nir_core::{Core, CoreInput, ValidatedProgram};

    fn test_program_from_nirs(source: &str) -> Program {
        let stage = Stage {
            width: 1280,
            height: 720,
        };
        let lowered = lower_nirs(Path::new("story.nirs"), source, &stage).unwrap();
        let mut locales = BTreeMap::new();
        locales.insert("en".to_owned(), lowered.text_docs);
        let mut assets = BTreeMap::new();
        assets.insert(
            "bg.room".into(),
            Asset {
                kind: AssetKind::Image,
                object: "a".repeat(64),
                bytes: 128,
                width: 1280,
                height: 720,
                duration_us: Micros(0),
                decoded_bytes: 1280 * 720 * 4,
            },
        );
        assets.insert(
            "font.reader".into(),
            Asset {
                kind: AssetKind::Font,
                object: "b".repeat(64),
                bytes: 1024,
                width: 0,
                height: 0,
                duration_us: Micros(0),
                decoded_bytes: 1024,
            },
        );
        let fonts = vec!["font.reader".to_owned()];
        let font_objects = BTreeMap::from([("font.reader".to_owned(), "b".repeat(64))]);
        let plan = LocaleFontPlan {
            digest: LocaleFontPlan::digest_for(&fonts, &font_objects),
            fonts,
        };
        let locale_config = LocaleConfig {
            default_ui: "en".into(),
            default_text: "en".into(),
            ui: BTreeMap::from([("en".into(), plan.clone())]),
            text: BTreeMap::from([("en".into(), plan)]),
        };
        Program {
            format: FORMAT_VERSION,
            game_id: "test.nirs".into(),
            revision: "1".into(),
            entry: "main".into(),
            requires: CAPABILITIES.iter().map(|s| (*s).to_owned()).collect(),
            stage,
            variables: lowered.fragment.variables,
            functions: lowered.fragment.functions,
            modules: BTreeMap::new(),
            scenes: lowered.fragment.scenes,
            sprite_timelines: lowered.fragment.sprite_timelines,
            cues: lowered.fragment.cues,
            choices: lowered.fragment.choices,
            texts: lowered.text_contracts,
            locales,
            locale_config,
            assets,
            default_locale: "en".into(),
            title_scene: Some("room".into()),
            theme: Theme::default(),
            player: PlayerDefaults::default(),
        }
    }

    #[test]
    fn lowers_and_executes_declarative_nirs_script_end_to_end() {
        let script = r#"
# Demo declarative narrative script
@var score = 0
@var greeted = false

@scene room
@bg bg.room

@function main
@block start
@show room cut
Aya: Hello from declarative NIR!
"The rain taps softly against the glass."
@set score = score + 10
@set greeted = true
@choice next_step
- Stay and talk -> talk if score >= 10
- Leave quietly -> leave

@block talk
@set score = score + 5
Aya: Glad you stayed.
@unlock ending.talk
@end stayed

@block leave
> You step back into the corridor.
@end left
"#;
        let program = test_program_from_nirs(script);
        let validated = ValidatedProgram::new(program).expect("valid lowered program");
        let mut core = Core::new(validated, "test-release".into(), "en".into()).unwrap();

        let mut seen_dialogue = Vec::new();
        let mut unlocked_keys = Vec::new();
        for sequence in 1..=200 {
            if core.state().outcome.is_some() {
                break;
            }
            let input = if let Some(p) = &core.state().pending {
                CoreInput::Prepared { activation: p.id }
            } else if let Some(ch) = &core.state().choice {
                let opt = ch.options[0].id.clone();
                CoreInput::Choose {
                    interaction: ch.interaction,
                    option: opt,
                    sequence,
                }
            } else if let Some((_, d)) = core.dialogue() {
                seen_dialogue.push((d.speaker.clone(), d.full_text()));
                CoreInput::Advance {
                    interaction: d.interaction,
                    sequence,
                }
            } else {
                CoreInput::Time {
                    delta_us: 1_000_000,
                }
            };
            let step = core.step(input, 10_000);
            for intent in step.intents {
                if let nir_core::CoreIntent::ProfileMerge { key } = intent {
                    unlocked_keys.push(key);
                }
            }
            assert!(
                core.state().fault.is_none(),
                "unexpected fault: {:?}",
                core.state().fault
            );
        }

        assert_eq!(core.state().outcome.as_deref(), Some("stayed"));
        assert_eq!(core.state().variables.get("score"), Some(&Value::I32(15)));
        assert_eq!(
            core.state().variables.get("greeted"),
            Some(&Value::Bool(true))
        );
        assert!(seen_dialogue
            .iter()
            .any(|(spk, txt)| spk == "Aya" && txt.contains("Hello from declarative NIR!")));
        assert!(seen_dialogue
            .iter()
            .any(|(spk, txt)| spk.is_empty() && txt.contains("rain taps softly")));
        assert!(seen_dialogue
            .iter()
            .any(|(spk, txt)| spk == "Aya" && txt.contains("Glad you stayed.")));
        assert!(unlocked_keys.contains(&"ending.talk".to_owned()));
        assert_eq!(
            unlocked_keys
                .iter()
                .filter(|k| k.starts_with("read:"))
                .count(),
            3
        );
    }

    #[test]
    fn reports_accurate_line_diagnostics_for_malformed_nirs() {
        let stage = Stage {
            width: 1280,
            height: 720,
        };
        let err = lower_nirs(
            Path::new("bad.nirs"),
            "@function main\n@block start\n@unknown_cmd foo\n",
            &stage,
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("E_NIRS_SYNTAX") && err.contains("bad.nirs:3"),
            "{err}"
        );

        let err = lower_nirs(
            Path::new("unterminated.nirs"),
            "@function main\n@block start\nAya: hi\n",
            &stage,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("E_NIRS_SEMANTIC"), "{err}");
    }
}
