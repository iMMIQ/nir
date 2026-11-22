//! Emits a `VnProject` as a zero-boilerplate, human-editable `.nirs` project directory.

use crate::ir::{VnConst, VnProject, VnStmt};
use anyhow::{Context, Result};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

/// Converts an arbitrary source label/variable/slot identifier into a valid `.nirs` ASCII identifier.
/// Non-ASCII or punctuation characters are hashed deterministically into a short suffix so
/// distinct Japanese/Unicode identifiers never collide.
pub fn sanitize_ident(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "_empty".into();
    }
    let mut out = String::with_capacity(trimmed.len() + 8);
    let mut lossy = false;
    for (idx, ch) in trimmed.chars().enumerate() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            if idx == 0 && ch.is_ascii_digit() {
                out.push('_');
            }
            out.push(ch);
        } else {
            lossy = true;
            if !out.ends_with('_') && !out.is_empty() {
                out.push('_');
            }
        }
    }
    if out.is_empty() || is_nirs_keyword(&out) {
        out.insert_str(0, "id_");
        lossy = true;
    }
    if lossy {
        let mut h: u32 = 0x811c_9dc5;
        for b in trimmed.as_bytes() {
            h ^= *b as u32;
            h = h.wrapping_mul(0x0100_0193);
        }
        let _ = write!(out, "_{h:06x}");
    }
    out
}

fn is_nirs_keyword(s: &str) -> bool {
    matches!(
        s,
        "bg" | "show"
            | "hide"
            | "clear"
            | "commit"
            | "tween"
            | "shake"
            | "bgm"
            | "sfx"
            | "voice"
            | "video"
            | "window"
            | "wait"
            | "if"
            | "elif"
            | "else"
            | "endif"
            | "menu"
            | "label"
            | "jump"
            | "call"
            | "return"
            | "end"
            | "var"
            | "character"
            | "unlock"
            | "true"
            | "false"
            | "null"
            | "and"
            | "or"
            | "not"
            | "stop"
    )
}

fn escape_quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            '\t' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// Writes `game.toml`, `scripts/*.nirs`, and `assets/**/*` into `out_dir`.
pub fn emit_nirs_project(project: &VnProject, out_dir: &Path) -> Result<()> {
    fs::create_dir_all(out_dir)
        .with_context(|| format!("creating output directory `{}`", out_dir.display()))?;
    let scripts_dir = out_dir.join("scripts");
    let assets_dir = out_dir.join("assets");
    fs::create_dir_all(&scripts_dir)?;
    fs::create_dir_all(&assets_dir)?;

    // 1. Emit minimal `game.toml`
    let toml = format!(
        "[game]\nid = \"{}\"\ntitle = \"{}\"\nversion = \"{}\"\nlocale = \"{}\"\nstage = [{}, {}]\n",
        escape_quoted(&project.id),
        escape_quoted(&project.title),
        escape_quoted(&project.version),
        escape_quoted(&project.source_locale),
        project.stage_width.max(320),
        project.stage_height.max(240),
    );
    fs::write(out_dir.join("game.toml"), toml)?;

    // 2. Emit assets
    for (rel, bytes) in &project.assets {
        let dest = assets_dir.join(rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&dest, bytes)
            .with_context(|| format!("writing converted asset `{}`", dest.display()))?;
    }

    // 3. Emit `.nirs` scripts (place character & var declarations at top of entry script)
    let entry_name = if project.scripts.contains_key(&project.entry_script) {
        project.entry_script.clone()
    } else {
        project
            .scripts
            .keys()
            .next()
            .cloned()
            .unwrap_or_else(|| "main".into())
    };

    for (stem, stmts) in &project.scripts {
        let mut buf = String::new();
        let _ = writeln!(buf, "# Auto-converted to NIR v2 (.nirs) from `{stem}`");
        if stem == &entry_name {
            for (id, ch) in &project.characters {
                let ident = sanitize_ident(id);
                if let Some(color) = &ch.color {
                    let _ = writeln!(
                        buf,
                        "character {ident} \"{}\" color={color}",
                        escape_quoted(&ch.display_name)
                    );
                } else {
                    let _ = writeln!(
                        buf,
                        "character {ident} \"{}\"",
                        escape_quoted(&ch.display_name)
                    );
                }
            }
            for (name, val) in &project.vars {
                let ident = sanitize_ident(name);
                match val {
                    VnConst::Bool(b) => {
                        let _ = writeln!(buf, "var {ident} = {b}");
                    }
                    VnConst::Int(i) => {
                        let _ = writeln!(buf, "var {ident} = {i}");
                    }
                    VnConst::Float(f) => {
                        let _ = writeln!(buf, "var {ident} = {f}");
                    }
                    VnConst::Str(s) => {
                        let _ = writeln!(buf, "var {ident} = \"{}\"", escape_quoted(s));
                    }
                }
            }
            if !project.characters.is_empty() || !project.vars.is_empty() {
                buf.push('\n');
            }
        }
        emit_stmts(stmts, 0, &mut buf);
        let file_name = format!("{}.nirs", sanitize_ident(stem));
        fs::write(scripts_dir.join(file_name), buf)?;
    }

    Ok(())
}

