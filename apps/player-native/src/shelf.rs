//! Built-in universal `.nirpak` Shelf (书架) for Desktop and Android.
//!
//! Renders natively through `nir_ui::DrawPacket` + `nir_render::Renderer` with zero extra UI
//! toolkits, supporting:
//! - Automatic `.nirpak` scanning across local / Android external `games/` directories
//! - Persistent `recent_games.json` history
//! - Fast zero-copy `mmap` header + `title_bg` cover extraction
//! - Touch, mouse, scroll wheel, keyboard, and gamepad navigation

use crate::pak::PakArchive;
use nir_ir::{AssetId, TextAlign};
use nir_render::Renderer;
use nir_ui::{DrawPacket, Quad, SafeInsets, TextRun};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Reserved virtual `AssetId` range for Shelf cover art (distinct from save thumbs & video frames).
pub const SHELF_COVER_ASSET_BASE: u32 = 0xFFFF_E000;
const MAX_RECENT_GAMES: usize = 32;
const MAX_COVER_BYTES: usize = 8 * 1024 * 1024;
const COVER_THUMB_MAX_DIM: u32 = 320;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShelfAction {
    Launch(PathBuf),
    OpenFilePicker,
    Rescan,
}

#[derive(Debug, Clone)]
pub struct ShelfEntry {
    pub path: PathBuf,
    pub game_id: String,
    pub title: String,
    pub version: String,
    pub locales: Vec<String>,
    pub file_size_bytes: u64,
    pub save_count: usize,
    pub cover_asset: Option<AssetId>,
    pub cover_rgba: Option<(u32, u32, Vec<u8>)>,
}

pub struct ShelfState {
    pub scan_dirs: Vec<PathBuf>,
    pub recent_file: PathBuf,
    pub storage_root: PathBuf,
    pub entries: Vec<ShelfEntry>,
    pub scroll_y: f32,
    pub max_scroll_y: f32,
    pub selected_idx: usize,
    pub hovered_hit: Option<usize>,
    pub hit_rects: Vec<([f32; 4], ShelfAction)>,
    pub safe_insets: SafeInsets,
    pub is_touch: bool,
    pub is_android: bool,
    pub covers_uploaded: bool,
    pub dirty: bool,
    toast: Option<(String, Instant)>,
}

impl ShelfState {
    pub fn new(
        scan_dirs: Vec<PathBuf>,
        storage_root: PathBuf,
        is_touch: bool,
        is_android: bool,
    ) -> Self {
        let recent_file = storage_root.join("nir").join("recent_games.json");
        let mut s = Self {
            scan_dirs,
            recent_file,
            storage_root,
            entries: Vec::new(),
            scroll_y: 0.0,
            max_scroll_y: 0.0,
            selected_idx: 0,
            hovered_hit: None,
            hit_rects: Vec::new(),
            safe_insets: SafeInsets::default(),
            is_touch,
            is_android,
            covers_uploaded: false,
            dirty: true,
            toast: None,
        };
        s.rescan();
        s
    }

    pub fn show_toast(&mut self, msg: impl Into<String>, duration_ms: u64) {
        self.toast = Some((
            msg.into(),
            Instant::now() + Duration::from_millis(duration_ms.max(800)),
        ));
        self.dirty = true;
    }

    pub fn record_recent(&self, pak_path: &Path) {
        record_recent_game(&self.recent_file, pak_path);
    }

    pub fn rescan(&mut self) {
        for dir in &self.scan_dirs {
            let _ = fs::create_dir_all(dir);
        }
        self.entries = scan_shelf_entries(&self.scan_dirs, &self.recent_file, &self.storage_root);
        self.selected_idx = self.selected_idx.min(self.entries.len().saturating_sub(1));
        self.scroll_y = 0.0;
        self.covers_uploaded = false;
        self.dirty = true;
    }

    pub fn ensure_covers_uploaded(&mut self, renderer: &mut Renderer) {
        if self.covers_uploaded {
            return;
        }
        self.covers_uploaded = true;
        for (idx, entry) in self.entries.iter_mut().enumerate() {
            if let Some((w, h, rgba)) = entry.cover_rgba.as_ref() {
                let id = AssetId(SHELF_COVER_ASSET_BASE + idx as u32);
                if renderer.upload_rgba_premultiplied(id, *w, *h, rgba).is_ok() {
                    entry.cover_asset = Some(id);
                    self.dirty = true;
                }
            }
        }
    }

