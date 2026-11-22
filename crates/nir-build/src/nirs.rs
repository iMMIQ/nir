//! Modern `.nirs` Galgame DSL compiler → flat `nir-ir` numeric bytecode.
//!
//! Designed for developer ergonomics AND as a clean target for multi-engine converters.
use anyhow::{anyhow, bail, Result};
use nir_ir::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct ScriptFile {
    pub name: String,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct CharacterDef {
    pub display_name: String,
    pub color: Option<[f32; 4]>,
    pub voice_prefix: Option<String>,
}

/// Asset lookup supplied by the project scanner (maps logical asset name → `AssetId`).
pub struct AssetResolver {
    pub by_name: BTreeMap<String, AssetId>,
    pub short_name: BTreeMap<String, AssetId>,
}

impl AssetResolver {
    pub fn from_assets(assets: &[(String, AssetId)]) -> Self {
        let mut by_name = BTreeMap::new();
        let mut short_counts: BTreeMap<String, usize> = BTreeMap::new();
        let mut short_map = BTreeMap::new();
        for (name, id) in assets {
            by_name.insert(name.clone(), *id);
            let stem = name.rsplit('/').next().unwrap_or(name).to_owned();
            *short_counts.entry(stem.clone()).or_default() += 1;
            short_map.insert(stem, *id);
        }
        let short_name = short_map
            .into_iter()
            .filter(|(k, _)| short_counts.get(k) == Some(&1))
            .collect();
        Self {
            by_name,
            short_name,
        }
    }

    pub fn resolve(&self, raw: &str) -> Option<AssetId> {
        let clean = raw.trim().trim_matches('"').replace('\\', "/");
        if let Some(&id) = self.by_name.get(&clean) {
            return Some(id);
        }
        // Strip extension if author wrote `bg/station.png`
        if let Some(stem) = clean.rsplit_once('.').map(|(a, _)| a) {
            if let Some(&id) = self.by_name.get(stem) {
                return Some(id);
            }
        }
        self.short_name.get(&clean).copied()
    }
}

pub struct CompileOutput {
    pub strings: Vec<String>,
    pub vars: Vec<VarMeta>,
    pub funcs: Vec<FuncMeta>,
    pub labels: Vec<LabelMeta>,
    pub ops: Vec<Op>,
    pub texts: Vec<TextEntry>,
    pub used_assets: BTreeSet<AssetId>,
}

struct Compiler<'a> {
    assets: &'a AssetResolver,
    stage: [u32; 2],
    strings: Vec<String>,
    string_map: BTreeMap<String, StrId>,
    vars: Vec<VarMeta>,
    var_map: BTreeMap<String, VarId>,
    layers: BTreeMap<String, LayerId>,
    channels: BTreeMap<String, ChannelId>,
    characters: BTreeMap<String, CharacterDef>,
    funcs: Vec<FuncMeta>,
    func_map: BTreeMap<String, FuncId>,
    labels: Vec<LabelMeta>,
    label_pcs: BTreeMap<String, u32>,
    ops: Vec<Op>,
    texts: Vec<TextEntry>,
    used_assets: BTreeSet<AssetId>,
    /// Patch sites: `(op_index, target_label_name, source_location)`
    jump_patches: Vec<(usize, String, String)>,
    choice_patches: Vec<(usize, usize, String, String)>,
    call_patches: Vec<(usize, String, String)>,
}

impl<'a> Compiler<'a> {
    fn new(assets: &'a AssetResolver, stage: [u32; 2]) -> Self {
        let mut channels = BTreeMap::new();
        channels.insert("bgm".into(), ChannelId(0));
        channels.insert("voice".into(), ChannelId(1));
        channels.insert("sfx".into(), ChannelId(2));
        let mut layers = BTreeMap::new();
        layers.insert("bg".into(), LayerId(0));
        Self {
            assets,
            stage,
            strings: Vec::new(),
            string_map: BTreeMap::new(),
            vars: Vec::new(),
            var_map: BTreeMap::new(),
            layers,
            channels,
            characters: BTreeMap::new(),
            funcs: Vec::new(),
            func_map: BTreeMap::new(),
            labels: Vec::new(),
            label_pcs: BTreeMap::new(),
            ops: Vec::new(),
            texts: Vec::new(),
            used_assets: BTreeSet::new(),
            jump_patches: Vec::new(),
            choice_patches: Vec::new(),
            call_patches: Vec::new(),
        }
    }

    fn intern_str(&mut self, s: &str) -> StrId {
        if let Some(&id) = self.string_map.get(s) {
            return id;
        }
        let id = StrId(self.strings.len() as u32);
        self.strings.push(s.to_owned());
        self.string_map.insert(s.to_owned(), id);
        id
    }

    fn intern_layer(&mut self, name: &str) -> LayerId {
        if let Some(&id) = self.layers.get(name) {
            return id;
        }
        let id = LayerId(self.layers.len() as u32);
        self.layers.insert(name.to_owned(), id);
        id
    }

    fn intern_channel(&mut self, name: &str) -> ChannelId {
        if let Some(&id) = self.channels.get(name) {
            return id;
        }
        let id = ChannelId(self.channels.len() as u32);
        self.channels.insert(name.to_owned(), id);
        id
    }

    fn ensure_var(&mut self, name: &str, default: Value) -> VarId {
        if let Some(&id) = self.var_map.get(name) {
            return id;
        }
        let str_id = self.intern_str(name);
        let id = VarId(self.vars.len() as u32);
        self.vars.push(VarMeta {
            name: str_id,
            default,
        });
        self.var_map.insert(name.to_owned(), id);
        id
    }

    fn resolve_asset(&mut self, raw: &str, loc: &str) -> Result<AssetId> {
        let id = self
            .assets
            .resolve(raw)
            .ok_or_else(|| anyhow!("E_ASSET ({loc}): unknown asset `{raw}`"))?;
        self.used_assets.insert(id);
        Ok(id)
    }

