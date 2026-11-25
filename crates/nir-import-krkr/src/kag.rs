//! KiriKiri2 / KAG3 `.ks` scenario parser and lowerer to `nir-import-core::VnStmt`.

use nir_import_core::{sanitize_ident, ConversionReport, VnChoice, VnConst, VnProject, VnStmt};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub enum KagItem {
    Label(String),
    Text(String),
    Tag {
        name: String,
        attrs: BTreeMap<String, String>,
        line: usize,
    },
}

/// Callback used by the KAG lowerer to resolve and copy/convert media assets into `VnProject`.
pub trait AssetResolver {
    fn resolve_image(&mut self, src_loc: &str, storage: &str) -> Option<String>;
    fn resolve_audio(&mut self, src_loc: &str, storage: &str) -> Option<String>;
    fn resolve_video(&mut self, src_loc: &str, storage: &str) -> Option<String>;
}

/// Parses a `.ks` file into linear `KagItem`s (handling `;` comments, `*labels`,
/// `@tag` lines, inline `[tag ...]` brackets, and text runs).
fn tokenize_ks(source: &str) -> Vec<KagItem> {
    let mut items = Vec::new();
    for (idx, raw_line) in source.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim_end_matches('\r');
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with(';') {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('*') {
            let label = rest.split('|').next().unwrap_or("").trim();
            if !label.is_empty() {
                items.push(KagItem::Label(label.to_owned()));
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('@') {
            if let Some((name, attrs)) = parse_tag_body(rest) {
                items.push(KagItem::Tag {
                    name,
                    attrs,
                    line: line_no,
                });
            }
            continue;
        }

        // Mixed text and inline `[tag ...]`
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0usize;
        let mut text_buf = String::new();
        while i < chars.len() {
            if chars[i] == '[' {
                // Escaped `[[` -> literal `[`
                if i + 1 < chars.len() && chars[i + 1] == '[' {
                    text_buf.push('[');
                    i += 2;
                    continue;
                }
                if !text_buf.is_empty() {
                    items.push(KagItem::Text(std::mem::take(&mut text_buf)));
                }
                i += 1;
                let mut tag_str = String::new();
                let mut in_quote: Option<char> = None;
                while i < chars.len() {
                    let c = chars[i];
                    if let Some(q) = in_quote {
                        tag_str.push(c);
                        if c == q {
                            in_quote = None;
                        }
                    } else if c == '"' || c == '\'' {
                        in_quote = Some(c);
                        tag_str.push(c);
                    } else if c == ']' {
                        i += 1;
                        break;
                    } else {
                        tag_str.push(c);
                    }
                    i += 1;
                }
                if let Some((name, attrs)) = parse_tag_body(&tag_str) {
                    items.push(KagItem::Tag {
                        name,
                        attrs,
                        line: line_no,
                    });
                }
            } else {
                text_buf.push(chars[i]);
                i += 1;
            }
        }
        if !text_buf.is_empty() {
            items.push(KagItem::Text(text_buf));
        }
    }
    items
}

fn parse_tag_body(raw: &str) -> Option<(String, BTreeMap<String, String>)> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let mut chars = s.chars().peekable();
    let mut name = String::new();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            break;
        }
        name.push(c);
        chars.next();
    }
    let name = name.to_ascii_lowercase();
    let mut attrs = BTreeMap::new();

    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }
        let mut key = String::new();
        while let Some(&c) = chars.peek() {
            if c == '=' || c.is_whitespace() {
                break;
            }
            key.push(c);
            chars.next();
        }
        let key = key.to_ascii_lowercase();
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.peek() == Some(&'=') {
            chars.next();
            while chars.peek().is_some_and(|c| c.is_whitespace()) {
                chars.next();
            }
            let val = if let Some(&q) = chars.peek().filter(|&&c| c == '"' || c == '\'') {
                chars.next();
                let mut v = String::new();
                for c in chars.by_ref() {
                    if c == q {
                        break;
                    }
                    v.push(c);
                }
                v
            } else {
                let mut v = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() {
                        break;
                    }
                    v.push(c);
                    chars.next();
                }
                v
            };
            if !key.is_empty() {
                attrs.insert(key, val);
            }
        } else if !key.is_empty() {
            attrs.insert(key, "true".into());
        }
    }
    Some((name, attrs))
}

