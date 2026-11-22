#![forbid(unsafe_code)]

//! LiveMaker / LiveNovel engine importer (`nir-import-livemaker`).
//!
//! Uses battle-tested binary parsers (`lsb.rs`, `media.rs`, `cinema.rs`) and lowers
//! directly onto NIR v2's Persistent Scene Graph via `nir-import-core::VnProject`.
//! Zero LiveMaker hacks leak into the NIR VM or runtime.

mod cinema;
mod events;
mod expr;
mod layout;
mod livenovel;
mod lpm;
mod lsb;
mod media;

use anyhow::{Context, Result};
use nir_import_core::{
    sanitize_ident, ArchiveSource, CompositeVfs, ConversionReport, DirArchive, DirectImport,
    EngineImporter, EngineProbe, ImportOptions, VnChoice, VnConst, VnProject, VnStmt,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub struct LiveMakerImporter;

impl EngineImporter for LiveMakerImporter {
    fn id(&self) -> &'static str {
        "livemaker"
    }

    fn display_name(&self) -> &'static str {
        "LiveMaker / LiveNovel (LSB 116/117)"
    }

    fn detect(&self, source: &Path) -> Option<EngineProbe> {
        if !source.is_dir() {
            return None;
        }
        let dir = DirArchive::open(source).ok()?;
        let files = dir.list_files();
        let has_lpb = files.iter().any(|f| f.ends_with(".lpb"));
        let lsb_count = files.iter().filter(|f| f.ends_with(".lsb")).count();
        let gal_count = files.iter().filter(|f| f.ends_with(".gal")).count();
        if has_lpb || lsb_count > 0 {
            let confidence = if has_lpb && lsb_count > 0 {
                98
            } else if lsb_count > 0 {
                90
            } else {
                80
            };
            Some(EngineProbe {
                engine_id: self.id().into(),
                display_name: self.display_name().into(),
                confidence,
                summary: format!("{lsb_count} .lsb script(s), {gal_count} .gal image(s)"),
            })
        } else {
            None
        }
    }

    fn import(&self, source: &Path, opts: &ImportOptions) -> Result<(VnProject, ConversionReport)> {
        let mut vfs = CompositeVfs::new();
        vfs.mount(DirArchive::open(source)?);

        let mut report = ConversionReport::new(self.id());
        let mut project = VnProject {
            id: format!(
                "org.nir.lm.{}",
                sanitize_ident(
                    source
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("game")
                )
            ),
            title: source
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("LiveMaker Game")
                .to_owned(),
            source_locale: opts.source_locale.clone().unwrap_or_else(|| "ja".into()),
            stage_width: 1280,
            stage_height: 720,
            ..VnProject::default()
        };

        let files = vfs.list_files();

        // 1. Inspect `.lpb` project settings (startup script, stage dimensions)
        let mut startup_lsb: Option<String> = None;
        if let Some(lpb_path) = files.iter().find(|f| f.ends_with(".lpb")) {
            if let Ok(data) = vfs.read(lpb_path) {
                if let Ok(start) = lsb::startup(&data) {
                    if !start.trim().is_empty() {
                        startup_lsb = Some(start.replace('\\', "/").to_ascii_lowercase());
                    }
                }
                if let Ok(settings) = lsb::project_settings(&data) {
                    if let Some(lsb::Literal::Int(w)) = settings.get("画面幅").or_else(|| settings.get("ScreenWidth")) {
                        if *w >= 320 {
                            project.stage_width = *w as u32;
                        }
                    }
                    if let Some(lsb::Literal::Int(h)) = settings.get("画面高さ").or_else(|| settings.get("ScreenHeight")) {
                        if *h >= 240 {
                            project.stage_height = *h as u32;
                        }
                    }
                    if let Some(lsb::Literal::String(t)) = settings.get("タイトル").or_else(|| settings.get("Title")) {
                        if !t.trim().is_empty() {
                            project.title = t.clone();
                        }
                    }
                    report.record_exact();
                }
            }
        }

        // 2. Discover all `.lsb` scripts
        let mut lsb_files: Vec<String> = files
            .iter()
            .filter(|f| f.ends_with(".lsb"))
            .cloned()
            .collect();
        if let Some(ref start) = startup_lsb {
            if let Some(pos) = lsb_files.iter().position(|f| f == start || f.ends_with(start)) {
                let first = lsb_files.remove(pos);
                lsb_files.insert(0, first);
            }
        }
        lsb_files.truncate(opts.max_scripts);

        let mut ctx = LowerCtx {
            vfs: &vfs,
            project: &mut project,
            report: &mut report,
            converted_assets: BTreeMap::new(),
            declared_vars: BTreeSet::new(),
        };

        for (idx, lsb_path) in lsb_files.iter().enumerate() {
            let data = ctx
                .vfs
                .read(lsb_path)
                .with_context(|| format!("reading `{lsb_path}`"))?;
            let script = match lsb::parse(&data) {
                Ok(s) => s,
                Err(e) => {
                    ctx.report
                        .record_fallback(lsb_path, format!("failed to parse LSB: {e}"));
                    continue;
                }
            };
            let page_stem = script_stem(lsb_path);
            let script_key = if idx == 0 {
                "main".to_owned()
            } else {
                page_stem.clone()
            };
            let mut stmts = Vec::new();
            stmts.push(VnStmt::Label(page_entry_label(&page_stem)));
            ctx.lower_script(&page_stem, &script, &mut stmts);
            ctx.report.scripts_converted += 1;
            ctx.report.statements_emitted += stmts.len();
            ctx.project.scripts.insert(script_key, stmts);
        }

        // Ensure entry script exists even on empty fixtures
        if project.scripts.is_empty() {
            project
                .scripts
                .insert("main".into(), vec![VnStmt::End { outcome: "end".into() }]);
        }
        report.assets_converted = project.assets.len();

        Ok((project, report))
    }

    fn import_direct(
        &self,
        source: &Path,
        opts: &ImportOptions,
    ) -> Option<Result<(DirectImport, ConversionReport)>> {
        Some(livenovel::import_direct(source, opts))
    }
}

