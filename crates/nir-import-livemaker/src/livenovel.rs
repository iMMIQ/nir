//! Direct-to-`NirProgram` LiveMaker / LiveNovel 116/117 compiler.
//!
//! Architecture:
//! 1. Cuts cleanly at the `ノベルシステム/` ABI boundary (never emulates Delphi/Win32 system GUI scripts).
//! 2. Statically extracts dialogue styles (`メッセージボックス作成.lsb`) & project settings (`live.lpb`).
//! 3. Compiles all author `.lsb` scripts into flat `nir_ir::Op` bytecode with full cross-page CFG,
//!    exact LSB expression operators (`0..=20`), pure `BuiltinFn` intrinsics, persistent `scope == 3`
//!    profile variables, all 24 `TpWord` narrative events, LPM106/LPM200 menus, Cinema 111/112/113
//!    timelines, and Replay/CG Gallery metadata.
//! 4. Registers media lazily via `RawAssetData::Lazy` so `nir-build` tree-shakes unused files BEFORE
//!    decoding a single `.gal` pixel and optimizes all kept assets in parallel across CPU cores.

use crate::cinema::{self, Clip, OtherFields};
use crate::events::{self, EventBridge};
use crate::expr::{
    self, eval_static_const, is_const_false, is_const_true, literal_i32, literal_string, ExprBridge,
};
use crate::layout;
use crate::lpm;
use crate::lsb::{self, Body, Command, Expression, Glyph, Literal, Reference, Script};
use crate::media;
use anyhow::{Context, Result};
use nir_import_core::vfs::{decode_text_bytes, normalize_vfs_path, ArchiveSource, DirArchive};
use nir_import_core::{
    file_fingerprint, sanitize_ident, ConversionReport, DecodedPixels, DirectImport, ImportOptions,
    RawAssetData, RawAssetInput,
};
use nir_ir::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const DEFAULT_LOCALE: &str = "ja";

pub fn import_direct(
    source: &Path,
    opts: &ImportOptions,
) -> Result<(DirectImport, ConversionReport)> {
    let archive = Arc::new(DirArchive::open(source)?);
    let raw_all_files = archive.list_files();
    // Exclude nested standalone sub-games (subdirectories containing their own `live.lpb` or `ノベルシステム/`),
    // e.g. bundled bonus mini-games under `おまけ/`. Users can convert sub-games directly by pointing at that subdir.
    let mut nested_prefixes: Vec<String> = Vec::new();
    for f in &raw_all_files {
        if let Some(prefix) = f.strip_suffix("/live.lpb") {
            nested_prefixes.push(format!("{prefix}/"));
        } else if let Some(idx) = f.find("/ノベルシステム/") {
            nested_prefixes.push(format!("{}/", &f[..idx]));
        }
    }
    nested_prefixes.sort();
    nested_prefixes.dedup();
    let all_files: Vec<String> = raw_all_files
        .into_iter()
        .filter(|f| !nested_prefixes.iter().any(|p| f.starts_with(p)))
        .collect();
    let mut report = ConversionReport::new("livemaker");

    // 1. Read `live.lpb` project settings & auto-detect stage dimensions
    let lpb_path = all_files
        .iter()
        .find(|f| f.as_str() == "live.lpb" || (!f.contains('/') && f.ends_with(".lpb")))
        .cloned();
    let lpb_bytes = lpb_path
        .as_deref()
        .and_then(|p| archive.read_if_exists(p).ok().flatten());
    let lpb_settings_map = lpb_bytes
        .as_deref()
        .and_then(|b| lsb::project_settings(b).ok())
        .unwrap_or_default();
    let lpb_startup = lpb_bytes
        .as_deref()
        .and_then(|b| lsb::startup(b).ok())
        .map(|s| normalize_vfs_path(&s))
        .filter(|s| !s.is_empty());
    let settings = layout::extract_settings(lpb_bytes.as_deref(), &archive);

    let folder_name = source
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("livemaker_game");
    let title = settings
        .title
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| folder_name.to_owned());
    let game_id = format!("org.nir.lm.{}", sanitize_ident(folder_name));
    let locale = opts
        .source_locale
        .clone()
        .unwrap_or_else(|| DEFAULT_LOCALE.to_owned());

    // 2. Detect LiveNovel topology vs generic standalone LiveMaker project
    let has_novelsys = all_files.iter().any(|f| f.starts_with("ノベルシステム/"));
    let has_game_main = all_files.iter().any(|f| f == "ゲームメイン.lsb");
    let is_livenovel = has_novelsys || has_game_main;

    // 3. Parse all author `.lsb` scripts (skipping `ノベルシステム/` ABI internals)
    let mut author_scripts: BTreeMap<String, Script> = BTreeMap::new();
    let mut factory_script: Option<Script> = None;
    let mut game_main_script: Option<Script> = None;
    let mut replay_dispatch_script: Option<Script> = None;

    for path in &all_files {
        if !path.ends_with(".lsb") {
            continue;
        }
        if is_livenovel && path.starts_with("ノベルシステム/") {
            continue;
        }
        if author_scripts.len() >= opts.max_scripts {
            break;
        }
        let Some(bytes) = archive.read_if_exists(path)? else {
            continue;
        };
        let script = match lsb::parse(&bytes) {
            Ok(s) => s,
            Err(e) => {
                report.record_fallback(path, format!("failed to parse LSB: {e}"));
                continue;
            }
        };
        if is_livenovel {
            match path.as_str() {
                "メッセージボックス作成.lsb" => {
                    factory_script = Some(script);
                    continue;
                }
                "メッセージボックス座標.lsb" => {
                    continue;
                }
                "シーン回想.lsb" => {
                    replay_dispatch_script = Some(script);
                    continue;
                }
                "ゲームメイン.lsb" => {
                    game_main_script = Some(script.clone());
                }
                _ => {}
            }
        }
        author_scripts.insert(path.clone(), script);
    }

    // 4. Initialize compiler state
    let stage = [settings.stage_width, settings.stage_height];
    let mut c = Compiler::new(archive.clone(), all_files, stage, &mut report);

    // 5. Extract dialogue box styles from `メッセージボックス作成.lsb`
    let extracted_styles = layout::extract_dialogue_styles(
        factory_script.as_ref(),
        game_main_script.as_ref(),
        stage,
        &archive,
    );
    let mut style_index_by_name: BTreeMap<String, u16> = BTreeMap::new();
    let mut dialogue_styles: Vec<DialogueStyleDef> = Vec::new();
    for st in &extracted_styles {
        let idx = dialogue_styles.len() as u16;
        style_index_by_name.insert(st.name.clone(), idx);
        let bg = st
            .bg_path
            .as_deref()
            .and_then(|p| c.register_image_asset(p));
        let name_sid = c.intern_str(&st.name);
        dialogue_styles.push(DialogueStyleDef {
            name: name_sid,
            dialogue_bg: bg,
            dialogue_rect: st.dialogue_rect,
            text_rect: st.text_rect,
            font_size: st.font_size,
            line_height: st.line_height,
            text_color: st.text_color,
            face_pos: st.face_pos,
            name_rect: None,
        });
    }
    c.style_index_by_name = style_index_by_name;

    // 6. Parse CGモード.TXT & シーン回想.TXT + シーン回想.lsb
    let replay_entries = c.parse_replay_gallery(replay_dispatch_script.as_ref());
    let cg_gallery_items = c.parse_cg_gallery();

    // 7. Pre-declare all variables across all author scripts (capturing `scope == 3` profile vars!)
    c.ensure_var("選択値", Value::Str(String::new()));
    c.ensure_var("選択番号", Value::Int(-1));
    c.ensure_var("最終選択値", Value::Str(String::new()));
    c.ensure_var("最終選択番号", Value::Int(-1));
    c.ensure_var("選択実行中", Value::Int(0));
    c.ensure_var("回想番号", Value::Int(-1));
    for (k, lit) in &lpb_settings_map {
        let val = match lit {
            Literal::Int(n) => Value::Int(*n as i64),
            Literal::Float(f) => Value::Float(*f),
            Literal::String(s) => Value::Str(s.clone()),
            _ => continue,
        };
        c.ensure_var(k, val);
    }
    for script in author_scripts.values() {
        c.prescan_vars(script);
    }

    // 8. Reserve FuncId(0) for `__entry__`
    let entry_name = c.intern_str("__entry__");
    c.funcs.push(FuncMeta {
        name: entry_name,
        entry_pc: 0,
        param_count: 0,
        local_count: 1,
    });

    // Emit entry prologue: restore persisted `scope == 3` profile values, then jump to startup page!
    let profile_vars: Vec<(VarId, StrId)> = c
        .profile_vars
        .iter()
        .filter_map(|(name, &key)| c.var_map.get(name).map(|&vid| (vid, key)))
        .collect();
    for (vid, key) in profile_vars {
        c.ops.push(Op::ProfileGetVal { dst: vid, key });
    }
    if !dialogue_styles.is_empty() {
        c.ops.push(Op::SetDialogueStyle(0));
    }

    // Determine entry script target
    let entry_page = if is_livenovel && author_scripts.contains_key("ゲームメイン.lsb") {
        Some("ゲームメイン.lsb".to_owned())
    } else if let Some(ref start) = lpb_startup {
        if author_scripts.contains_key(start) {
            Some(start.clone())
        } else {
            author_scripts.keys().next().cloned()
        }
    } else {
        author_scripts.keys().next().cloned()
    };

    if let Some(ref ep) = entry_page {
        let jmp_idx = c.ops.len();
        c.ops.push(Op::Jump(0));
        c.jump_patches.push((jmp_idx, ep.clone(), 0));
    } else {
        let end_sid = c.intern_str("end");
        c.ops.push(Op::End { outcome: end_sid });
    }

    // 9. Compile every author `.lsb` script into the unified flat bytecode stream
    let script_pages: Vec<String> = author_scripts.keys().cloned().collect();
    for page in &script_pages {
        let script = &author_scripts[page];
        c.compile_page(page, script);
        c.report.scripts_converted += 1;
    }

    // 10. Register replay wrapper functions (sets `回想番号 = idx` then jumps into the replay scene)
    let mut gallery: Vec<GalleryItem> = Vec::new();
    let replay_num_var = c.ensure_var("回想番号", Value::Int(-1));
    for (idx, rep) in replay_entries.into_iter().enumerate() {
        let target_func = c.ensure_call_func(&rep.target_page, rep.target_line);
        let wrapper_pc = c.ops.len() as u32;
        let wrapper_fid = FuncId(c.funcs.len() as u32);
        let wrapper_name = c.intern_str(&format!("__replay_{}", rep.replay_id));
        c.funcs.push(FuncMeta {
            name: wrapper_name,
            entry_pc: wrapper_pc,
            param_count: 0,
            local_count: 1,
        });
        c.ops.push(Op::PushInt(idx as i64));
        c.ops.push(Op::StoreVar(replay_num_var));
        c.ops.push(Op::PushNull);
        c.ops.push(Op::Call {
            func: target_func,
            argc: 1,
        });
        c.ops.push(Op::Ret { has_value: false });

        let id_sid = c.intern_str(&format!("replay_{}", rep.replay_id));
        let title_sid = c.intern_str(&format!("Scene {:02}", idx + 1));
        let flag_sid = c.intern_str(&format!("replay_{}", rep.replay_id));
        gallery.push(GalleryItem {
            id: id_sid,
            title: title_sid,
            thumb: rep.thumb,
            unlock_flag: Some(flag_sid),
            replay_func: Some(wrapper_fid),
            cg_assets: Vec::new(),
        });
    }
    gallery.extend(cg_gallery_items);

    // 11. Backpatch all jumps & function entry PCs
    c.backpatch_all();

    // 12. If any event used dynamic `?var` paths, statically narrow prefixes & register dynamic assets!
    c.populate_dynamic_assets(is_livenovel, &author_scripts);

    // 13. Register any bundled TTF/OTF fonts
    let mut fonts = Vec::new();
    for f in c.all_files.clone() {
        if f.ends_with(".ttf") || f.ends_with(".otf") {
            if let Some(id) = c.register_font_asset(&f) {
                fonts.push(id);
            }
        }
    }

    // 14. Assemble `UiTheme` and `NirProgram`
    let mut ui = UiTheme::default();
    if let Some(std_style) = dialogue_styles.first() {
        ui.dialogue_bg = std_style.dialogue_bg;
        ui.dialogue_rect = std_style.dialogue_rect;
        ui.text_rect = std_style.text_rect;
        if let Some(fs) = std_style.font_size {
            ui.font_size = fs;
        }
        if let Some(lh) = std_style.line_height {
            ui.line_height = lh;
        }
        if let Some(tc) = std_style.text_color {
            ui.text = tc;
        }
    }
    ui.dialogue_styles = dialogue_styles;
    ui.gallery = gallery;

    let title_bg = c.first_title_bg;
    let title_bgm = c.first_title_bgm;

    c.report.statements_emitted = c.ops.len();
    c.report.assets_converted = c.raw_assets.len();

    let mut texts_map = BTreeMap::new();
    texts_map.insert(locale.clone(), c.texts);

    let program = NirProgram {
        features: 0,
        meta: GameMeta {
            game_id,
            title,
            version: "1.0.0".into(),
            stage_width: stage[0],
            stage_height: stage[1],
            default_locale: locale.clone(),
            locales: vec![locale],
            entry_func: FuncId(0),
            save_slots: 12,
            default_reveal_us: settings.default_reveal_us,
            title_bg,
            title_bgm,
            ui_fonts: fonts.clone(),
            text_fonts: fonts,
        },
        strings: c.strings,
        vars: c.vars,
        funcs: c.funcs,
        labels: c.labels,
        ops: c.ops,
        texts: texts_map,
        ui,
        assets: Vec::new(),
        asset_lookup: c.asset_lookup,
        timelines: c.timelines,
        menus: c.menus,
    };

    let direct = DirectImport {
        program,
        raw_assets: c.raw_assets,
        keep_assets: c.keep_assets,
    };
    Ok((direct, report))
}

