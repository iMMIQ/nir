//! Unified declarative responsive UI & persistent scene graph projector.
//! Replaces legacy 8,433-line `nir-presentation` + 1,622-line `menu.rs`.
#![forbid(unsafe_code)]

pub mod draw;
pub mod messages;
pub mod text_engine;

pub use draw::*;
pub use messages::Messages;
pub use text_engine::TextEngine;

use nir_ir::*;
use nir_vm::{LayerState, Vm};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Title,
    Story,
    Menu,
    Saves { save_mode: bool },
    Settings,
    History,
    Gallery,
    Ended,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SlotSummary {
    pub slot: u32,
    pub exists: bool,
    pub summary: String,
    pub saved_at_unix: u64,
}

/// Read-only projection input supplied by `nir-runtime`.
pub struct UiContext<'a> {
    pub vm: &'a Vm,
    pub screen: Screen,
    pub prefs: &'a Preferences,
    pub auto: bool,
    pub skip: bool,
    pub interface_hidden: bool,
    pub replay_active: bool,
    pub slots: &'a [SlotSummary],
    pub history_scroll: usize,
    pub hovered_hit: Option<u32>,
    pub toast: Option<&'a str>,
}

/// Computes letterboxed stage viewport `(x, y, w, h, scale)` inside `(view_w, view_h)`.
pub fn fit_stage(stage_w: f32, stage_h: f32, view_w: f32, view_h: f32) -> ([f32; 4], f32) {
    let scale = (view_w / stage_w.max(1.0)).min(view_h / stage_h.max(1.0));
    let w = stage_w * scale;
    let h = stage_h * scale;
    let x = (view_w - w) * 0.5;
    let y = (view_h - h) * 0.5;
    ([x, y, w, h], scale)
}

/// Projects persistent scene graph layers into renderer quads, applying:
/// - Parent hierarchy inheritance (position, scale, opacity)
/// - Normalized anchor & rotation (`Transform2D`)
/// - Auto-trimmed sprite offset restoration (`SpriteTrim`)
/// - Z-order stable sorting & blend modes
pub fn project_layers(
    program: &NirProgram,
    layers: &[LayerState],
    stage_rect: [f32; 4],
    scale: f32,
) -> Vec<Quad> {
    if layers.is_empty() {
        return Vec::new();
    }
    let already_sorted = layers
        .windows(2)
        .all(|w| (w[0].transform.z_order, w[0].id.0) <= (w[1].transform.z_order, w[1].id.0));
    let mut ordered_storage: Vec<&LayerState>;
    let ordered_slice: &[LayerState];
    let mut out = Vec::with_capacity(layers.len());

    let project_one = |layer: &LayerState, out: &mut Vec<Quad>| {
        // Accumulate parent transforms (bounded depth 16, linear slice scan avoids BTreeMap heap alloc).
        let mut px = layer.transform.x + layer.shake_offset[0];
        let mut py = layer.transform.y + layer.shake_offset[1];
        let mut sx = layer.transform.scale[0];
        let mut sy = layer.transform.scale[1];
        let mut op = layer.transform.opacity;
        let mut cur_parent = layer.parent;
        for _ in 0..16 {
            let Some(pid) = cur_parent else { break };
            let Some(p) = layers.iter().find(|l| l.id == pid) else { break };
            px = p.transform.x + p.shake_offset[0] + px * p.transform.scale[0];
            py = p.transform.y + p.shake_offset[1] + py * p.transform.scale[1];
            sx *= p.transform.scale[0];
            sy *= p.transform.scale[1];
            op *= p.transform.opacity;
            cur_parent = p.parent;
        }
        if op <= 0.001 {
            return;
        }

        let asset_meta = layer.asset.and_then(|id| program.asset(id));
        let (orig_w, orig_h) = if layer.transform.width > 0.0 && layer.transform.height > 0.0 {
            (layer.transform.width, layer.transform.height)
        } else if let Some(a) = asset_meta {
            if let Some(trim) = a.trim {
                (trim.orig_w as f32, trim.orig_h as f32)
            } else {
                (a.width as f32, a.height as f32)
            }
        } else {
            (
                program.meta.stage_width as f32,
                program.meta.stage_height as f32,
            )
        };

        // Restore auto-trimmed transparent margins seamlessly!
        let (draw_off_x, draw_off_y, draw_w, draw_h) = if let Some(trim) = asset_meta.and_then(|a| a.trim) {
            let rx = orig_w / trim.orig_w.max(1) as f32;
            let ry = orig_h / trim.orig_h.max(1) as f32;
            (
                trim.x as f32 * rx,
                trim.y as f32 * ry,
                trim.w as f32 * rx,
                trim.h as f32 * ry,
            )
        } else {
            (0.0, 0.0, orig_w, orig_h)
        };

        let anchor_px = layer.transform.anchor[0] * orig_w;
        let anchor_py = layer.transform.anchor[1] * orig_h;
        let local_x0 = (draw_off_x - anchor_px) * sx;
        let local_y0 = (draw_off_y - anchor_py) * sy;
        let local_w = draw_w * sx;
        let local_h = draw_h * sy;

        let cx = stage_rect[0] + px * scale;
        let cy = stage_rect[1] + py * scale;
        let rect = [
            cx + local_x0 * scale,
            cy + local_y0 * scale,
            local_w * scale,
            local_h * scale,
        ];

        let corners = if layer.transform.rotation_deg.abs() > 0.01 {
            let rad = layer.transform.rotation_deg.to_radians();
            let (sin, cos) = rad.sin_cos();
            let rot = |lx: f32, ly: f32| {
                [
                    cx + (lx * cos - ly * sin) * scale,
                    cy + (lx * sin + ly * cos) * scale,
                ]
            };
            Some([
                rot(local_x0, local_y0),
                rot(local_x0 + local_w, local_y0),
                rot(local_x0, local_y0 + local_h),
                rot(local_x0 + local_w, local_y0 + local_h),
            ])
        } else {
            None
        };

        let clip = layer.transform.clip.map(|c| {
            [
                stage_rect[0] + c[0] * scale,
                stage_rect[1] + c[1] * scale,
                c[2] * scale,
                c[3] * scale,
            ]
        });

        let mut color = layer.transform.color;
        color[3] *= op;

        out.push(Quad {
            rect,
            uv: [0.0, 0.0, 1.0, 1.0],
            corners,
            color,
            asset: layer.asset,
            blend: layer.transform.blend,
            clip,
            special: SpecialQuad::None,
        });
    };

    if already_sorted {
        ordered_slice = layers;
        for layer in ordered_slice {
            project_one(layer, &mut out);
        }
    } else {
        ordered_storage = layers.iter().collect();
        ordered_storage.sort_by_key(|l| (l.transform.z_order, l.id.0));
        for layer in ordered_storage {
            project_one(layer, &mut out);
        }
    }
    out
}