struct LowerCtx<'a> {
    vfs: &'a CompositeVfs,
    project: &'a mut VnProject,
    report: &'a mut ConversionReport,
    /// Maps source normalized path -> target relative path under `assets/`.
    converted_assets: BTreeMap<String, String>,
    declared_vars: BTreeSet<String>,
}

impl<'a> LowerCtx<'a> {
    fn lower_script(&mut self, page_stem: &str, script: &lsb::Script, out: &mut Vec<VnStmt>) {
        // Pre-scan labels & jump line targets so cross-page line jumps have labels.
        let mut line_labels: BTreeSet<u32> = BTreeSet::new();
        for cmd in &script.commands {
            match &cmd.body {
                lsb::Body::Jump(r, _) | lsb::Body::Call { target: r, .. }
                    if r.page.is_empty() || script_stem(&r.page) == page_stem =>
                {
                    line_labels.insert(r.line);
                }
                _ => {}
            }
        }

        let mut pending_choices: Vec<VnChoice> = Vec::new();
        let mut i = 0usize;
        while i < script.commands.len() {
            let cmd = &script.commands[i];
            if cmd.muted {
                i += 1;
                continue;
            }
            if line_labels.contains(&cmd.line) {
                out.push(VnStmt::Label(line_label(page_stem, cmd.line)));
            }

            let src_loc = format!("{page_stem}.lsb:{}", cmd.line);

            match &cmd.body {
                lsb::Body::Label(name) => {
                    out.push(VnStmt::Label(named_label(page_stem, name)));
                    self.report.record_exact();
                }
                lsb::Body::Jump(target, cond) => {
                    let lbl = resolve_ref_label(page_stem, target);
                    if expr_always_true(cond) {
                        out.push(VnStmt::Jump(lbl));
                        self.report.record_exact();
                    } else if let Some(c) = lower_expr(cond) {
                        out.push(VnStmt::If {
                            cond: c,
                            then_body: vec![VnStmt::Jump(lbl)],
                            else_body: Vec::new(),
                        });
                        self.report.record_exact();
                    } else {
                        out.push(VnStmt::Jump(lbl));
                        self.report.record_adapted(
                            &src_loc,
                            "dynamic Jump condition simplified to unconditional jump",
                        );
                    }
                }
                lsb::Body::Call {
                    target, condition, ..
                } => {
                    let lbl = resolve_ref_label(page_stem, target);
                    if expr_always_true(condition) {
                        out.push(VnStmt::Call(lbl));
                        self.report.record_exact();
                    } else if let Some(c) = lower_expr(condition) {
                        out.push(VnStmt::If {
                            cond: c,
                            then_body: vec![VnStmt::Call(lbl)],
                            else_body: Vec::new(),
                        });
                        self.report.record_exact();
                    } else {
                        out.push(VnStmt::Call(lbl));
                        self.report
                            .record_adapted(&src_loc, "dynamic Call condition simplified");
                    }
                }
                lsb::Body::Exit(_) => {
                    out.push(VnStmt::Return);
                    self.report.record_exact();
                }
                lsb::Body::Wait(args) => {
                    let ms = args
                        .first()
                        .and_then(expr_i32)
                        .unwrap_or(500)
                        .clamp(0, 60_000) as u32;
                    out.push(VnStmt::Wait { ms });
                    self.report.record_exact();
                }
                lsb::Body::Variable { name, initial, .. } => {
                    let ident = sanitize_ident(name);
                    if self.declared_vars.insert(ident.clone()) {
                        let init_val = match &initial.literal {
                            Some(lsb::Literal::Int(n)) => VnConst::Int(*n as i64),
                            Some(lsb::Literal::Float(f)) => VnConst::Float(*f),
                            Some(lsb::Literal::String(s)) => VnConst::Str(s.clone()),
                            _ => VnConst::Int(0),
                        };
                        self.project.vars.insert(ident, init_val);
                    }
                    self.report.record_exact();
                }
                lsb::Body::Calc(expr) => {
                    if let Some((var, rhs)) = lower_assignment(expr) {
                        self.ensure_var(&var);
                        out.push(VnStmt::SetVar {
                            name: var,
                            expr: rhs,
                        });
                        self.report.record_exact();
                    } else {
                        self.report.record_adapted(
                            &src_loc,
                            "complex LiveMaker Calc expression skipped",
                        );
                    }
                }
                lsb::Body::Condition(expr) if cmd.name() == "If" => {
                    let base_indent = cmd.indent;
                    let cond = lower_expr(expr).unwrap_or_else(|| {
                        self.report
                            .record_adapted(&src_loc, "complex If condition defaulted to true");
                        "true".into()
                    });
                    // Collect indented child block
                    let mut then_cmds = Vec::new();
                    let mut j = i + 1;
                    while j < script.commands.len() && script.commands[j].indent > base_indent {
                        then_cmds.push(script.commands[j].clone());
                        j += 1;
                    }
                    let mut else_cmds = Vec::new();
                    if j < script.commands.len()
                        && script.commands[j].indent == base_indent
                        && script.commands[j].name() == "Else"
                    {
                        j += 1;
                        while j < script.commands.len() && script.commands[j].indent > base_indent {
                            else_cmds.push(script.commands[j].clone());
                            j += 1;
                        }
                    }
                    let mut then_body = Vec::new();
                    let sub_then = lsb::Script {
                        source_sha256: String::new(),
                        version: script.version,
                        commands: then_cmds,
                    };
                    self.lower_script(page_stem, &sub_then, &mut then_body);
                    let mut else_body = Vec::new();
                    if !else_cmds.is_empty() {
                        let sub_else = lsb::Script {
                            source_sha256: String::new(),
                            version: script.version,
                            commands: else_cmds,
                        };
                        self.lower_script(page_stem, &sub_else, &mut else_body);
                    }
                    out.push(VnStmt::If {
                        cond,
                        then_body,
                        else_body,
                    });
                    self.report.record_exact();
                    i = j;
                    continue;
                }
                lsb::Body::Object(props) => {
                    self.lower_object_cmd(&src_loc, cmd.name(), props, out, &mut pending_choices);
                }
                lsb::Body::Delete(target_expr) => {
                    if let Some(name) = expr_string(target_expr) {
                        out.push(VnStmt::Hide {
                            slot: sanitize_ident(&name),
                        });
                        self.report.record_exact();
                    } else {
                        self.report.record_adapted(&src_loc, "dynamic ObjDel skipped");
                    }
                }
                lsb::Body::Flip { parameters, .. } => {
                    let duration_ms = parameters
                        .get("時間")
                        .or_else(|| parameters.get("Time"))
                        .and_then(expr_i32)
                        .map(|v| v.clamp(0, 10_000) as u32);
                    let transition = Some("dissolve".to_owned());
                    out.push(VnStmt::Commit {
                        transition,
                        duration_ms,
                    });
                    self.report.record_exact();
                }
                lsb::Body::Text { text, .. } => {
                    if !pending_choices.is_empty() {
                        out.push(VnStmt::Choice(std::mem::take(&mut pending_choices)));
                    }
                    self.lower_text(&src_loc, text, out);
                }
                lsb::Body::Motion(args) => {
                    if let (Some(slot), Some(ms)) = (
                        args.first().and_then(expr_string),
                        args.get(2).and_then(expr_i32),
                    ) {
                        out.push(VnStmt::Tween {
                            slot: sanitize_ident(&slot),
                            prop: "opacity".into(),
                            to: 1.0,
                            duration_ms: ms.clamp(16, 10_000) as u32,
                            ease: Some("ease_out".into()),
                        });
                        self.report.record_adapted(
                            &src_loc,
                            "PropMotion mapped to NIR v2 Tween",
                        );
                    } else {
                        self.report
                            .record_adapted(&src_loc, "PropMotion parameters simplified");
                    }
                }
                lsb::Body::Other => match cmd.name() {
                    "Terminate" => {
                        out.push(VnStmt::End {
                            outcome: "end".into(),
                        });
                        self.report.record_exact();
                    }
                    "MenuClose" => {
                        if !pending_choices.is_empty() {
                            out.push(VnStmt::Choice(std::mem::take(&mut pending_choices)));
                        }
                        self.report.record_exact();
                    }
                    "Comment" | "ClrHist" | "ClrRead" | "DoEvent" | "TextClr" => {
                        self.report.record_exact();
                    }
                    other => {
                        self.report.record_fallback(
                            &src_loc,
                            format!("unsupported LiveMaker command `{other}` omitted"),
                        );
                        out.push(VnStmt::Comment(format!("[fallback] LiveMaker {other}")));
                    }
                },
                _ => {
                    self.report.record_adapted(
                        &src_loc,
                        format!("LiveMaker command `{}` adapted/omitted", cmd.name()),
                    );
                }
            }
            i += 1;
        }
        if !pending_choices.is_empty() {
            out.push(VnStmt::Choice(pending_choices));
        }
    }