/// Lowers a single KAG3 `.ks` script into `VnStmt`s.
pub fn lower_ks_script(
    script_stem: &str,
    source: &str,
    project: &mut VnProject,
    report: &mut ConversionReport,
    resolver: &mut dyn AssetResolver,
    macros: &mut BTreeMap<String, Vec<KagItem>>,
) -> Vec<VnStmt> {
    let raw_items = tokenize_ks(source);
    let mut items = Vec::with_capacity(raw_items.len());

    // First pass: capture `[macro]` definitions and expand macro invocations
    let mut i = 0;
    while i < raw_items.len() {
        if let KagItem::Tag { name, attrs, .. } = &raw_items[i] {
            if name == "macro" {
                let mname = attrs
                    .get("name")
                    .map(|s| s.to_ascii_lowercase())
                    .unwrap_or_default();
                let mut body = Vec::new();
                i += 1;
                while i < raw_items.len() {
                    if matches!(&raw_items[i], KagItem::Tag { name, .. } if name == "endmacro") {
                        i += 1;
                        break;
                    }
                    body.push(raw_items[i].clone());
                    i += 1;
                }
                if !mname.is_empty() {
                    macros.insert(mname, body);
                }
                continue;
            }
            if let Some(body) = macros.get(name) {
                for m_item in body {
                    items.push(substitute_macro_item(m_item, attrs));
                }
                i += 1;
                continue;
            }
        }
        items.push(raw_items[i].clone());
        i += 1;
    }

    let mut out = Vec::new();
    out.push(VnStmt::Label(ks_entry_label(script_stem)));
    let mut declared_vars: BTreeSet<String> = project.vars.keys().cloned().collect();
    let mut pos = 0;
    lower_slice(
        script_stem,
        &items,
        &mut pos,
        false,
        project,
        &mut declared_vars,
        report,
        resolver,
        &mut out,
    );
    out
}

fn substitute_macro_item(item: &KagItem, call_attrs: &BTreeMap<String, String>) -> KagItem {
    match item {
        KagItem::Tag { name, attrs, line } => {
            let mut new_attrs = BTreeMap::new();
            for (k, v) in attrs {
                if let Some(param) = v.strip_prefix('%') {
                    let (pname, default) = param.split_once('|').unwrap_or((param, ""));
                    let resolved = call_attrs
                        .get(&pname.to_ascii_lowercase())
                        .cloned()
                        .unwrap_or_else(|| default.to_owned());
                    new_attrs.insert(k.clone(), resolved);
                } else {
                    new_attrs.insert(k.clone(), v.clone());
                }
            }
            KagItem::Tag {
                name: name.clone(),
                attrs: new_attrs,
                line: *line,
            }
        }
        other => other.clone(),
    }
}