    fn add_text(&mut self, raw: &str) -> TextId {
        let entry = self.parse_rich_text(raw);
        let id = TextId(self.texts.len() as u32);
        self.texts.push(entry);
        id
    }

    fn parse_rich_text(&mut self, raw: &str) -> TextEntry {
        let mut spans = Vec::new();
        let mut buf = String::new();
        let bytes = raw.as_bytes();
        let mut i = 0;

        let flush = |buf: &mut String, spans: &mut Vec<RichSpan>| {
            if !buf.is_empty() {
                spans.push(RichSpan::Text(std::mem::take(buf)));
            }
        };

        while i < bytes.len() {
            if bytes[i] == b'\\' && i + 1 < bytes.len() {
                match bytes[i + 1] {
                    b'n' => {
                        buf.push('\n');
                        i += 2;
                        continue;
                    }
                    b'"' | b'\\' | b'{' | b'}' => {
                        buf.push(bytes[i + 1] as char);
                        i += 2;
                        continue;
                    }
                    _ => {}
                }
            }
            if bytes[i] == b'{' {
                if let Some(close) = raw[i + 1..].find('}') {
                    let tag = &raw[i + 1..i + 1 + close];
                    let after_tag = i + 2 + close;
                    if tag == "b" {
                        if let Some(end_b) = raw[after_tag..].find("{/b}") {
                            flush(&mut buf, &mut spans);
                            spans.push(RichSpan::Emphasis(
                                raw[after_tag..after_tag + end_b].to_owned(),
                            ));
                            i = after_tag + end_b + 4;
                            continue;
                        }
                    } else if tag == "w" {
                        flush(&mut buf, &mut spans);
                        spans.push(RichSpan::Pause {
                            duration_us: 0,
                            click: true,
                        });
                        i = after_tag;
                        continue;
                    } else if let Some(secs) = tag.strip_prefix("w=") {
                        if let Ok(s) = secs.parse::<f64>() {
                            flush(&mut buf, &mut spans);
                            spans.push(RichSpan::Pause {
                                duration_us: (s.max(0.0) * 1_000_000.0) as u64,
                                click: false,
                            });
                            i = after_tag;
                            continue;
                        }
                    } else if let Some(rb) = tag.strip_prefix("ruby=") {
                        if let Some((base, ruby)) = rb.split_once('|') {
                            flush(&mut buf, &mut spans);
                            spans.push(RichSpan::Ruby {
                                base: base.to_owned(),
                                ruby: ruby.to_owned(),
                            });
                            i = after_tag;
                            continue;
                        }
                    } else if let Some(hex) = tag.strip_prefix('#') {
                        if let Some(rgba) = parse_hex_color(hex) {
                            if let Some(end_c) = raw[after_tag..].find("{/color}") {
                                flush(&mut buf, &mut spans);
                                spans.push(RichSpan::Color {
                                    text: raw[after_tag..after_tag + end_c].to_owned(),
                                    rgba,
                                });
                                i = after_tag + end_c + 8;
                                continue;
                            }
                        }
                    } else if let Some(&vid) = self.var_map.get(tag) {
                        flush(&mut buf, &mut spans);
                        spans.push(RichSpan::Var(vid));
                        i = after_tag;
                        continue;
                    }
                }
            }
            let ch = raw[i..].chars().next().unwrap();
            buf.push(ch);
            i += ch.len_utf8();
        }
        flush(&mut buf, &mut spans);
        if spans.is_empty() {
            spans.push(RichSpan::Text(String::new()));
        }
        TextEntry { spans }
    }