struct ReplayEntry {
    replay_id: u32,
    thumb: Option<AssetId>,
    target_page: String,
    target_line: u32,
}

struct Compiler<'a> {
    archive: Arc<DirArchive>,
    all_files: Vec<String>,
    file_set: BTreeSet<String>,
    stage: [u32; 2],
    report: &'a mut ConversionReport,

    strings: Vec<String>,
    str_map: BTreeMap<String, StrId>,

    vars: Vec<VarMeta>,
    var_map: BTreeMap<String, VarId>,
    profile_vars: BTreeMap<String, StrId>,

    funcs: Vec<FuncMeta>,
    call_func_map: BTreeMap<(String, u32), FuncId>,
    labels: Vec<LabelMeta>,
    ops: Vec<Op>,
    texts: Vec<TextEntry>,

    layer_map: BTreeMap<String, LayerId>,
    channel_map: BTreeMap<String, (ChannelId, AudioBus)>,
    next_channel_id: u32,

    raw_assets: Vec<RawAssetInput>,
    asset_by_vfs: BTreeMap<String, AssetId>,
    asset_lookup: BTreeMap<String, AssetId>,
    keep_assets: BTreeSet<AssetId>,
    cg_unlock_by_asset: BTreeMap<AssetId, StrId>,

    timelines: Vec<TimelineDef>,
    timeline_by_vfs: BTreeMap<String, TimelineId>,

    menus: Vec<MenuTemplate>,
    menu_by_vfs: BTreeMap<String, MenuId>,

    style_index_by_name: BTreeMap<String, u16>,

    // CFG backpatch tables
    line_pcs: BTreeMap<(String, u32), u32>,
    page_lines: BTreeMap<String, Vec<(u32, u32)>>,
    jump_patches: Vec<(usize, String, u32)>,

    // Per-function local temp slots (slot 0 is reserved for `@ParamStr`)
    temp_locals: BTreeMap<String, u16>,
    max_local_count: u16,

    // Dynamic path variable sets & title hints
    dynamic_image_vars: BTreeSet<String>,
    dynamic_audio_vars: BTreeSet<String>,
    first_title_bg: Option<AssetId>,
    first_title_bgm: Option<AssetId>,
    current_loc: String,
}

impl<'a> Compiler<'a> {
    fn new(
        archive: Arc<DirArchive>,
        all_files: Vec<String>,
        stage: [u32; 2],
        report: &'a mut ConversionReport,
    ) -> Self {
        let file_set: BTreeSet<String> = all_files.iter().cloned().collect();
        let mut channel_map = BTreeMap::new();
        channel_map.insert("BGM".into(), (ChannelId(0), AudioBus::Bgm));
        channel_map.insert("VOICE".into(), (ChannelId(1), AudioBus::Voice));
        channel_map.insert("VOICE2".into(), (ChannelId(2), AudioBus::Voice));
        channel_map.insert("SE".into(), (ChannelId(3), AudioBus::Sfx));
        channel_map.insert("SE2".into(), (ChannelId(4), AudioBus::Sfx));

        Self {
            archive,
            all_files,
            file_set,
            stage,
            report,
            strings: Vec::new(),
            str_map: BTreeMap::new(),
            vars: Vec::new(),
            var_map: BTreeMap::new(),
            profile_vars: BTreeMap::new(),
            funcs: Vec::new(),
            call_func_map: BTreeMap::new(),
            labels: Vec::new(),
            ops: Vec::new(),
            texts: Vec::new(),
            layer_map: BTreeMap::new(),
            channel_map,
            next_channel_id: 8,
            raw_assets: Vec::new(),
            asset_by_vfs: BTreeMap::new(),
            asset_lookup: BTreeMap::new(),
            keep_assets: BTreeSet::new(),
            cg_unlock_by_asset: BTreeMap::new(),
            timelines: Vec::new(),
            timeline_by_vfs: BTreeMap::new(),
            menus: Vec::new(),
            menu_by_vfs: BTreeMap::new(),
            style_index_by_name: BTreeMap::new(),
            line_pcs: BTreeMap::new(),
            page_lines: BTreeMap::new(),
            jump_patches: Vec::new(),
            temp_locals: BTreeMap::new(),
            max_local_count: 1,
            dynamic_image_vars: BTreeSet::new(),
            dynamic_audio_vars: BTreeSet::new(),
            first_title_bg: None,
            first_title_bgm: None,
            current_loc: String::new(),
        }
    }

    fn intern_str(&mut self, s: &str) -> StrId {
        if let Some(&id) = self.str_map.get(s) {
            return id;
        }
        let id = StrId(self.strings.len() as u32);
        self.strings.push(s.to_owned());
        self.str_map.insert(s.to_owned(), id);
        id
    }

    fn ensure_var(&mut self, name: &str, default: Value) -> VarId {
        if let Some(&id) = self.var_map.get(name) {
            return id;
        }
        let sid = self.intern_str(name);
        let id = VarId(self.vars.len() as u32);
        self.vars.push(VarMeta { name: sid, default });
        self.var_map.insert(name.to_owned(), id);
        id
    }

    fn prescan_vars(&mut self, script: &Script) {
        for cmd in &script.commands {
            if cmd.muted {
                continue;
            }
            if let Body::Variable {
                name,
                value_type,
                initial,
                scope,
            } = &cmd.body
            {
                let default =
                    eval_static_const(initial, self.stage).unwrap_or_else(|| match value_type {
                        4 => Value::Str(String::new()),
                        2 => Value::Float(0.0),
                        _ => Value::Int(0),
                    });
                self.ensure_var(name, default);
                if *scope == 3 {
                    let key = self.intern_str(&format!("lm_prof_{name}"));
                    self.profile_vars.insert(name.clone(), key);
                }
            }
        }
    }

    fn ensure_call_func(&mut self, page: &str, line: u32) -> FuncId {
        let key = (page.to_owned(), line);
        if let Some(&fid) = self.call_func_map.get(&key) {
            return fid;
        }
        let fid = FuncId(self.funcs.len() as u32);
        let fname = self.intern_str(&format!("{page}:{line}"));
        self.funcs.push(FuncMeta {
            name: fname,
            entry_pc: 0,
            param_count: 1,
            local_count: 1,
        });
        self.call_func_map.insert(key, fid);
        fid
    }