struct Builder<'a> {
    pkt: DrawPacket,
    theme: &'a UiTheme,
    ui_fonts: Vec<AssetId>,
    text_fonts: Vec<AssetId>,
    hovered: Option<u32>,
    next_hit_id: u32,
}

impl<'a> Builder<'a> {
    fn push_text(&mut self, mut run: TextRun) {
        run.compute_key();
        self.pkt.texts.push(run);
    }

    fn button(
        &mut self,
        rect: [f32; 4],
        label: String,
        action: UiAction,
        enabled: bool,
        primary: bool,
    ) {
        let id = self.next_hit_id;
        self.next_hit_id += 1;
        let is_hover = enabled && self.hovered == Some(id);
        let bg = if !enabled {
            [0.14, 0.16, 0.20, 0.55]
        } else if is_hover {
            [
                self.theme.accent[0] * 0.85,
                self.theme.accent[1] * 0.85,
                self.theme.accent[2] * 0.85,
                0.92,
            ]
        } else if primary {
            [
                self.theme.accent[0] * 0.38,
                self.theme.accent[1] * 0.38,
                self.theme.accent[2] * 0.38,
                0.90,
            ]
        } else {
            self.theme.panel
        };
        self.pkt.quads.push(Quad::solid(rect, bg));
        // Subtle 1px top/bottom border
        let border = if is_hover {
            self.theme.accent
        } else {
            self.theme.panel_border
        };
        self.pkt
            .quads
            .push(Quad::solid([rect[0], rect[1], rect[2], 1.5], border));
        self.pkt.quads.push(Quad::solid(
            [rect[0], rect[1] + rect[3] - 1.5, rect[2], 1.5],
            border,
        ));

        let size = (rect[3] * 0.44).clamp(14.0, 24.0);
        let pad_x = 14.0;
        let pad_y = (rect[3] - size * 1.2) * 0.5;
        let text_color = if !enabled {
            self.theme.muted
        } else if is_hover {
            [0.05, 0.07, 0.10, 1.0]
        } else {
            self.theme.text
        };
        self.push_text(TextRun {
            text: label.clone(),
            visible_bytes: None,
            x: rect[0] + pad_x,
            y: rect[1] + pad_y.max(2.0),
            width: (rect[2] - pad_x * 2.0).max(8.0),
            height: rect[3],
            size,
            line_height: size * 1.3,
            color: text_color,
            emphasis: Vec::new(),
            shadow: None,
            scroll: 0.0,
            clip: Some(rect),
            font_assets: self.ui_fonts.clone(),
            cache_key: 0,
        });
        self.pkt.hits.push(HitRegion {
            id,
            rect,
            action,
            label,
            enabled,
        });
    }
}