    pub fn set_hover(&mut self, lx: f32, ly: f32) -> bool {
        let hit = self
            .hit_rects
            .iter()
            .enumerate()
            .rev()
            .find(|(_, (r, _))| point_in_rect(lx, ly, *r))
            .map(|(i, _)| i);
        if hit != self.hovered_hit {
            self.hovered_hit = hit;
            self.dirty = true;
        }
        hit.is_some()
    }

    pub fn hit_test(&self, lx: f32, ly: f32) -> Option<ShelfAction> {
        self.hit_rects
            .iter()
            .rev()
            .find(|(r, _)| point_in_rect(lx, ly, *r))
            .map(|(_, a)| a.clone())
    }

    pub fn scroll(&mut self, delta_px: f32) {
        let next = (self.scroll_y + delta_px).clamp(0.0, self.max_scroll_y);
        if (next - self.scroll_y).abs() > 0.5 {
            self.scroll_y = next;
            self.dirty = true;
        }
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        let len = self.entries.len() as isize;
        let next = (self.selected_idx as isize + delta).rem_euclid(len) as usize;
        if next != self.selected_idx {
            self.selected_idx = next;
            self.dirty = true;
        }
    }

    pub fn selected_path(&self) -> Option<PathBuf> {
        self.entries.get(self.selected_idx).map(|e| e.path.clone())
    }

    pub fn next_wakeup_delay_us(&mut self) -> Option<u64> {
        if let Some((_, until)) = &self.toast {
            let now = Instant::now();
            if now >= *until {
                self.toast = None;
                self.dirty = true;
                None
            } else {
                Some(until.duration_since(now).as_micros() as u64)
            }
        } else {
            None
        }
    }