    fn find_vfs_file(&self, raw: &str, prefixes: &[&str], exts: &[&str]) -> Option<String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        let norm = normalize_vfs_path(trimmed);
        if self.file_set.contains(&norm) {
            return Some(norm);
        }
        for ext in exts {
            let cand = format!("{norm}.{ext}");
            if self.file_set.contains(&cand) {
                return Some(cand);
            }
        }
        for prefix in prefixes {
            let cand = normalize_vfs_path(&format!("{prefix}/{trimmed}"));
            if self.file_set.contains(&cand) {
                return Some(cand);
            }
            for ext in exts {
                let cand_ext = format!("{cand}.{ext}");
                if self.file_set.contains(&cand_ext) {
                    return Some(cand_ext);
                }
            }
        }
        None
    }

    fn register_image_asset(&mut self, raw: &str) -> Option<AssetId> {
        let vfs = self.find_vfs_file(
            raw,
            &["グラフィック", "グラフィック/顔"],
            &["gal", "png", "jpg", "jpeg", "webp", "bmp"],
        )?;
        let id = self.register_vfs_image(&vfs)?;
        if self.first_title_bg.is_none() {
            self.first_title_bg = Some(id);
        }
        Some(id)
    }

    fn register_vfs_image(&mut self, vfs_path: &str) -> Option<AssetId> {
        if let Some(&id) = self.asset_by_vfs.get(vfs_path) {
            return Some(id);
        }
        let real_path = self.archive.real_path(vfs_path)?.to_path_buf();
        let fp = file_fingerprint(&real_path);
        let (codec, data) = if vfs_path.ends_with(".gal") {
            let p = real_path.clone();
            (
                MediaCodec::Png,
                RawAssetData::LazyImage(Arc::new(move || {
                    let bytes = std::fs::read(&p)
                        .with_context(|| format!("reading GAL `{}`", p.display()))?;
                    media::gal_pixels(&bytes)
                        .with_context(|| format!("decoding GAL `{}`", p.display()))
                })),
            )
        } else if vfs_path.ends_with(".bmp") {
            let p = real_path.clone();
            (
                MediaCodec::Png,
                RawAssetData::LazyImage(Arc::new(move || {
                    let bytes = std::fs::read(&p)?;
                    let img = image::load_from_memory(&bytes)?.to_rgba8();
                    let (width, height) = img.dimensions();
                    Ok(DecodedPixels::Rgba {
                        width,
                        height,
                        data: img.into_raw(),
                    })
                })),
            )
        } else {
            let codec = if vfs_path.ends_with(".jpg") || vfs_path.ends_with(".jpeg") {
                MediaCodec::Jpeg
            } else if vfs_path.ends_with(".webp") {
                MediaCodec::Webp
            } else {
                MediaCodec::Png
            };
            let p = real_path.clone();
            (
                codec,
                RawAssetData::Lazy(Arc::new(move || Ok(std::fs::read(&p)?))),
            )
        };

        let id = AssetId(self.raw_assets.len() as u32);
        self.raw_assets.push(RawAssetInput {
            name: vfs_path.to_owned(),
            kind: AssetKind::Image,
            codec,
            data,
            source_fingerprint: fp,
        });
        self.asset_by_vfs.insert(vfs_path.to_owned(), id);
        Some(id)
    }

    fn register_audio_asset(&mut self, raw: &str) -> Option<AssetId> {
        let vfs = self.find_vfs_file(raw, &["サウンド"], &["ogg", "wav", "mp3"])?;
        let id = self.register_vfs_audio(&vfs)?;
        if self.first_title_bgm.is_none()
            && (vfs.contains("bgm") || raw.to_ascii_lowercase().contains("bgm"))
        {
            self.first_title_bgm = Some(id);
        }
        Some(id)
    }

    fn register_vfs_audio(&mut self, vfs_path: &str) -> Option<AssetId> {
        if let Some(&id) = self.asset_by_vfs.get(vfs_path) {
            return Some(id);
        }
        let real_path: PathBuf = self.archive.real_path(vfs_path)?.to_path_buf();
        let fp = file_fingerprint(&real_path);
        let codec = if vfs_path.ends_with(".ogg") {
            MediaCodec::Ogg
        } else if vfs_path.ends_with(".mp3") {
            MediaCodec::Mp3
        } else {
            MediaCodec::Wav
        };
        let p = real_path;
        let id = AssetId(self.raw_assets.len() as u32);
        self.raw_assets.push(RawAssetInput {
            name: vfs_path.to_owned(),
            kind: AssetKind::Audio,
            codec,
            data: RawAssetData::Lazy(Arc::new(move || Ok(std::fs::read(&p)?))),
            source_fingerprint: fp,
        });
        self.asset_by_vfs.insert(vfs_path.to_owned(), id);
        Some(id)
    }

    fn register_font_asset(&mut self, vfs_path: &str) -> Option<AssetId> {
        if let Some(&id) = self.asset_by_vfs.get(vfs_path) {
            return Some(id);
        }
        let real_path = self.archive.real_path(vfs_path)?.to_path_buf();
        let fp = file_fingerprint(&real_path);
        let codec = if vfs_path.ends_with(".otf") {
            MediaCodec::Otf
        } else {
            MediaCodec::Ttf
        };
        let id = AssetId(self.raw_assets.len() as u32);
        self.raw_assets.push(RawAssetInput {
            name: vfs_path.to_owned(),
            kind: AssetKind::Font,
            codec,
            data: RawAssetData::Lazy(Arc::new(move || Ok(std::fs::read(&real_path)?))),
            source_fingerprint: fp,
        });
        self.asset_by_vfs.insert(vfs_path.to_owned(), id);
        Some(id)
    }

    fn insert_lookup_keys(&mut self, vfs_path: &str, id: AssetId) {
        let lower = vfs_path.to_lowercase();
        self.asset_lookup.entry(lower.clone()).or_insert(id);
        if let Some(rel) = lower
            .strip_prefix("グラフィック/")
            .or_else(|| lower.strip_prefix("サウンド/"))
        {
            self.asset_lookup.entry(rel.to_owned()).or_insert(id);
        }
    }

    fn register_timeline_asset(&mut self, raw: &str) -> Option<TimelineId> {
        let vfs = self.find_vfs_file(raw, &["グラフィック"], &["lcm", "lmt"])?;
        if let Some(&id) = self.timeline_by_vfs.get(&vfs) {
            return Some(id);
        }
        let bytes = self.archive.read_if_exists(&vfs).ok().flatten()?;
        let cin = cinema::parse(&bytes).ok()?;
        let base_dir = vfs.rsplit_once('/').map(|(d, _)| d).unwrap_or("");

        let mut clips = Vec::new();
        let mut max_end = 0u32;
        for clip in &cin.clips {
            match clip {
                Clip::Image(ic) => {
                    let media_path = if base_dir.is_empty() {
                        normalize_vfs_path(&ic.media)
                    } else {
                        normalize_vfs_path(&format!("{base_dir}/{}", ic.media))
                    };
                    let Some(asset) = self
                        .register_vfs_image(&media_path)
                        .or_else(|| self.register_image_asset(&ic.media))
                    else {
                        continue;
                    };
                    max_end = max_end.max(ic.start.saturating_add(ic.duration));
                    clips.push(TimelineClip {
                        start_frame: ic.start,
                        duration_frames: ic.duration,
                        z_order: ic.order,
                        kind: TimelineClipKind::Image {
                            asset,
                            pivot: [ic.pivot[0] as f32, ic.pivot[1] as f32],
                            pos_start: [ic.position_start[0] as f32, ic.position_start[1] as f32],
                            pos_end: [ic.position_end[0] as f32, ic.position_end[1] as f32],
                            pos_easing: [
                                lm_curve(ic.position_curve[0]),
                                lm_curve(ic.position_curve[1]),
                            ],
                            angle: [ic.angle[0] as f32, ic.angle[1] as f32],
                            angle_easing: lm_curve(ic.angle_curve),
                            scale_x: [ic.scale_x[0] as f32, ic.scale_x[1] as f32],
                            scale_y: [ic.scale_y[0] as f32, ic.scale_y[1] as f32],
                            scale_easing: [
                                lm_curve(ic.scale_curve[0]),
                                lm_curve(ic.scale_curve[1]),
                            ],
                            opacity: [
                                (ic.opacity[0] as f32 / 255.0).clamp(0.0, 1.0),
                                (ic.opacity[1] as f32 / 255.0).clamp(0.0, 1.0),
                            ],
                            clip_start: None,
                            clip_end: None,
                        },
                    });
                }
                Clip::Other(oc) => {
                    max_end = max_end.max(oc.start.saturating_add(oc.duration));
                    match &oc.fields {
                        OtherFields::Sound { media, values, .. } => {
                            let media_path = if base_dir.is_empty() {
                                normalize_vfs_path(media)
                            } else {
                                normalize_vfs_path(&format!("{base_dir}/{media}"))
                            };
                            if let Some(asset) = self
                                .register_vfs_audio(&media_path)
                                .or_else(|| self.register_audio_asset(media))
                            {
                                let gain = (values[0] as f32 / 1000.0).clamp(0.0, 1.0);
                                clips.push(TimelineClip {
                                    start_frame: oc.start,
                                    duration_frames: oc.duration,
                                    z_order: oc.order,
                                    kind: TimelineClipKind::Sound {
                                        asset,
                                        gain: if gain > 0.0 { gain } else { 1.0 },
                                        bus: AudioBus::Sfx,
                                    },
                                });
                            }
                        }
                        OtherFields::Box { points, values, .. } => {
                            let c_raw = values[0];
                            let r = (c_raw & 0xff) as f32 / 255.0;
                            let g = ((c_raw >> 8) & 0xff) as f32 / 255.0;
                            let b = ((c_raw >> 16) & 0xff) as f32 / 255.0;
                            clips.push(TimelineClip {
                                start_frame: oc.start,
                                duration_frames: oc.duration,
                                z_order: oc.order,
                                kind: TimelineClipKind::Box {
                                    pos_start: [points[0][0] as f32, points[0][1] as f32],
                                    pos_end: [points[1][0] as f32, points[1][1] as f32],
                                    size_start: [values[1].max(1) as f32, values[2].max(1) as f32],
                                    size_end: [values[3].max(1) as f32, values[4].max(1) as f32],
                                    color: [r, g, b, 1.0],
                                    opacity: [1.0, 1.0],
                                },
                            });
                        }
                    }
                }
            }
        }

        let id = TimelineId(self.timelines.len() as u32);
        self.timelines.push(TimelineDef {
            fps: 60.0,
            size: cin.size,
            duration_frames: max_end.max(1),
            clips,
        });
        self.timeline_by_vfs.insert(vfs, id);
        Some(id)
    }

    fn register_menu_lpm(&mut self, raw: &str) -> Option<MenuId> {
        let vfs = self.find_vfs_file(raw, &["グラフィック"], &["lpm"])?;
        if let Some(&id) = self.menu_by_vfs.get(&vfs) {
            return Some(id);
        }
        let bytes = self.archive.read_if_exists(&vfs).ok().flatten()?;
        let parsed = lpm::parse_lpm(&bytes, &vfs).ok()?;

        let background = parsed.background.as_deref().and_then(|p| {
            self.register_vfs_image(p)
                .or_else(|| self.register_image_asset(p))
        });

        let mut items = Vec::with_capacity(parsed.buttons.len());
        for btn in &parsed.buttons {
            let image = if !btn.image.is_empty() {
                self.register_vfs_image(&btn.image)
                    .or_else(|| self.register_image_asset(&btn.image))
            } else {
                None
            };
            let hover_image = btn.hover_image.as_deref().and_then(|p| {
                self.register_vfs_image(p)
                    .or_else(|| self.register_image_asset(p))
            });
            let disabled_image = btn.disabled_image.as_deref().and_then(|p| {
                self.register_vfs_image(p)
                    .or_else(|| self.register_image_asset(p))
            });
            let preview_image = btn.preview_image.as_deref().and_then(|p| {
                self.register_vfs_image(p)
                    .or_else(|| self.register_image_asset(p))
            });

            // Probe button image dimensions from header ONLY (fast!)
            let (bw, bh) = layout::probe_image_size(&self.archive, &btn.image)
                .or_else(|| {
                    btn.hover_image
                        .as_deref()
                        .and_then(|p| layout::probe_image_size(&self.archive, p))
                })
                .unwrap_or((220, 56));

            let name_sid = self.intern_str(&btn.name);
            items.push(MenuItemDef {
                name: name_sid,
                label: name_sid,
                rect: [btn.left as f32, btn.top as f32, bw as f32, bh as f32],
                image,
                hover_image,
                disabled_image,
                preview_image,
                preview_pos: [btn.preview_pos[0] as f32, btn.preview_pos[1] as f32],
            });
        }

        let id = MenuId(self.menus.len() as u32);
        self.menus.push(MenuTemplate {
            size: [parsed.width, parsed.height],
            background,
            items,
        });
        self.menu_by_vfs.insert(vfs, id);
        Some(id)
    }

    fn parse_replay_gallery(&mut self, dispatch_script: Option<&Script>) -> Vec<ReplayEntry> {
        let Some(script) = dispatch_script else {
            return Vec::new();
        };
        // Map ordinal -> Call(page, line) from `シーン回想.lsb`
        let mut dispatch: BTreeMap<usize, (String, u32)> = BTreeMap::new();
        let cmds: Vec<&Command> = script
            .commands
            .iter()
            .filter(|c| !c.muted && c.kind != 27)
            .collect();
        let mut cur_ordinal: Option<usize> = None;
        for cmd in cmds {
            match &cmd.body {
                Body::Condition(expr) if matches!(cmd.kind, 0 | 1) => {
                    // Find integer ordinal compared against Int(@ParamStr[0])
                    cur_ordinal = expr.operations.iter().find_map(|(op, _, args)| {
                        if *op == 12 {
                            args.iter().find_map(|a| match a {
                                Literal::Int(n) if *n >= 0 => Some(*n as usize),
                                _ => None,
                            })
                        } else {
                            None
                        }
                    });
                }
                Body::Call { target, .. } => {
                    if let Some(ord) = cur_ordinal.take() {
                        let page = normalize_vfs_path(&target.page);
                        if !page.is_empty() {
                            dispatch.insert(ord, (page, target.line));
                        }
                    }
                }
                _ => {}
            }
        }
        if dispatch.is_empty() {
            return Vec::new();
        }

        // Read `グラフィック/シーン回想/シーン回想.TXT` for thumbs & replay flag IDs
        let mut entries = Vec::new();
        let txt_path = self
            .all_files
            .iter()
            .find(|f| f.ends_with("シーン回想.txt"))
            .cloned();
        if let Some(tp) = txt_path {
            if let Ok(Some(bytes)) = self.archive.read_if_exists(&tp) {
                let (decoded, _) = decode_text_bytes(&bytes);
                for (ord, line) in decoded
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .enumerate()
                {
                    let Some((target_page, target_line)) = dispatch.get(&ord).cloned() else {
                        continue;
                    };
                    let (thumb_raw, replay_id) = match line.rsplit_once('/') {
                        Some((p, id_str)) => {
                            (p.trim(), id_str.trim().parse::<u32>().unwrap_or(ord as u32))
                        }
                        None => (line, ord as u32),
                    };
                    let thumb = self.register_image_asset(thumb_raw);
                    entries.push(ReplayEntry {
                        replay_id,
                        thumb,
                        target_page,
                        target_line,
                    });
                }
            }
        }

        if entries.is_empty() {
            for (ord, (target_page, target_line)) in dispatch {
                entries.push(ReplayEntry {
                    replay_id: ord as u32,
                    thumb: None,
                    target_page,
                    target_line,
                });
            }
        }
        entries
    }

    fn parse_cg_gallery(&mut self) -> Vec<GalleryItem> {
        let Some(txt_path) = self
            .all_files
            .iter()
            .find(|f| f.ends_with("cgモード.txt"))
            .cloned()
        else {
            return Vec::new();
        };
        let Ok(Some(bytes)) = self.archive.read_if_exists(&txt_path) else {
            return Vec::new();
        };
        let (decoded, _) = decode_text_bytes(&bytes);
        let mut cg_paths: Vec<String> = Vec::new();
        for line in decoded.lines().map(str::trim) {
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            // Some lines have `/flag` or `,` suffixes; take the path segment before `/` if not path sep
            let raw = line.split(',').next().unwrap_or(line).trim();
            if !raw.is_empty() {
                cg_paths.push(raw.to_owned());
            }
        }
        let mut items = Vec::new();
        // Register up to 512 CG gallery thumbnails (grouping variants by stem prefix keeps gallery crisp!)
        let mut groups: BTreeMap<String, Vec<AssetId>> = BTreeMap::new();
        let mut order: Vec<String> = Vec::new();
        for raw in cg_paths {
            let Some(asset_id) = self.register_image_asset(&raw) else {
                continue;
            };
            let norm = normalize_vfs_path(&raw);
            let stem = norm
                .rsplit('/')
                .next()
                .unwrap_or(&norm)
                .split('.')
                .next()
                .unwrap_or(&norm);
            let group_key = stem.split(['-', '_']).next().unwrap_or(stem).to_owned();
            let flag = self.intern_str(&format!("cg_{group_key}"));
            self.cg_unlock_by_asset.insert(asset_id, flag);
            if !groups.contains_key(&group_key) {
                order.push(group_key.clone());
            }
            groups.entry(group_key).or_default().push(asset_id);
        }

        for (idx, gk) in order.into_iter().take(256).enumerate() {
            let assets = groups.remove(&gk).unwrap_or_default();
            let thumb = assets.first().copied();
            let id_sid = self.intern_str(&format!("cg_{gk}"));
            let title_sid = self.intern_str(&format!("CG {:02}", idx + 1));
            items.push(GalleryItem {
                id: id_sid,
                title: title_sid,
                thumb,
                unlock_flag: Some(id_sid),
                replay_func: None,
                cg_assets: assets,
            });
        }
        items
    }

    fn populate_dynamic_assets(
        &mut self,
        is_livenovel: bool,
        author_scripts: &BTreeMap<String, Script>,
    ) {
        if self.dynamic_image_vars.is_empty() && self.dynamic_audio_vars.is_empty() {
            return;
        }
        let img_filter = analyze_dynamic_var_prefixes(&self.dynamic_image_vars, author_scripts);
        let aud_filter = analyze_dynamic_var_prefixes(&self.dynamic_audio_vars, author_scripts);

        let files = self.all_files.clone();
        for f in files {
            if is_livenovel && f.starts_with("ノベルシステム/") {
                continue;
            }
            if !self.dynamic_image_vars.is_empty()
                && (f.ends_with(".gal")
                    || f.ends_with(".png")
                    || f.ends_with(".jpg")
                    || f.ends_with(".jpeg"))
                && img_filter.matches(&f, "グラフィック/")
            {
                if let Some(id) = self.register_vfs_image(&f) {
                    self.insert_lookup_keys(&f, id);
                    self.keep_assets.insert(id);
                }
            } else if !self.dynamic_audio_vars.is_empty()
                && (f.ends_with(".ogg") || f.ends_with(".wav") || f.ends_with(".mp3"))
                && aud_filter.matches(&f, "サウンド/")
            {
                if let Some(id) = self.register_vfs_audio(&f) {
                    self.insert_lookup_keys(&f, id);
                    self.keep_assets.insert(id);
                }
            }
        }
    }

    fn compile_page(&mut self, page: &str, script: &Script) {
        self.temp_locals.clear();
        let file_sid = self.intern_str(page);
        let entry_lbl_sid = self.intern_str(&format!("page_{}", sanitize_ident(page)));
        self.labels.push(LabelMeta {
            name: entry_lbl_sid,
            pc: self.ops.len() as u32,
            file: file_sid,
            line: 0,
        });
        self.compile_block(page, 0, &script.commands);
        // Trailing return so `Call`ed pages return cleanly and main page terminates cleanly.
        self.ops.push(Op::Ret { has_value: false });
    }

    fn record_line_pc(&mut self, page: &str, cmd_idx: u32) {
        let pc = self.ops.len() as u32;
        self.line_pcs
            .entry((page.to_owned(), cmd_idx))
            .or_insert(pc);
        self.page_lines
            .entry(page.to_owned())
            .or_default()
            .push((cmd_idx, pc));
    }

    fn compile_block(&mut self, page: &str, base_idx: usize, cmds: &[Command]) {
        let mut i = 0usize;
        while i < cmds.len() {
            let cmd = &cmds[i];
            let abs_idx = (base_idx + i) as u32;
            self.record_line_pc(page, abs_idx);
            if cmd.muted {
                i += 1;
                continue;
            }
            self.current_loc = format!("{page}:{}", cmd.line);

            // Handle structured `If` / `Elseif`* / `Else`? chain via LiveMaker indentation
            if cmd.kind == 0 {
                let base_indent = cmd.indent;
                let mut arms: Vec<(Option<&Expression>, u32, usize, &[Command])> = Vec::new();
                let mut cursor = i;
                while cursor < cmds.len() {
                    let head = &cmds[cursor];
                    if head.indent != base_indent {
                        break;
                    }
                    let is_first = cursor == i;
                    let cond_opt = match (is_first, head.kind, &head.body) {
                        (true, 0, Body::Condition(e)) => Some(Some(e)),
                        (false, 1, Body::Condition(e)) => Some(Some(e)),
                        (false, 2, _) => Some(None),
                        _ => None,
                    };
                    let Some(cond) = cond_opt else {
                        break;
                    };
                    let arm_idx = (base_idx + cursor) as u32;
                    let body_start = cursor + 1;
                    let mut body_end = body_start;
                    while body_end < cmds.len() && cmds[body_end].indent > base_indent {
                        body_end += 1;
                    }
                    arms.push((
                        cond,
                        arm_idx,
                        base_idx + body_start,
                        &cmds[body_start..body_end],
                    ));
                    cursor = body_end;
                    if cond.is_none() {
                        break;
                    }
                }

                let mut end_jumps = Vec::new();
                let arm_count = arms.len();
                for (arm_idx, (cond_opt, arm_cmd_idx, body_base, body)) in
                    arms.into_iter().enumerate()
                {
                    self.record_line_pc(page, arm_cmd_idx);
                    let jfalse_idx = if let Some(cond_expr) = cond_opt {
                        let mut expr_ops = Vec::new();
                        expr::compile_value_expr(cond_expr, self, &mut expr_ops);
                        self.ops.extend(expr_ops);
                        let idx = self.ops.len();
                        self.ops.push(Op::JumpIfFalse(0));
                        Some(idx)
                    } else {
                        None
                    };
                    self.compile_block(page, body_base, body);
                    if arm_idx + 1 < arm_count {
                        let jmp_end = self.ops.len();
                        self.ops.push(Op::Jump(0));
                        end_jumps.push(jmp_end);
                    }
                    if let Some(jf) = jfalse_idx {
                        let next_pc = self.ops.len() as u32;
                        self.ops[jf] = Op::JumpIfFalse(next_pc);
                    }
                }
                let after_pc = self.ops.len() as u32;
                for je in end_jumps {
                    self.ops[je] = Op::Jump(after_pc);
                }
                self.report.record_exact();
                i = cursor;
                continue;
            }

            // Orphan Elseif/Else (defensive)
            if matches!(cmd.kind, 1 | 2) {
                i += 1;
                continue;
            }

            // WhileInit / While / WhileLoop structured loops (rare in non-RPG, supported cleanly)
            if cmd.kind == 31 {
                if let Body::LoopCondition { condition, target } = &cmd.body {
                    let mut expr_ops = Vec::new();
                    expr::compile_value_expr(condition, self, &mut expr_ops);
                    self.ops.extend(expr_ops);
                    let jf = self.ops.len();
                    self.ops.push(Op::JumpIfFalse(0));
                    self.jump_patches.push((jf, page.to_owned(), *target));
                    self.report.record_exact();
                }
                i += 1;
                continue;
            }
            if matches!(cmd.kind, 32 | 33) {
                if let Body::LoopUpdate { expression, target } = &cmd.body {
                    let mut expr_ops = Vec::new();
                    expr::compile_calc_stmt(expression, self, &mut expr_ops);
                    self.ops.extend(expr_ops);
                    if let Some(tgt) = target {
                        let jmp = self.ops.len();
                        self.ops.push(Op::Jump(0));
                        self.jump_patches.push((jmp, page.to_owned(), *tgt));
                    }
                    self.report.record_exact();
                }
                i += 1;
                continue;
            }

            self.compile_command(page, cmd);
            i += 1;
        }
    }

    fn compile_command(&mut self, page: &str, cmd: &Command) {
        match &cmd.body {
            Body::Label(name) => {
                let name_sid = self.intern_str(&format!("{page}::{name}"));
                let file_sid = self.intern_str(page);
                self.labels.push(LabelMeta {
                    name: name_sid,
                    pc: self.ops.len() as u32,
                    file: file_sid,
                    line: cmd.line,
                });
                self.report.record_exact();
            }
            Body::Jump(target, cond) => {
                if is_const_false(cond) {
                    self.report.record_exact();
                    return;
                }
                let tgt_page = resolve_ref_page(page, target);
                let jf = if is_const_true(cond) {
                    None
                } else {
                    let mut expr_ops = Vec::new();
                    expr::compile_value_expr(cond, self, &mut expr_ops);
                    self.ops.extend(expr_ops);
                    let idx = self.ops.len();
                    self.ops.push(Op::JumpIfFalse(0));
                    Some(idx)
                };
                let jmp = self.ops.len();
                self.ops.push(Op::Jump(0));
                self.jump_patches.push((jmp, tgt_page, target.line));
                if let Some(jf_idx) = jf {
                    let after = self.ops.len() as u32;
                    self.ops[jf_idx] = Op::JumpIfFalse(after);
                }
                self.report.record_exact();
            }
            Body::Call {
                target,
                condition,
                params,
                ..
            } => {
                if is_const_false(condition) {
                    self.report.record_exact();
                    return;
                }
                let tgt_page = resolve_ref_page(page, target);
                let jf = if is_const_true(condition) {
                    None
                } else {
                    let mut expr_ops = Vec::new();
                    expr::compile_value_expr(condition, self, &mut expr_ops);
                    self.ops.extend(expr_ops);
                    let idx = self.ops.len();
                    self.ops.push(Op::JumpIfFalse(0));
                    Some(idx)
                };

                if !self.try_lower_abi_call(&tgt_page, target.line, params) {
                    // Normal author subroutine call: pack params into a `Value::List` as `@ParamStr`
                    self.ops.push(Op::PushNull);
                    for p in params {
                        let mut p_ops = Vec::new();
                        expr::compile_value_expr(p, self, &mut p_ops);
                        self.ops.extend(p_ops);
                        self.ops.push(Op::CallBuiltin {
                            func: BuiltinFn::ListPush,
                            argc: 2,
                        });
                    }
                    let fid = self.ensure_call_func(&tgt_page, target.line);
                    self.ops.push(Op::Call { func: fid, argc: 1 });
                    self.report.record_exact();
                }

                if let Some(jf_idx) = jf {
                    let after = self.ops.len() as u32;
                    self.ops[jf_idx] = Op::JumpIfFalse(after);
                }
            }
            Body::Exit(cond) => {
                if is_const_false(cond) {
                    self.report.record_exact();
                    return;
                }
                let jf = if is_const_true(cond) {
                    None
                } else {
                    let mut expr_ops = Vec::new();
                    expr::compile_value_expr(cond, self, &mut expr_ops);
                    self.ops.extend(expr_ops);
                    let idx = self.ops.len();
                    self.ops.push(Op::JumpIfFalse(0));
                    Some(idx)
                };
                self.ops.push(Op::Ret { has_value: false });
                if let Some(jf_idx) = jf {
                    let after = self.ops.len() as u32;
                    self.ops[jf_idx] = Op::JumpIfFalse(after);
                }
                self.report.record_exact();
            }
            Body::Variable {
                name,
                value_type,
                initial,
                scope,
            } => {
                let default = match value_type {
                    4 => Value::Str(String::new()),
                    2 => Value::Float(0.0),
                    _ => Value::Int(0),
                };
                let vid = self.ensure_var(name, default);
                // For `scope == 3` persistent profile vars, preserve persisted value if already loaded!
                if *scope != 3 {
                    if !initial.operations.is_empty() || initial.literal.is_some() {
                        let mut init_ops = Vec::new();
                        expr::compile_value_expr(initial, self, &mut init_ops);
                        self.ops.extend(init_ops);
                        self.ops.push(Op::StoreVar(vid));
                    } else {
                        match value_type {
                            4 => {
                                let empty = self.intern_str("");
                                self.ops.push(Op::PushStr(empty));
                            }
                            2 => self.ops.push(Op::PushFloat(0.0)),
                            _ => self.ops.push(Op::PushInt(0)),
                        }
                        self.ops.push(Op::StoreVar(vid));
                    }
                }
                self.report.record_exact();
            }
            Body::VariableDelete(name) => {
                // Reset block temporary (`_tmp`, `_tmp2`, `_tmp3`, `_tmpid`) to empty string/null
                if let Some(&vid) = self.var_map.get(name) {
                    if !self.profile_vars.contains_key(name) {
                        let empty = self.intern_str("");
                        self.ops.push(Op::PushStr(empty));
                        self.ops.push(Op::StoreVar(vid));
                    }
                }
                self.report.record_exact();
            }
            Body::Calc(expr) => {
                let mut expr_ops = Vec::new();
                expr::compile_calc_stmt(expr, self, &mut expr_ops);
                self.ops.extend(expr_ops);
                self.report.record_exact();
            }
            Body::Wait(args) => {
                // LiveMaker Wait: [0:condition, 1:time_ms, 2:click_flag]
                // - time_ms > 0: timed wait
                // - cond == const_true && time_ms == 0 && click == 0: flowchart node terminator (stops fallthrough!)
                // - dynamic cond (e.g. GetProp(msg, 138)) with time_ms == 0: message-done guard (no-op)
                let ms = args.get(1).and_then(literal_i32).unwrap_or(0);
                let click = args.get(2).and_then(literal_i32).unwrap_or(0);
                if ms > 0 {
                    self.ops.push(Op::Wait {
                        duration_us: (ms as u64).min(60_000) * 1000,
                        skippable: click != 0,
                    });
                } else if args.first().is_some_and(is_const_true) && click == 0 {
                    self.ops.push(Op::Ret { has_value: false });
                }
                self.report.record_exact();
            }
            Body::Delete(target_expr) => {
                if let Some(name) = literal_string(target_expr) {
                    if let Some(&(ch, _)) = self.channel_map.get(&name.to_ascii_uppercase()) {
                        self.ops.push(Op::AudioStop {
                            channel: ch,
                            fade_out_us: 0,
                        });
                    } else if !matches!(
                        name,
                        "メッセージボックス"
                            | "メッセージボックス土台"
                            | "メッセージボックスコントロール"
                    ) {
                        let lid = self.resolve_layer(name);
                        self.ops.push(Op::LayerRemove { layer: lid });
                    }
                }
                self.report.record_exact();
            }
            Body::Text { text, .. } => {
                self.lower_text_ins(text);
            }
            Body::Object(props) if matches!(cmd.name(), "Sound" | "MediaPlay") => {
                let name = props.get(&1).and_then(literal_string).unwrap_or("SE");
                let src = props.get(&3).and_then(literal_string);
                let vol = props
                    .get(&29)
                    .and_then(literal_i32)
                    .map(|v| (v as f32 / 1000.0).clamp(0.0, 2.0))
                    .unwrap_or(1.0);
                let looped = props.get(&30).and_then(literal_i32).unwrap_or(0) != 0;
                if let Some(src_path) = src {
                    let (channel, bus) = self.resolve_channel(name);
                    if let Some(asset) = self.register_audio_asset(src_path) {
                        self.ops.push(Op::AudioPlay {
                            channel,
                            bus,
                            asset,
                            gain: vol,
                            looped,
                            fade_in_us: 0,
                            loop_region: None,
                        });
                        self.report.record_exact();
                    } else {
                        self.report.record_adapted(
                            &self.current_loc,
                            format!("direct Sound `{src_path}` not found"),
                        );
                    }
                } else {
                    self.report.record_exact();
                }
            }
            Body::Object(props) if matches!(cmd.name(), "ImgNew" | "TileNew" | "MapImgNew") => {
                let name = props.get(&1).and_then(literal_string).unwrap_or("img");
                let src = props.get(&3).and_then(literal_string);
                let x = props.get(&4).and_then(literal_i32).unwrap_or(0) as f32;
                let y = props.get(&5).and_then(literal_i32).unwrap_or(0) as f32;
                let z = props.get(&13).and_then(literal_i32).unwrap_or(0);
                if let Some(src_path) = src {
                    if let Some(asset) = self.register_image_asset(src_path) {
                        let lid = self.resolve_layer(name);
                        self.ops.push(Op::LayerSet {
                            layer: lid,
                            parent: None,
                            asset: Some(asset),
                            transform: Box::new(Transform2D {
                                x,
                                y,
                                z_order: z,
                                ..Transform2D::default()
                            }),
                        });
                        self.report.record_exact();
                        return;
                    }
                }
                self.report.record_adapted(
                    &self.current_loc,
                    format!("{} without resolvable image", cmd.name()),
                );
            }
            Body::Flip { parameters, .. } => {
                let dur_ms = parameters
                    .get("time")
                    .and_then(literal_i32)
                    .unwrap_or(0)
                    .clamp(0, 30_000) as u64;
                let wipe = parameters.get("wipe").and_then(literal_i32).unwrap_or(0);
                self.ops.push(Op::StageCommit {
                    transition: if dur_ms == 0 {
                        Transition::Cut
                    } else if wipe == 2 {
                        Transition::Wipe {
                            dir: WipeDir::LeftToRight,
                            softness: 0.15,
                        }
                    } else {
                        Transition::Dissolve
                    },
                    duration_us: dur_ms * 1000,
                    wait: dur_ms > 0,
                });
                self.report.record_exact();
            }
            Body::Other if cmd.name() == "Terminate" => {
                let end = self.intern_str("end");
                self.ops.push(Op::End { outcome: end });
                self.report.record_exact();
            }
            Body::Other
                if matches!(
                    cmd.name(),
                    "Comment" | "ClrHist" | "ClrRead" | "DoEvent" | "TextClr" | "MenuClose"
                ) =>
            {
                self.report.record_exact();
            }
            Body::HistoryFormat { .. } | Body::SetProperty { .. } => {
                self.report.record_exact();
            }
            _ => {
                self.report.record_adapted(
                    &self.current_loc,
                    format!("command `{}` adapted/omitted", cmd.name()),
                );
            }
        }
    }

    /// Intercepts the 12 LiveNovel `ノベルシステム/` ABI calls cleanly at the boundary.
    /// Returns `true` if handled as an ABI shim.
    fn try_lower_abi_call(&mut self, tgt_page: &str, _line: u32, params: &[Expression]) -> bool {
        if tgt_page == "メッセージボックス作成.lsb" {
            let style_name = params.first().and_then(literal_string).unwrap_or("(標準)");
            let idx = self
                .style_index_by_name
                .get(style_name)
                .copied()
                .unwrap_or(0);
            self.ops.push(Op::SetDialogueStyle(idx));
            self.report.record_exact();
            return true;
        }
        if tgt_page == "メッセージボックス座標.lsb" {
            self.report.record_exact();
            return true;
        }
        if !tgt_page.starts_with("ノベルシステム/") {
            return false;
        }

        if tgt_page.ends_with("メッセージボックス/終了待ち.lsb")
            || tgt_page.ends_with("■初期化.lsb")
        {
            self.report.record_exact();
            return true;
        }
        if tgt_page.ends_with("メッセージボックス/終了.lsb") {
            self.ops.push(Op::SetDialoguePortrait(None));
            self.ops.push(Op::TextWindow {
                visible: false,
                transition: Transition::Cut,
                duration_us: 0,
            });
            self.report.record_exact();
            return true;
        }
        if tgt_page.ends_with("メッセージボックス/プレビューメニュー処理.lsb") {
            self.report
                .record_adapted(&self.current_loc, "message box toolbar handled by NIR HUD");
            return true;
        }
        if tgt_page.ends_with("シーン回想/■フラグon.lsb") {
            if let Some(id) = params.first().and_then(literal_i32) {
                let flag = self.intern_str(&format!("replay_{id}"));
                self.ops.push(Op::ProfileSetFlag(flag));
                self.report.record_exact();
            } else {
                self.report
                    .record_adapted(&self.current_loc, "dynamic replay unlock ID omitted");
            }
            return true;
        }
        if tgt_page.ends_with("シーン回想/■開始.lsb") {
            self.ops.push(Op::OpenModal(ModalKind::Replay));
            self.report.record_exact();
            return true;
        }
        if tgt_page.ends_with("cgモード/■開始.lsb") {
            self.ops.push(Op::OpenModal(ModalKind::Gallery));
            self.report.record_exact();
            return true;
        }
        if tgt_page.ends_with("■ロード.lsb") {
            self.ops.push(Op::OpenModal(ModalKind::Load));
            self.report.record_exact();
            return true;
        }
        if tgt_page.ends_with("選択メニュー/■選択実行.lsb") {
            // Param[0]: labels (CRLF string or ArrayToString(_tmp))
            // Param[1]: cancelable (0/1)
            // Param[4]: timeout_ms (or .lcm)
            // Param[6]: click_se
            if let Some(p0) = params.first() {
                let mut ops0 = Vec::new();
                expr::compile_value_expr(p0, self, &mut ops0);
                self.ops.extend(ops0);
            } else {
                self.ops.push(Op::PushNull);
            }
            // Push `_tmpid` if present in scope, else PushNull
            if let Some(&tmpid_var) = self.var_map.get("_tmpid") {
                self.ops.push(Op::LoadVar(tmpid_var));
            } else {
                self.ops.push(Op::PushNull);
            }
            let cancelable = params.get(1).and_then(literal_i32).unwrap_or(0) != 0;
            let timeout_us = params
                .get(4)
                .and_then(literal_i32)
                .filter(|&ms| ms > 0)
                .map(|ms| (ms as u64) * 1000);
            let click_se = params
                .get(6)
                .and_then(literal_string)
                .filter(|s| !s.is_empty())
                .and_then(|s| self.register_audio_asset(s));

            let res_var = self.ensure_var("選択値", Value::Str(String::new()));
            let idx_var = self.ensure_var("選択番号", Value::Int(-1));
            self.ops.push(Op::ChoiceList {
                result_var: Some(res_var),
                index_var: Some(idx_var),
                cancelable,
                click_se,
                timeout_us,
            });
            self.emit_choice_last_sync(res_var, idx_var);
            self.report.record_exact();
            return true;
        }
        if tgt_page.ends_with("プレビューメニュー/■選択実行.lsb") {
            // Param[1]: lpm path
            // Param[2]: fade_in_ms, Param[3]: fade_out_ms
            // Param[4]: hover_se, Param[5]: click_se
            // Param[6]: cancelable (0/1)
            // Param[7]: _tmp (disabled button names!)
            // Param[8]: _tmp2 (hidden button names!)
            // Param[9]: timeout_ms
            let lpm_raw = params.get(1).and_then(literal_string).unwrap_or("");
            let res_var = self.ensure_var("選択値", Value::Str(String::new()));
            let idx_var = self.ensure_var("選択番号", Value::Int(-1));
            let Some(vfs_lpm) = self.find_vfs_file(lpm_raw, &["グラフィック"], &["lpm"])
            else {
                // Missing LPM file in source archive (commonly an unlinked flowchart node).
                let empty = self.intern_str("");
                self.ops.push(Op::PushStr(empty));
                self.ops.push(Op::StoreVar(res_var));
                self.ops.push(Op::PushInt(-1));
                self.ops.push(Op::StoreVar(idx_var));
                self.emit_choice_last_sync(res_var, idx_var);
                self.report.record_adapted(
                    &self.current_loc,
                    format!("LPM preview menu `{lpm_raw}` missing from source archive"),
                );
                return true;
            };
            let Some(menu_id) = self.register_menu_lpm(&vfs_lpm) else {
                self.report.record_fallback(
                    &self.current_loc,
                    format!("LPM preview menu `{lpm_raw}` could not be parsed"),
                );
                return true;
            };
            let fade_in_us = params
                .get(2)
                .and_then(literal_i32)
                .unwrap_or(0)
                .clamp(0, 10_000) as u64
                * 1000;
            let fade_out_us = params
                .get(3)
                .and_then(literal_i32)
                .unwrap_or(0)
                .clamp(0, 10_000) as u64
                * 1000;
            let hover_se = params
                .get(4)
                .and_then(literal_string)
                .filter(|s| !s.is_empty())
                .and_then(|s| self.register_audio_asset(s));
            let click_se = params
                .get(5)
                .and_then(literal_string)
                .filter(|s| !s.is_empty())
                .and_then(|s| self.register_audio_asset(s));
            let cancelable = params.get(6).and_then(literal_i32).unwrap_or(0) != 0;
            let timeout_us = params
                .get(9)
                .and_then(literal_i32)
                .filter(|&ms| ms > 0)
                .map(|ms| (ms as u64) * 1000);

            // Stack order for Op::ChoiceMenu: [hidden_names (Param[8]), disabled_names (Param[7])]
            if let Some(p8) = params.get(8) {
                let mut ops8 = Vec::new();
                expr::compile_value_expr(p8, self, &mut ops8);
                self.ops.extend(ops8);
            } else {
                self.ops.push(Op::PushNull);
            }
            if let Some(p7) = params.get(7) {
                let mut ops7 = Vec::new();
                expr::compile_value_expr(p7, self, &mut ops7);
                self.ops.extend(ops7);
            } else {
                self.ops.push(Op::PushNull);
            }

            self.ops.push(Op::ChoiceMenu {
                menu: menu_id,
                result_var: Some(res_var),
                index_var: Some(idx_var),
                cancelable,
                hover_se,
                click_se,
                fade_in_us,
                fade_out_us,
                timeout_us,
            });
            self.emit_choice_last_sync(res_var, idx_var);
            self.report.record_exact();
            return true;
        }
        if tgt_page.ends_with("メッセージボックス/文字列入力.lsb") {
            self.report.record_adapted(
                &self.current_loc,
                "text input modal (dev/RPG helper) omitted in non-RPG mode",
            );
            return true;
        }

        // Any other unhandled `ノベルシステム/` helper call
        self.report.record_adapted(
            &self.current_loc,
            format!("system helper call `{tgt_page}` omitted at ABI boundary"),
        );
        true
    }

    fn emit_choice_last_sync(&mut self, res_var: VarId, idx_var: VarId) {
        let last_res = self.ensure_var("最終選択値", Value::Str(String::new()));
        let last_idx = self.ensure_var("最終選択番号", Value::Int(-1));
        self.ops.push(Op::LoadVar(res_var));
        self.ops.push(Op::StoreVar(last_res));
        self.ops.push(Op::LoadVar(idx_var));
        self.ops.push(Op::StoreVar(last_idx));
    }

    fn lower_text_ins(&mut self, novel: &lsb::Novel) {
        let mut cur_spans: Vec<RichSpan> = Vec::new();
        for glyph in &novel.glyphs {
            match glyph {
                Glyph::Char(ch) => push_text_span(&mut cur_spans, ch),
                Glyph::RubyChar { text, reading, .. } => {
                    cur_spans.push(RichSpan::Ruby {
                        base: text.clone(),
                        ruby: reading.clone(),
                    });
                }
                Glyph::Variable(var_name) => {
                    let vid = self.ensure_var(var_name, Value::Int(0));
                    cur_spans.push(RichSpan::Var(vid));
                }
                Glyph::Break(kind) => {
                    // TpWord Break: 0 = soft line break `\n`, non-zero = page/click break
                    if *kind == 0 {
                        push_text_span(&mut cur_spans, "\n");
                    } else {
                        self.flush_dialogue(&mut cur_spans);
                    }
                }
                Glyph::Event(raw_parts) => {
                    let Some((ev_name, ev_args)) = events::normalize_event_parts(raw_parts) else {
                        continue;
                    };
                    if ev_name == "PAUSE" {
                        cur_spans.push(RichSpan::Pause {
                            duration_us: 0,
                            click: true,
                        });
                        self.report.record_exact();
                        continue;
                    }
                    self.flush_dialogue(&mut cur_spans);
                    let mut ev_ops = Vec::new();
                    events::lower_event(&ev_name, ev_args, self, &mut ev_ops);
                    self.ops.extend(ev_ops);
                }
                Glyph::InlineImage { source, .. } => {
                    if let Some(asset) = self.register_image_asset(source) {
                        let (width, height) = self
                            .find_vfs_file(source, &["グラフィック"], &["gal", "png", "jpg", "bmp"])
                            .and_then(|p| layout::probe_image_size(&self.archive, &p))
                            .unwrap_or((24, 24));
                        cur_spans.push(RichSpan::InlineImage {
                            asset,
                            width,
                            height,
                        });
                        self.report.record_exact();
                    } else {
                        self.report.record_adapted(
                            &self.current_loc,
                            format!("inline image `{source}` omitted"),
                        );
                    }
                }
                Glyph::Unsupported => {
                    self.report
                        .record_adapted(&self.current_loc, "unsupported TpWord glyph omitted");
                }
            }
        }
        self.flush_dialogue(&mut cur_spans);
    }

    fn flush_dialogue(&mut self, spans: &mut Vec<RichSpan>) {
        if spans.is_empty() {
            return;
        }
        let taken = std::mem::take(spans);
        // Skip purely empty/whitespace-only buffers (common around standalone events)
        let has_visible = taken.iter().any(|s| match s {
            RichSpan::Text(t) | RichSpan::Emphasis(t) | RichSpan::Color { text: t, .. } => {
                !t.trim().is_empty()
            }
            RichSpan::Ruby { base, .. } => !base.trim().is_empty(),
            RichSpan::Var(_) | RichSpan::InlineImage { .. } => true,
            RichSpan::Pause { .. } | RichSpan::Speed(_) => false,
        });
        if !has_visible {
            return;
        }

        // Trim leading/trailing newlines and extract optional speaker prefix (`【Name】` or `Name\n「...`)
        let (speaker_opt, cleaned_spans) = split_speaker_and_trim(taken);
        let speaker = speaker_opt.map(|s| self.intern_str(&s));
        let text_id = TextId(self.texts.len() as u32);
        self.texts.push(TextEntry {
            spans: cleaned_spans,
        });
        self.ops.push(Op::Say {
            speaker,
            speaker_color: None,
            text: text_id,
            voice: None,
            reveal_us: 0,
            append: false,
        });
        self.report.record_exact();
    }

    fn resolve_target_pc(&self, page: &str, line: u32) -> u32 {
        if let Some(&pc) = self.line_pcs.get(&(page.to_owned(), line)) {
            return pc;
        }
        if let Some(entries) = self.page_lines.get(page) {
            // Find smallest recorded line >= target line (handles jumps pointing to omitted comments)
            if let Some(&(_, pc)) = entries.iter().find(|&&(l, _)| l >= line) {
                return pc;
            }
            if let Some(&(_, pc)) = entries.last() {
                return pc;
            }
        }
        self.ops.len().saturating_sub(1) as u32
    }

    fn backpatch_all(&mut self) {
        let patches = std::mem::take(&mut self.jump_patches);
        for (op_idx, page, line) in patches {
            let pc = self.resolve_target_pc(&page, line);
            match &mut self.ops[op_idx] {
                Op::Jump(t) | Op::JumpIfFalse(t) => *t = pc,
                _ => {}
            }
        }
        let max_locals = self.max_local_count.max(1);
        for ((page, line), fid) in self.call_func_map.clone() {
            let pc = self.resolve_target_pc(&page, line);
            if let Some(meta) = self.funcs.get_mut(fid.0 as usize) {
                meta.entry_pc = pc;
                meta.local_count = max_locals;
            }
        }
        for meta in &mut self.funcs {
            meta.local_count = max_locals;
        }
    }
}