    fn lower_object_cmd(
        &mut self,
        src_loc: &str,
        cmd_name: &str,
        props: &BTreeMap<u16, lsb::Expression>,
        out: &mut Vec<VnStmt>,
        pending_choices: &mut Vec<VnChoice>,
    ) {
        // Common LiveMaker property IDs (1-based in LSB declaration masks):
        // 1: Name, 4: Visible, 5: Left(X), 6: Top(Y), 9: Priority(Z), 11: Source/File, 12: Opacity/Alpha
        let name = props
            .get(&1)
            .and_then(expr_string)
            .unwrap_or_else(|| format!("obj_{cmd_name}"));
        let x = props.get(&5).and_then(expr_i32).map(|v| v as f32);
        let y = props.get(&6).and_then(expr_i32).map(|v| v as f32);
        let z = props.get(&9).and_then(expr_i32);
        let source = props
            .values()
            .filter_map(expr_string)
            .find(|s| {
                let low = s.to_ascii_lowercase();
                low.ends_with(".gal")
                    || low.ends_with(".png")
                    || low.ends_with(".jpg")
                    || low.ends_with(".bmp")
                    || low.ends_with(".wav")
                    || low.ends_with(".ogg")
                    || low.ends_with(".mp3")
                    || low.ends_with(".lcm")
                    || low.ends_with(".lmt")
            });

        match cmd_name {
            "ImgNew" | "TileNew" | "MapImgNew" => {
                if let Some(src) = source {
                    if let Some(asset_path) = self.import_image_asset(src_loc, &src) {
                        let is_bg = name.to_ascii_lowercase().contains("bg")
                            || name.contains("背景")
                            || z.is_some_and(|zv| zv <= 0);
                        if is_bg {
                            out.push(VnStmt::Bg {
                                asset: asset_path,
                                transition: None,
                                duration_ms: None,
                            });
                        } else {
                            out.push(VnStmt::Show {
                                slot: sanitize_ident(&name),
                                asset: asset_path,
                                x,
                                y,
                                z,
                                opacity: None,
                                scale: None,
                            });
                        }
                        self.report.record_exact();
                    }
                } else {
                    self.report
                        .record_adapted(src_loc, format!("{cmd_name} without literal image source"));
                }
            }
            "Sound" | "MediaPlay" => {
                if let Some(src) = source {
                    if let Some(asset_path) = self.import_audio_asset(src_loc, &src) {
                        let is_bgm = name.to_ascii_lowercase().contains("bgm")
                            || cmd_name == "MediaPlay";
                        if is_bgm {
                            out.push(VnStmt::Bgm {
                                asset: asset_path,
                                fade_ms: Some(300),
                                volume: None,
                            });
                        } else {
                            out.push(VnStmt::Sfx {
                                asset: asset_path,
                                volume: None,
                            });
                        }
                        self.report.record_exact();
                    }
                } else {
                    out.push(VnStmt::StopBgm { fade_ms: Some(300) });
                    self.report.record_adapted(src_loc, "Sound without source mapped to bgm stop");
                }
            }
            "Cinema" => {
                if let Some(src) = source {
                    if let Some(vfs_path) = self.vfs.find_by_stem(&src, &["lcm", "lmt"]) {
                        if let Ok(bytes) = self.vfs.read(&vfs_path) {
                            if let Ok(cin) = cinema::parse(&bytes) {
                                for (clip_idx, clip) in cin.clips.iter().enumerate() {
                                    if let cinema::Clip::Image(ic) = clip {
                                        if let Some(asset) = self.import_image_asset(src_loc, &ic.media) {
                                            let slot = format!("cin_{clip_idx}");
                                            out.push(VnStmt::Show {
                                                slot: slot.clone(),
                                                asset,
                                                x: Some(ic.position_start[0] as f32),
                                                y: Some(ic.position_start[1] as f32),
                                                z: Some(ic.order),
                                                opacity: Some((ic.opacity[0] as f32 / 255.0).clamp(0.0, 1.0)),
                                                scale: None,
                                            });
                                            if ic.duration > 0 && ic.position_end != ic.position_start {
                                                if ic.position_end[0] != ic.position_start[0] {
                                                    out.push(VnStmt::Tween {
                                                        slot: slot.clone(),
                                                        prop: "x".into(),
                                                        to: ic.position_end[0] as f32,
                                                        duration_ms: ic.duration,
                                                        ease: Some("ease_out".into()),
                                                    });
                                                }
                                                if ic.position_end[1] != ic.position_start[1] {
                                                    out.push(VnStmt::Tween {
                                                        slot,
                                                        prop: "y".into(),
                                                        to: ic.position_end[1] as f32,
                                                        duration_ms: ic.duration,
                                                        ease: Some("ease_out".into()),
                                                    });
                                                }
                                            }
                                        }
                                    }
                                }
                                self.report.record_adapted(
                                    src_loc,
                                    format!("Cinema timeline `{src}` flattened to Persistent Layers"),
                                );
                                return;
                            }
                        }
                    }
                }
                self.report
                    .record_fallback(src_loc, "Cinema clip could not be resolved");
            }
            "Button" | "Menu" => {
                // Collect button text & label hint into a choice item
                let label = props
                    .values()
                    .filter_map(expr_string)
                    .find(|s| !s.is_empty() && s != &name)
                    .unwrap_or_else(|| name.clone());
                pending_choices.push(VnChoice {
                    text: label,
                    target_label: sanitize_ident(&name),
                    condition: None,
                });
                self.report.record_adapted(src_loc, "LiveMaker Button mapped to Choice");
            }
            "MesNew" | "BoxNew" => {
                out.push(VnStmt::Window { visible: true });
                self.report.record_adapted(src_loc, format!("{cmd_name} mapped to window show"));
            }
            other => {
                self.report.record_adapted(
                    src_loc,
                    format!("object command `{other}` adapted"),
                );
            }
        }
    }