    /// Projects the entire responsive Shelf UI into a GPU-ready `nir_ui::DrawPacket`.
    pub fn project_draw_packet(&mut self, width: f32, height: f32) -> DrawPacket {
        let w = width.max(240.0);
        let h = height.max(200.0);
        let safe = self.safe_insets.rect(w, h);
        let (sx, sy, sw, sh) = (safe[0], safe[1], safe[2], safe[3]);
        let touch = self.is_touch;

        let mut pkt = DrawPacket {
            width: w,
            height: h,
            stage_rect: [0.0, 0.0, w, h],
            ..Default::default()
        };
        self.hit_rects.clear();

        // Deep obsidian-navy ambient backdrop
        pkt.quads.push(Quad::gradient_v(
            [0.0, 0.0, w, h],
            [0.055, 0.075, 0.125, 1.0],
            [0.032, 0.044, 0.078, 1.0],
        ));
        // Subtle top luminous accent hairline
        pkt.quads.push(Quad::gradient_h(
            [sx, sy, sw, 2.0],
            [0.32, 0.62, 0.98, 0.85],
            [0.58, 0.42, 0.95, 0.25],
        ));

        let pad = (sw * 0.032).clamp(14.0, 28.0);
        let hdr_h = if touch { 58.0 } else { 54.0 };
        let hx = sx + pad;
        let hy = sy + 10.0;
        let hw = (sw - pad * 2.0).max(160.0);

        // Header title
        push_text(
            &mut pkt,
            format!("NIR 书架  ·  {} 部作品", self.entries.len()),
            [hx, hy, (hw - 210.0).max(120.0), hdr_h],
            if touch { 19.0 } else { 21.0 },
            [0.95, 0.97, 1.0, 1.0],
            TextAlign::Left,
        );

        // Header action buttons (Rescan + Open .nirpak on Desktop)
        let btn_h = if touch { 38.0 } else { 34.0 };
        let btn_y = hy + (hdr_h - btn_h) * 0.5;
        let rescan_w = if touch { 84.0 } else { 78.0 };
        let rescan_rect = [hx + hw - rescan_w, btn_y, rescan_w, btn_h];
        let rescan_idx = self.hit_rects.len();
        self.hit_rects.push((rescan_rect, ShelfAction::Rescan));
        draw_pill_button(
            &mut pkt,
            rescan_rect,
            "刷新",
            self.hovered_hit == Some(rescan_idx),
            false,
        );

        if !self.is_android {
            let open_w = 124.0;
            let open_rect = [rescan_rect[0] - 10.0 - open_w, btn_y, open_w, btn_h];
            let open_idx = self.hit_rects.len();
            self.hit_rects.push((open_rect, ShelfAction::OpenFilePicker));
            draw_pill_button(
                &mut pkt,
                open_rect,
                "打开 .nirpak…",
                self.hovered_hit == Some(open_idx),
                true,
            );
        }

        let content_y = hy + hdr_h + 10.0;
        let content_h = (sy + sh - content_y - pad).max(120.0);
        let content_clip = [hx, content_y, hw, content_h];

        if self.entries.is_empty() {
            self.max_scroll_y = 0.0;
            self.scroll_y = 0.0;
            let card_w = hw.min(580.0);
            let card_h = content_h.min(if touch { 250.0 } else { 230.0 });
            let cx = hx + (hw - card_w) * 0.5;
            let cy = content_y + (content_h - card_h) * 0.42;
            pkt.quads.push(Quad::panel(
                [cx, cy, card_w, card_h],
                [0.10, 0.13, 0.21, 0.92],
                Some([0.075, 0.095, 0.16, 0.94]),
                [0.36, 0.58, 0.92, 0.36],
                1.2,
                14.0,
            ));
            let headline = "尚未发现 .nirpak 游戏包";
            let body = if self.is_android {
                let primary_dir = self
                    .scan_dirs
                    .first()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "Android/data/org.nir.player/files/games".into());
                format!(
                    "将任意 .nirpak 放入以下目录后点击「刷新」即可开玩：\n{primary_dir}"
                )
            } else {
                "将任意 .nirpak 文件拖拽到此窗口，或点击下方按钮选择游戏包。".into()
            };
            push_text(
                &mut pkt,
                headline.into(),
                [cx + 22.0, cy + 24.0, card_w - 44.0, 34.0],
                19.0,
                [0.95, 0.97, 1.0, 1.0],
                TextAlign::Center,
            );
            push_text(
                &mut pkt,
                body,
                [cx + 22.0, cy + 68.0, card_w - 44.0, 82.0],
                13.5,
                [0.74, 0.80, 0.90, 1.0],
                TextAlign::Center,
            );
            let cta_w = 186.0;
            let cta_h = if touch { 44.0 } else { 40.0 };
            let cta_rect = [
                cx + (card_w - cta_w) * 0.5,
                cy + card_h - cta_h - 24.0,
                cta_w,
                cta_h,
            ];
            let cta_idx = self.hit_rects.len();
            let (cta_label, cta_action) = if self.is_android {
                ("重新扫描目录", ShelfAction::Rescan)
            } else {
                ("选择 .nirpak 文件…", ShelfAction::OpenFilePicker)
            };
            self.hit_rects.push((cta_rect, cta_action));
            draw_pill_button(
                &mut pkt,
                cta_rect,
                cta_label,
                self.hovered_hit == Some(cta_idx),
                true,
            );
        } else {
            let cols = if hw >= 920.0 {
                3usize
            } else if hw >= 540.0 {
                2usize
            } else {
                1usize
            };
            let gap = if touch { 14.0 } else { 16.0 };
            let card_w = (hw - gap * (cols - 1) as f32) / cols as f32;
            let cover_h = (card_w * 0.54).clamp(112.0, 186.0);
            let info_h = if touch { 72.0 } else { 66.0 };
            let card_h = cover_h + info_h;
            let rows = self.entries.len().div_ceil(cols);
            let total_h = rows as f32 * card_h + rows.saturating_sub(1) as f32 * gap;
            self.max_scroll_y = (total_h - content_h).max(0.0);
            self.scroll_y = self.scroll_y.clamp(0.0, self.max_scroll_y);

            for (idx, entry) in self.entries.iter().enumerate() {
                let col = idx % cols;
                let row = idx / cols;
                let cx = hx + col as f32 * (card_w + gap);
                let cy = content_y + row as f32 * (card_h + gap) - self.scroll_y;
                if cy + card_h < content_y - 20.0 || cy > content_y + content_h + 20.0 {
                    continue;
                }
                let rect = [cx, cy, card_w, card_h];
                let hit_idx = self.hit_rects.len();
                self.hit_rects
                    .push((rect, ShelfAction::Launch(entry.path.clone())));
                let active =
                    self.hovered_hit == Some(hit_idx) || (!touch && self.selected_idx == idx);

                let border = if active {
                    [0.42, 0.72, 1.0, 0.92]
                } else {
                    [0.32, 0.42, 0.58, 0.34]
                };
                let mut card_bg = Quad::panel(
                    rect,
                    [0.10, 0.135, 0.215, 0.94],
                    Some([0.075, 0.098, 0.16, 0.96]),
                    border,
                    if active { 1.8 } else { 1.0 },
                    12.0,
                );
                card_bg.clip = Some(content_clip);
                pkt.quads.push(card_bg);

                let cover_rect = [cx + 4.0, cy + 4.0, card_w - 8.0, cover_h - 4.0];
                if let Some(asset_id) = entry.cover_asset {
                    let mut img = Quad::rounded_image(cover_rect, asset_id, 9.0);
                    img.clip = Some(content_clip);
                    pkt.quads.push(img);
                } else {
                    let mut fallback = Quad::gradient_v(
                        cover_rect,
                        [0.16, 0.24, 0.40, 1.0],
                        [0.09, 0.13, 0.23, 1.0],
                    );
                    fallback.corner_radius = 9.0;
                    fallback.clip = Some(content_clip);
                    pkt.quads.push(fallback);
                    let badge_txt = entry.title.chars().take(2).collect::<String>();
                    push_text_clipped(
                        &mut pkt,
                        badge_txt,
                        cover_rect,
                        32.0,
                        [0.75, 0.86, 1.0, 0.65],
                        TextAlign::Center,
                        Some(content_clip),
                    );
                }

                // Title & subtitle metadata
                push_text_clipped(
                    &mut pkt,
                    entry.title.clone(),
                    [cx + 12.0, cy + cover_h + 8.0, card_w - 24.0, 26.0],
                    15.5,
                    [0.96, 0.98, 1.0, 1.0],
                    TextAlign::Left,
                    Some(content_clip),
                );
                let size_mb = entry.file_size_bytes as f64 / (1024.0 * 1024.0);
                let save_part = if entry.save_count > 0 {
                    format!(" · {} 存档", entry.save_count)
                } else {
                    String::new()
                };
                let loc_part = if entry.locales.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", entry.locales.join("/"))
                };
                let sub = format!("v{} · {:.1} MB{}{}", entry.version, size_mb, loc_part, save_part);
                push_text_clipped(
                    &mut pkt,
                    sub,
                    [cx + 12.0, cy + cover_h + 36.0, card_w - 24.0, 22.0],
                    12.0,
                    [0.66, 0.74, 0.86, 1.0],
                    TextAlign::Left,
                    Some(content_clip),
                );
            }
        }

        // Optional transient toast notification
        if let Some((msg, until)) = &self.toast {
            if Instant::now() < *until {
                let tw = (sw - 40.0).min(420.0);
                let th = 38.0;
                let tx = sx + (sw - tw) * 0.5;
                let ty = sy + sh - th - 18.0;
                pkt.quads.push(Quad::panel(
                    [tx, ty, tw, th],
                    [0.10, 0.16, 0.27, 0.95],
                    None,
                    [0.42, 0.72, 1.0, 0.70],
                    1.2,
                    19.0,
                ));
                push_text(
                    &mut pkt,
                    msg.clone(),
                    [tx + 14.0, ty, tw - 28.0, th],
                    13.5,
                    [0.95, 0.98, 1.0, 1.0],
                    TextAlign::Center,
                );
            }
        }

        self.dirty = false;
        pkt
    }
}