impl ExprBridge for Compiler<'_> {
    fn stage_size(&self) -> [u32; 2] {
        self.stage
    }

    fn intern_str(&mut self, s: &str) -> StrId {
        Compiler::intern_str(self, s)
    }

    fn resolve_var(&mut self, name: &str) -> VarId {
        self.ensure_var(name, Value::Int(0))
    }

    fn var_exists(&self, name: &str) -> bool {
        self.var_map.contains_key(name)
    }

    fn profile_key(&self, name: &str) -> Option<StrId> {
        self.profile_vars.get(name).copied()
    }

    fn temp_local(&mut self, name: &str) -> u16 {
        if let Some(&slot) = self.temp_locals.get(name) {
            return slot;
        }
        // Local 0 is reserved for `@ParamStr`.
        let slot = (self.temp_locals.len() as u16) + 1;
        self.temp_locals.insert(name.to_owned(), slot);
        self.max_local_count = self.max_local_count.max(slot + 1);
        slot
    }

    fn note_adapted(&mut self, msg: String) {
        self.report.record_adapted(&self.current_loc, msg);
    }
}

impl EventBridge for Compiler<'_> {
    fn stage_size(&self) -> [u32; 2] {
        self.stage
    }

    fn intern_str(&mut self, s: &str) -> StrId {
        Compiler::intern_str(self, s)
    }

    fn resolve_var(&mut self, name: &str) -> VarId {
        self.ensure_var(name, Value::Int(0))
    }

    fn resolve_layer(&mut self, name: &str) -> LayerId {
        if let Some(&id) = self.layer_map.get(name) {
            return id;
        }
        let id = LayerId(self.layer_map.len() as u32);
        self.layer_map.insert(name.to_owned(), id);
        id
    }

    fn resolve_channel(&mut self, name: &str) -> (ChannelId, AudioBus) {
        let key = name.trim().to_ascii_uppercase();
        if let Some(&pair) = self.channel_map.get(&key) {
            return pair;
        }
        let bus = if key.contains("BGM") {
            AudioBus::Bgm
        } else if key.contains("VOICE") {
            AudioBus::Voice
        } else {
            AudioBus::Sfx
        };
        let id = ChannelId(self.next_channel_id);
        self.next_channel_id += 1;
        self.channel_map.insert(key, (id, bus));
        (id, bus)
    }

    fn register_image(&mut self, raw_path: &str) -> Option<AssetId> {
        self.register_image_asset(raw_path)
    }

    fn register_audio(&mut self, raw_path: &str) -> Option<AssetId> {
        self.register_audio_asset(raw_path)
    }

    fn register_timeline(&mut self, raw_path: &str) -> Option<TimelineId> {
        self.register_timeline_asset(raw_path)
    }

    fn cg_unlock_flag(&self, asset: AssetId) -> Option<StrId> {
        self.cg_unlock_by_asset.get(&asset).copied()
    }

    fn mark_dynamic_image_var(&mut self, var_name: &str) {
        self.dynamic_image_vars.insert(var_name.to_owned());
    }

    fn mark_dynamic_audio_var(&mut self, var_name: &str) {
        self.dynamic_audio_vars.insert(var_name.to_owned());
    }

    fn record_exact(&mut self) {
        self.report.record_exact();
    }

    fn record_adapted(&mut self, detail: String) {
        self.report.record_adapted(&self.current_loc, detail);
    }
}