/// Projects the full engine frame (stage + transitions + HUD + overlays + toast).
pub fn project(ctx: &UiContext<'_>, width: f32, height: f32, messages: &Messages) -> DrawPacket {
    let prog = ctx.vm.program();
    let sw = prog.meta.stage_width as f32;
    let sh = prog.meta.stage_height as f32;
    let (stage_rect, scale) = fit_stage(sw, sh, width, height);
    let loc = &ctx.prefs.ui_locale;

    let mut b = Builder {
        pkt: DrawPacket {
            width,
            height,
            stage_size: [prog.meta.stage_width, prog.meta.stage_height],
            stage_rect,
            ..Default::default()
        },
        theme: &prog.ui,
        ui_fonts: prog.meta.ui_fonts.clone(),
        text_fonts: prog.meta.text_fonts.clone(),
        hovered: ctx.hovered_hit,
        next_hit_id: 1,
    };

    // 1. Background / Stage rendering
    if ctx.screen == Screen::Title {
        if let Some(bg) = prog
            .ui
            .title_screen
            .as_ref()
            .and_then(|s| s.background)
            .or(prog.meta.title_bg)
        {
            b.pkt.quads.push(Quad::image(stage_rect, bg));
        } else {
            b.pkt
                .quads
                .push(Quad::solid([0.0, 0.0, width, height], prog.ui.surface));
        }
        project_title(&mut b, ctx, messages, loc, stage_rect, scale);
    } else {
        // Active story stage (with optional live transition!)
        if let Some(tr) = &ctx.vm.transition {
            let local_rect = [0.0, 0.0, sw, sh];
            b.pkt.transition_from = project_layers(prog, &tr.from_layers, local_rect, 1.0);
            b.pkt.transition_to = project_layers(prog, &ctx.vm.layers, local_rect, 1.0);
            b.pkt.transition = tr.transition;
            b.pkt.transition_progress = tr.progress();
            b.pkt.quads.push(Quad {
                rect: stage_rect,
                uv: [0.0, 0.0, 1.0, 1.0],
                corners: None,
                color: [1.0, 1.0, 1.0, 1.0],
                asset: None,
                blend: BlendMode::Normal,
                clip: None,
                special: SpecialQuad::StageTransition,
            });
        } else {
            let layer_quads = project_layers(prog, &ctx.vm.layers, stage_rect, scale);
            b.pkt.quads.extend(layer_quads);
        }

        // Story HUD (unless hidden)
        if !ctx.interface_hidden && ctx.screen == Screen::Story {
            project_story_hud(&mut b, ctx, messages, loc, stage_rect, scale);
        }

        // Modal overlays
        match ctx.screen {
            Screen::Story | Screen::Title => {}
            Screen::Menu => project_menu_overlay(&mut b, ctx, messages, loc),
            Screen::Saves { save_mode } => project_saves_overlay(&mut b, ctx, messages, loc, save_mode),
            Screen::Settings => project_settings_overlay(&mut b, ctx, messages, loc),
            Screen::History => project_history_overlay(&mut b, ctx, messages, loc),
            Screen::Gallery => project_gallery_overlay(&mut b, ctx, messages, loc),
            Screen::Ended => project_ended_overlay(&mut b, messages, loc),
        }
    }

    // Lightweight Toast notification (replaces 1,500 lines of recovery UI state machines!)
    if let Some(msg) = ctx.toast {
        let tw = (width * 0.5).clamp(240.0, 520.0);
        let th = 40.0;
        let rect = [(width - tw) * 0.5, 18.0, tw, th];
        b.pkt.quads.push(Quad::solid(rect, [0.08, 0.11, 0.16, 0.92]));
        b.pkt.quads.push(Quad::solid(
            [rect[0], rect[1] + th - 2.0, tw, 2.0],
            b.theme.accent,
        ));
        b.push_text(TextRun {
            text: msg.to_owned(),
            visible_bytes: None,
            x: rect[0] + 16.0,
            y: rect[1] + 9.0,
            width: tw - 32.0,
            height: th,
            size: 16.0,
            line_height: 22.0,
            color: b.theme.text,
            emphasis: Vec::new(),
            shadow: None,
            scroll: 0.0,
            clip: Some(rect),
            font_assets: b.ui_fonts.clone(),
            cache_key: 0,
        });
    }

    b.pkt
}