    fn lower_text(&mut self, src_loc: &str, novel: &lsb::Novel, out: &mut Vec<VnStmt>) {
        let mut buf = String::new();
        for glyph in &novel.glyphs {
            match glyph {
                lsb::Glyph::Char(ch) => buf.push_str(ch),
                lsb::Glyph::RubyChar { text, reading, .. } => {
                    buf.push_str(&format!("{{ruby={text}|{reading}}}"));
                }
                lsb::Glyph::Variable(var) => {
                    let ident = sanitize_ident(var);
                    self.ensure_var(&ident);
                    buf.push_str(&format!("{{{ident}}}"));
                }
                lsb::Glyph::Break(_) => {
                    flush_dialogue_line(&mut buf, out);
                }
                lsb::Glyph::Event(parts) => {
                    let head = parts.first().map(String::as_str).unwrap_or("");
                    if head.contains("WAIT") || head.contains("Wait") {
                        flush_dialogue_line(&mut buf, out);
                        out.push(VnStmt::Wait { ms: 400 });
                    }
                }
                lsb::Glyph::InlineImage { .. } | lsb::Glyph::Unsupported => {
                    self.report
                        .record_adapted(src_loc, "inline text glyph omitted");
                }
            }
        }
        flush_dialogue_line(&mut buf, out);
        self.report.record_exact();
    }