enum DynamicFilter {
    All,
    Prefixes(BTreeSet<String>),
}

impl DynamicFilter {
    fn matches(&self, vfs_path: &str, default_root: &str) -> bool {
        let Self::Prefixes(prefixes) = self else {
            return true;
        };
        let low = vfs_path.to_lowercase();
        let rel = low.strip_prefix(default_root).unwrap_or(&low);
        let fname = low.rsplit('/').next().unwrap_or(&low);
        prefixes.iter().any(|pref| {
            let p_rel = pref.strip_prefix(default_root).unwrap_or(pref);
            low.starts_with(pref.as_str())
                || rel.starts_with(p_rel)
                || (!p_rel.contains('/') && fname.starts_with(p_rel))
        })
    }
}

/// Statically analyzes string prefixes assigned to every dynamic `?var` variable across all
/// author scripts. If every non-empty assignment to every variable in `vars` starts with a known
/// literal prefix (>= 2 chars), narrows dynamic registration strictly to matching files;
/// otherwise conservatively falls back to `DynamicFilter::All` so fidelity is never compromised.
fn analyze_dynamic_var_prefixes(
    vars: &BTreeSet<String>,
    author_scripts: &BTreeMap<String, Script>,
) -> DynamicFilter {
    if vars.is_empty() {
        return DynamicFilter::Prefixes(BTreeSet::new());
    }
    let mut prefixes: BTreeSet<String> = BTreeSet::new();
    let mut matched_vars: BTreeSet<String> = BTreeSet::new();

    let norm_prefix = |raw: &str| -> String { normalize_vfs_path(raw.trim()).to_lowercase() };

    let eval_expr_prefix = |expr: &Expression| -> Option<Option<String>> {
        if expr.operations.is_empty() {
            return match &expr.literal {
                Some(Literal::String(s)) => Some(Some(s.clone())),
                None => Some(Some(String::new())),
                _ => Some(None),
            };
        }
        let mut ssa: BTreeMap<String, Option<String>> = BTreeMap::new();
        let mut last_dst: Option<String> = None;
        for (op_code, dst, operands) in &expr.operations {
            let eval_lit =
                |lit: &Literal, ssa: &BTreeMap<String, Option<String>>| -> Option<String> {
                    match lit {
                        Literal::String(s) => Some(s.clone()),
                        Literal::Variable(name) => ssa.get(name).cloned().flatten(),
                        _ => None,
                    }
                };
            let val = match (*op_code, operands.as_slice()) {
                // `+` (2) or `&` string concatenation (19)
                (2 | 19, [lhs, rhs]) => match (eval_lit(lhs, &ssa), eval_lit(rhs, &ssa)) {
                    (Some(lp), Some(rp)) => Some(format!("{lp}{rp}")),
                    (Some(lp), None) if !lp.is_empty() => Some(lp),
                    _ => None,
                },
                // `=` assignment (0 | 1) or unary pass-through
                (0 | 1, [only]) => eval_lit(only, &ssa),
                _ => None,
            };
            ssa.insert(dst.clone(), val);
            last_dst = Some(dst.clone());
        }
        last_dst.map(|d| ssa.get(&d).cloned().flatten())
    };

    for script in author_scripts.values() {
        // Straight-line local prefix environment within a basic block (cleared on any branch/call/indent change).
        // 100% sound: only propagates prefixes across uninterrupted sequential assignments!
        let mut block_env: BTreeMap<String, Option<String>> = BTreeMap::new();
        let mut prev_indent: Option<u32> = None;

        for cmd in &script.commands {
            if cmd.muted {
                continue;
            }
            if prev_indent != Some(cmd.indent) {
                block_env.clear();
                prev_indent = Some(cmd.indent);
            }
            match &cmd.body {
                Body::Variable { name, initial, .. } => {
                    let pref = eval_expr_prefix(initial).flatten();
                    if vars.contains(name) {
                        if let Some(pref_opt) = eval_expr_prefix(initial) {
                            match pref_opt {
                                Some(s) if s.trim().is_empty() => {} // empty init resolves to nothing
                                Some(s) => {
                                    let p = norm_prefix(&s);
                                    if p.chars().count() < 2 {
                                        return DynamicFilter::All;
                                    }
                                    prefixes.insert(p);
                                    matched_vars.insert(name.clone());
                                }
                                None => return DynamicFilter::All,
                            }
                        }
                    }
                    block_env.insert(name.clone(), pref);
                }
                Body::Calc(expr) => {
                    let mut ssa: BTreeMap<String, Option<String>> = BTreeMap::new();
                    for (op_code, dst, operands) in &expr.operations {
                        let eval_lit = |lit: &Literal,
                                        ssa: &BTreeMap<String, Option<String>>,
                                        block_env: &BTreeMap<String, Option<String>>|
                         -> Option<String> {
                            match lit {
                                Literal::String(s) => Some(s.clone()),
                                Literal::Variable(name) => ssa
                                    .get(name)
                                    .cloned()
                                    .flatten()
                                    .or_else(|| block_env.get(name).cloned().flatten()),
                                _ => None,
                            }
                        };
                        let val = match (*op_code, operands.as_slice()) {
                            (2 | 19, [lhs, rhs]) => {
                                match (
                                    eval_lit(lhs, &ssa, &block_env),
                                    eval_lit(rhs, &ssa, &block_env),
                                ) {
                                    (Some(lp), Some(rp)) => Some(format!("{lp}{rp}")),
                                    (Some(lp), None) if !lp.is_empty() => Some(lp),
                                    _ => None,
                                }
                            }
                            (0 | 1, [only]) => eval_lit(only, &ssa, &block_env),
                            _ => None,
                        };
                        if vars.contains(dst) {
                            match &val {
                                Some(s) if s.trim().is_empty() => {} // reset to ""
                                Some(s) => {
                                    let p = norm_prefix(s);
                                    if p.chars().count() < 2 {
                                        return DynamicFilter::All;
                                    }
                                    prefixes.insert(p);
                                    matched_vars.insert(dst.clone());
                                }
                                None => return DynamicFilter::All,
                            }
                        }
                        if !dst.starts_with("____") {
                            block_env.insert(dst.clone(), val.clone());
                        }
                        ssa.insert(dst.clone(), val);
                    }
                }
                Body::Text { .. } => {}
                _ => {
                    block_env.clear();
                }
            }
        }
    }

    if matched_vars.len() == vars.len() && !prefixes.is_empty() {
        DynamicFilter::Prefixes(prefixes)
    } else {
        DynamicFilter::All
    }
}