fn project_title(
    b: &mut Builder<'_>,
    ctx: &UiContext<'_>,
    messages: &Messages,
    loc: &str,
    stage_rect: [f32; 4],
    scale: f32,
) {
    let prog = ctx.vm.program();
    if let Some(custom) = &prog.ui.title_screen {
        for node in &custom.nodes {
            project_ui_node(b, prog, ctx, node, stage_rect, scale);
        }
        return;
    }

    // Sleek built-in title screen
    let w = b.pkt.width;
    let h = b.pkt.height;
    b.pkt
        .quads
        .push(Quad::solid([0.0, 0.0, w, h], [0.04, 0.06, 0.10, 0.45]));

    let title_size = (h * 0.075).clamp(30.0, 64.0);
    b.push_text(TextRun {
        text: prog.meta.title.clone(),
        visible_bytes: None,
        x: w * 0.08,
        y: h * 0.22,
        width: w * 0.84,
        height: title_size * 2.0,
        size: title_size,
        line_height: title_size * 1.25,
        color: b.theme.text,
        emphasis: Vec::new(),
        shadow: b.theme.text_shadow,
        scroll: 0.0,
        clip: None,
        font_assets: b.ui_fonts.clone(),
        cache_key: 0,
    });

    let bw = (w * 0.24).clamp(200.0, 320.0);
    let bh = 48.0;
    let bx = w * 0.08;
    let mut by = h * 0.46;
    let gap = 12.0;

    b.button(
        [bx, by, bw, bh],
        messages.get(loc, "new-game"),
        UiAction::NewGame,
        true,
        true,
    );
    by += bh + gap;

    let has_save = ctx.slots.iter().any(|s| s.exists);
    b.button(
        [bx, by, bw, bh],
        messages.get(loc, "load"),
        UiAction::OpenSaves { save_mode: false },
        has_save,
        false,
    );
    by += bh + gap;

    if !prog.ui.gallery.is_empty() {
        b.button(
            [bx, by, bw, bh],
            if loc.starts_with("zh") {
                "鉴赏 / 回想".into()
            } else {
                "Gallery".into()
            },
            UiAction::OpenGallery,
            true,
            false,
        );
        by += bh + gap;
    }

    b.button(
        [bx, by, bw, bh],
        messages.get(loc, "settings"),
        UiAction::OpenSettings,
        true,
        false,
    );
}

fn project_ui_node(
    b: &mut Builder<'_>,
    prog: &NirProgram,
    ctx: &UiContext<'_>,
    node: &UiNode,
    stage_rect: [f32; 4],
    scale: f32,
) {
    if let Some(flag_id) = node.visible_flag {
        if !ctx.vm.profile.flags.contains(prog.str(flag_id)) {
            return;
        }
    }
    let rect = [
        stage_rect[0] + node.rect[0] * scale,
        stage_rect[1] + node.rect[1] * scale,
        node.rect[2] * scale,
        node.rect[3] * scale,
    ];
    match &node.widget {
        UiWidget::Container { bg_color, children } => {
            if bg_color[3] > 0.0 {
                let mut c = *bg_color;
                c[3] *= node.opacity;
                b.pkt.quads.push(Quad::solid(rect, c));
            }
            for child in children {
                project_ui_node(b, prog, ctx, child, stage_rect, scale);
            }
        }
        UiWidget::Stack { children, .. } => {
            for child in children {
                project_ui_node(b, prog, ctx, child, stage_rect, scale);
            }
        }
        UiWidget::Image { asset } => {
            let mut q = Quad::image(rect, *asset);
            q.color[3] = node.opacity;
            b.pkt.quads.push(q);
        }
        UiWidget::Text { text, size, color } => {
            let mut c = *color;
            c[3] *= node.opacity;
            b.push_text(TextRun {
                text: prog.str(*text).to_owned(),
                visible_bytes: None,
                x: rect[0],
                y: rect[1],
                width: rect[2],
                height: rect[3],
                size: size * scale,
                line_height: size * scale * 1.3,
                color: c,
                emphasis: Vec::new(),
                shadow: b.theme.text_shadow,
                scroll: 0.0,
                clip: Some(rect),
                font_assets: b.ui_fonts.clone(),
                cache_key: 0,
            });
        }
        UiWidget::Button {
            label,
            image,
            hover_image,
            action,
        } => {
            let id = b.next_hit_id;
            b.next_hit_id += 1;
            let is_hover = b.hovered == Some(id);
            if let Some(img) = if is_hover { hover_image.or(*image) } else { *image } {
                let mut q = Quad::image(rect, img);
                q.color[3] = node.opacity;
                b.pkt.quads.push(q);
            } else {
                b.next_hit_id -= 1;
                b.button(rect, prog.str(*label).to_owned(), action.clone(), true, false);
                return;
            }
            b.pkt.hits.push(HitRegion {
                id,
                rect,
                action: action.clone(),
                label: prog.str(*label).to_owned(),
                enabled: true,
            });
        }
    }
}