fn draw_pill_button(pkt: &mut DrawPacket, rect: [f32; 4], label: &str, hovered: bool, primary: bool) {
    let (top, bot, border, fg) = if primary {
        if hovered {
            (
                [0.34, 0.62, 0.98, 0.98],
                [0.24, 0.48, 0.88, 0.98],
                [0.64, 0.84, 1.0, 0.92],
                [1.0, 1.0, 1.0, 1.0],
            )
        } else {
            (
                [0.25, 0.52, 0.90, 0.94],
                [0.18, 0.40, 0.78, 0.94],
                [0.48, 0.74, 1.0, 0.70],
                [0.98, 0.99, 1.0, 1.0],
            )
        }
    } else if hovered {
        (
            [0.20, 0.27, 0.40, 0.95],
            [0.14, 0.19, 0.30, 0.95],
            [0.52, 0.70, 0.96, 0.75],
            [0.96, 0.98, 1.0, 1.0],
        )
    } else {
        (
            [0.14, 0.19, 0.29, 0.88],
            [0.10, 0.14, 0.22, 0.90],
            [0.34, 0.46, 0.64, 0.45],
            [0.86, 0.91, 0.98, 1.0],
        )
    };
    pkt.quads
        .push(Quad::panel(rect, top, Some(bot), border, 1.2, rect[3] * 0.45));
    push_text(
        pkt,
        label.to_owned(),
        [rect[0] + 8.0, rect[1], (rect[2] - 16.0).max(20.0), rect[3]],
        13.5,
        fg,
        TextAlign::Center,
    );
}

