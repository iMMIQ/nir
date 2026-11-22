//! Bounded static layout & project settings extractor for LiveNovel games.
//!
//! Replaces brittle SHA-256 script pinning with a clean static evaluator over
//! `live.lpb`, `ゲームメイン.lsb`, `メッセージボックス作成.lsb`, and `メッセージボックス座標.lsb`.

use crate::expr::{eval_static_const, literal_i32, literal_string};
use crate::lsb::{self, Body, Command, Expression, Literal, Script};
use crate::media;
use nir_import_core::vfs::{normalize_vfs_path, ArchiveSource, DirArchive};
use nir_ir::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct ExtractedStyle {
    pub name: String,
    pub bg_path: Option<String>,
    pub dialogue_rect: Option<[f32; 4]>,
    pub text_rect: Option<[f32; 4]>,
    pub font_size: Option<f32>,
    pub line_height: Option<f32>,
    pub text_color: Option<[f32; 4]>,
    pub face_pos: Option<[f32; 2]>,
}

#[derive(Debug, Clone)]
pub struct LiveNovelSettings {
    pub title: Option<String>,
    pub stage_width: u32,
    pub stage_height: u32,
    pub default_reveal_us: u64,
    pub bgm_volume: f32,
    pub se_volume: f32,
    pub voice_volume: f32,
}

impl Default for LiveNovelSettings {
    fn default() -> Self {
        Self {
            title: None,
            stage_width: 1024,
            stage_height: 768,
            default_reveal_us: 25_000,
            bgm_volume: 1.0,
            se_volume: 1.0,
            voice_volume: 1.0,
        }
    }
}

/// Extracts title, stage size, and default settings from `live.lpb` + optional `lpm` hints.
pub fn extract_settings(lpb_data: Option<&[u8]>, archive: &DirArchive) -> LiveNovelSettings {
    let mut cfg = LiveNovelSettings::default();
    if let Some(data) = lpb_data {
        // Read LPB project title (first string after version u32)
        if data.len() >= 8 {
            let title_len = u32::from_le_bytes(data[4..8].try_into().unwrap_or([0; 4])) as usize;
            if let Some(raw) = data.get(8..8 + title_len) {
                if let Ok(t) = lsb::decode(raw) {
                    if !t.trim().is_empty() {
                        cfg.title = Some(t.trim().to_owned());
                    }
                }
            }
        }
        if let Ok(settings) = lsb::project_settings(data) {
            if let Some(Literal::Int(w)) = settings
                .get("画面幅")
                .or_else(|| settings.get("ScreenWidth"))
            {
                if (320..=8192).contains(w) {
                    cfg.stage_width = *w as u32;
                }
            }
            if let Some(Literal::Int(h)) = settings
                .get("画面高さ")
                .or_else(|| settings.get("ScreenHeight"))
            {
                if (240..=8192).contains(h) {
                    cfg.stage_height = *h as u32;
                }
            }
            if let Some(Literal::String(t)) = settings
                .get("タイトル")
                .or_else(|| settings.get("Title"))
            {
                if !t.trim().is_empty() {
                    cfg.title = Some(t.trim().to_owned());
                }
            }
            if let Some(Literal::Int(v)) = settings.get("StatusBGMVolume") {
                cfg.bgm_volume = (*v as f32 / 1000.0).clamp(0.0, 1.0);
            }
            if let Some(Literal::Int(v)) = settings.get("StatusSEVolume") {
                cfg.se_volume = (*v as f32 / 1000.0).clamp(0.0, 1.0);
            }
            if let Some(Literal::Int(v)) = settings.get("StatusVoiceVolume") {
                cfg.voice_volume = (*v as f32 / 1000.0).clamp(0.0, 1.0);
            }
            if let Some(Literal::Int(spd)) = settings.get("StatusTextSpeed") {
                // LiveNovel StatusTextSpeed is roughly ms per char (0 = instant, default ~20-40ms).
                if *spd >= 0 && *spd <= 200 {
                    cfg.default_reveal_us = (*spd as u64) * 1000;
                }
            }
        }
    }

    // Many LiveMaker 116/117 games store stage resolution inside `live.lpb` editor blocks
    // beyond `project_settings`, or inside their main `.lpm` files (`<Width>`/`<Height>`).
    // Probe non-system `.lpm` files to auto-detect stage dimensions accurately (e.g. 1024x768 vs 1280x720)!
    let files = archive.list_files();
    for f in &files {
        if f.starts_with("ノベルシステム/") || !f.ends_with(".lpm") {
            continue;
        }
        if let Ok(Some(bytes)) = archive.read_if_exists(f) {
            if let Ok(menu) = crate::lpm::parse_lpm(&bytes, f) {
                if menu.width >= 640 && menu.height >= 480 {
                    cfg.stage_width = menu.width;
                    cfg.stage_height = menu.height;
                    break;
                }
            }
        }
    }

    cfg
}