fn project_story_hud(
    b: &mut Builder<'_>,
    ctx: &UiContext<'_>,
    messages: &Messages,
    loc: &str,
    stage_rect: [f32; 4],
    scale: f32,
) {
    let w = b.pkt.width;
    let h = b.pkt.height;
    let alpha = ctx.vm.dialogue_alpha();

    // Dialogue box
    if alpha > 0.01 {
        if let Some(dlg) = &ctx.vm.dialogue {
            let box_rect = if let Some(r) = b.theme.dialogue_rect {
                [
                    stage_rect[0] + r[0] * scale,
                    stage_rect[1] + r[1] * scale,
                    r[2] * scale,
                    r[3] * scale,
                ]
            } else {
                let bh = (h * 0.27).clamp(150.0, 240.0);
                let bw = (w * 0.88).min(1160.0);
                [(w - bw) * 0.5, h - bh - 18.0, bw, bh]
            };

            if let Some(bg) = b.theme.dialogue_bg {
                let mut q = Quad::image(box_rect, bg);
                q.color[3] = alpha;
                b.pkt.quads.push(q);
            } else {
                let mut panel = b.theme.panel;
                panel[3] *= alpha;
                b.pkt.quads.push(Quad::solid(box_rect, panel));
                let mut border = b.theme.panel_border;
                border[3] *= alpha;
                b.pkt.quads.push(Quad::solid(
                    [box_rect[0], box_rect[1], box_rect[2], 2.0],
                    border,
                ));
            }

            let shake = ctx.vm.dialogue_shake;
            let text_rect = if let Some(tr) = b.theme.text_rect {
                [
                    stage_rect[0] + (tr[0] + shake[0]) * scale,
                    stage_rect[1] + (tr[1] + shake[1]) * scale,
                    tr[2] * scale,
                    tr[3] * scale,
                ]
            } else {
                [
                    box_rect[0] + 28.0 + shake[0] * scale,
                    box_rect[1] + (if dlg.speaker.is_empty() { 22.0 } else { 52.0 }) + shake[1] * scale,
                    box_rect[2] - 56.0,
                    box_rect[3] - 86.0,
                ]
            };

            // Speaker name badge
            if !dlg.speaker.is_empty() {
                let mut sc = dlg.speaker_color.unwrap_or(b.theme.accent);
                sc[3] *= alpha;
                let sp_size = (b.theme.font_size * ctx.prefs.font_scale * 0.92).clamp(16.0, 32.0);
                b.push_text(TextRun {
                    text: dlg.speaker.clone(),
                    visible_bytes: None,
                    x: box_rect[0] + 28.0,
                    y: box_rect[1] + 14.0,
                    width: box_rect[2] - 56.0,
                    height: sp_size * 1.5,
                    size: sp_size,
                    line_height: sp_size * 1.3,
                    color: sc,
                    emphasis: Vec::new(),
                    shadow: b.theme.text_shadow,
                    scroll: 0.0,
                    clip: Some(box_rect),
                    font_assets: b.text_fonts.clone(),
                    cache_key: 0,
                });
            }

            // Dialogue text with grapheme-cluster clipping reveal
            let fsize = (b.theme.font_size * ctx.prefs.font_scale).clamp(16.0, 44.0);
            let lh = (b.theme.line_height * ctx.prefs.font_scale).clamp(24.0, 62.0);
            let emphasis: Vec<(usize, usize)> = dlg
                .spans
                .iter()
                .filter(|s| s.emphasis)
                .map(|s| (s.byte_start, s.byte_end))
                .collect();
            let mut tc = b.theme.text;
            tc[3] *= alpha;
            b.push_text(TextRun {
                text: dlg.full_text.clone(),
                visible_bytes: if dlg.finished {
                    None
                } else {
                    Some(dlg.visible_byte_len())
                },
                x: text_rect[0],
                y: text_rect[1],
                width: text_rect[2],
                height: text_rect[3],
                size: fsize,
                line_height: lh,
                color: tc,
                emphasis,
                shadow: b.theme.text_shadow,
                scroll: 0.0,
                clip: Some(text_rect),
                font_assets: b.text_fonts.clone(),
                cache_key: 0,
            });

            // Compact bottom quick-action toolbar
            let bar_h = 30.0;
            let bar_y = box_rect[1] + box_rect[3] - bar_h - 8.0;
            let buttons: [(&str, UiAction, bool); 7] = [
                ("auto", UiAction::ToggleAuto, ctx.auto),
                ("skip", UiAction::ToggleSkip, ctx.skip),
                ("history-compact", UiAction::OpenHistory, false),
                ("rollback", UiAction::Rollback, false),
                ("save", UiAction::OpenSaves { save_mode: true }, !ctx.replay_active),
                ("hide-interface", UiAction::ToggleHideUi, false),
                ("menu", UiAction::OpenMenu, false),
            ];
            let btn_w = ((box_rect[2] - 48.0) / buttons.len() as f32).clamp(56.0, 108.0);
            let total_w = btn_w * buttons.len() as f32 + 6.0 * (buttons.len() - 1) as f32;
            let mut bx = box_rect[0] + box_rect[2] - total_w - 16.0;
            for (key, action, active) in buttons {
                let enabled = !matches!(action, UiAction::OpenSaves { save_mode: true }) || !ctx.replay_active;
                b.button(
                    [bx, bar_y, btn_w, bar_h],
                    messages.get(loc, key),
                    action,
                    enabled,
                    active,
                );
                bx += btn_w + 6.0;
            }
        }
    }

    // Choice buttons
    if let Some(choice) = &ctx.vm.choice {
        let count = choice.items.len();
        let cw = (w * 0.52).clamp(280.0, 620.0);
        let ch = 52.0;
        let gap = 14.0;
        let total_h = count as f32 * ch + count.saturating_sub(1) as f32 * gap;
        let mut cy = (h * 0.42 - total_h * 0.5).max(36.0);
        for (i, item) in choice.items.iter().enumerate() {
            let rect = if let Some(r) = item.rect {
                [
                    stage_rect[0] + r[0] * scale,
                    stage_rect[1] + r[1] * scale,
                    r[2] * scale,
                    r[3] * scale,
                ]
            } else {
                let r = [(w - cw) * 0.5, cy, cw, ch];
                cy += ch + gap;
                r
            };
            if let Some(img) = item.image {
                let id = b.next_hit_id;
                b.next_hit_id += 1;
                let asset = if b.hovered == Some(id) {
                    item.hover_image.unwrap_or(img)
                } else {
                    img
                };
                b.pkt.quads.push(Quad::image(rect, asset));
                b.pkt.hits.push(HitRegion {
                    id,
                    rect,
                    action: UiAction::Choose(i as u16),
                    label: item.label.clone(),
                    enabled: true,
                });
            } else {
                b.button(rect, item.label.clone(), UiAction::Choose(i as u16), true, false);
            }
        }
    }
}