fn resolve_ref_page(current_page: &str, r: &Reference) -> String {
    if r.page.trim().is_empty() {
        current_page.to_owned()
    } else {
        normalize_vfs_path(&r.page)
    }
}

fn lm_curve(c: u8) -> Easing {
    match c {
        1 => Easing::EaseIn,
        2 => Easing::EaseOut,
        _ => Easing::Linear,
    }
}

fn push_text_span(spans: &mut Vec<RichSpan>, text: &str) {
    if let Some(RichSpan::Text(last)) = spans.last_mut() {
        last.push_str(text);
    } else {
        spans.push(RichSpan::Text(text.to_owned()));
    }
}

fn split_speaker_and_trim(mut spans: Vec<RichSpan>) -> (Option<String>, Vec<RichSpan>) {
    // Trim leading/trailing newlines on first/last text span
    if let Some(RichSpan::Text(first)) = spans.first_mut() {
        let trimmed = first.trim_start_matches(['\r', '\n']);
        *first = trimmed.to_owned();
    }
    if let Some(RichSpan::Text(last)) = spans.last_mut() {
        let trimmed = last.trim_end_matches(['\r', '\n']);
        *last = trimmed.to_owned();
    }

    let mut speaker = None;
    if let Some(RichSpan::Text(first)) = spans.first_mut() {
        if let Some(rest) = first.strip_prefix('【') {
            if let Some((sp, after)) = rest.split_once('】') {
                let sp_t = sp.trim();
                let after_t = after.trim_start_matches(['\r', '\n', ' ', '　']);
                if !sp_t.is_empty() && sp_t.chars().count() <= 20 {
                    speaker = Some(sp_t.to_owned());
                    *first = after_t.to_owned();
                }
            }
        } else if let Some((line1, rest)) = first.split_once('\n') {
            let l1 = line1.trim();
            let rest_t = rest.trim_start_matches(['\r', '\n']);
            if !l1.is_empty()
                && l1.chars().count() <= 16
                && !l1.contains(['。', '、', '「', '」', '（', '）', '！', '？'])
                && (rest_t.starts_with('「')
                    || rest_t.starts_with('（')
                    || rest_t.starts_with('『'))
            {
                speaker = Some(l1.to_owned());
                *first = rest_t.to_owned();
            }
        }
    }
    spans.retain(|s| !matches!(s, RichSpan::Text(t) if t.is_empty()));
    (speaker, spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nir_vm::{Vm, WaitReason};

    fn simulate_vm(
        program: Arc<NirProgram>,
        entry: FuncId,
        max_turns: usize,
    ) -> Result<(usize, usize, usize, Option<String>)> {
        let mut vm = Vm::new_at(program, entry, None)?;
        let is_replay = entry != FuncId(0);
        let mut lines_seen = 0usize;
        let mut choices_seen = 0usize;
        let mut modals_seen = 0usize;
        let mut seen_choice_pcs: BTreeMap<u32, usize> = BTreeMap::new();
        for _ in 0..max_turns {
            vm.step(50_000)?;
            if vm.outcome.is_some() {
                break;
            }
            match vm.wait.clone() {
                WaitReason::None => continue,
                WaitReason::Dialogue => {
                    lines_seen += 1;
                    vm.advance(false);
                    vm.advance(true);
                }
                WaitReason::Choice => {
                    choices_seen += 1;
                    let visits = seen_choice_pcs.entry(vm.pc).or_default();
                    *visits += 1;
                    // Stop once a full playthrough loops back to the title menu twice.
                    if *visits > 2 && lines_seen > 100 {
                        break;
                    }
                    let idx = vm
                        .choice
                        .as_ref()
                        .and_then(|c| c.items.iter().position(|it| !it.disabled))
                        .unwrap_or(0);
                    vm.choose(idx);
                }
                WaitReason::Timer { remaining_us, .. } => {
                    vm.tick(remaining_us.saturating_add(1000), 1.0, false);
                }
                WaitReason::StageTransition | WaitReason::Tween | WaitReason::Shake => {
                    vm.tick(60_000_000, 1.0, true);
                }
                WaitReason::Timeline { .. } => {
                    vm.tick(120_000_000, 1.0, true);
                }
                WaitReason::Video { .. } => {
                    vm.notify_video_finished();
                }
                WaitReason::Modal(kind) => {
                    modals_seen += 1;
                    if is_replay && matches!(kind, ModalKind::Replay | ModalKind::Gallery) {
                        break;
                    }
                    vm.notify_modal_closed();
                }
            }
        }
        Ok((lines_seen, choices_seen, modals_seen, vm.outcome))
    }

    #[test]
    fn direct_import_synthetic_livenovel_runs_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("ノベルシステム")).unwrap();
        std::fs::create_dir_all(root.join("グラフィック")).unwrap();

        // Create minimal PNG image + LPM200 menu
        let img = image::RgbaImage::from_pixel(32, 32, image::Rgba([200, 100, 50, 255]));
        img.save(root.join("グラフィック/bg.png")).unwrap();
        img.save(root.join("グラフィック/btn.png")).unwrap();
        std::fs::write(
            root.join("グラフィック/title.lpm"),
            r#"<?xml version="1.0"?><PrevMenu><Version>200</Version><Width>1280</Width><Height>720</Height><Button><Item><Left>100</Left><Top>100</Top><Path>btn.png</Path><Name>Start</Name></Item></Button></PrevMenu>"#,
        )
        .unwrap();

        // Minimal valid LSB 116 script: 00000001.lsb with TextIns (CREATECG + dialogue) + Exit
        let mut tpw = b"TpWord105".to_vec();
        lsb::tests::u32b(&mut tpw, 1);
        tpw.extend([0u8; 22]);
        lsb::tests::string(&mut tpw, "");
        lsb::tests::string(&mut tpw, "");
        tpw.extend([0u8; 8]);
        lsb::tests::u32b(&mut tpw, 0);
        lsb::tests::u32b(&mut tpw, 0);
        // 2 glyphs: CREATECG event + "A"
        lsb::tests::u32b(&mut tpw, 2);
        // Glyph 0: Event CREATECG
        tpw.push(6);
        lsb::tests::u32b(&mut tpw, u32::MAX);
        lsb::tests::string(
            &mut tpw,
            "\u{1}CREATECG\r\nbg\r\nbg.png\r\nNORMAL\r\n#1\r\nC\r\nC\r\n0\r\n0\r\n0\r\n",
        );
        // Glyph 1: Char 'A'
        tpw.push(1);
        lsb::tests::u32b(&mut tpw, u32::MAX);
        lsb::tests::u32b(&mut tpw, u32::MAX);
        lsb::tests::u32b(&mut tpw, 0);
        tpw.extend_from_slice(&[b'A', 0]);
        lsb::tests::u32b(&mut tpw, 0);

        let mut text_args = Vec::new();
        lsb::tests::u32b(&mut text_args, tpw.len() as u32);
        text_args.extend(tpw);
        lsb::tests::u32b(&mut text_args, 1);
        text_args.push(1);
        lsb::tests::string(&mut text_args, "____arg");
        lsb::tests::u32b(&mut text_args, 1);
        text_args.push(4);
        lsb::tests::string(&mut text_args, "msg");
        for _ in 0..3 {
            lsb::tests::literal(&mut text_args, 1);
        }

        let script = lsb::tests::script(&[
            lsb::tests::command(20, 0, &text_args),
            lsb::tests::command(6, 1, &lsb::tests::integer(1)),
        ]);
        std::fs::write(root.join("00000001.lsb"), &script).unwrap();

        let (direct, report) = import_direct(root, &ImportOptions::default()).unwrap();
        assert_eq!(report.fallback_count, 0);
        assert_eq!(direct.program.meta.stage_width, 1280);
        assert_eq!(direct.program.meta.stage_height, 720);
        assert_eq!(direct.raw_assets.len(), 1);

        let prog = Arc::new(direct.program);
        let (lines, _choices, _modals, outcome) = simulate_vm(prog, FuncId(0), 100).unwrap();
        assert_eq!(lines, 1);
        assert_eq!(outcome.as_deref(), Some("end"));
    }

    #[test]
    #[ignore = "private batch directory supplied through NIR_TEST_LM_BATCH"]
    fn external_batch_verify_direct_and_vm() {
        let batch_root = std::env::var("NIR_TEST_LM_BATCH").unwrap();
        let skip_list: Vec<String> = std::env::var("NIR_TEST_LM_SKIP")
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&batch_root)
            .unwrap()
            .filter_map(|e| e.ok().map(|x| x.path()))
            .filter(|p| p.is_dir() && p.join("live.lpb").is_file())
            .collect();
        dirs.sort();

        let mut failures = Vec::new();
        for dir in dirs {
            let name = dir.file_name().unwrap().to_string_lossy().to_string();
            if skip_list.iter().any(|s| name.contains(s)) {
                continue;
            }
            let (direct, report) = match import_direct(&dir, &ImportOptions::default()) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("FAIL IMPORT {name}: {e:#}");
                    failures.push(format!("{name}: import error {e}"));
                    continue;
                }
            };
            let asset_count = direct.raw_assets.len();
            let prog = Arc::new(direct.program);
            let vm_res = simulate_vm(prog.clone(), FuncId(0), 25_000);
            let mut replay_ok = 0usize;
            let mut replay_err = 0usize;
            for item in &prog.ui.gallery {
                if let Some(rf) = item.replay_func {
                    match simulate_vm(prog.clone(), rf, 5_000) {
                        Ok(_) => replay_ok += 1,
                        Err(e) => {
                            replay_err += 1;
                            eprintln!("  REPLAY ERR {name} func={}: {e}", rf.0);
                        }
                    }
                }
            }
            let mut diag_cats: BTreeMap<String, usize> = BTreeMap::new();
            for d in &report.diagnostics {
                let key = format!("{:?}: {}", d.fidelity, d.message);
                *diag_cats.entry(key).or_default() += 1;
            }
            match vm_res {
                Ok((lines, choices, modals, outcome)) => {
                    println!(
                        "OK   {:10} | {}x{} | ops={:6} texts={:5} assets={:5} lookup={:5} menus={:2} tl={:2} st={} gal={:3} | exact={:6} adapt={:3} fb={:2} | VM: lines={:4} choices={:3} modals={} replay={}/{} out={:?}",
                        name.chars().take(10).collect::<String>(),
                        prog.meta.stage_width,
                        prog.meta.stage_height,
                        prog.ops.len(),
                        prog.texts.values().next().map_or(0, Vec::len),
                        asset_count,
                        prog.asset_lookup.len(),
                        prog.menus.len(),
                        prog.timelines.len(),
                        prog.ui.dialogue_styles.len(),
                        prog.ui.gallery.len(),
                        report.exact_count,
                        report.adapted_count,
                        report.fallback_count,
                        lines,
                        choices,
                        modals,
                        replay_ok,
                        replay_ok + replay_err,
                        outcome
                    );
                    if report.fallback_count > 0 || replay_err > 0 {
                        for (k, v) in diag_cats.iter().take(8) {
                            println!("       diag x{v}: {k}");
                        }
                        if report.fallback_count > 0 {
                            failures.push(format!("{name}: fallback={}", report.fallback_count));
                        }
                        if replay_err > 0 {
                            failures.push(format!("{name}: replay_err={replay_err}"));
                        }
                    } else if !diag_cats.is_empty() {
                        for (k, v) in diag_cats.iter().take(5) {
                            println!("       note x{v}: {k}");
                        }
                    }
                }
                Err(e) => {
                    eprintln!("FAIL VM     {name}: {e:#}");
                    failures.push(format!("{name}: vm error {e}"));
                }
            }
        }
        assert!(failures.is_empty(), "batch failures: {failures:#?}");
    }
}