#[allow(clippy::too_many_arguments)]
fn lower_slice(
    script_stem: &str,
    items: &[KagItem],
    pos: &mut usize,
    stop_on_branch: bool,
    project: &mut VnProject,
    declared_vars: &mut BTreeSet<String>,
    report: &mut ConversionReport,
    resolver: &mut dyn AssetResolver,
    out: &mut Vec<VnStmt>,
) {
    let mut text_buf = String::new();
    let mut pending_ruby: Option<String> = None;
    let mut active_link: Option<(String, Option<String>, String)> = None; // (target, cond, label_buf)
    let mut pending_choices: Vec<VnChoice> = Vec::new();

    while *pos < items.len() {
        let item = &items[*pos];
        if stop_on_branch {
            if let KagItem::Tag { name, .. } = item {
                if matches!(name.as_str(), "else" | "elsif" | "endif") {
                    break;
                }
            }
        }
        *pos += 1;

        match item {
            KagItem::Label(lbl) => {
                flush_kag_text(&mut text_buf, out);
                if !pending_choices.is_empty() {
                    out.push(VnStmt::Choice(std::mem::take(&mut pending_choices)));
                }
                out.push(VnStmt::Label(ks_label(script_stem, lbl)));
                report.record_exact();
            }
            KagItem::Text(t) => {
                if let Some((_, _, ref mut lbuf)) = active_link {
                    lbuf.push_str(t);
                } else if let Some(reading) = pending_ruby.take() {
                    // KAG `[ruby text="reading"]` applies to the next character!
                    let mut chars = t.chars();
                    if let Some(first) = chars.next() {
                        text_buf.push_str(&format!("{{ruby={first}|{reading}}}"));
                        text_buf.push_str(chars.as_str());
                    }
                } else {
                    text_buf.push_str(t);
                }
            }
            KagItem::Tag { name, attrs, line } => {
                let src_loc = format!("{script_stem}.ks:{line}");
                match name.as_str() {
                    "r" => {
                        if active_link.is_none() && !text_buf.is_empty() {
                            text_buf.push('\n');
                        }
                        report.record_exact();
                    }
                    "l" | "p" | "cm" | "ct" | "er" => {
                        flush_kag_text(&mut text_buf, out);
                        report.record_exact();
                    }
                    "ch" => {
                        if let Some(t) = attrs.get("text") {
                            text_buf.push_str(t);
                        }
                        report.record_exact();
                    }
                    "ruby" => {
                        if let Some(reading) = attrs.get("text") {
                            pending_ruby = Some(reading.clone());
                        }
                        report.record_exact();
                    }
                    "emb" => {
                        if let Some(exp) = attrs.get("exp") {
                            let var = tjs_var_ident(exp);
                            ensure_var(project, declared_vars, &var);
                            text_buf.push_str(&format!("{{{var}}}"));
                            report.record_exact();
                        }
                    }
                    "link" => {
                        flush_kag_text(&mut text_buf, out);
                        let target = resolve_kag_target(
                            script_stem,
                            attrs.get("storage").map(String::as_str),
                            attrs.get("target").map(String::as_str),
                        );
                        let cond = attrs
                            .get("exp")
                            .map(|e| translate_tjs_expr(e, project, declared_vars));
                        active_link = Some((target, cond, String::new()));
                        report.record_exact();
                    }
                    "endlink" => {
                        if let Some((target, cond, label)) = active_link.take() {
                            let text = label.trim().to_owned();
                            pending_choices.push(VnChoice {
                                text: if text.is_empty() {
                                    target.clone()
                                } else {
                                    text
                                },
                                target_label: target,
                                condition: cond,
                            });
                        }
                        report.record_exact();
                    }
                    "s" => {
                        flush_kag_text(&mut text_buf, out);
                        if !pending_choices.is_empty() {
                            out.push(VnStmt::Choice(std::mem::take(&mut pending_choices)));
                        } else {
                            out.push(VnStmt::End {
                                outcome: "end".into(),
                            });
                        }
                        report.record_exact();
                    }
                    "jump" => {
                        flush_kag_text(&mut text_buf, out);
                        let target = resolve_kag_target(
                            script_stem,
                            attrs.get("storage").map(String::as_str),
                            attrs.get("target").map(String::as_str),
                        );
                        if let Some(exp) = attrs.get("cond") {
                            let cond = translate_tjs_expr(exp, project, declared_vars);
                            out.push(VnStmt::If {
                                cond,
                                then_body: vec![VnStmt::Jump(target)],
                                else_body: Vec::new(),
                            });
                        } else {
                            out.push(VnStmt::Jump(target));
                        }
                        report.record_exact();
                    }
                    "call" => {
                        flush_kag_text(&mut text_buf, out);
                        let target = resolve_kag_target(
                            script_stem,
                            attrs.get("storage").map(String::as_str),
                            attrs.get("target").map(String::as_str),
                        );
                        out.push(VnStmt::Call(target));
                        report.record_exact();
                    }
                    "return" => {
                        flush_kag_text(&mut text_buf, out);
                        out.push(VnStmt::Return);
                        report.record_exact();
                    }
                    "eval" => {
                        flush_kag_text(&mut text_buf, out);
                        if let Some(exp) = attrs.get("exp") {
                            if let Some((var, rhs)) =
                                parse_tjs_assignment(exp, project, declared_vars)
                            {
                                ensure_var(project, declared_vars, &var);
                                out.push(VnStmt::SetVar {
                                    name: var,
                                    expr: rhs,
                                });
                                report.record_exact();
                            } else {
                                report.record_adapted(
                                    &src_loc,
                                    format!("complex TJS [eval exp=\"{exp}\"] skipped"),
                                );
                            }
                        }
                    }
                    "if" => {
                        flush_kag_text(&mut text_buf, out);
                        let cond = attrs
                            .get("exp")
                            .map(|e| translate_tjs_expr(e, project, declared_vars))
                            .unwrap_or_else(|| "true".into());
                        let mut then_body = Vec::new();
                        lower_slice(
                            script_stem,
                            items,
                            pos,
                            true,
                            project,
                            declared_vars,
                            report,
                            resolver,
                            &mut then_body,
                        );
                        let mut else_body = Vec::new();
                        while *pos < items.len() {
                            if let KagItem::Tag {
                                name: bname,
                                attrs: battrs,
                                ..
                            } = &items[*pos]
                            {
                                match bname.as_str() {
                                    "else" => {
                                        *pos += 1;
                                        lower_slice(
                                            script_stem,
                                            items,
                                            pos,
                                            true,
                                            project,
                                            declared_vars,
                                            report,
                                            resolver,
                                            &mut else_body,
                                        );
                                    }
                                    "elsif" => {
                                        let econd = battrs
                                            .get("exp")
                                            .map(|e| translate_tjs_expr(e, project, declared_vars))
                                            .unwrap_or_else(|| "true".into());
                                        *pos += 1;
                                        let mut ethen = Vec::new();
                                        lower_slice(
                                            script_stem,
                                            items,
                                            pos,
                                            true,
                                            project,
                                            declared_vars,
                                            report,
                                            resolver,
                                            &mut ethen,
                                        );
                                        else_body.push(VnStmt::If {
                                            cond: econd,
                                            then_body: ethen,
                                            else_body: Vec::new(),
                                        });
                                    }
                                    "endif" => {
                                        *pos += 1;
                                        break;
                                    }
                                    _ => break,
                                }
                            } else {
                                break;
                            }
                        }
                        out.push(VnStmt::If {
                            cond,
                            then_body,
                            else_body,
                        });
                        report.record_exact();
                    }
                    "image" | "bg" => {
                        flush_kag_text(&mut text_buf, out);
                        if let Some(storage) = attrs.get("storage") {
                            if let Some(asset) = resolver.resolve_image(&src_loc, storage) {
                                let layer = attrs
                                    .get("layer")
                                    .map(String::as_str)
                                    .unwrap_or(if name == "bg" { "base" } else { "0" });
                                if layer == "base" || name == "bg" {
                                    let dur = attrs.get("time").and_then(|v| v.parse::<u32>().ok());
                                    out.push(VnStmt::Bg {
                                        asset,
                                        transition: dur.map(|_| "dissolve".into()),
                                        duration_ms: dur,
                                    });
                                } else {
                                    let slot = format!("layer_{}", sanitize_ident(layer));
                                    let x = attrs.get("left").and_then(|v| v.parse::<f32>().ok());
                                    let y = attrs.get("top").and_then(|v| v.parse::<f32>().ok());
                                    let z = attrs.get("index").and_then(|v| v.parse::<i32>().ok());
                                    let opacity = attrs
                                        .get("opacity")
                                        .and_then(|v| v.parse::<f32>().ok())
                                        .map(|o| (o / 255.0).clamp(0.0, 1.0));
                                    out.push(VnStmt::Show {
                                        slot,
                                        asset,
                                        x,
                                        y,
                                        z,
                                        opacity,
                                        scale: None,
                                    });
                                }
                                report.record_exact();
                            }
                        }
                    }
                    "freeimage" => {
                        flush_kag_text(&mut text_buf, out);
                        let layer = attrs.get("layer").map(String::as_str).unwrap_or("0");
                        if layer == "base" {
                            out.push(VnStmt::Clear);
                        } else {
                            out.push(VnStmt::Hide {
                                slot: format!("layer_{}", sanitize_ident(layer)),
                            });
                        }
                        report.record_exact();
                    }
                    "backlay" | "wt" | "wm" | "ws" | "wb" | "wq" => {
                        // Staged layer sync / wait-for-completion markers handled natively
                        report.record_exact();
                    }
                    "trans" => {
                        flush_kag_text(&mut text_buf, out);
                        let duration_ms = attrs
                            .get("time")
                            .and_then(|v| v.parse::<u32>().ok())
                            .or(Some(400));
                        let method = attrs
                            .get("method")
                            .map(|m| match m.as_str() {
                                "scroll" => "wipe".to_owned(),
                                _ => "dissolve".to_owned(),
                            })
                            .or_else(|| Some("dissolve".into()));
                        out.push(VnStmt::Commit {
                            transition: method,
                            duration_ms,
                        });
                        report.record_exact();
                    }
                    "quake" => {
                        flush_kag_text(&mut text_buf, out);
                        let ms = attrs
                            .get("time")
                            .and_then(|v| v.parse::<u32>().ok())
                            .unwrap_or(500);
                        let h = attrs
                            .get("hmax")
                            .and_then(|v| v.parse::<f32>().ok())
                            .unwrap_or(10.0);
                        let v = attrs
                            .get("vmax")
                            .and_then(|v| v.parse::<f32>().ok())
                            .unwrap_or(10.0);
                        out.push(VnStmt::Shake {
                            target: "stage".into(),
                            amp: h.max(v),
                            duration_ms: ms,
                        });
                        report.record_exact();
                    }
                    "move" => {
                        flush_kag_text(&mut text_buf, out);
                        let layer = attrs.get("layer").map(String::as_str).unwrap_or("0");
                        let ms = attrs
                            .get("time")
                            .and_then(|v| v.parse::<u32>().ok())
                            .unwrap_or(400);
                        if let Some(path) = attrs.get("path") {
                            let nums: Vec<f32> = path
                                .trim_matches(|c| c == '(' || c == ')')
                                .split(',')
                                .filter_map(|p| p.trim().parse::<f32>().ok())
                                .collect();
                            let slot = format!("layer_{}", sanitize_ident(layer));
                            if let Some(&x) = nums.first() {
                                out.push(VnStmt::Tween {
                                    slot: slot.clone(),
                                    prop: "x".into(),
                                    to: x,
                                    duration_ms: ms,
                                    ease: Some("ease_out".into()),
                                });
                            }
                            if let Some(&y) = nums.get(1) {
                                out.push(VnStmt::Tween {
                                    slot: slot.clone(),
                                    prop: "y".into(),
                                    to: y,
                                    duration_ms: ms,
                                    ease: Some("ease_out".into()),
                                });
                            }
                            if let Some(&o) = nums.get(2) {
                                out.push(VnStmt::Tween {
                                    slot,
                                    prop: "opacity".into(),
                                    to: (o / 255.0).clamp(0.0, 1.0),
                                    duration_ms: ms,
                                    ease: Some("ease_out".into()),
                                });
                            }
                            report.record_exact();
                        } else {
                            report.record_adapted(&src_loc, "[move] without literal path");
                        }
                    }
                    "wait" => {
                        flush_kag_text(&mut text_buf, out);
                        let ms = attrs
                            .get("time")
                            .and_then(|v| v.parse::<u32>().ok())
                            .unwrap_or(500);
                        out.push(VnStmt::Wait { ms });
                        report.record_exact();
                    }
                    "layopt" => {
                        if let Some(layer) = attrs.get("layer") {
                            if layer.starts_with("message") {
                                if let Some(vis) = attrs.get("visible") {
                                    out.push(VnStmt::Window {
                                        visible: vis != "false" && vis != "0",
                                    });
                                }
                            }
                        }
                        report.record_exact();
                    }
                    "playbgm" | "fadeinbgm" | "xchgbgm" => {
                        flush_kag_text(&mut text_buf, out);
                        if let Some(storage) = attrs.get("storage") {
                            if let Some(asset) = resolver.resolve_audio(&src_loc, storage) {
                                let fade_ms = attrs.get("time").and_then(|v| v.parse::<u32>().ok());
                                let volume = attrs
                                    .get("volume")
                                    .and_then(|v| v.parse::<f32>().ok())
                                    .map(|v| (v / 100.0).clamp(0.0, 1.0));
                                out.push(VnStmt::Bgm {
                                    asset,
                                    fade_ms,
                                    volume,
                                });
                                report.record_exact();
                            }
                        }
                    }
                    "stopbgm" | "fadeoutbgm" => {
                        flush_kag_text(&mut text_buf, out);
                        let fade_ms = attrs.get("time").and_then(|v| v.parse::<u32>().ok());
                        out.push(VnStmt::StopBgm { fade_ms });
                        report.record_exact();
                    }
                    "playse" | "fadeinse" => {
                        flush_kag_text(&mut text_buf, out);
                        if let Some(storage) = attrs.get("storage") {
                            if let Some(asset) = resolver.resolve_audio(&src_loc, storage) {
                                let volume = attrs
                                    .get("volume")
                                    .and_then(|v| v.parse::<f32>().ok())
                                    .map(|v| (v / 100.0).clamp(0.0, 1.0));
                                out.push(VnStmt::Sfx { asset, volume });
                                report.record_exact();
                            }
                        }
                    }
                    "playvideo" | "video" => {
                        flush_kag_text(&mut text_buf, out);
                        if let Some(storage) = attrs.get("storage") {
                            if let Some(asset) = resolver.resolve_video(&src_loc, storage) {
                                out.push(VnStmt::Video { asset });
                                report.record_exact();
                            }
                        }
                    }
                    "current" | "position" | "deffont" | "font" | "resetfont" | "style"
                    | "resetstyle" | "history" | "rclick" | "clickskip" | "title" | "delay"
                    | "stopse" | "fadeoutse" | "bgmopt" | "seopt" => {
                        if name == "title" {
                            if let Some(t) = attrs.get("name") {
                                if !t.trim().is_empty() {
                                    project.title = t.clone();
                                }
                            }
                        }
                        report.record_adapted(&src_loc, format!("KAG tag `[{name}]` normalized"));
                    }
                    other => {
                        report.record_fallback(
                            &src_loc,
                            format!("unrecognized KAG tag `[{other}]` omitted"),
                        );
                        out.push(VnStmt::Comment(format!("[fallback] KAG [{other}]")));
                    }
                }
            }
        }
    }

    flush_kag_text(&mut text_buf, out);
    if !pending_choices.is_empty() {
        out.push(VnStmt::Choice(pending_choices));
    }
}