fn overlay_shell(b: &mut Builder<'_>, title: String, messages: &Messages, loc: &str) -> [f32; 4] {
    let w = b.pkt.width;
    let h = b.pkt.height;
    b.pkt
        .quads
        .push(Quad::solid([0.0, 0.0, w, h], [0.03, 0.05, 0.09, 0.78]));
    let pw = (w * 0.80).clamp(320.0, 960.0);
    let ph = (h * 0.80).clamp(320.0, 640.0);
    let px = (w - pw) * 0.5;
    let py = (h - ph) * 0.5;
    b.pkt.quads.push(Quad::solid([px, py, pw, ph], b.theme.surface));
    b.pkt
        .quads
        .push(Quad::solid([px, py, pw, 2.0], b.theme.accent));
    b.push_text(TextRun {
        text: title,
        visible_bytes: None,
        x: px + 24.0,
        y: py + 18.0,
        width: pw - 160.0,
        height: 40.0,
        size: 26.0,
        line_height: 34.0,
        color: b.theme.text,
        emphasis: Vec::new(),
        shadow: None,
        scroll: 0.0,
        clip: None,
        font_assets: b.ui_fonts.clone(),
        cache_key: 0,
    });
    b.button(
        [px + pw - 124.0, py + 14.0, 100.0, 38.0],
        messages.get(loc, "back"),
        UiAction::CloseOverlay,
        true,
        false,
    );
    [px + 24.0, py + 72.0, pw - 48.0, ph - 96.0]
}