/// Evaluates `メッセージボックス作成.lsb` (and optional `ゲームメイン.lsb` face coords)
/// to extract named dialogue box themes cleanly without hash pinning.
pub fn extract_dialogue_styles(
    factory_script: Option<&Script>,
    game_main_script: Option<&Script>,
    stage: [u32; 2],
    archive: &DirArchive,
) -> Vec<ExtractedStyle> {
    let mut default_face_pos: Option<[f32; 2]> = None;
    if let Some(gm) = game_main_script {
        let mut fx = None;
        let mut fy = None;
        let mut manual = false;
        for cmd in gm.commands.iter().filter(|c| !c.muted) {
            if let Body::Variable { name, initial, .. } = &cmd.body {
                if name == "__テキスト顔位置合わせ" {
                    if let Some(s) = literal_string(initial) {
                        if s != "自動" {
                            manual = true;
                        }
                    }
                } else if name == "__テキスト顔座標X" {
                    fx = literal_i32(initial).map(|v| v as f32);
                } else if name == "__テキスト顔座標Y" {
                    fy = literal_i32(initial).map(|v| v as f32);
                }
            }
        }
        if manual {
            if let (Some(x), Some(y)) = (fx, fy) {
                default_face_pos = Some([x, y]);
            }
        }
    }

    let Some(script) = factory_script else {
        return Vec::new();
    };

    // Group top-level style branches (`If @ParamStr[0] == "<style>"` / `Elseif ...`)
    let mut styles = Vec::new();
    let cmds = &script.commands;
    let mut i = 0usize;
    while i < cmds.len() {
        let cmd = &cmds[i];
        if cmd.muted {
            i += 1;
            continue;
        }
        if matches!(cmd.kind, 0 | 1) {
            if let Body::Condition(cond) = &cmd.body {
                if let Some(style_name) = match_param0_eq_string(cond) {
                    let base_indent = cmd.indent;
                    let mut j = i + 1;
                    while j < cmds.len() && cmds[j].indent > base_indent {
                        j += 1;
                    }
                    if let Some(st) = eval_style_branch(
                        &style_name,
                        &cmds[i + 1..j],
                        stage,
                        default_face_pos,
                        archive,
                    ) {
                        if !styles.iter().any(|s: &ExtractedStyle| s.name == st.name) {
                            styles.push(st);
                        }
                    }
                    i = j;
                    continue;
                }
            }
        }
        i += 1;
    }

    // Fallback: if no explicit `@ParamStr[0] == ...` branches, scan whole script as "(標準)"
    if styles.is_empty() {
        if let Some(st) =
            eval_style_branch("(標準)", cmds, stage, default_face_pos, archive)
        {
            styles.push(st);
        }
    }

    // Ensure "(標準)" is style index 0 if present.
    if let Some(pos) = styles.iter().position(|s| s.name == "(標準)") {
        if pos != 0 {
            let std_style = styles.remove(pos);
            styles.insert(0, std_style);
        }
    }

    styles
}

fn match_param0_eq_string(expr: &Expression) -> Option<String> {
    let mut has_param0 = false;
    let mut candidate_str = None;
    for (op, _, args) in &expr.operations {
        if *op == 10
            && matches!(
                args.as_slice(),
                [Literal::Variable(v), Literal::Int(0)] if v == "@ParamStr"
            )
        {
            has_param0 = true;
        }
        if matches!(*op, 0 | 1 | 12) {
            for a in args {
                if let Literal::String(s) = a {
                    candidate_str = Some(s.clone());
                }
            }
        }
    }
    if has_param0 {
        candidate_str
    } else {
        None
    }
}