fn emit_stmts(stmts: &[VnStmt], indent: usize, out: &mut String) {
    let pad = "  ".repeat(indent);
    for stmt in stmts {
        match stmt {
            VnStmt::Comment(c) => {
                for line in c.lines() {
                    let _ = writeln!(out, "{pad}# {line}");
                }
            }
            VnStmt::Label(name) => {
                let _ = writeln!(out, "\n{pad}label {}:", sanitize_ident(name));
            }
            VnStmt::Jump(target) => {
                let _ = writeln!(out, "{pad}jump {}", sanitize_ident(target));
            }
            VnStmt::Call(target) => {
                let _ = writeln!(out, "{pad}call {}", sanitize_ident(target));
            }
            VnStmt::Return => {
                let _ = writeln!(out, "{pad}return");
            }
            VnStmt::SetVar { name, expr } => {
                let _ = writeln!(out, "{pad}$ {} = {}", sanitize_ident(name), expr);
            }
            VnStmt::If {
                cond,
                then_body,
                else_body,
            } => {
                let _ = writeln!(out, "{pad}if {cond}:");
                emit_stmts(then_body, indent + 1, out);
                if !else_body.is_empty() {
                    let _ = writeln!(out, "{pad}else:");
                    emit_stmts(else_body, indent + 1, out);
                }
                let _ = writeln!(out, "{pad}endif");
            }
            VnStmt::Bg {
                asset,
                transition,
                duration_ms,
            } => {
                let _ = write!(out, "{pad}bg \"{asset}\"");
                if let Some(tr) = transition {
                    let ms = duration_ms.unwrap_or(300);
                    let _ = write!(out, " with {tr} {ms}");
                }
                out.push('\n');
            }
            VnStmt::Show {
                slot,
                asset,
                x,
                y,
                z,
                opacity,
                scale,
            } => {
                let _ = write!(out, "{pad}show {} \"{asset}\"", sanitize_ident(slot));
                if let Some(v) = x {
                    let _ = write!(out, " x={v:.1}");
                }
                if let Some(v) = y {
                    let _ = write!(out, " y={v:.1}");
                }
                if let Some(v) = z {
                    let _ = write!(out, " z={v}");
                }
                if let Some(v) = opacity {
                    let _ = write!(out, " opacity={v:.3}");
                }
                if let Some(v) = scale {
                    let _ = write!(out, " scale={v:.3}");
                }
                out.push('\n');
            }
            VnStmt::Hide { slot } => {
                let _ = writeln!(out, "{pad}hide {}", sanitize_ident(slot));
            }
            VnStmt::Clear => {
                let _ = writeln!(out, "{pad}clear");
            }
            VnStmt::Commit {
                transition,
                duration_ms,
            } => {
                let _ = write!(out, "{pad}commit");
                if let Some(tr) = transition {
                    let ms = duration_ms.unwrap_or(300);
                    let _ = write!(out, " {tr} {ms}");
                }
                out.push('\n');
            }
            VnStmt::Tween {
                slot,
                prop,
                to,
                duration_ms,
                ease,
            } => {
                let _ = write!(
                    out,
                    "{pad}tween {} {prop} {to:.2} {duration_ms}",
                    sanitize_ident(slot)
                );
                if let Some(e) = ease {
                    let _ = write!(out, " {e}");
                }
                out.push('\n');
            }
            VnStmt::Shake {
                target,
                amp,
                duration_ms,
            } => {
                let _ = writeln!(
                    out,
                    "{pad}shake {} amp={amp:.1} ms={duration_ms}",
                    sanitize_ident(target)
                );
            }
            VnStmt::Say {
                speaker,
                text,
                voice,
            } => {
                if let Some(v) = voice {
                    let _ = writeln!(out, "{pad}voice \"{v}\"");
                }
                let escaped = escape_quoted(text);
                if let Some(sp) = speaker.as_deref().filter(|s| !s.trim().is_empty()) {
                    let _ = writeln!(out, "{pad}\"{}\": \"{escaped}\"", escape_quoted(sp));
                } else {
                    let _ = writeln!(out, "{pad}\"{escaped}\"");
                }
            }
            VnStmt::Choice(choices) => {
                if choices.is_empty() {
                    continue;
                }
                let _ = writeln!(out, "{pad}menu:");
                for ch in choices {
                    if let Some(cond) = &ch.condition {
                        let _ = writeln!(
                            out,
                            "{pad}  - \"{}\" if {cond} -> {}",
                            escape_quoted(&ch.text),
                            sanitize_ident(&ch.target_label)
                        );
                    } else {
                        let _ = writeln!(
                            out,
                            "{pad}  - \"{}\" -> {}",
                            escape_quoted(&ch.text),
                            sanitize_ident(&ch.target_label)
                        );
                    }
                }
            }
            VnStmt::Bgm {
                asset,
                fade_ms,
                volume,
            } => {
                let _ = write!(out, "{pad}bgm \"{asset}\"");
                if let Some(ms) = fade_ms {
                    let _ = write!(out, " fade={ms}");
                }
                if let Some(v) = volume {
                    let _ = write!(out, " vol={v:.2}");
                }
                out.push('\n');
            }
            VnStmt::StopBgm { fade_ms } => {
                if let Some(ms) = fade_ms {
                    let _ = writeln!(out, "{pad}stop_bgm {ms}");
                } else {
                    let _ = writeln!(out, "{pad}stop_bgm");
                }
            }
            VnStmt::Sfx { asset, volume } => {
                let _ = write!(out, "{pad}sfx \"{asset}\"");
                if let Some(v) = volume {
                    let _ = write!(out, " vol={v:.2}");
                }
                out.push('\n');
            }
            VnStmt::Voice { asset } => {
                let _ = writeln!(out, "{pad}voice \"{asset}\"");
            }
            VnStmt::Wait { ms } => {
                let _ = writeln!(out, "{pad}wait {ms}");
            }
            VnStmt::Window { visible } => {
                let state = if *visible { "show" } else { "hide" };
                let _ = writeln!(out, "{pad}window {state}");
            }
            VnStmt::Video { asset } => {
                let _ = writeln!(out, "{pad}video \"{asset}\"");
            }
            VnStmt::Unlock { kind, id } => {
                let _ = writeln!(
                    out,
                    "{pad}unlock \"{}:{}\"",
                    escape_quoted(kind),
                    escape_quoted(id)
                );
            }
            VnStmt::End { outcome } => {
                let clean: String = outcome
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                    .collect();
                let name = if clean.is_empty() { "end" } else { &clean };
                let _ = writeln!(out, "{pad}end_game {name}");
            }
        }
    }
}