    fn import_image_asset(&mut self, src_loc: &str, raw_name: &str) -> Option<String> {
        let vfs_path = self
            .vfs
            .find_by_stem(raw_name, &["gal", "png", "jpg", "jpeg", "webp", "bmp"])?;
        if let Some(existing) = self.converted_assets.get(&vfs_path) {
            return Some(existing.clone());
        }
        let bytes = match self.vfs.read(&vfs_path) {
            Ok(b) => b,
            Err(e) => {
                self.report
                    .record_fallback(src_loc, format!("missing image `{raw_name}`: {e}"));
                return None;
            }
        };
        let stem = asset_stem_key(&vfs_path);
        let (target_rel, out_bytes) = if vfs_path.ends_with(".gal") {
            match media::gal(&bytes).and_then(|img| media::png(&img)) {
                Ok(png) => (format!("img/{stem}.png"), png),
                Err(e) => {
                    self.report
                        .record_fallback(src_loc, format!("GAL decode failed `{vfs_path}`: {e}"));
                    return None;
                }
            }
        } else if vfs_path.ends_with(".bmp") {
            match image::load_from_memory(&bytes)
                .map_err(anyhow::Error::from)
                .and_then(|img| media::png(&img.to_rgba8()))
            {
                Ok(png) => (format!("img/{stem}.png"), png),
                Err(e) => {
                    self.report
                        .record_fallback(src_loc, format!("BMP decode failed `{vfs_path}`: {e}"));
                    return None;
                }
            }
        } else {
            let ext = vfs_path.rsplit('.').next().unwrap_or("png");
            (format!("img/{stem}.{ext}"), bytes)
        };
        self.project.assets.insert(target_rel.clone(), out_bytes);
        self.converted_assets.insert(vfs_path, target_rel.clone());
        Some(target_rel)
    }