fn eval_style_branch(
    name: &str,
    cmds: &[Command],
    stage: [u32; 2],
    default_face_pos: Option<[f32; 2]>,
    archive: &DirArchive,
) -> Option<ExtractedStyle> {
    let mut obj_sizes: BTreeMap<String, [f32; 2]> = BTreeMap::new();
    let mut bg_path: Option<String> = None;
    let mut base_rect: Option<[f32; 4]> = None;
    let mut text_rect: Option<[f32; 4]> = None;
    let mut font_size: Option<f32> = None;
    let mut line_height: Option<f32> = None;
    let mut text_color: Option<[f32; 4]> = None;
    let mut face_x = default_face_pos.map(|p| p[0]);
    let mut face_y = default_face_pos.map(|p| p[1]);

    for cmd in cmds.iter().filter(|c| !c.muted) {
        match &cmd.body {
            Body::Calc(expr) => {
                // Capture `__テキスト顔座標X` / `Y` overrides inside style branch
                if let Some((_, dst, _)) = expr.operations.last() {
                    if dst == "__テキスト顔座標X" {
                        if let Some(v) = eval_layout_i32(expr, stage, &obj_sizes) {
                            face_x = Some(v as f32);
                        }
                    } else if dst == "__テキスト顔座標Y" {
                        if let Some(v) = eval_layout_i32(expr, stage, &obj_sizes) {
                            face_y = Some(v as f32);
                        }
                    }
                }
            }
            Body::Object(props) if matches!(cmd.name(), "ImgNew" | "BoxNew") => {
                let obj_name = props
                    .get(&1)
                    .and_then(literal_string)
                    .unwrap_or("")
                    .to_owned();
                if obj_name != "メッセージボックス土台" {
                    continue;
                }
                let src = props.get(&3).and_then(literal_string);
                let mut w = props
                    .get(&6)
                    .and_then(|e| eval_layout_i32(e, stage, &obj_sizes))
                    .unwrap_or(0) as f32;
                let mut h = props
                    .get(&7)
                    .and_then(|e| eval_layout_i32(e, stage, &obj_sizes))
                    .unwrap_or(0) as f32;

                if let Some(raw_src) = src {
                    let norm = normalize_vfs_path(raw_src);
                    if let Some((iw, ih)) = probe_image_size(archive, &norm) {
                        w = iw as f32;
                        h = ih as f32;
                        bg_path = Some(norm);
                    }
                }
                if w > 0.0 && h > 0.0 {
                    obj_sizes.insert(obj_name.clone(), [w, h]);
                }
                let x = props
                    .get(&4)
                    .and_then(|e| eval_layout_i32(e, stage, &obj_sizes))
                    .unwrap_or(0) as f32;
                let y = props
                    .get(&5)
                    .and_then(|e| eval_layout_i32(e, stage, &obj_sizes))
                    .unwrap_or(0) as f32;
                if w > 0.0 && h > 0.0 {
                    base_rect = Some([x, y, w, h]);
                }
            }
            Body::Object(props) if cmd.name() == "MesNew" => {
                let obj_name = props.get(&1).and_then(literal_string).unwrap_or("");
                if obj_name != "メッセージボックス" {
                    continue;
                }
                let [bx, by, bw, bh] = base_rect.unwrap_or([0.0, 0.0, stage[0] as f32, stage[1] as f32]);
                let rx = props
                    .get(&4)
                    .and_then(|e| eval_layout_i32(e, stage, &obj_sizes))
                    .unwrap_or(20) as f32;
                let ry = props
                    .get(&5)
                    .and_then(|e| eval_layout_i32(e, stage, &obj_sizes))
                    .unwrap_or(20) as f32;
                let rw = props
                    .get(&6)
                    .and_then(|e| eval_layout_i32(e, stage, &obj_sizes))
                    .map(|v| v as f32)
                    .unwrap_or_else(|| (bw - rx * 2.0).max(100.0));
                let rh = props
                    .get(&7)
                    .and_then(|e| eval_layout_i32(e, stage, &obj_sizes))
                    .map(|v| v as f32)
                    .unwrap_or_else(|| (bh - ry * 2.0).max(60.0));
                text_rect = Some([bx + rx, by + ry, rw.max(64.0), rh.max(40.0)]);

                // Property 17 (1-based) = FontSize, 19 = LineSpace, 20 = FontColor (BGR TColor)
                if let Some(fs) = props
                    .get(&17)
                    .and_then(|e| eval_layout_i32(e, stage, &obj_sizes))
                {
                    if (10..=72).contains(&fs) {
                        font_size = Some(fs as f32);
                        let ls = props
                            .get(&19)
                            .and_then(|e| eval_layout_i32(e, stage, &obj_sizes))
                            .unwrap_or(6)
                            .clamp(0, 40) as f32;
                        line_height = Some(fs as f32 + ls + 4.0);
                    }
                }
                if let Some(c) = props
                    .get(&20)
                    .and_then(|e| eval_layout_i32(e, stage, &obj_sizes))
                {
                    if c >= 0 {
                        let r = (c & 0xff) as f32 / 255.0;
                        let g = ((c >> 8) & 0xff) as f32 / 255.0;
                        let b = ((c >> 16) & 0xff) as f32 / 255.0;
                        text_color = Some([r, g, b, 1.0]);
                    }
                }
            }
            _ => {}
        }
    }

    if base_rect.is_none() && text_rect.is_none() && bg_path.is_none() {
        return None;
    }

    Some(ExtractedStyle {
        name: name.to_owned(),
        bg_path,
        dialogue_rect: base_rect,
        text_rect,
        font_size,
        line_height,
        text_color,
        face_pos: match (face_x, face_y) {
            (Some(x), Some(y)) if x != 0.0 || y != 0.0 => Some([x, y]),
            _ => None,
        },
    })
}