fn flush_kag_text(buf: &mut String, out: &mut Vec<VnStmt>) {
    let trimmed = buf.trim().to_owned();
    buf.clear();
    if trimmed.is_empty() {
        return;
    }
    if let Some(rest) = trimmed.strip_prefix('【') {
        if let Some((sp, body)) = rest.split_once('】') {
            if !sp.is_empty() && !body.trim().is_empty() {
                out.push(VnStmt::Say {
                    speaker: Some(sp.trim().to_owned()),
                    text: body.trim().to_owned(),
                    voice: None,
                });
                return;
            }
        }
    }
    if let Some((sp, rest)) = trimmed.split_once('「') {
        let sp_t = sp.trim();
        if !sp_t.is_empty() && sp_t.chars().count() <= 16 && !sp_t.contains('。') {
            out.push(VnStmt::Say {
                speaker: Some(sp_t.to_owned()),
                text: format!("「{rest}"),
                voice: None,
            });
            return;
        }
    }
    out.push(VnStmt::Say {
        speaker: None,
        text: trimmed,
        voice: None,
    });
}

pub fn ks_stem(path: &str) -> String {
    let base = path.replace('\\', "/");
    let file = base.rsplit('/').next().unwrap_or(&base);
    file.strip_suffix(".ks")
        .or_else(|| file.strip_suffix(".KS"))
        .unwrap_or(file)
        .to_ascii_lowercase()
}