fn push_text(
    pkt: &mut DrawPacket,
    text: String,
    rect: [f32; 4],
    size: f32,
    color: [f32; 4],
    align: TextAlign,
) {
    push_text_clipped(pkt, text, rect, size, color, align, Some(rect));
}

fn push_text_clipped(
    pkt: &mut DrawPacket,
    text: String,
    rect: [f32; 4],
    size: f32,
    color: [f32; 4],
    align: TextAlign,
    clip: Option<[f32; 4]>,
) {
    let mut run = TextRun {
        text,
        visible_bytes: None,
        x: rect[0],
        y: rect[1],
        width: rect[2].max(16.0),
        height: rect[3].max(16.0),
        size,
        line_height: size * 1.32,
        color,
        emphasis: Vec::new(),
        color_spans: Vec::new(),
        ruby_spans: Vec::new(),
        shadow: None,
        outline: None,
        scroll: 0.0,
        clip,
        font_assets: Vec::new(),
        align,
        valign_center: true,
        auto_fit: true,
        inline_images: Vec::new(),
        cache_key: 0,
    };
    run.compute_key();
    pkt.texts.push(run);
}

fn point_in_rect(x: f32, y: f32, r: [f32; 4]) -> bool {
    x >= r[0] && x <= r[0] + r[2] && y >= r[1] && y <= r[1] + r[3]
}

/// Persists a recently opened `.nirpak` path at the front of `recent_games.json`.
pub fn record_recent_game(recent_file: &Path, pak_path: &Path) {
    let canonical = fs::canonicalize(pak_path).unwrap_or_else(|_| pak_path.to_path_buf());
    let mut list = load_recent_games(recent_file);
    list.retain(|p| p != &canonical);
    list.insert(0, canonical);
    list.truncate(MAX_RECENT_GAMES);
    if let Some(parent) = recent_file.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let strings: Vec<String> = list
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    if let Ok(bytes) = serde_json::to_vec_pretty(&strings) {
        let _ = fs::write(recent_file, bytes);
    }
}

pub fn load_recent_games(recent_file: &Path) -> Vec<PathBuf> {
    let Ok(text) = fs::read_to_string(recent_file) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<String>>(&text)
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .collect()
}

/// Scans `scan_dirs` and `recent_file` for valid `.nirpak` archives, returning deduplicated
/// shelf entries with metadata, save counts, and lightweight cover art thumbnails.
pub fn scan_shelf_entries(
    scan_dirs: &[PathBuf],
    recent_file: &Path,
    storage_root: &Path,
) -> Vec<ShelfEntry> {
    let mut candidates = load_recent_games(recent_file);
    for dir in scan_dirs {
        collect_nirpaks_in_dir(dir, 1, &mut candidates);
    }

    let mut seen = HashSet::<PathBuf>::new();
    let mut out = Vec::new();
    for path in candidates {
        let key = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if !seen.insert(key) {
            continue;
        }
        if let Some(entry) = inspect_nirpak_entry(&path, storage_root) {
            out.push(entry);
        }
    }
    out
}

fn collect_nirpaks_in_dir(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_file()
            && path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("nirpak"))
        {
            out.push(path);
        } else if depth > 0 && path.is_dir() {
            collect_nirpaks_in_dir(&path, depth - 1, out);
        }
    }
}