    fn import_audio_asset(&mut self, src_loc: &str, raw_name: &str) -> Option<String> {
        let vfs_path = self
            .vfs
            .find_by_stem(raw_name, &["ogg", "wav", "mp3"])?;
        if let Some(existing) = self.converted_assets.get(&vfs_path) {
            return Some(existing.clone());
        }
        let bytes = match self.vfs.read(&vfs_path) {
            Ok(b) => b,
            Err(e) => {
                self.report
                    .record_fallback(src_loc, format!("missing audio `{raw_name}`: {e}"));
                return None;
            }
        };
        let stem = asset_stem_key(&vfs_path);
        let ext = vfs_path.rsplit('.').next().unwrap_or("wav");
        let target_rel = if ext == "ogg" || ext == "mp3" || ext == "wav" {
            format!("audio/{stem}.{ext}")
        } else {
            // Normalize via media::audio if unknown container
            match media::audio(&bytes, 1.0) {
                Ok(wav) => {
                    let rel = format!("audio/{stem}.wav");
                    self.project.assets.insert(rel.clone(), wav);
                    self.converted_assets.insert(vfs_path, rel.clone());
                    return Some(rel);
                }
                Err(e) => {
                    self.report
                        .record_fallback(src_loc, format!("audio decode failed `{vfs_path}`: {e}"));
                    return None;
                }
            }
        };
        self.project.assets.insert(target_rel.clone(), bytes);
        self.converted_assets.insert(vfs_path, target_rel.clone());
        Some(target_rel)
    }