fn project_menu_overlay(b: &mut Builder<'_>, ctx: &UiContext<'_>, messages: &Messages, loc: &str) {
    let area = overlay_shell(b, messages.get(loc, "menu"), messages, loc);
    let bw = area[2].min(360.0);
    let bx = area[0] + (area[2] - bw) * 0.5;
    let mut by = area[1] + 12.0;
    let bh = 46.0;
    let gap = 12.0;

    let items: [(&str, UiAction, bool, bool); 6] = [
        ("close", UiAction::CloseOverlay, true, true),
        ("save", UiAction::OpenSaves { save_mode: true }, !ctx.replay_active, false),
        ("load", UiAction::OpenSaves { save_mode: false }, !ctx.replay_active, false),
        ("history", UiAction::OpenHistory, true, false),
        ("settings", UiAction::OpenSettings, true, false),
        (
            if ctx.replay_active { "back" } else { "exit" },
            if ctx.replay_active {
                UiAction::ExitReplay
            } else {
                UiAction::ReturnTitle
            },
            true,
            false,
        ),
    ];
    for (key, action, enabled, primary) in items {
        b.button([bx, by, bw, bh], messages.get(loc, key), action, enabled, primary);
        by += bh + gap;
    }
}

fn project_saves_overlay(
    b: &mut Builder<'_>,
    ctx: &UiContext<'_>,
    messages: &Messages,
    loc: &str,
    save_mode: bool,
) {
    let title = messages.get(loc, if save_mode { "save" } else { "load" });
    let area = overlay_shell(b, title, messages, loc);
    let cols = if area[2] >= 640.0 { 2 } else { 1 };
    let gap = 12.0;
    let cell_w = (area[2] - gap * (cols - 1) as f32) / cols as f32;
    let cell_h = 66.0;

    for (idx, slot) in ctx.slots.iter().take(12).enumerate() {
        let col = idx % cols;
        let row = idx / cols;
        let x = area[0] + col as f32 * (cell_w + gap);
        let y = area[1] + row as f32 * (cell_h + gap);
        if y + cell_h > area[1] + area[3] {
            break;
        }
        let label = if slot.exists {
            format!("#{:02}  {}", slot.slot + 1, slot.summary)
        } else {
            format!("#{:02}  {}", slot.slot + 1, messages.get(loc, "empty-slot"))
        };
        let action = if save_mode {
            UiAction::SaveSlot(slot.slot)
        } else {
            UiAction::LoadSlot(slot.slot)
        };
        let enabled = save_mode || slot.exists;
        b.button([x, y, cell_w, cell_h], label, action, enabled, false);
    }
}

fn project_settings_overlay(b: &mut Builder<'_>, ctx: &UiContext<'_>, messages: &Messages, loc: &str) {
    let area = overlay_shell(b, messages.get(loc, "settings"), messages, loc);
    let mut y = area[1];
    let row_h = 46.0;
    let gap = 12.0;

    let sliders: [(&str, AudioBus, f32); 3] = [
        ("music", AudioBus::Bgm, ctx.prefs.bgm_volume),
        ("voice", AudioBus::Voice, ctx.prefs.voice_volume),
        ("sfx", AudioBus::Sfx, ctx.prefs.sfx_volume),
    ];
    for (key, bus, val) in sliders {
        b.push_text(TextRun {
            text: format!("{}: {}%", messages.get(loc, key), (val * 100.0).round() as u32),
            visible_bytes: None,
            x: area[0],
            y: y + 10.0,
            width: 240.0,
            height: row_h,
            size: 18.0,
            line_height: 24.0,
            color: b.theme.text,
            emphasis: Vec::new(),
            shadow: None,
            scroll: 0.0,
            clip: None,
            font_assets: b.ui_fonts.clone(),
            cache_key: 0,
        });
        b.button(
            [area[0] + 250.0, y, 54.0, row_h],
            "−".into(),
            UiAction::SetVolume {
                bus,
                value: (val - 0.1).clamp(0.0, 1.0),
            },
            true,
            false,
        );
        b.button(
            [area[0] + 314.0, y, 54.0, row_h],
            "+".into(),
            UiAction::SetVolume {
                bus,
                value: (val + 0.1).clamp(0.0, 1.0),
            },
            true,
            false,
        );
        y += row_h + gap;
    }

    // Text speed
    b.push_text(TextRun {
        text: format!("{}: {:.1}x", messages.get(loc, "text-speed"), ctx.prefs.text_speed),
        visible_bytes: None,
        x: area[0],
        y: y + 10.0,
        width: 240.0,
        height: row_h,
        size: 18.0,
        line_height: 24.0,
        color: b.theme.text,
        emphasis: Vec::new(),
        shadow: None,
        scroll: 0.0,
        clip: None,
        font_assets: b.ui_fonts.clone(),
        cache_key: 0,
    });
    b.button(
        [area[0] + 250.0, y, 54.0, row_h],
        "−".into(),
        UiAction::SetTextSpeed((ctx.prefs.text_speed - 0.5).clamp(0.5, 4.0)),
        true,
        false,
    );
    b.button(
        [area[0] + 314.0, y, 54.0, row_h],
        "+".into(),
        UiAction::SetTextSpeed((ctx.prefs.text_speed + 0.5).clamp(0.5, 4.0)),
        true,
        false,
    );
    y += row_h + gap;

    // Language switchers
    let mut lx = area[0];
    for locale in &ctx.vm.program().meta.locales {
        let active = locale == &ctx.prefs.text_locale;
        b.button(
            [lx, y, 110.0, row_h],
            locale.clone(),
            UiAction::SetLocale(locale.clone()),
            true,
            active,
        );
        lx += 120.0;
    }
}