fn inspect_nirpak_entry(path: &Path, storage_root: &Path) -> Option<ShelfEntry> {
    let file_size_bytes = fs::metadata(path).ok()?.len();
    let pak = PakArchive::open_file(path).ok()?;
    let meta = &pak.program.meta;
    let save_count = count_saved_slots(storage_root, &meta.game_id);

    let cover_rgba = meta.title_bg.and_then(|bg_id| {
        let bytes = pak.asset_bytes(bg_id)?;
        if bytes.is_empty() || bytes.len() > MAX_COVER_BYTES {
            return None;
        }
        let (w, h, rgba) = nir_render::decode_image(bytes).ok()?;
        Some(downscale_rgba_cover(w, h, &rgba, COVER_THUMB_MAX_DIM))
    });

    Some(ShelfEntry {
        path: path.to_path_buf(),
        game_id: meta.game_id.clone(),
        title: if meta.title.is_empty() {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Untitled")
                .to_owned()
        } else {
            meta.title.clone()
        },
        version: meta.version.clone(),
        locales: meta.locales.clone(),
        file_size_bytes,
        save_count,
        cover_asset: None,
        cover_rgba,
    })
}

fn count_saved_slots(storage_root: &Path, game_id: &str) -> usize {
    let safe_id: String = game_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let dir = storage_root.join("nir").join(safe_id);
    let mut count = 0usize;
    if let Ok(text) = fs::read_to_string(dir.join("slots_index.json")) {
        if let Ok(map) =
            serde_json::from_str::<std::collections::BTreeMap<u32, nir_ui::SlotSummary>>(&text)
        {
            count += map.values().filter(|s| s.exists).count();
        }
    }
    if dir.join("autosave.json").is_file() {
        count += 1;
    }
    if dir.join("quicksave.json").is_file() {
        count += 1;
    }
    count
}

fn downscale_rgba_cover(w: u32, h: u32, rgba: &[u8], max_dim: u32) -> (u32, u32, Vec<u8>) {
    let max_side = w.max(h);
    if max_side <= max_dim || w == 0 || h == 0 {
        return (w, h, rgba.to_vec());
    }
    let scale = max_dim as f32 / max_side as f32;
    let dw = ((w as f32 * scale).round() as u32).max(1);
    let dh = ((h as f32 * scale).round() as u32).max(1);
    let mut out = vec![0u8; (dw * dh * 4) as usize];
    for y in 0..dh {
        let sy = ((y as u64 * h as u64) / dh as u64).min((h - 1) as u64) as usize;
        for x in 0..dw {
            let sx = ((x as u64 * w as u64) / dw as u64).min((w - 1) as u64) as usize;
            let src_i = (sy * w as usize + sx) * 4;
            let dst_i = (y as usize * dw as usize + x as usize) * 4;
            out[dst_i..dst_i + 4].copy_from_slice(&rgba[src_i..src_i + 4]);
        }
    }
    (dw, dh, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shelf_scans_nirpak_persists_recent_and_projects_ui() {
        let tmp = std::env::temp_dir().join(format!(
            "nir-shelf-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let games_dir = tmp.join("games");
        fs::create_dir_all(&games_dir).unwrap();

        let mut prog = nir_ir::NirProgram::empty_placeholder();
        prog.meta.game_id = "org.nir.shelf_demo".into();
        prog.meta.title = "Shelf Test Novel".into();
        prog.meta.version = "1.2.0".into();
        prog.meta.locales = vec!["zh-Hans".into(), "en".into()];
        let pak_bytes = nir_ir::encode_nirpak(&prog, &[]).unwrap();
        let pak_path = games_dir.join("demo.nirpak");
        fs::write(&pak_path, &pak_bytes).unwrap();

        let mut shelf = ShelfState::new(vec![games_dir.clone()], tmp.clone(), false, false);
        assert_eq!(shelf.entries.len(), 1);
        assert_eq!(shelf.entries[0].title, "Shelf Test Novel");
        assert_eq!(shelf.entries[0].game_id, "org.nir.shelf_demo");

        shelf.record_recent(&pak_path);
        assert_eq!(load_recent_games(&shelf.recent_file).len(), 1);

        let pkt_desktop = shelf.project_draw_packet(1280.0, 720.0);
        assert!(!pkt_desktop.quads.is_empty());
        assert!(!pkt_desktop.texts.is_empty());
        assert!(shelf
            .hit_rects
            .iter()
            .any(|(_, a)| matches!(a, ShelfAction::Launch(_))));

        // Android empty shelf projection (isolated storage root with no recent games)
        let mut android_empty = ShelfState::new(
            vec![tmp.join("empty")],
            tmp.join("empty-storage"),
            true,
            true,
        );
        let pkt_android = android_empty.project_draw_packet(390.0, 844.0);
        assert!(pkt_android
            .texts
            .iter()
            .any(|t| t.text.contains("尚未发现")));

        let _ = fs::remove_dir_all(&tmp);
    }
}