    fn ensure_var(&mut self, ident: &str) {
        if self.declared_vars.insert(ident.to_owned()) {
            self.project
                .vars
                .entry(ident.to_owned())
                .or_insert(VnConst::Int(0));
        }
    }
}

fn flush_dialogue_line(buf: &mut String, out: &mut Vec<VnStmt>) {
    let trimmed = buf.trim().to_owned();
    buf.clear();
    if trimmed.is_empty() {
        return;
    }
    // Heuristic speaker extraction: `【Speaker】Body` or `Speaker「Body」`
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

fn script_stem(path: &str) -> String {
    let base = path.replace('\\', "/");
    let file = base.rsplit('/').next().unwrap_or(&base);
    file.strip_suffix(".lsb")
        .or_else(|| file.strip_suffix(".LSB"))
        .unwrap_or(file)
        .to_ascii_lowercase()
}

fn asset_stem_key(vfs_path: &str) -> String {
    let without_ext = match vfs_path.rsplit_once('.') {
        Some((prefix, _)) => prefix,
        None => vfs_path,
    };
    sanitize_ident(without_ext)
}

fn page_entry_label(page_stem: &str) -> String {
    format!("lm_{}", sanitize_ident(page_stem))
}

fn named_label(page_stem: &str, name: &str) -> String {
    format!("lm_{}_{}", sanitize_ident(page_stem), sanitize_ident(name))
}

fn line_label(page_stem: &str, line: u32) -> String {
    format!("lm_{}_L{line}", sanitize_ident(page_stem))
}

fn resolve_ref_label(current_page: &str, target: &lsb::Reference) -> String {
    let page = if target.page.is_empty() {
        current_page.to_owned()
    } else {
        script_stem(&target.page)
    };
    if target.line == 0 {
        page_entry_label(&page)
    } else {
        line_label(&page, target.line)
    }
}

fn expr_string(e: &lsb::Expression) -> Option<String> {
    match &e.literal {
        Some(lsb::Literal::String(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

fn expr_i32(e: &lsb::Expression) -> Option<i32> {
    match &e.literal {
        Some(lsb::Literal::Int(n)) => Some(*n),
        Some(lsb::Literal::Float(f)) => Some(*f as i32),
        _ => None,
    }
}

fn expr_always_true(e: &lsb::Expression) -> bool {
    e.operations.is_empty() || matches!(e.literal, Some(lsb::Literal::Int(n)) if n != 0)
}

fn format_lit(l: &lsb::Literal) -> Option<String> {
    match l {
        lsb::Literal::Int(n) => Some(n.to_string()),
        lsb::Literal::Float(f) => Some(format!("{f}")),
        lsb::Literal::String(s) => Some(format!("\"{}\"", s.replace('"', "\\\""))),
        lsb::Literal::Variable(v) => Some(sanitize_ident(v)),
        lsb::Literal::Unsupported => None,
    }
}

fn lower_expr(e: &lsb::Expression) -> Option<String> {
    if let Some(lit) = &e.literal {
        return format_lit(lit);
    }
    if e.operations.len() == 1 {
        let (op, _name, args) = &e.operations[0];
        if args.len() == 2 {
            let a = format_lit(&args[0])?;
            let b = format_lit(&args[1])?;
            let sym = match *op {
                2 => "+",
                3 => "-",
                4 => "*",
                5 => "/",
                6 => "==",
                7 => "!=",
                8 => "<",
                9 => "<=",
                10 => ">",
                12 => ">=",
                13 => "and",
                14 => "or",
                _ => return None,
            };
            return Some(format!("({a} {sym} {b})"));
        }
    }
    None
}

fn lower_assignment(e: &lsb::Expression) -> Option<(String, String)> {
    let last = e.operations.last()?;
    let (op, target, args) = last;
    if target.is_empty() || target.starts_with("____") {
        return None;
    }
    if matches!(*op, 0 | 1) && args.len() == 1 {
        let rhs = format_lit(&args[0])?;
        return Some((sanitize_ident(target), rhs));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use nir_import_core::emit_nirs_project;

    #[test]
    fn end_to_end_synthetic_livemaker_project_compiles_and_runs() {
        let dir = tempfile::tempdir().unwrap();
        // Minimal LPB116 settings
        let lpb = lsb::tests::settings_file(&[("ScreenWidth", 1280), ("ScreenHeight", 720)]);
        std::fs::write(dir.path().join("project.lpb"), lpb).unwrap();

        // Minimal LSB116 script with a Terminate command (kind = 45)
        let lsb_bytes = lsb::tests::script(&[lsb::tests::command(45, 1, &[])]);
        std::fs::write(dir.path().join("main.lsb"), lsb_bytes).unwrap();

        let importer = LiveMakerImporter;
        let probe = importer.detect(dir.path()).expect("should detect LiveMaker");
        assert_eq!(probe.engine_id, "livemaker");

        let (project, report) = importer
            .import(dir.path(), &ImportOptions::default())
            .unwrap();
        assert_eq!(report.scripts_converted, 1);
        assert!(report.is_strict_clean());

        let out_dir = tempfile::tempdir().unwrap();
        emit_nirs_project(&project, out_dir.path()).unwrap();

        let built = nir_build::compile_project(
            out_dir.path(),
            &nir_build::optimize::OptimizeConfig {
                enabled: false,
                ..Default::default()
            },
        )
        .unwrap();
        let mut vm = nir_vm::Vm::new(std::sync::Arc::new(built.program), None).unwrap();
        let _ = vm.step(1000).unwrap();
        assert_eq!(vm.outcome.as_deref(), Some("end"));
    }
}