fn project_history_overlay(b: &mut Builder<'_>, ctx: &UiContext<'_>, messages: &Messages, loc: &str) {
    let area = overlay_shell(b, messages.get(loc, "history"), messages, loc);
    let row_h = 64.0;
    let visible_rows = ((area[3] - 48.0) / row_h).max(1.0) as usize;
    let total = ctx.vm.history.len();
    let end = total.saturating_sub(ctx.history_scroll);
    let start = end.saturating_sub(visible_rows);

    let mut y = area[1];
    for entry in ctx.vm.history.range(start..end) {
        let text = if entry.speaker.is_empty() {
            entry.text.clone()
        } else {
            format!("【{}】 {}", entry.speaker, entry.text)
        };
        b.push_text(TextRun {
            text,
            visible_bytes: None,
            x: area[0] + 8.0,
            y,
            width: area[2] - 16.0,
            height: row_h - 6.0,
            size: 18.0,
            line_height: 26.0,
            color: b.theme.text,
            emphasis: Vec::new(),
            shadow: None,
            scroll: 0.0,
            clip: Some([area[0], y, area[2], row_h - 4.0]),
            font_assets: b.text_fonts.clone(),
            cache_key: 0,
        });
        y += row_h;
    }

    let by = area[1] + area[3] - 40.0;
    b.button(
        [area[0], by, 120.0, 38.0],
        messages.get(loc, "scroll-back"),
        UiAction::ScrollHistory(3),
        start > 0,
        false,
    );
    b.button(
        [area[0] + 132.0, by, 120.0, 38.0],
        messages.get(loc, "scroll-forward"),
        UiAction::ScrollHistory(-3),
        ctx.history_scroll > 0,
        false,
    );
}

fn project_gallery_overlay(b: &mut Builder<'_>, ctx: &UiContext<'_>, messages: &Messages, loc: &str) {
    let prog = ctx.vm.program();
    let area = overlay_shell(
        b,
        if loc.starts_with("zh") {
            "鉴赏 / 回想".into()
        } else {
            "Gallery".into()
        },
        messages,
        loc,
    );
    let cols = 3;
    let gap = 14.0;
    let cell_w = (area[2] - gap * (cols - 1) as f32) / cols as f32;
    let cell_h = 110.0;

    for (idx, item) in prog.ui.gallery.iter().enumerate() {
        let col = idx % cols;
        let row = idx / cols;
        let x = area[0] + col as f32 * (cell_w + gap);
        let y = area[1] + row as f32 * (cell_h + gap);
        if y + cell_h > area[1] + area[3] {
            break;
        }
        let unlocked = item
            .unlock_flag
            .is_none_or(|f| ctx.vm.profile.flags.contains(prog.str(f)));
        let label = if unlocked {
            prog.str(item.title).to_owned()
        } else {
            "???".into()
        };
        if let Some(func) = item.replay_func {
            b.button(
                [x, y, cell_w, cell_h],
                label,
                UiAction::StartReplay(func),
                unlocked,
                false,
            );
        } else {
            b.pkt.quads.push(Quad::solid([x, y, cell_w, cell_h], b.theme.panel));
            if unlocked {
                if let Some(thumb) = item.thumb {
                    b.pkt.quads.push(Quad::image([x, y, cell_w, cell_h], thumb));
                }
            }
        }
    }
}

fn project_ended_overlay(b: &mut Builder<'_>, messages: &Messages, loc: &str) {
    let area = overlay_shell(b, messages.get(loc, "ending"), messages, loc);
    b.button(
        [area[0] + (area[2] - 240.0) * 0.5, area[1] + 60.0, 240.0, 50.0],
        messages.get(loc, "exit"),
        UiAction::ReturnTitle,
        true,
        true,
    );
}