    fn compile_files(&mut self, files: &[ScriptFile]) -> Result<()> {
        // Pass 1: Pre-register all `character` and `var` declarations across all files
        // so forward references and `{var}` interpolations always resolve!
        for file in files {
            for (idx, raw) in file.source.lines().enumerate() {
                let line_no = idx + 1;
                let trimmed = raw.trim();
                if let Some(rest) = trimmed.strip_prefix("character ") {
                    self.parse_character(rest, &format!("{}:{line_no}", file.name))?;
                } else if let Some(rest) = trimmed
                    .strip_prefix("var ")
                    .or_else(|| trimmed.strip_prefix("@var "))
                {
                    let (name, expr) = rest.split_once('=').ok_or_else(|| {
                        anyhow!("E_NIRS ({}:{line_no}): expected `var <name> = <literal>`", file.name)
                    })?;
                    let val = parse_literal(expr.trim())?;
                    self.ensure_var(name.trim(), val);
                }
            }
        }

        // Ensure `main` function exists as FuncId(0).
        let main_name = self.intern_str("main");
        self.funcs.push(FuncMeta {
            name: main_name,
            entry_pc: 0,
            param_count: 0,
            local_count: 0,
        });
        self.func_map.insert("main".into(), FuncId(0));

        let mut has_explicit_main = false;

        // Pass 2: Compile statements sequentially
        for file in files {
            let file_str = self.intern_str(&file.name);
            let lines: Vec<(usize, &str)> = file
                .source
                .lines()
                .enumerate()
                .map(|(i, l)| (i + 1, l))
                .collect();

            let mut idx = 0;
            let mut if_stack: Vec<IfFrame> = Vec::new();
            let mut in_func = false;

            while idx < lines.len() {
                let (line_no, raw) = lines[idx];
                idx += 1;
                let trimmed = raw.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
                    continue;
                }
                let loc = format!("{}:{line_no}", file.name);

                // Skip pass-1 declarations
                if trimmed.starts_with("character ")
                    || trimmed.starts_with("var ")
                    || trimmed.starts_with("@var ")
                {
                    continue;
                }

                // Function definitions: `func name:`
                if let Some(rest) = trimmed
                    .strip_prefix("func ")
                    .or_else(|| trimmed.strip_prefix("@function "))
                {
                    let fname = rest.trim().trim_end_matches(':').trim();
                    if in_func {
                        self.ops.push(Op::Ret { has_value: false });
                    }
                    let pc = self.ops.len() as u32;
                    if fname == "main" {
                        self.funcs[0].entry_pc = pc;
                        has_explicit_main = true;
                    } else {
                        let s = self.intern_str(fname);
                        let fid = FuncId(self.funcs.len() as u32);
                        self.funcs.push(FuncMeta {
                            name: s,
                            entry_pc: pc,
                            param_count: 0,
                            local_count: 0,
                        });
                        self.func_map.insert(fname.to_owned(), fid);
                    }
                    self.record_label(fname, pc, file_str, line_no as u32);
                    in_func = true;
                    continue;
                }

                // Labels: `label foo:` or `*foo`
                if let Some(rest) = trimmed
                    .strip_prefix("label ")
                    .or_else(|| trimmed.strip_prefix("@block "))
                {
                    let lname = rest.trim().trim_end_matches(':').trim();
                    let pc = self.ops.len() as u32;
                    self.record_label(lname, pc, file_str, line_no as u32);
                    continue;
                }

                // Control blocks: `if <expr>:`, `elif <expr>:`, `else:`, `endif` / `end`
                if let Some(cond_str) = trimmed.strip_prefix("if ").and_then(|s| s.strip_suffix(':')) {
                    self.compile_expr(cond_str.trim(), &loc)?;
                    let jif_idx = self.ops.len();
                    self.ops.push(Op::JumpIfFalse(0));
                    if_stack.push(IfFrame {
                        pending_jif: Some(jif_idx),
                        end_jumps: Vec::new(),
                    });
                    continue;
                }
                if let Some(cond_str) = trimmed
                    .strip_prefix("elif ")
                    .and_then(|s| s.strip_suffix(':'))
                {
                    let frame = if_stack
                        .last_mut()
                        .ok_or_else(|| anyhow!("E_NIRS ({loc}): `elif` without matching `if`"))?;
                    let jmp_end = self.ops.len();
                    self.ops.push(Op::Jump(0));
                    frame.end_jumps.push(jmp_end);
                    let here = self.ops.len() as u32;
                    if let Some(jif) = frame.pending_jif.take() {
                        self.ops[jif] = Op::JumpIfFalse(here);
                    }
                    self.compile_expr(cond_str.trim(), &loc)?;
                    let next_jif = self.ops.len();
                    self.ops.push(Op::JumpIfFalse(0));
                    frame.pending_jif = Some(next_jif);
                    continue;
                }
                if trimmed == "else:" {
                    let frame = if_stack
                        .last_mut()
                        .ok_or_else(|| anyhow!("E_NIRS ({loc}): `else` without matching `if`"))?;
                    let jmp_end = self.ops.len();
                    self.ops.push(Op::Jump(0));
                    frame.end_jumps.push(jmp_end);
                    let here = self.ops.len() as u32;
                    if let Some(jif) = frame.pending_jif.take() {
                        self.ops[jif] = Op::JumpIfFalse(here);
                    }
                    continue;
                }
                if trimmed == "endif" || trimmed == "end" {
                    if let Some(frame) = if_stack.pop() {
                        let here = self.ops.len() as u32;
                        if let Some(jif) = frame.pending_jif {
                            self.ops[jif] = Op::JumpIfFalse(here);
                        }
                        for j in frame.end_jumps {
                            self.ops[j] = Op::Jump(here);
                        }
                        continue;
                    }
                }

                // Menu block: `menu:` followed by `- "Label" [if <expr>] -> <target_label>`
                if trimmed == "menu:" || trimmed.starts_with("@choice ") {
                    let mut items = Vec::new();
                    let mut patch_targets = Vec::new();
                    let mut opt_idx = 0usize;
                    while idx < lines.len() {
                        let (oln, oraw) = lines[idx];
                        let ot = oraw.trim();
                        if ot.is_empty() || ot.starts_with('#') {
                            idx += 1;
                            continue;
                        }
                        let Some(rest) = ot.strip_prefix('-') else {
                            break;
                        };
                        idx += 1;
                        let oloc = format!("{}:{oln}", file.name);
                        let (left, target) = rest.split_once("->").ok_or_else(|| {
                            anyhow!("E_NIRS ({oloc}): choice option expects `- \"Label\" -> target`")
                        })?;
                        let target = target.trim().to_owned();
                        let (label_part, cond_part) = if let Some((l, c)) = left.rsplit_once(" if ") {
                            (l.trim(), Some(c.trim()))
                        } else {
                            (left.trim(), None)
                        };
                        let cond_var = if let Some(cond) = cond_part {
                            let tmp_name = format!("__cond_{}_{}", self.ops.len(), opt_idx);
                            let vid = self.ensure_var(&tmp_name, Value::Bool(true));
                            self.compile_expr(cond, &oloc)?;
                            self.ops.push(Op::StoreVar(vid));
                            Some(vid)
                        } else {
                            None
                        };
                        let label_str = unquote(label_part);
                        let text_id = self.add_text(&label_str);
                        items.push(ChoiceItem {
                            text: text_id,
                            image: None,
                            hover_image: None,
                            disabled_image: None,
                            preview: None,
                            rect: None,
                            target_pc: 0,
                            cond_var,
                            disabled_var: None,
                            value: None,
                        });
                        patch_targets.push((opt_idx, target, oloc));
                        opt_idx += 1;
                    }
                    let op_idx = self.ops.len();
                    self.ops.push(Op::Choice {
                        items,
                        result_var: None,
                        cancel_pc: None,
                        timeout_us: None,
                        default_idx: None,
                    });
                    for (oi, target, oloc) in patch_targets {
                        self.choice_patches.push((op_idx, oi, target, oloc));
                    }
                    continue;
                }

                self.compile_statement(trimmed, &loc)?;
            }

            if !if_stack.is_empty() {
                bail!("E_NIRS ({}): unclosed `if` block (missing `endif`)", file.name);
            }
            if in_func {
                self.ops.push(Op::Ret { has_value: false });
            }
        }