fn eval_layout_i32(
    expr: &Expression,
    stage: [u32; 2],
    obj_sizes: &BTreeMap<String, [f32; 2]>,
) -> Option<i32> {
    if let Some(v) = eval_static_const(expr, stage).and_then(|v| v.as_i64()) {
        return Some(v as i32);
    }
    let mut env: BTreeMap<String, Value> = BTreeMap::new();
    let resolve = |lit: &Literal, env: &BTreeMap<String, Value>| -> Option<Value> {
        match lit {
            Literal::Int(n) => Some(Value::Int(*n as i64)),
            Literal::Float(f) => Some(Value::Float(*f)),
            Literal::String(s) => Some(Value::Str(s.clone())),
            Literal::Variable(v) => match v.as_str() {
                "@ScrWidth" => Some(Value::Int(stage[0] as i64)),
                "@ScrHeight" => Some(Value::Int(stage[1] as i64)),
                _ => env.get(v).cloned(),
            },
            Literal::Unsupported => None,
        }
    };
    let mut last = None;
    for (idx, (op, dst, args)) in expr.operations.iter().enumerate() {
        let val = match *op {
            0 | 1 if args.len() == 1 => resolve(&args[0], &env)?,
            2 if args.len() == 2 => {
                Value::Int(resolve(&args[0], &env)?.as_i64()? + resolve(&args[1], &env)?.as_i64()?)
            }
            3 if args.len() == 2 => {
                Value::Int(resolve(&args[0], &env)?.as_i64()? - resolve(&args[1], &env)?.as_i64()?)
            }
            4 if args.len() == 2 => {
                Value::Int(resolve(&args[0], &env)?.as_i64()? * resolve(&args[1], &env)?.as_i64()?)
            }
            5 if args.len() == 2 => {
                let b = resolve(&args[1], &env)?.as_i64()?;
                if b == 0 {
                    return None;
                }
                Value::Int(resolve(&args[0], &env)?.as_i64()? / b)
            }
            11 => {
                let fid = expr.functions.get(&idx).copied()?;
                match fid {
                    2 if args.len() >= 2 => {
                        let obj = resolve(&args[0], &env)?.to_display_string();
                        let prop = resolve(&args[1], &env)?.as_i64()?;
                        let sz = obj_sizes.get(&obj).copied().unwrap_or([0.0, 0.0]);
                        // GetProp 0-based: 5 = Width, 6 = Height
                        match prop {
                            5 => Value::Int(sz[0] as i64),
                            6 => Value::Int(sz[1] as i64),
                            _ => Value::Int(0),
                        }
                    }
                    35 => Value::Int(stage[0] as i64),
                    36 => Value::Int(stage[1] as i64),
                    37 if args.len() == 1 => Value::Int(resolve(&args[0], &env)?.as_i64().unwrap_or(0)),
                    _ => return None,
                }
            }
            _ => return None,
        };
        env.insert(dst.clone(), val.clone());
        last = Some(val);
    }
    last.and_then(|v| v.as_i64()).map(|n| n as i32)
}

/// Reads image dimensions from header only (zero full pixel decode!).
pub fn probe_image_size(archive: &DirArchive, norm_path: &str) -> Option<(u32, u32)> {
    let real = archive.real_path(norm_path)?;
    if norm_path.ends_with(".gal") {
        media::gal_file_size(real).ok()
    } else {
        image::image_dimensions(real).ok()
    }
}