pub fn ks_entry_label(stem: &str) -> String {
    format!("ks_{}", sanitize_ident(stem))
}

pub fn ks_label(stem: &str, label: &str) -> String {
    let clean = label.trim().trim_start_matches('*');
    format!("ks_{}_{}", sanitize_ident(stem), sanitize_ident(clean))
}

fn resolve_kag_target(current_stem: &str, storage: Option<&str>, target: Option<&str>) -> String {
    let stem = storage
        .filter(|s| !s.trim().is_empty())
        .map(ks_stem)
        .unwrap_or_else(|| current_stem.to_owned());
    match target
        .map(|t| t.trim().trim_start_matches('*'))
        .filter(|t| !t.is_empty())
    {
        Some(lbl) => ks_label(&stem, lbl),
        None => ks_entry_label(&stem),
    }
}

fn tjs_var_ident(raw: &str) -> String {
    let clean = raw
        .trim()
        .strip_prefix("f.")
        .or_else(|| raw.trim().strip_prefix("sf."))
        .or_else(|| raw.trim().strip_prefix("tf."))
        .unwrap_or(raw.trim());
    sanitize_ident(clean)
}

fn ensure_var(project: &mut VnProject, declared: &mut BTreeSet<String>, ident: &str) {
    if declared.insert(ident.to_owned()) {
        project
            .vars
            .entry(ident.to_owned())
            .or_insert(VnConst::Int(0));
    }
}