        let _ = has_explicit_main;
        // Terminate fallthrough cleanly
        let end_str = self.intern_str("end");
        self.ops.push(Op::End { outcome: end_str });

        // Pass 3: Resolve all label & function patches
        for (op_idx, target, loc) in std::mem::take(&mut self.jump_patches) {
            let pc = *self
                .label_pcs
                .get(&target)
                .ok_or_else(|| anyhow!("E_NIRS ({loc}): unknown jump target `{target}`"))?;
            match &mut self.ops[op_idx] {
                Op::Jump(t) | Op::JumpIfFalse(t) => *t = pc,
                _ => unreachable!(),
            }
        }
        for (op_idx, opt_idx, target, loc) in std::mem::take(&mut self.choice_patches) {
            let pc = *self
                .label_pcs
                .get(&target)
                .ok_or_else(|| anyhow!("E_NIRS ({loc}): unknown choice target `{target}`"))?;
            if let Op::Choice { items, .. } = &mut self.ops[op_idx] {
                items[opt_idx].target_pc = pc;
            }
        }
        for (op_idx, target, loc) in std::mem::take(&mut self.call_patches) {
            let fid = *self
                .func_map
                .get(&target)
                .ok_or_else(|| anyhow!("E_NIRS ({loc}): unknown function `{target}`"))?;
            if let Op::Call { func, .. } = &mut self.ops[op_idx] {
                *func = fid;
            }
        }

        Ok(())
    }

    fn record_label(&mut self, name: &str, pc: u32, file: StrId, line: u32) {
        let s = self.intern_str(name);
        self.label_pcs.insert(name.to_owned(), pc);
        self.labels.push(LabelMeta {
            name: s,
            pc,
            file,
            line,
        });
    }

    fn parse_character(&mut self, rest: &str, loc: &str) -> Result<()> {
        let tokens = split_tokens(rest);
        if tokens.len() < 2 {
            bail!("E_NIRS ({loc}): expected `character <id> \"<Display Name>\" [color=#rrggbb]`");
        }
        let id = tokens[0].clone();
        let display_name = unquote(&tokens[1]);
        let mut color = None;
        let mut voice_prefix = None;
        for tok in &tokens[2..] {
            if let Some(hex) = tok.strip_prefix("color=") {
                color = parse_hex_color(hex.trim_start_matches('#'));
            } else if let Some(vp) = tok.strip_prefix("voice=") {
                voice_prefix = Some(unquote(vp));
            }
        }
        self.characters.insert(
            id,
            CharacterDef {
                display_name,
                color,
                voice_prefix,
            },
        );
        Ok(())
    }

    fn compile_statement(&mut self, line: &str, loc: &str) -> Result<()> {
        // Strip optional `@` prefix for compatibility with `@bgm`, `@jump`, etc.
        let s = line.strip_prefix('@').unwrap_or(line);

        // `$ x += 1` / `set x = expr`
        if let Some(rest) = s.strip_prefix("$ ").or_else(|| s.strip_prefix("set ")) {
            return self.compile_assignment(rest.trim(), loc);
        }

        // Control keywords
        if let Some(target) = s.strip_prefix("jump ") {
            let op_idx = self.ops.len();
            self.ops.push(Op::Jump(0));
            self.jump_patches
                .push((op_idx, target.trim().to_owned(), loc.to_owned()));
            return Ok(());
        }
        if let Some(target) = s.strip_prefix("call ") {
            let op_idx = self.ops.len();
            self.ops.push(Op::Call {
                func: FuncId(0),
                argc: 0,
            });
            self.call_patches
                .push((op_idx, target.trim().to_owned(), loc.to_owned()));
            return Ok(());
        }
        if s == "return" || s.starts_with("return ") {
            let expr = s.strip_prefix("return").unwrap().trim();
            if expr.is_empty() {
                self.ops.push(Op::Ret { has_value: false });
            } else {
                self.compile_expr(expr, loc)?;
                self.ops.push(Op::Ret { has_value: true });
            }
            return Ok(());
        }
        if s == "end_game" || s.starts_with("end_game ") {
            let outcome = s.strip_prefix("end_game").unwrap().trim();
            let sid = self.intern_str(if outcome.is_empty() { "end" } else { outcome });
            self.ops.push(Op::End { outcome: sid });
            return Ok(());
        }
        if let Some(flag) = s.strip_prefix("unlock ") {
            let sid = self.intern_str(unquote(flag.trim()).as_str());
            self.ops.push(Op::ProfileSetFlag(sid));
            return Ok(());
        }

        // Scene commands: `bg <asset> [with ...]`
        if let Some(rest) = s.strip_prefix("bg ") {
            let (body, with_part) = split_with_clause(rest);
            let toks = split_tokens(body);
            if toks.is_empty() {
                bail!("E_NIRS ({loc}): expected `bg <asset>`");
            }
            let asset = self.resolve_asset(&toks[0], loc)?;
            let bg_layer = self.intern_layer("bg");
            let transform = Transform2D {
                width: self.stage[0] as f32,
                height: self.stage[1] as f32,
                z_order: -100,
                ..Default::default()
            };
            self.ops.push(Op::LayerSet {
                layer: bg_layer,
                parent: None,
                asset: Some(asset),
                transform: Box::new(transform),
            });
            if let Some(tr_str) = with_part {
                let (tr, dur) = self.parse_transition(tr_str, loc)?;
                self.ops.push(Op::StageCommit {
                    transition: tr,
                    duration_us: dur,
                    wait: dur > 0,
                });
            }
            return Ok(());
        }

        // `show <layer> <asset> [at left|center|right|x,y] [scale=F] [opacity=F] [z=N] [rot=F] [blend=...] [with ...]`
        if let Some(rest) = s.strip_prefix("show ") {
            let (body, with_part) = split_with_clause(rest);
            let toks = split_tokens(body);
            if toks.len() < 2 {
                bail!("E_NIRS ({loc}): expected `show <layer> <asset> [options]`");
            }
            let layer = self.intern_layer(&toks[0]);
            let asset = self.resolve_asset(&toks[1], loc)?;
            let sw = self.stage[0] as f32;
            let sh = self.stage[1] as f32;
            let mut tr = Transform2D {
                x: sw * 0.5,
                y: sh,
                anchor: [0.5, 1.0],
                z_order: 10,
                ..Default::default()
            };
            let mut i = 2;
            while i < toks.len() {
                if toks[i] == "at" && i + 1 < toks.len() {
                    let pos = &toks[i + 1];
                    match pos.as_str() {
                        "left" => {
                            tr.x = sw * 0.25;
                            tr.y = sh;
                            tr.anchor = [0.5, 1.0];
                        }
                        "center" => {
                            tr.x = sw * 0.5;
                            tr.y = sh;
                            tr.anchor = [0.5, 1.0];
                        }
                        "right" => {
                            tr.x = sw * 0.75;
                            tr.y = sh;
                            tr.anchor = [0.5, 1.0];
                        }
                        coords => {
                            if let Some((xs, ys)) = coords.split_once(',') {
                                tr.x = xs.parse().unwrap_or(0.0);
                                tr.y = ys.parse().unwrap_or(0.0);
                                tr.anchor = [0.0, 0.0];
                            }
                        }
                    }
                    i += 2;
                    continue;
                }
                if let Some((k, v)) = toks[i].split_once('=') {
                    match k {
                        "x" => {
                            tr.x = v.parse().unwrap_or(tr.x);
                            tr.anchor = [0.0, 0.0];
                        }
                        "y" => {
                            tr.y = v.parse().unwrap_or(tr.y);
                            tr.anchor = [0.0, 0.0];
                        }
                        "w" | "width" => tr.width = v.parse().unwrap_or(0.0),
                        "h" | "height" => tr.height = v.parse().unwrap_or(0.0),
                        "scale" => {
                            let sc: f32 = v.parse().unwrap_or(1.0);
                            tr.scale = [sc, sc];
                        }
                        "opacity" => tr.opacity = v.parse().unwrap_or(1.0),
                        "z" | "order" => tr.z_order = v.parse().unwrap_or(10),
                        "rot" | "rotation" => tr.rotation_deg = v.parse().unwrap_or(0.0),
                        "blend" => {
                            tr.blend = match v {
                                "add" => BlendMode::Add,
                                "multiply" => BlendMode::Multiply,
                                "screen" => BlendMode::Screen,
                                _ => BlendMode::Normal,
                            }
                        }
                        _ => {}
                    }
                }
                i += 1;
            }
            self.ops.push(Op::LayerSet {
                layer,
                parent: None,
                asset: Some(asset),
                transform: Box::new(tr),
            });
            if let Some(tr_str) = with_part {
                let (t, dur) = self.parse_transition(tr_str, loc)?;
                self.ops.push(Op::StageCommit {
                    transition: t,
                    duration_us: dur,
                    wait: dur > 0,
                });
            }
            return Ok(());
        }

        // `hide <layer> [with ...]`
        if let Some(rest) = s.strip_prefix("hide ") {
            let (body, with_part) = split_with_clause(rest);
            let layer = self.intern_layer(body.trim());
            self.ops.push(Op::LayerRemove { layer });
            if let Some(tr_str) = with_part {
                let (t, dur) = self.parse_transition(tr_str, loc)?;
                self.ops.push(Op::StageCommit {
                    transition: t,
                    duration_us: dur,
                    wait: dur > 0,
                });
            }
            return Ok(());
        }

        if s == "clear" || s.starts_with("clear ") {
            let (_, with_part) = split_with_clause(s.strip_prefix("clear").unwrap());
            self.ops.push(Op::LayerClear);
            if let Some(tr_str) = with_part {
                let (t, dur) = self.parse_transition(tr_str, loc)?;
                self.ops.push(Op::StageCommit {
                    transition: t,
                    duration_us: dur,
                    wait: dur > 0,
                });
            }
            return Ok(());
        }

        // `commit [dissolve 300]`
        if s == "commit" || s.starts_with("commit ") {
            let rest = s.strip_prefix("commit").unwrap().trim();
            let (t, dur) = if rest.is_empty() {
                (Transition::Cut, 0)
            } else {
                self.parse_transition(rest, loc)?
            };
            self.ops.push(Op::StageCommit {
                transition: t,
                duration_us: dur,
                wait: dur > 0,
            });
            return Ok(());
        }

        // `tween <layer> <prop> <to> <ms> [easing] [wait]`
        if let Some(rest) = s.strip_prefix("tween ") {
            let toks = split_tokens(rest);
            if toks.len() < 4 {
                bail!("E_NIRS ({loc}): expected `tween <layer> <prop> <to> <ms> [easing] [wait]`");
            }
            let target = if toks[0] == "dialogue" {
                TweenTarget::DialogueBox
            } else {
                TweenTarget::Layer(self.intern_layer(&toks[0]))
            };
            let prop = match toks[1].as_str() {
                "x" => TweenProp::X,
                "y" => TweenProp::Y,
                "scale_x" | "scale" => TweenProp::ScaleX,
                "scale_y" => TweenProp::ScaleY,
                "rot" | "rotation" => TweenProp::Rotation,
                "opacity" => TweenProp::Opacity,
                other => bail!("E_NIRS ({loc}): unknown tween property `{other}`"),
            };
            let to: f32 = toks[2].parse()?;
            let ms: u64 = toks[3].parse()?;
            let mut easing = Easing::Smooth;
            let mut wait = false;
            for tok in &toks[4..] {
                match tok.as_str() {
                    "linear" => easing = Easing::Linear,
                    "smooth" => easing = Easing::Smooth,
                    "ease_in" => easing = Easing::EaseIn,
                    "ease_out" => easing = Easing::EaseOut,
                    "ease_in_out" => easing = Easing::EaseInOut,
                    "wait" => wait = true,
                    _ => {}
                }
            }
            self.ops.push(Op::Tween {
                target,
                prop,
                to,
                duration_us: ms * 1000,
                easing,
                wait,
            });
            return Ok(());
        }

        // `shake <layer|dialogue> [amp=x,y] [freq=hz] [ms=ms] [mode=quake|wave|bound] [wait]`
        if let Some(rest) = s.strip_prefix("shake ") {
            let toks = split_tokens(rest);
            if toks.is_empty() {
                bail!("E_NIRS ({loc}): expected `shake <target>`");
            }
            let target = if toks[0] == "dialogue" {
                TweenTarget::DialogueBox
            } else {
                TweenTarget::Layer(self.intern_layer(&toks[0]))
            };
            let mut spec = ShakeSpec {
                amplitude: [12.0, 12.0],
                freq_hz: 18.0,
                duration_us: 400_000,
                decay: true,
                mode: ShakeMode::Quake,
            };
            let mut wait = false;
            for tok in &toks[1..] {
                if tok == "wait" {
                    wait = true;
                } else if let Some((k, v)) = tok.split_once('=') {
                    match k {
                        "amp" => {
                            if let Some((a, b)) = v.split_once(',') {
                                spec.amplitude = [a.parse().unwrap_or(12.0), b.parse().unwrap_or(12.0)];
                            } else if let Ok(a) = v.parse::<f32>() {
                                spec.amplitude = [a, a];
                            }
                        }
                        "freq" => spec.freq_hz = v.parse().unwrap_or(18.0),
                        "ms" => spec.duration_us = v.parse::<u64>().unwrap_or(400) * 1000,
                        "mode" => {
                            spec.mode = match v {
                                "wave" => ShakeMode::Wave,
                                "bound" => ShakeMode::Bound,
                                _ => ShakeMode::Quake,
                            }
                        }
                        _ => {}
                    }
                }
            }
            self.ops.push(Op::Shake { target, spec, wait });
            return Ok(());
        }

        // Audio commands
        if let Some(rest) = s
            .strip_prefix("bgm ")
            .or_else(|| s.strip_prefix("sfx "))
            .or_else(|| s.strip_prefix("voice "))
        {
            let kind = s.split_whitespace().next().unwrap();
            let toks = split_tokens(rest);
            if toks.is_empty() {
                bail!("E_NIRS ({loc}): expected `{kind} <asset>`");
            }
            let asset = self.resolve_asset(&toks[0], loc)?;
            let (bus, channel, looped) = match kind {
                "bgm" => (AudioBus::Bgm, self.intern_channel("bgm"), true),
                "voice" => (AudioBus::Voice, self.intern_channel("voice"), false),
                _ => (AudioBus::Sfx, self.intern_channel("sfx"), false),
            };
            let mut gain = 1.0f32;
            let mut fade_in_us = 0u64;
            let mut loop_region = None;
            for tok in &toks[1..] {
                if let Some((k, v)) = tok.split_once('=') {
                    match k {
                        "gain" | "vol" => gain = v.parse().unwrap_or(1.0),
                        "fade" | "fadein" => fade_in_us = v.parse::<u64>().unwrap_or(0) * 1000,
                        "loop" => {
                            if let Some((a, b)) = v.split_once("..") {
                                if let (Ok(s_ms), Ok(e_ms)) = (a.parse::<u64>(), b.parse::<u64>()) {
                                    loop_region = Some(AudioLoopRegion {
                                        start_us: s_ms * 1000,
                                        end_us: e_ms * 1000,
                                    });
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            self.ops.push(Op::AudioPlay {
                channel,
                bus,
                asset,
                gain,
                looped,
                fade_in_us,
                loop_region,
            });
            return Ok(());
        }

        if s == "stop_bgm" || s.starts_with("stop_bgm ") {
            let fade_ms: u64 = s
                .strip_prefix("stop_bgm")
                .unwrap()
                .trim()
                .parse()
                .unwrap_or(0);
            let ch = self.intern_channel("bgm");
            self.ops.push(Op::AudioStop {
                channel: ch,
                fade_out_us: fade_ms * 1000,
            });
            return Ok(());
        }

        if let Some(rest) = s.strip_prefix("video ") {
            let toks = split_tokens(rest);
            let asset = self.resolve_asset(&toks[0], loc)?;
            let skippable = !toks.iter().any(|t| t == "skippable=false");
            self.ops.push(Op::VideoPlay { asset, skippable });
            return Ok(());
        }

        if let Some(rest) = s.strip_prefix("window ") {
            let toks = split_tokens(rest);
            let visible = toks.first().map(String::as_str) != Some("hide");
            let (tr, dur) = if toks.len() > 1 {
                self.parse_transition(&toks[1..].join(" "), loc)?
            } else {
                (Transition::Cut, 0)
            };
            self.ops.push(Op::TextWindow {
                visible,
                transition: tr,
                duration_us: dur,
            });
            return Ok(());
        }

        if let Some(rest) = s.strip_prefix("wait ") {
            let toks = split_tokens(rest);
            let ms: u64 = toks
                .first()
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| anyhow!("E_NIRS ({loc}): expected `wait <ms>`"))?;
            let skippable = toks.iter().any(|t| t == "skippable" || t == "click");
            self.ops.push(Op::Wait {
                duration_us: ms * 1000,
                skippable,
            });
            return Ok(());
        }

        // Otherwise: Dialogue or Narration!
        self.compile_dialogue(line, loc)
    }

    fn compile_dialogue(&mut self, line: &str, loc: &str) -> Result<()> {
        // Narration: `"text"` or `> text`
        if let Some(rest) = line.strip_prefix('>') {
            let tid = self.add_text(rest.trim());
            self.ops.push(Op::Say {
                speaker: None,
                speaker_color: None,
                text: tid,
                voice: None,
                reveal_us: 0,
                append: false,
            });
            return Ok(());
        }
        if line.starts_with('"') && line.ends_with('"') && line.len() >= 2 {
            let tid = self.add_text(&line[1..line.len() - 1]);
            self.ops.push(Op::Say {
                speaker: None,
                speaker_color: None,
                text: tid,
                voice: None,
                reveal_us: 0,
                append: false,
            });
            return Ok(());
        }

        // Speaker line: `aki [voice=v01]: "Hello!"` or `秋：你好`
        let split_pos = find_speaker_colon(line);
        if let Some((head, body)) = split_pos {
            let head = head.trim();
            let mut voice = None;
            let speaker_key = if let Some((sp, meta)) = head.split_once('[') {
                let meta = meta.trim_end_matches(']');
                for part in meta.split_whitespace() {
                    if let Some(v) = part.strip_prefix("voice=") {
                        voice = Some(self.resolve_asset(v, loc)?);
                    }
                }
                sp.trim()
            } else {
                head
            };
            let (display_name, color) = if let Some(ch) = self.characters.get(speaker_key) {
                (ch.display_name.clone(), ch.color)
            } else {
                (unquote(speaker_key), None)
            };
            let speaker_id = if display_name.is_empty() {
                None
            } else {
                Some(self.intern_str(&display_name))
            };
            let text_raw = unquote(body.trim());
            let tid = self.add_text(&text_raw);
            self.ops.push(Op::Say {
                speaker: speaker_id,
                speaker_color: color,
                text: tid,
                voice,
                reveal_us: 0,
                append: false,
            });
            return Ok(());
        }

        // Fallback: plain narration line
        let tid = self.add_text(line);
        self.ops.push(Op::Say {
            speaker: None,
            speaker_color: None,
            text: tid,
            voice: None,
            reveal_us: 0,
            append: false,
        });
        Ok(())
    }

    fn compile_assignment(&mut self, expr: &str, loc: &str) -> Result<()> {
        for (op_str, binop) in [("+=", BinOp::Add), ("-=", BinOp::Sub), ("*=", BinOp::Mul)] {
            if let Some((lhs, rhs)) = expr.split_once(op_str) {
                let name = lhs.trim();
                let vid = self.ensure_var(name, Value::Int(0));
                self.ops.push(Op::LoadVar(vid));
                self.compile_expr(rhs.trim(), loc)?;
                self.ops.push(Op::BinOp(binop));
                self.ops.push(Op::StoreVar(vid));
                return Ok(());
            }
        }
        let (lhs, rhs) = expr
            .split_once('=')
            .ok_or_else(|| anyhow!("E_NIRS ({loc}): expected assignment `<var> = <expr>`"))?;
        let name = lhs.trim();
        let vid = self.ensure_var(name, Value::Int(0));
        // Built-in `rand(min, max)`
        if let Some(inner) = rhs.trim().strip_prefix("rand(").and_then(|s| s.strip_suffix(')')) {
            if let Some((a, b)) = inner.split_once(',') {
                let min: i64 = a.trim().parse()?;
                let max: i64 = b.trim().parse()?;
                self.ops.push(Op::RandInt { dst: vid, min, max });
                return Ok(());
            }
        }
        self.compile_expr(rhs.trim(), loc)?;
        self.ops.push(Op::StoreVar(vid));
        Ok(())
    }

    #[allow(clippy::only_used_in_recursion)]
    fn compile_expr(&mut self, expr: &str, loc: &str) -> Result<()> {
        let e = expr.trim();
        if e.is_empty() {
            self.ops.push(Op::PushNull);
            return Ok(());
        }
        // Strip outer balanced parens
        if e.starts_with('(') && e.ends_with(')') && is_wrapped_parens(e) {
            return self.compile_expr(&e[1..e.len() - 1], loc);
        }
        // Operator precedence (lowest → highest): or, and, cmp, add/sub, mul/div/rem
        for (tok, op) in [
            (" or ", BinOp::Or),
            (" || ", BinOp::Or),
            (" and ", BinOp::And),
            (" && ", BinOp::And),
            ("==", BinOp::Eq),
            ("!=", BinOp::Ne),
            ("<=", BinOp::Le),
            (">=", BinOp::Ge),
            ("<", BinOp::Lt),
            (">", BinOp::Gt),
            ("+", BinOp::Add),
            ("-", BinOp::Sub),
            ("*", BinOp::Mul),
            ("/", BinOp::Div),
            ("%", BinOp::Rem),
        ] {
            if let Some((l, r)) = split_top_level(e, tok) {
                self.compile_expr(l, loc)?;
                self.compile_expr(r, loc)?;
                self.ops.push(Op::BinOp(op));
                return Ok(());
            }
        }
        if let Some(rest) = e.strip_prefix("not ").or_else(|| e.strip_prefix('!')) {
            self.compile_expr(rest, loc)?;
            self.ops.push(Op::UnaryOp(UnaryOp::Not));
            return Ok(());
        }
        if let Some(rest) = e.strip_prefix('-') {
            if let Ok(n) = e.parse::<i64>() {
                self.ops.push(Op::PushInt(n));
                return Ok(());
            }
            if let Ok(f) = e.parse::<f64>() {
                self.ops.push(Op::PushFloat(f));
                return Ok(());
            }
            self.compile_expr(rest, loc)?;
            self.ops.push(Op::UnaryOp(UnaryOp::Neg));
            return Ok(());
        }
        if e == "true" {
            self.ops.push(Op::PushBool(true));
        } else if e == "false" {
            self.ops.push(Op::PushBool(false));
        } else if e == "null" {
            self.ops.push(Op::PushNull);
        } else if e.starts_with('"') && e.ends_with('"') && e.len() >= 2 {
            let sid = self.intern_str(&e[1..e.len() - 1]);
            self.ops.push(Op::PushStr(sid));
        } else if let Ok(n) = e.parse::<i64>() {
            self.ops.push(Op::PushInt(n));
        } else if let Ok(f) = e.parse::<f64>() {
            self.ops.push(Op::PushFloat(f));
        } else {
            let vid = self.ensure_var(e, Value::Int(0));
            self.ops.push(Op::LoadVar(vid));
        }
        Ok(())
    }

    fn parse_transition(&mut self, spec: &str, loc: &str) -> Result<(Transition, u64)> {
        let toks = split_tokens(spec);
        let kind = toks.first().map(String::as_str).unwrap_or("cut");
        let dur_ms: u64 = toks
            .get(1)
            .and_then(|s| s.trim_end_matches("ms").parse().ok())
            .unwrap_or(if kind == "cut" { 0 } else { 350 });
        let tr = match kind {
            "cut" => Transition::Cut,
            "dissolve" | "fade" => Transition::Dissolve,
            "wipe_left" => Transition::Wipe {
                dir: WipeDir::LeftToRight,
                softness: 0.15,
            },
            "wipe_right" => Transition::Wipe {
                dir: WipeDir::RightToLeft,
                softness: 0.15,
            },
            "wipe_up" => Transition::Wipe {
                dir: WipeDir::BottomToTop,
                softness: 0.15,
            },
            "wipe_down" => Transition::Wipe {
                dir: WipeDir::TopToBottom,
                softness: 0.15,
            },
            other => {
                if let Some(asset_name) = other
                    .strip_prefix("mask(")
                    .and_then(|s| s.strip_suffix(')'))
                {
                    let mask = self.resolve_asset(asset_name, loc)?;
                    Transition::RuleMask {
                        mask,
                        invert: false,
                        softness: 0.15,
                    }
                } else {
                    Transition::Dissolve
                }
            }
        };
        Ok((tr, dur_ms * 1000))
    }
}

struct IfFrame {
    pending_jif: Option<usize>,
    end_jumps: Vec<usize>,
}

pub fn compile_scripts(
    files: &[ScriptFile],
    assets: &AssetResolver,
    stage: [u32; 2],
) -> Result<CompileOutput> {
    let mut c = Compiler::new(assets, stage);
    c.compile_files(files)?;
    Ok(CompileOutput {
        strings: c.strings,
        vars: c.vars,
        funcs: c.funcs,
        labels: c.labels,
        ops: c.ops,
        texts: c.texts,
        used_assets: c.used_assets,
    })
}

pub fn load_script_file(path: &Path, name: String) -> Result<ScriptFile> {
    Ok(ScriptFile {
        name,
        source: std::fs::read_to_string(path)?,
    })
}

fn split_with_clause(s: &str) -> (&str, Option<&str>) {
    if let Some((a, b)) = s.rsplit_once(" with ") {
        (a.trim(), Some(b.trim()))
    } else {
        (s.trim(), None)
    }
}

fn split_tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for ch in s.chars() {
        if ch == '"' {
            in_quote = !in_quote;
            cur.push(ch);
        } else if ch.is_whitespace() && !in_quote {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(ch);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn unquote(s: &str) -> String {
    let t = s.trim();
    if t.starts_with('"') && t.ends_with('"') && t.len() >= 2 {
        t[1..t.len() - 1].to_owned()
    } else {
        t.to_owned()
    }
}

fn find_speaker_colon(line: &str) -> Option<(&str, &str)> {
    let mut in_quote = false;
    let mut in_bracket = false;
    for (idx, ch) in line.char_indices() {
        match ch {
            '"' => in_quote = !in_quote,
            '[' if !in_quote => in_bracket = true,
            ']' if !in_quote => in_bracket = false,
            ':' | '：' if !in_quote && !in_bracket => {
                return Some((&line[..idx], &line[idx + ch.len_utf8()..]));
            }
            _ => {}
        }
    }
    None
}

fn parse_literal(s: &str) -> Result<Value> {
    if s == "true" {
        Ok(Value::Bool(true))
    } else if s == "false" {
        Ok(Value::Bool(false))
    } else if s == "null" {
        Ok(Value::Null)
    } else if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        Ok(Value::Str(s[1..s.len() - 1].to_owned()))
    } else if let Ok(n) = s.parse::<i64>() {
        Ok(Value::Int(n))
    } else if let Ok(f) = s.parse::<f64>() {
        Ok(Value::Float(f))
    } else {
        bail!("E_NIRS: invalid literal `{s}`")
    }
}

pub fn parse_hex_color(hex: &str) -> Option<[f32; 4]> {
    let h = hex.trim().trim_start_matches('#');
    let byte = |i| u8::from_str_radix(&h[i..i + 2], 16).ok().map(|v| v as f32 / 255.0);
    match h.len() {
        6 => Some([byte(0)?, byte(2)?, byte(4)?, 1.0]),
        8 => Some([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        _ => None,
    }
}

fn is_wrapped_parens(s: &str) -> bool {
    let mut depth = 0i32;
    for (i, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 && i + 1 < s.len() {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

fn split_top_level<'a>(s: &'a str, op: &str) -> Option<(&'a str, &'a str)> {
    let mut depth = 0i32;
    let mut in_quote = false;
    let bytes = s.as_bytes();
    let op_bytes = op.as_bytes();
    for i in (0..bytes.len()).rev() {
        match bytes[i] {
            b'"' => in_quote = !in_quote,
            b')' if !in_quote => depth += 1,
            b'(' if !in_quote => depth -= 1,
            _ => {}
        }
        if depth == 0 && !in_quote && i > 0 && s[i..].starts_with(op) {
            // Avoid splitting `<=` or `>=` or `==` on `<`, `>`, or `-` unary
            if op_bytes.len() == 1
                && matches!(op_bytes[0], b'<' | b'>')
                && bytes.get(i + 1) == Some(&b'=')
            {
                continue;
            }
            let left = s[..i].trim();
            let right = s[i + op.len()..].trim();
            if !left.is_empty() && !right.is_empty() {
                return Some((left, right));
            }
        }
    }
    None
}