fn parse_tjs_assignment(
    exp: &str,
    project: &mut VnProject,
    declared: &mut BTreeSet<String>,
) -> Option<(String, String)> {
    let s = exp.trim().trim_end_matches(';');
    for (op_sym, bin) in [("+=", "+"), ("-=", "-"), ("*=", "*"), ("/=", "/")] {
        if let Some((lhs, rhs)) = s.split_once(op_sym) {
            let var = tjs_var_ident(lhs);
            let rhs_t = translate_tjs_expr(rhs, project, declared);
            return Some((var.clone(), format!("({var} {bin} {rhs_t})")));
        }
    }
    if let Some(lhs) = s.strip_suffix("++") {
        let var = tjs_var_ident(lhs);
        return Some((var.clone(), format!("({var} + 1)")));
    }
    if let Some(lhs) = s.strip_suffix("--") {
        let var = tjs_var_ident(lhs);
        return Some((var.clone(), format!("({var} - 1)")));
    }
    // Plain `lhs = rhs` (excluding `==`, `!=`, `<=`, `>=`)
    let bytes = s.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'='
            && (i == 0 || !matches!(bytes[i - 1], b'=' | b'!' | b'<' | b'>'))
            && (i + 1 >= bytes.len() || bytes[i + 1] != b'=')
        {
            let var = tjs_var_ident(&s[..i]);
            let rhs = translate_tjs_expr(&s[i + 1..], project, declared);
            return Some((var, rhs));
        }
    }
    None
}

fn translate_tjs_expr(
    raw: &str,
    project: &mut VnProject,
    declared: &mut BTreeSet<String>,
) -> String {
    let mut s = raw
        .trim()
        .replace("&&", " and ")
        .replace("||", " or ")
        .replace("===", "==")
        .replace("!==", "!=");
    // Replace `f.foo`, `sf.bar`, `tf.baz` identifiers
    for prefix in ["sf.", "tf.", "f."] {
        while let Some(pos) = s.find(prefix) {
            let after = pos + prefix.len();
            let end = s[after..]
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .map(|o| after + o)
                .unwrap_or(s.len());
            let vname = &s[after..end];
            if vname.is_empty() {
                break;
            }
            let ident = sanitize_ident(vname);
            ensure_var(project, declared, &ident);
            s.replace_range(pos..end, &ident);
        }
    }
    // Single-quoted TJS strings -> double-quoted `.nirs` strings
    if s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2 {
        s = format!("\"{}\"", s[1..s.len() - 1].replace('"', "\\\""));
    }
    s
}
