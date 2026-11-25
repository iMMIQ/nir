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
use nir_vm::{ActiveTimeline, LayerState, Vm};

/// Reserved virtual `AssetId` range for host-uploaded save-slot thumbnails (`SAVE_THUMB_ASSET_BASE + slot`).
pub const SAVE_THUMB_ASSET_BASE: u32 = 0xFFFF_0000;
pub const AUTOSAVE_THUMB_ASSET_ID: AssetId = AssetId(SAVE_THUMB_ASSET_BASE + 1000);
pub const QUICKSAVE_THUMB_ASSET_ID: AssetId = AssetId(SAVE_THUMB_ASSET_BASE + 1001);

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmKind {
    OverwriteSlot(u32),
    DeleteSlot(u32),
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SlotSummary {
    pub slot: u32,
    pub exists: bool,
    pub summary: String,
    pub saved_at_unix: u64,
    #[serde(default)]
    pub has_thumb: bool,
}

/// Safe-area insets in logical pixels (notch, Dynamic Island, status bar, Home Indicator).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SafeInsets {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl SafeInsets {
    /// Clamps insets to reasonable bounds and returns the usable inner rect `[x, y, w, h]`.
    pub fn rect(self, width: f32, height: f32) -> [f32; 4] {
        let l = self.left.clamp(0.0, width * 0.25);
        let r = self.right.clamp(0.0, width * 0.25);
        let t = self.top.clamp(0.0, height * 0.25);
        let b = self.bottom.clamp(0.0, height * 0.25);
        [
            l,
            t,
            (width - l - r).max(120.0),
            (height - t - b).max(120.0),
        ]
    }
}

/// Active pointer modality for responsive touch ergonomics vs desktop pointer ergonomics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InputMode {
    #[default]
    Pointer,
    Touch,
}

/// Read-only projection input supplied by `nir-runtime`.
pub struct UiContext<'a> {
    pub vm: &'a Vm,
    pub screen: Screen,
    pub prefs: &'a Preferences,
    pub auto: bool,
    pub skip: bool,
    pub hold_skip: bool,
    pub interface_hidden: bool,
    pub replay_active: bool,
    pub slots: &'a [SlotSummary],
    pub autosave: Option<&'a SlotSummary>,
    pub quicksave: Option<&'a SlotSummary>,
    pub saves_page: usize,
    pub history_scroll: usize,
    pub gallery_page: usize,
    pub viewing_cg: Option<(usize, usize)>,
    /// CG lightbox zoom multiplier (`1.0..=3.0`).
    pub cg_zoom: f32,
    /// CG lightbox pan offset in logical pixels while zoomed in.
    pub cg_pan: [f32; 2],
    pub settings_tab: u8,
    pub pending_confirm: Option<ConfirmKind>,
    pub host_has_shelf: bool,
    pub loading_visible: bool,
    pub tz_offset_mins: i32,
    pub hovered_hit: Option<u32>,
    pub pressed_hit: Option<u32>,
    pub safe_insets: SafeInsets,
    pub input_mode: InputMode,
    pub portrait_reader: bool,
    pub toast: Option<&'a str>,
    /// Smooth 0..1 modal overlay reveal animation progress (`1.0` when static or `reduced_motion`).
    pub overlay_anim: f32,
    /// Smooth 0..1 toast slide/fade progress (`1.0` when static or `reduced_motion`).
    pub toast_anim: f32,
    /// Smooth 0..1 dialogue line transition alpha (`1.0` when static or `reduced_motion`).
    pub dialogue_fade: f32,
    /// Monotonic UI animation clock in seconds (used only for subtle breathing indicator when waiting).
    pub ui_time_s: f32,
    /// True when desktop pointer hovers the quick-action toolbar (brightens ghost pill bar).
    pub hud_toolbar_hovered: bool,
    /// Extra top-left logical px clearance reserved for host chrome (e.g. Web `#shelf-btn`).
    pub host_top_left_pad: f32,
    /// Extra top-right logical px clearance reserved for host chrome (e.g. Web `#fs-btn`).
    pub host_top_right_pad: f32,
}

/// Pure-Rust civil date/time formatter (`YYYY-MM-DD HH:MM`) using Howard Hinnant's algorithm.
pub fn format_unix_hm(unix_secs: u64, tz_offset_mins: i32) -> String {
    if unix_secs == 0 {
        return String::new();
    }
    let shifted = (unix_secs as i64).saturating_add(tz_offset_mins as i64 * 60);
    let days = shifted.div_euclid(86_400);
    let secs_of_day = shifted.rem_euclid(86_400) as u32;
    let hour = secs_of_day / 3600;
    let min = (secs_of_day % 3600) / 60;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + i64::from(m <= 2);
    format!("{year:04}-{m:02}-{d:02} {hour:02}:{min:02}")
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

/// Computes top-docked stage viewport inside `[area_x, area_y, area_w, area_h]` (used by Portrait Docked Reader).
fn fit_stage_top_in(stage_w: f32, stage_h: f32, area: [f32; 4]) -> ([f32; 4], f32) {
    let scale = (area[2] / stage_w.max(1.0)).min(area[3] / stage_h.max(1.0));
    let w = stage_w * scale;
    let h = stage_h * scale;
    let x = area[0] + (area[2] - w) * 0.5;
    let y = area[1];
    ([x, y, w, h], scale)
}

fn is_portrait_docked(ctx: &UiContext<'_>, width: f32, height: f32) -> bool {
    if !ctx.portrait_reader || ctx.screen != Screen::Story {
        return false;
    }
    if height <= width * 1.12 {
        return false;
    }
    // Keep exact stage mapping when an image/positioned choice menu (e.g. LiveMaker .lpm) is active.
    let choice_hides_dialogue = ctx.vm.choice.as_ref().is_some_and(|c| {
        c.background.is_some()
            || c.items
                .iter()
                .any(|it| it.image.is_some() || it.rect.is_some())
    });
    !choice_hides_dialogue
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
            let Some(p) = layers.iter().find(|l| l.id == pid) else {
                break;
            };
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
        // Text-only layers are projected into `DrawPacket.texts` by `project_layer_texts`.
        if layer.text.is_some() && layer.asset.is_none() {
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
        let (draw_off_x, draw_off_y, draw_w, draw_h) =
            if let Some(trim) = asset_meta.and_then(|a| a.trim) {
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
            ..Quad::solid(rect, color)
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

fn layer_world_pose(layers: &[LayerState], layer: &LayerState) -> ([f32; 2], [f32; 2], f32) {
    let mut px = layer.transform.x + layer.shake_offset[0];
    let mut py = layer.transform.y + layer.shake_offset[1];
    let mut sx = layer.transform.scale[0];
    let mut sy = layer.transform.scale[1];
    let mut op = layer.transform.opacity;
    let mut cur_parent = layer.parent;
    for _ in 0..16 {
        let Some(pid) = cur_parent else { break };
        let Some(p) = layers.iter().find(|l| l.id == pid) else {
            break;
        };
        px = p.transform.x + p.shake_offset[0] + px * p.transform.scale[0];
        py = p.transform.y + p.shake_offset[1] + py * p.transform.scale[1];
        sx *= p.transform.scale[0];
        sy *= p.transform.scale[1];
        op *= p.transform.opacity;
        cur_parent = p.parent;
    }
    ([px, py], [sx, sy], op)
}

fn project_layer_texts(
    b: &mut Builder<'_>,
    layers: &[LayerState],
    stage_rect: [f32; 4],
    scale: f32,
) {
    for layer in layers {
        let Some(lt) = &layer.text else { continue };
        let ([px, py], [sx, sy], op) = layer_world_pose(layers, layer);
        if op <= 0.001 || lt.text.is_empty() {
            continue;
        }
        let mut color = lt.color;
        color[3] *= op;
        let size = (lt.font_size * sy * scale).clamp(8.0, 256.0);
        let box_w = if layer.transform.width > 0.0 {
            layer.transform.width * sx * scale
        } else {
            (lt.text.chars().count().max(1) as f32 * size * 1.1).max(64.0)
        };
        let box_h = if layer.transform.height > 0.0 {
            layer.transform.height * sy * scale
        } else {
            size * 1.6
        };
        let base_x = stage_rect[0] + px * scale;
        let x = match lt.align {
            TextAlign::Left => base_x,
            TextAlign::Center => base_x - box_w * 0.5,
            TextAlign::Right => base_x - box_w,
        };
        let y = stage_rect[1] + py * scale;
        b.push_text(TextRun {
            text: lt.text.clone(),
            visible_bytes: None,
            x,
            y,
            width: box_w.max(8.0),
            height: box_h.max(8.0),
            size,
            line_height: size * 1.25,
            color,
            emphasis: Vec::new(),
            color_spans: Vec::new(),
            ruby_spans: Vec::new(),
            shadow: b.theme.text_shadow,
            outline: None,
            scroll: 0.0,
            clip: None,
            font_assets: b.text_fonts.clone(),
            align: lt.align,
            valign_center: false,
            auto_fit: false,
            inline_images: Vec::new(),
            cache_key: 0,
        });
    }
}

fn project_layers_and_timelines(
    program: &NirProgram,
    layers: &[LayerState],
    timelines: &[ActiveTimeline],
    stage_rect: [f32; 4],
    scale: f32,
) -> Vec<Quad> {
    if timelines.is_empty() {
        return project_layers(program, layers, stage_rect, scale);
    }
    // Sort layers by (z_order, id) and interleave each timeline right at its host layer's z_order!
    let mut ordered: Vec<&LayerState> = layers.iter().collect();
    ordered.sort_by_key(|l| (l.transform.z_order, l.id.0));

    let mut out = Vec::with_capacity(layers.len() + timelines.len() * 2);
    let mut emitted_tl = vec![false; timelines.len()];
    for layer in ordered {
        let one = project_layers(program, std::slice::from_ref(layer), stage_rect, scale);
        out.extend(one);
        for (idx, tl) in timelines.iter().enumerate() {
            if !emitted_tl[idx] && tl.layer == layer.id {
                emitted_tl[idx] = true;
                out.extend(project_timelines(
                    program,
                    layers,
                    std::slice::from_ref(tl),
                    stage_rect,
                    scale,
                ));
            }
        }
    }
    // Any timeline whose host layer is absent renders on top cleanly.
    for (idx, tl) in timelines.iter().enumerate() {
        if !emitted_tl[idx] {
            out.extend(project_timelines(
                program,
                layers,
                std::slice::from_ref(tl),
                stage_rect,
                scale,
            ));
        }
    }
    out
}

fn project_timelines(
    program: &NirProgram,
    layers: &[LayerState],
    timelines: &[ActiveTimeline],
    stage_rect: [f32; 4],
    scale: f32,
) -> Vec<Quad> {
    if timelines.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for active in timelines {
        let Some(def) = program.timelines.get(active.timeline.0 as usize) else {
            continue;
        };
        let ([base_x, base_y], [base_sx, base_sy], base_op) = layers
            .iter()
            .find(|l| l.id == active.layer)
            .map(|l| layer_world_pose(layers, l))
            .unwrap_or(([0.0, 0.0], [1.0, 1.0], 1.0));
        if base_op <= 0.001 {
            continue;
        }
        let fps = def.fps.max(1.0) as f64;
        let frame = (active.elapsed_us as f64 / 1_000_000.0) * fps;
        let mut active_clips: Vec<&TimelineClip> = def
            .clips
            .iter()
            .filter(|c| {
                let start = c.start_frame as f64;
                let end = (c.start_frame + c.duration_frames) as f64;
                frame >= start && frame <= end
            })
            .collect();
        active_clips.sort_by_key(|c| c.z_order);

        for clip in active_clips {
            let t = if clip.duration_frames == 0 {
                1.0
            } else {
                ((frame - clip.start_frame as f64) / clip.duration_frames as f64).clamp(0.0, 1.0)
                    as f32
            };
            match &clip.kind {
                TimelineClipKind::Image {
                    asset,
                    pivot,
                    pos_start,
                    pos_end,
                    pos_easing,
                    angle,
                    angle_easing,
                    scale_x,
                    scale_y,
                    scale_easing,
                    opacity,
                    clip_start: _,
                    clip_end: _,
                } => {
                    let op = interpolate(opacity[0], opacity[1], t, Easing::Linear) * base_op;
                    if op <= 0.001 {
                        continue;
                    }
                    let lx = interpolate(pos_start[0], pos_end[0], t, pos_easing[0]);
                    let ly = interpolate(pos_start[1], pos_end[1], t, pos_easing[1]);
                    let sx = interpolate(scale_x[0], scale_x[1], t, scale_easing[0]) * base_sx;
                    let sy = interpolate(scale_y[0], scale_y[1], t, scale_easing[1]) * base_sy;
                    let deg = interpolate(angle[0], angle[1], t, *angle_easing);

                    let asset_meta = program.asset(*asset);
                    let (orig_w, orig_h) = asset_meta.map_or((0.0, 0.0), |a| {
                        if let Some(tr) = a.trim {
                            (tr.orig_w as f32, tr.orig_h as f32)
                        } else {
                            (a.width as f32, a.height as f32)
                        }
                    });
                    if orig_w <= 0.0 || orig_h <= 0.0 {
                        continue;
                    }
                    let (draw_off_x, draw_off_y, draw_w, draw_h) =
                        if let Some(tr) = asset_meta.and_then(|a| a.trim) {
                            (tr.x as f32, tr.y as f32, tr.w as f32, tr.h as f32)
                        } else {
                            (0.0, 0.0, orig_w, orig_h)
                        };
                    let cx = stage_rect[0] + (base_x + lx) * scale;
                    let cy = stage_rect[1] + (base_y + ly) * scale;
                    let local_x0 = (draw_off_x - pivot[0]) * sx;
                    let local_y0 = (draw_off_y - pivot[1]) * sy;
                    let local_w = draw_w * sx;
                    let local_h = draw_h * sy;
                    let rect = [
                        cx + local_x0 * scale,
                        cy + local_y0 * scale,
                        local_w * scale,
                        local_h * scale,
                    ];
                    let corners = if deg.abs() > 0.01 {
                        let rad = deg.to_radians();
                        let (sin, cos) = rad.sin_cos();
                        let rot = |rx: f32, ry: f32| {
                            [
                                cx + (rx * cos - ry * sin) * scale,
                                cy + (rx * sin + ry * cos) * scale,
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
                    let col = [1.0, 1.0, 1.0, op.clamp(0.0, 1.0)];
                    out.push(Quad {
                        rect,
                        uv: [0.0, 0.0, 1.0, 1.0],
                        corners,
                        color: col,
                        asset: Some(*asset),
                        ..Quad::solid(rect, col)
                    });
                }
                TimelineClipKind::Box {
                    pos_start,
                    pos_end,
                    size_start,
                    size_end,
                    color,
                    opacity,
                } => {
                    let op = interpolate(opacity[0], opacity[1], t, Easing::Linear) * base_op;
                    if op <= 0.001 {
                        continue;
                    }
                    let lx = interpolate(pos_start[0], pos_end[0], t, Easing::Linear);
                    let ly = interpolate(pos_start[1], pos_end[1], t, Easing::Linear);
                    let lw = interpolate(size_start[0], size_end[0], t, Easing::Linear) * base_sx;
                    let lh = interpolate(size_start[1], size_end[1], t, Easing::Linear) * base_sy;
                    let mut c = *color;
                    c[3] *= op.clamp(0.0, 1.0);
                    out.push(Quad::solid(
                        [
                            stage_rect[0] + (base_x + lx) * scale,
                            stage_rect[1] + (base_y + ly) * scale,
                            lw * scale,
                            lh * scale,
                        ],
                        c,
                    ));
                }
                TimelineClipKind::Sound { .. } => {}
            }
        }
    }
    out
}

fn cover_uv(src_w: f32, src_h: f32, dst_w: f32, dst_h: f32) -> [f32; 4] {
    let src_ar = src_w.max(1.0) / src_h.max(1.0);
    let dst_ar = dst_w.max(1.0) / dst_h.max(1.0);
    if dst_ar > src_ar {
        let vis_v = (src_ar / dst_ar).clamp(0.15, 1.0);
        let pad = (1.0 - vis_v) * 0.5;
        [0.0, pad, 1.0, 1.0 - pad]
    } else {
        let vis_u = (dst_ar / src_ar).clamp(0.15, 1.0);
        let pad = (1.0 - vis_u) * 0.5;
        [pad, 0.0, 1.0 - pad, 1.0]
    }
}

fn ease_out_cubic(t: f32) -> f32 {
    let u = 1.0 - t.clamp(0.0, 1.0);
    1.0 - u * u * u
}

fn shade_color(c: [f32; 4], mult: f32) -> [f32; 4] {
    [
        (c[0] * mult).clamp(0.0, 1.0),
        (c[1] * mult).clamp(0.0, 1.0),
        (c[2] * mult).clamp(0.0, 1.0),
        c[3],
    ]
}

struct Builder<'a> {
    pkt: DrawPacket,
    theme: &'a UiTheme,
    ui_fonts: Vec<AssetId>,
    text_fonts: Vec<AssetId>,
    hovered: Option<u32>,
    pressed: Option<u32>,
    input_mode: InputMode,
    safe: [f32; 4],
    next_hit_id: u32,
}

impl<'a> Builder<'a> {
    fn is_touch(&self) -> bool {
        self.input_mode == InputMode::Touch
    }

    fn push_text(&mut self, mut run: TextRun) {
        run.compute_key();
        self.pkt.texts.push(run);
    }

    /// Single-pass DPI-adaptive SDF rounded panel with subtle top-lit vertical gradient & crisp stroke.
    fn rounded_panel(&mut self, rect: [f32; 4], fill: [f32; 4], border: [f32; 4], radius: f32) {
        let r = radius.min(rect[2].min(rect[3]) * 0.48);
        let top = shade_color(fill, 1.10);
        let bot = shade_color(fill, 0.90);
        self.pkt.quads.push(Quad::panel(
            rect,
            top,
            Some(bot),
            border,
            if border[3] > 0.01 { 1.0 } else { 0.0 },
            r,
        ));
    }

    /// Soft grounded drop-shadow behind elevated cards & modal dialogs.
    fn soft_shadow(&mut self, rect: [f32; 4], radius: f32, alpha: f32) {
        if alpha <= 0.01 {
            return;
        }
        let spread = 6.0;
        let srect = [
            rect[0] - spread * 0.5,
            rect[1] + 4.0,
            rect[2] + spread,
            rect[3] + spread * 0.6,
        ];
        self.pkt.quads.push(Quad::rounded(
            srect,
            [0.01, 0.02, 0.04, (0.36 * alpha).clamp(0.0, 0.6)],
            (radius + 3.0).min(srect[2].min(srect[3]) * 0.45),
        ));
    }

    fn button(
        &mut self,
        rect: [f32; 4],
        label: String,
        action: UiAction,
        enabled: bool,
        primary: bool,
    ) {
        self.button_styled(rect, label, action, enabled, primary, 1.0);
    }

    /// Styled rounded button supporting subtle ghost-mode opacity (`opacity_mult < 1.0`) for Story HUD.
    fn button_styled(
        &mut self,
        rect: [f32; 4],
        label: String,
        action: UiAction,
        enabled: bool,
        primary: bool,
        opacity_mult: f32,
    ) {
        let id = self.next_hit_id;
        self.next_hit_id += 1;
        let is_pressed = enabled && self.pressed == Some(id);
        let is_hover = enabled && !self.is_touch() && self.hovered == Some(id);
        let eff_alpha = if is_hover || is_pressed || primary {
            1.0
        } else {
            opacity_mult.clamp(0.25, 1.0)
        };
        let mut bg = if !enabled {
            [0.12, 0.15, 0.20, 0.44]
        } else if is_pressed {
            [
                (self.theme.accent[0] * 1.06).min(1.0),
                (self.theme.accent[1] * 1.06).min(1.0),
                (self.theme.accent[2] * 1.06).min(1.0),
                0.96,
            ]
        } else if is_hover {
            [
                self.theme.accent[0] * 0.90,
                self.theme.accent[1] * 0.90,
                self.theme.accent[2] * 0.90,
                0.94,
            ]
        } else if primary {
            [
                self.theme.accent[0] * 0.46,
                self.theme.accent[1] * 0.46,
                self.theme.accent[2] * 0.46,
                0.92,
            ]
        } else {
            self.theme.panel
        };
        bg[3] *= eff_alpha;
        let mut border = if !enabled {
            [0.32, 0.40, 0.52, 0.22]
        } else if is_pressed || is_hover {
            [
                (self.theme.accent[0] * 1.15).min(1.0),
                (self.theme.accent[1] * 1.15).min(1.0),
                (self.theme.accent[2] * 1.15).min(1.0),
                0.95,
            ]
        } else if primary {
            [
                self.theme.accent[0],
                self.theme.accent[1],
                self.theme.accent[2],
                0.84,
            ]
        } else {
            self.theme.panel_border
        };
        border[3] *= eff_alpha;
        let radius = (rect[3] * 0.24).clamp(6.0, 11.0);
        self.rounded_panel(rect, bg, border, radius);

        let size = (rect[3] * 0.44).clamp(11.0, 21.0);
        let pad_x = (rect[2] * 0.08).clamp(4.0, 14.0);
        let mut text_color = if !enabled {
            self.theme.muted
        } else if is_pressed || is_hover {
            [0.04, 0.06, 0.09, 1.0]
        } else {
            self.theme.text
        };
        text_color[3] *= eff_alpha.max(0.62);
        self.push_text(TextRun {
            text: label.clone(),
            visible_bytes: None,
            x: rect[0] + pad_x,
            y: rect[1] + if is_pressed { 2.0 } else { 1.0 },
            width: (rect[2] - pad_x * 2.0).max(8.0),
            height: (rect[3] - 2.0).max(8.0),
            size,
            line_height: size * 1.22,
            color: text_color,
            emphasis: Vec::new(),
            color_spans: Vec::new(),
            ruby_spans: Vec::new(),
            shadow: None,
            outline: None,
            scroll: 0.0,
            clip: Some(rect),
            font_assets: self.ui_fonts.clone(),
            align: TextAlign::Center,
            valign_center: true,
            auto_fit: true,
            inline_images: Vec::new(),
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

/// Projects the full engine frame (ambient backdrop + stage + transitions + HUD + frosted overlays + toast).
pub fn project(ctx: &UiContext<'_>, width: f32, height: f32, messages: &Messages) -> DrawPacket {
    let prog = ctx.vm.program();
    let sw = prog.meta.stage_width as f32;
    let sh = prog.meta.stage_height as f32;
    let safe = ctx.safe_insets.rect(width, height);
    let portrait_docked = is_portrait_docked(ctx, width, height);
    let (stage_rect, scale) = if portrait_docked {
        let max_stage_h = (safe[3] * 0.56).max(160.0);
        fit_stage_top_in(sw, sh, [safe[0], safe[1], safe[2], max_stage_h])
    } else {
        fit_stage(sw, sh, width, height)
    };
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
        pressed: ctx.pressed_hit,
        input_mode: ctx.input_mode,
        safe,
        next_hit_id: 1,
    };

    // Determine whether letterbox margins / portrait bottom area need an Ambient Backdrop
    let has_margins = (stage_rect[0] > 2.0)
        || (stage_rect[1] > 2.0)
        || (stage_rect[0] + stage_rect[2] < width - 2.0)
        || (stage_rect[1] + stage_rect[3] < height - 2.0);
    let has_stage_art = if ctx.screen == Screen::Title {
        prog.ui
            .title_screen
            .as_ref()
            .and_then(|s| s.background)
            .or(prog.meta.title_bg)
            .is_some()
    } else {
        ctx.vm.transition.is_some() || !ctx.vm.layers.is_empty()
    };
    let use_ambient = ctx.prefs.ambient_backdrop && has_margins && has_stage_art;
    if use_ambient {
        let mut amb = Quad::solid([0.0, 0.0, width, height], [0.26, 0.29, 0.36, 1.0]);
        amb.uv = cover_uv(stage_rect[2], stage_rect[3], width, height);
        amb.special = SpecialQuad::AmbientBackdrop;
        b.pkt.quads.push(amb);
        b.pkt.needs_blur = true;
    }

    // Subtle CAS-lite edge sharpening ONLY when classic low-res CGs are meaningfully upscaled!
    let sharpen_amt = if ctx.prefs.image_sharpening && scale > 1.22 {
        ((scale - 1.15) * 0.48).clamp(0.18, 0.56)
    } else {
        0.0
    };

    // 1. Background / Stage rendering
    let stage_start = b.pkt.quads.len();
    if ctx.screen == Screen::Title {
        if let Some(bg) = prog
            .ui
            .title_screen
            .as_ref()
            .and_then(|s| s.background)
            .or(prog.meta.title_bg)
        {
            let mut q = Quad::image(stage_rect, bg);
            q.sharpen = sharpen_amt;
            b.pkt.quads.push(q);
            b.pkt.stage_quad_range = [stage_start, b.pkt.quads.len()];
        } else {
            b.pkt.quads.push(Quad::gradient_v(
                [0.0, 0.0, width, height],
                [0.09, 0.12, 0.19, 1.0],
                [0.04, 0.06, 0.10, 1.0],
            ));
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
                special: SpecialQuad::StageTransition,
                ..Quad::solid(stage_rect, [1.0, 1.0, 1.0, 1.0])
            });
            b.pkt.stage_quad_range = [stage_start, b.pkt.quads.len()];
        } else {
            let mut stage_quads = project_layers_and_timelines(
                prog,
                &ctx.vm.layers,
                &ctx.vm.timelines,
                stage_rect,
                scale,
            );
            if sharpen_amt > 0.0 {
                for q in &mut stage_quads {
                    if q.asset.is_some() {
                        q.sharpen = sharpen_amt;
                    }
                }
            }
            b.pkt.quads.extend(stage_quads);
            b.pkt.stage_quad_range = [stage_start, b.pkt.quads.len()];
            project_layer_texts(&mut b, &ctx.vm.layers, stage_rect, scale);
        }

        // Soft feathered bottom transition in Portrait Docked Reader so artwork flows into the reader panel!
        if portrait_docked {
            let feather_h = 26.0_f32.min(stage_rect[3] * 0.14);
            let fy = stage_rect[1] + stage_rect[3] - feather_h;
            b.pkt.quads.push(Quad::gradient_v(
                [stage_rect[0], fy, stage_rect[2], feather_h + 1.0],
                [0.05, 0.07, 0.11, 0.0],
                [0.05, 0.07, 0.11, 0.72],
            ));
        }

        // Story HUD (unless hidden)
        if !ctx.interface_hidden && ctx.screen == Screen::Story {
            project_story_hud(
                &mut b,
                ctx,
                messages,
                loc,
                stage_rect,
                scale,
                portrait_docked,
            );
        }

        // Modal overlays
        match ctx.screen {
            Screen::Story | Screen::Title => {}
            Screen::Menu => project_menu_overlay(&mut b, ctx, messages, loc),
            Screen::Saves { save_mode } => {
                project_saves_overlay(&mut b, ctx, messages, loc, save_mode)
            }
            Screen::Settings => project_settings_overlay(&mut b, ctx, messages, loc),
            Screen::History => project_history_overlay(&mut b, ctx, messages, loc),
            Screen::Gallery => project_gallery_overlay(&mut b, ctx, messages, loc),
            Screen::Ended => project_ended_overlay(&mut b, ctx, messages, loc),
        }
    }

    // Optional fullscreen CG lightbox (on top of Gallery)
    if let Some((item_idx, var_idx)) = ctx.viewing_cg {
        project_cg_viewer(&mut b, ctx, messages, loc, item_idx, var_idx);
    }

    // Confirmation dialog (overwrite / delete save)
    if let Some(confirm) = ctx.pending_confirm {
        project_confirm_modal(&mut b, ctx, messages, loc, confirm);
    }

    // Subtle non-blocking loading indicator when urgent scene images take >150ms (clears host top-left chrome!)
    if ctx.loading_visible && ctx.screen == Screen::Story {
        let lw = 148.0_f32.min((safe[2] - 24.0).max(110.0));
        let lh = 30.0;
        let lrect = [
            safe[0] + 10.0 + ctx.host_top_left_pad,
            safe[1] + 10.0,
            lw,
            lh,
        ];
        b.rounded_panel(
            lrect,
            [0.06, 0.09, 0.14, 0.88],
            [b.theme.accent[0], b.theme.accent[1], b.theme.accent[2], 0.55],
            9.0,
        );
        b.push_text(TextRun {
            text: messages.get(loc, "loading-assets"),
            visible_bytes: None,
            x: lrect[0] + 8.0,
            y: lrect[1] + 1.0,
            width: lw - 16.0,
            height: lh - 2.0,
            size: 13.0,
            line_height: 17.0,
            color: b.theme.muted,
            emphasis: Vec::new(),
            color_spans: Vec::new(),
            ruby_spans: Vec::new(),
            shadow: None,
            outline: None,
            scroll: 0.0,
            clip: Some(lrect),
            font_assets: b.ui_fonts.clone(),
            align: TextAlign::Center,
            valign_center: true,
            auto_fit: true,
            inline_images: Vec::new(),
            cache_key: 0,
        });
    }

    // Lightweight animated Toast notification (strictly inside safe area!)
    if let Some(msg) = ctx.toast {
        let t_ease = ease_out_cubic(ctx.toast_anim);
        let tw = (safe[2] * 0.65)
            .clamp(200.0, 520.0)
            .min((safe[2] - 20.0).max(160.0));
        let th = 40.0;
        let slide_y = (1.0 - t_ease) * -14.0;
        let rect = [
            safe[0] + (safe[2] - tw) * 0.5,
            safe[1] + 12.0 + slide_y,
            tw,
            th,
        ];
        b.soft_shadow(rect, 11.0, t_ease);
        b.rounded_panel(
            rect,
            [0.08, 0.11, 0.17, 0.94 * t_ease],
            [
                b.theme.accent[0],
                b.theme.accent[1],
                b.theme.accent[2],
                0.85 * t_ease,
            ],
            11.0,
        );
        let mut tc = b.theme.text;
        tc[3] *= t_ease;
        b.push_text(TextRun {
            text: msg.to_owned(),
            visible_bytes: None,
            x: rect[0] + 12.0,
            y: rect[1] + 2.0,
            width: (tw - 24.0).max(32.0),
            height: th - 4.0,
            size: 15.0,
            line_height: 20.0,
            color: tc,
            emphasis: Vec::new(),
            color_spans: Vec::new(),
            ruby_spans: Vec::new(),
            shadow: None,
            outline: None,
            scroll: 0.0,
            clip: Some(rect),
            font_assets: b.ui_fonts.clone(),
            align: TextAlign::Center,
            valign_center: true,
            auto_fit: true,
            inline_images: Vec::new(),
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

    // Sleek cinematic built-in title screen: left horizontal gradient scrim preserves vibrant cover art on the right!
    let w = b.pkt.width;
    let h = b.pkt.height;
    let [sx, sy, sw, sh] = b.safe;
    let scrim_w = (w * if b.is_touch() { 0.82 } else { 0.62 }).max(320.0).min(w);
    b.pkt.quads.push(Quad::gradient_h(
        [0.0, 0.0, scrim_w, h],
        [0.03, 0.05, 0.09, 0.84],
        [0.03, 0.05, 0.09, 0.0],
    ));
    b.pkt.quads.push(Quad::gradient_v(
        [0.0, h * 0.72, w, h * 0.28],
        [0.02, 0.03, 0.06, 0.0],
        [0.02, 0.03, 0.06, 0.56],
    ));

    let title_size = (sh * 0.072).clamp(22.0, 56.0);
    let title_y = sy + (sh * 0.14).clamp(18.0, sh * 0.20);
    let title_h = (title_size * 2.05).min(sh * 0.26);
    let title_x = sx + sw * 0.07;
    let title_w = (sw * if b.is_touch() { 0.84 } else { 0.56 }).max(220.0);
    b.push_text(TextRun {
        text: prog.meta.title.clone(),
        visible_bytes: None,
        x: title_x,
        y: title_y,
        width: title_w,
        height: title_h,
        size: title_size,
        line_height: title_size * 1.24,
        color: b.theme.text,
        emphasis: Vec::new(),
        color_spans: Vec::new(),
        ruby_spans: Vec::new(),
        shadow: b.theme.text_shadow,
        outline: Some([0.02, 0.03, 0.06, 0.65]),
        scroll: 0.0,
        clip: Some([title_x, title_y, title_w, title_h]),
        font_assets: b.ui_fonts.clone(),
        align: TextAlign::Left,
        valign_center: false,
        auto_fit: true,
        inline_images: Vec::new(),
        cache_key: 0,
    });
    // Accent rule under title
    b.pkt.quads.push(Quad::gradient_h(
        [title_x, title_y + title_h + 2.0, (title_w * 0.36).clamp(64.0, 180.0), 2.5],
        b.theme.accent,
        [b.theme.accent[0], b.theme.accent[1], b.theme.accent[2], 0.0],
    ));

    let has_any_save = ctx.autosave.is_some_and(|s| s.exists)
        || ctx.quicksave.is_some_and(|s| s.exists)
        || ctx.slots.iter().any(|s| s.exists);
    let has_gallery = !prog.ui.gallery.is_empty();

    let mut btns: Vec<(String, UiAction, bool, bool)> = Vec::with_capacity(6);
    if has_any_save {
        btns.push((messages.get(loc, "continue"), UiAction::Continue, true, true));
    }
    btns.push((
        messages.get(loc, "new-game"),
        UiAction::NewGame,
        true,
        !has_any_save,
    ));
    btns.push((
        messages.get(loc, "load"),
        UiAction::OpenSaves { save_mode: false },
        has_any_save,
        false,
    ));
    if has_gallery {
        btns.push((
            if loc.starts_with("zh") {
                "鉴赏 / 回想".into()
            } else {
                "Gallery".into()
            },
            UiAction::OpenGallery,
            true,
            false,
        ));
    }
    btns.push((
        messages.get(loc, "settings"),
        UiAction::OpenSettings,
        true,
        false,
    ));
    if ctx.host_has_shelf {
        btns.push((
            messages.get(loc, "return-shelf"),
            UiAction::ReturnShelf,
            true,
            false,
        ));
    }

    let btn_count = btns.len();
    let bw = (sw * if b.is_touch() { 0.56 } else { 0.28 })
        .clamp(196.0, 340.0)
        .min((sw * 0.84).max(160.0));
    let bx = sx + sw * 0.08;
    let start_y = (title_y + title_h + 14.0).max(sy + sh * 0.36);
    let avail_h = (sy + sh - start_y - 16.0).max(120.0);
    let gap = (avail_h * 0.04).clamp(7.0, 12.0);
    let min_bh = if b.is_touch() { 40.0 } else { 32.0 };
    let max_bh = if b.is_touch() { 50.0 } else { 46.0 };
    let bh = ((avail_h - gap * (btn_count - 1) as f32) / btn_count as f32).clamp(min_bh, max_bh);
    let total_h = bh * btn_count as f32 + gap * (btn_count - 1) as f32;
    let mut by = start_y.min((sy + sh - total_h - 12.0).max(sy + 12.0));

    for (label, action, enabled, primary) in btns {
        b.button([bx, by, bw, bh], label, action, enabled, primary);
        by += bh + gap;
    }
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
                color_spans: Vec::new(),
                ruby_spans: Vec::new(),
                shadow: b.theme.text_shadow,
                outline: None,
                scroll: 0.0,
                clip: Some(rect),
                font_assets: b.ui_fonts.clone(),
                align: TextAlign::Left,
                valign_center: false,
                auto_fit: true,
                inline_images: Vec::new(),
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
            if let Some(img) = if is_hover {
                hover_image.or(*image)
            } else {
                *image
            } {
                let mut q = Quad::image(rect, img);
                q.color[3] = node.opacity;
                b.pkt.quads.push(q);
            } else {
                b.next_hit_id -= 1;
                b.button(
                    rect,
                    prog.str(*label).to_owned(),
                    action.clone(),
                    true,
                    false,
                );
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

fn estimate_display_units(s: &str) -> f32 {
    s.chars()
        .map(|c| if c.is_ascii() { 0.62 } else { 1.05 })
        .sum()
}

fn project_story_hud(
    b: &mut Builder<'_>,
    ctx: &UiContext<'_>,
    messages: &Messages,
    loc: &str,
    stage_rect: [f32; 4],
    scale: f32,
    portrait_docked: bool,
) {
    let [sx, sy, sw, sh] = b.safe;
    let sright = sx + sw;
    let sbottom = sy + sh;
    let touch = b.is_touch();
    let alpha = ctx.vm.dialogue_alpha();

    // Suppress dialogue chrome when an image/positioned ChoiceMenu (e.g. LiveMaker .lpm menu) is active
    // so the dialogue box and HUD toolbar never cover menu icons or steal clicks!
    let choice_hides_dialogue = ctx.vm.choice.as_ref().is_some_and(|c| {
        c.background.is_some()
            || c.items
                .iter()
                .any(|it| it.image.is_some() || it.rect.is_some())
    });

    // Subtle top-right reading-mode indicator pill (Auto / Skip / Long-press HoldSkip) clearing host top-right chrome.
    if ctx.auto || ctx.skip || ctx.hold_skip {
        let mode_label = if ctx.skip || ctx.hold_skip {
            messages.get(loc, "skip-active")
        } else {
            messages.get(loc, "auto-active")
        };
        let pill_h = if touch { 28.0 } else { 24.0 };
        let pill_w = (estimate_display_units(&mode_label) * 13.0 + 24.0).clamp(72.0, 160.0);
        let pill_rect = [
            (sright - pill_w - 8.0 - ctx.host_top_right_pad).max(sx + 8.0),
            sy + 8.0,
            pill_w,
            pill_h,
        ];
        b.rounded_panel(
            pill_rect,
            [
                b.theme.accent[0] * 0.28,
                b.theme.accent[1] * 0.28,
                b.theme.accent[2] * 0.28,
                0.88,
            ],
            [b.theme.accent[0], b.theme.accent[1], b.theme.accent[2], 0.85],
            pill_h * 0.45,
        );
        b.push_text(TextRun {
            text: mode_label,
            visible_bytes: None,
            x: pill_rect[0] + 6.0,
            y: pill_rect[1] + 1.0,
            width: pill_w - 12.0,
            height: pill_h - 3.0,
            size: 13.0,
            line_height: 16.0,
            color: b.theme.text,
            emphasis: Vec::new(),
            color_spans: Vec::new(),
            ruby_spans: Vec::new(),
            shadow: None,
            outline: None,
            scroll: 0.0,
            clip: Some(pill_rect),
            font_assets: b.ui_fonts.clone(),
            align: TextAlign::Center,
            valign_center: true,
            auto_fit: true,
            inline_images: Vec::new(),
            cache_key: 0,
        });
    }

    let prog = ctx.vm.program();
    let style = prog.ui.dialogue_styles.get(ctx.vm.dialogue_style as usize);
    let mut dialogue_top_y: Option<f32> = None;

    if alpha > 0.01 && !choice_hides_dialogue {
        if let Some(dlg) = &ctx.vm.dialogue {
            let dlg_rect_cfg = style
                .and_then(|s| s.dialogue_rect)
                .or(b.theme.dialogue_rect);
            let dlg_bg_cfg = style.and_then(|s| s.dialogue_bg).or(b.theme.dialogue_bg);
            let text_rect_cfg = style.and_then(|s| s.text_rect).or(b.theme.text_rect);
            let style_font_size = style.and_then(|s| s.font_size).unwrap_or(b.theme.font_size);
            let style_line_height = style
                .and_then(|s| s.line_height)
                .unwrap_or(b.theme.line_height);
            let style_text_color = style.and_then(|s| s.text_color).unwrap_or(b.theme.text);

            let ui_s = scale.clamp(0.55, 1.35);
            let scaled_shadow = b.theme.text_shadow.map(|mut s| {
                let ss = if portrait_docked {
                    1.0
                } else {
                    scale.clamp(0.5, 2.0)
                };
                s.offset = [s.offset[0] * ss, s.offset[1] * ss];
                s
            });

            // Toolbar sizing (touch targets >= 40px on mobile!)
            let bar_h = if touch {
                (42.0 * ui_s.max(0.92)).clamp(40.0, 46.0)
            } else {
                (26.0 * ui_s).clamp(22.0, 32.0)
            };
            let gap = if touch {
                5.0
            } else {
                (4.0 * ui_s).clamp(2.0, 6.0)
            };
            let margin = if touch {
                8.0
            } else {
                (4.0 * ui_s).clamp(2.0, 8.0)
            };

            // 1. Dialogue box rect: Portrait Docked Reader places a spacious panel below the top-docked stage;
            //    Landscape / Classic mode places the box faithfully on the stage clamped within safe area.
            let box_rect = if portrait_docked {
                let pad_x = 10.0;
                let dock_top = stage_rect[1] + stage_rect[3] + 10.0;
                let bar_reserve = bar_h + margin + 8.0;
                let dock_bottom = (sbottom - bar_reserve).max(dock_top + 104.0);
                let bh = (dock_bottom - dock_top).max(104.0);
                let bw = (sw - pad_x * 2.0).max(180.0);
                [sx + pad_x, dock_top, bw, bh]
            } else if let Some(r) = dlg_rect_cfg {
                let raw = [
                    stage_rect[0] + r[0] * scale,
                    stage_rect[1] + r[1] * scale,
                    (r[2] * scale).max(64.0),
                    (r[3] * scale).max(40.0),
                ];
                let bx = raw[0].clamp(sx, (sright - 64.0).max(sx));
                let by = raw[1].clamp(sy, (sbottom - 40.0).max(sy));
                let bw = raw[2].min((sright - bx).max(64.0));
                let bh = raw[3].min((sbottom - by).max(40.0));
                [bx, by, bw, bh]
            } else {
                let bh = (stage_rect[3] * 0.28)
                    .clamp(88.0, 220.0)
                    .min((sh * 0.36).max(80.0));
                let bw = (stage_rect[2] * 0.92).min((sw - 16.0).max(180.0));
                let bx = (stage_rect[0] + (stage_rect[2] - bw) * 0.5)
                    .clamp(sx + 8.0, (sright - bw - 8.0).max(sx));
                let margin_b = (12.0 * scale).clamp(6.0, 18.0);
                let by = (stage_rect[1] + stage_rect[3] - bh - margin_b)
                    .clamp(sy + 8.0, (sbottom - bh - 6.0).max(sy + 8.0));
                [bx, by, bw, bh]
            };

            let box_opacity = ctx.prefs.dialogue_opacity.clamp(0.45, 1.0);
            if !portrait_docked {
                if let Some(bg) = dlg_bg_cfg {
                    let mut q = Quad::image(box_rect, bg);
                    q.color[3] = alpha * box_opacity;
                    b.pkt.quads.push(q);
                } else {
                    let mut panel = b.theme.panel;
                    panel[3] = (panel[3].max(0.86) * box_opacity) * alpha;
                    let mut border = b.theme.panel_border;
                    border[3] *= alpha * (0.65 + 0.35 * box_opacity);
                    b.soft_shadow(box_rect, 12.0, alpha * box_opacity);
                    b.rounded_panel(box_rect, panel, border, 12.0);
                    // Subtle top edge specular highlight for refined depth
                    b.pkt.quads.push(Quad::gradient_h(
                        [box_rect[0] + 14.0, box_rect[1] + 1.0, (box_rect[2] - 28.0).max(16.0), 1.5],
                        [b.theme.accent[0], b.theme.accent[1], b.theme.accent[2], 0.0],
                        [b.theme.accent[0], b.theme.accent[1], b.theme.accent[2], 0.42 * alpha * box_opacity],
                    ));
                }
            } else {
                let mut panel = b.theme.panel;
                panel[3] = (panel[3].max(0.92) * box_opacity) * alpha;
                let mut border = b.theme.accent;
                border[3] *= 0.62 * alpha;
                b.soft_shadow(box_rect, 13.0, alpha * 0.7);
                b.rounded_panel(box_rect, panel, border, 13.0);
            }

            // 2. Dialogue-bound face portrait decoration (fitted inside box_rect when auto-placed,
            //    and tracked so text_rect never overlaps the face icon!)
            let mut face_outer_rect: Option<[f32; 4]> = None;
            if let Some(face_id) = ctx.vm.dialogue_portrait {
                if let Some(meta) = prog.asset(face_id) {
                    let (orig_w, orig_h) = if let Some(tr) = meta.trim {
                        (tr.orig_w as f32, tr.orig_h as f32)
                    } else {
                        (meta.width as f32, meta.height as f32)
                    };
                    let (dx, dy, dw, dh) = if let Some(tr) = meta.trim {
                        (tr.x as f32, tr.y as f32, tr.w as f32, tr.h as f32)
                    } else {
                        (0.0, 0.0, orig_w, orig_h)
                    };
                    let (fx_out, fy_out, fw_out, fh_out, draw_rect) = if let Some([px, py]) =
                        style.and_then(|s| s.face_pos).filter(|_| !portrait_docked)
                    {
                        let fx0 = stage_rect[0] + px * scale;
                        let fy0 = stage_rect[1] + py * scale;
                        (
                            fx0,
                            fy0,
                            orig_w * scale,
                            orig_h * scale,
                            [fx0 + dx * scale, fy0 + dy * scale, dw * scale, dh * scale],
                        )
                    } else {
                        let fmargin = if portrait_docked {
                            12.0
                        } else {
                            (10.0 * scale).clamp(4.0, 14.0)
                        };
                        let max_h = (box_rect[3] - fmargin * 2.0)
                            .max(24.0)
                            .min(if portrait_docked { 128.0 } else { box_rect[3] });
                        let max_w = (box_rect[2] * 0.28).max(24.0);
                        let base_s = if portrait_docked { 1.0 } else { scale };
                        let fit = (max_h / (orig_h * base_s).max(1.0))
                            .min(max_w / (orig_w * base_s).max(1.0))
                            .min(1.0);
                        let s = base_s * fit;
                        let fw = orig_w * s;
                        let fh = orig_h * s;
                        let fx0 = box_rect[0] + fmargin;
                        let fy0 = box_rect[1] + (box_rect[3] - fh) * 0.5;
                        (
                            fx0,
                            fy0,
                            fw,
                            fh,
                            [fx0 + dx * s, fy0 + dy * s, dw * s, dh * s],
                        )
                    };
                    let mut q = Quad::image(draw_rect, face_id);
                    q.color[3] = alpha;
                    b.pkt.quads.push(q);
                    face_outer_rect = Some([fx_out, fy_out, fw_out, fh_out]);
                }
            }

            // 3. Dialogue text rect (scaled + safely clamped + indented around any overlapping face icon!)
            let shake = ctx.vm.dialogue_shake;
            let mut text_rect = if portrait_docked {
                let pad_x = 16.0;
                let pad_y = 14.0;
                [
                    box_rect[0] + pad_x + shake[0] * scale,
                    box_rect[1] + pad_y + shake[1] * scale,
                    (box_rect[2] - pad_x * 2.0).max(60.0),
                    (box_rect[3] - pad_y * 2.0).max(40.0),
                ]
            } else if let Some(tr) = text_rect_cfg {
                let raw_x = stage_rect[0] + (tr[0] + shake[0]) * scale;
                let raw_y = stage_rect[1] + (tr[1] + shake[1]) * scale;
                let raw_w = (tr[2] * scale).max(40.0);
                let raw_h = (tr[3] * scale).max(24.0);
                let x = raw_x.clamp(sx + 2.0, (sright - 42.0).max(sx + 2.0));
                let y = raw_y.clamp(sy + 2.0, (sbottom - 26.0).max(sy + 2.0));
                [
                    x,
                    y,
                    raw_w.min((sright - x - 2.0).max(40.0)),
                    raw_h.min((sbottom - y - 2.0).max(24.0)),
                ]
            } else {
                let pad_x = (22.0 * scale).clamp(10.0, 28.0);
                let pad_y = (14.0 * scale).clamp(8.0, 20.0);
                [
                    box_rect[0] + pad_x + shake[0] * scale,
                    box_rect[1] + pad_y + shake[1] * scale,
                    (box_rect[2] - pad_x * 2.0).max(48.0),
                    (box_rect[3] - pad_y * 2.0).max(28.0),
                ]
            };

            if let Some(fr) = face_outer_rect {
                let v_ov =
                    (fr[1] + fr[3]).min(text_rect[1] + text_rect[3]) - fr[1].max(text_rect[1]);
                let h_ov =
                    (fr[0] + fr[2]).min(text_rect[0] + text_rect[2]) - fr[0].max(text_rect[0]);
                if v_ov > text_rect[3] * 0.15 && h_ov > 2.0 {
                    let fgap = if portrait_docked {
                        12.0
                    } else {
                        (12.0 * scale).clamp(6.0, 16.0)
                    };
                    let face_cx = fr[0] + fr[2] * 0.5;
                    let box_cx = box_rect[0] + box_rect[2] * 0.5;
                    let min_text_w = (box_rect[2] * 0.35).max(60.0);
                    if face_cx <= box_cx {
                        let right = text_rect[0] + text_rect[2];
                        let desired_x = text_rect[0].max(fr[0] + fr[2] + fgap);
                        let clamped_x = desired_x.min((right - min_text_w).max(text_rect[0]));
                        text_rect[2] = (right - clamped_x).max(32.0);
                        text_rect[0] = clamped_x;
                    } else {
                        let desired_right = (text_rect[0] + text_rect[2]).min(fr[0] - fgap);
                        let next_w =
                            (desired_right - text_rect[0]).max(min_text_w.min(text_rect[2]));
                        text_rect[2] = next_w.max(32.0);
                    }
                }
            }

            let text_outline = ctx
                .prefs
                .text_outline
                .then_some([0.02, 0.03, 0.06, 0.86]);

            // 4. Speaker nameplate (never overlaps subtitles, stage art in portrait mode, or face portrait!)
            let mut speaker_top_pill_rect: Option<[f32; 4]> = None;
            if !dlg.speaker.is_empty() {
                let mut sc = dlg.speaker_color.unwrap_or(b.theme.accent);
                sc[3] *= alpha;
                let sp_size = if portrait_docked {
                    (16.0 * ctx.prefs.font_scale).clamp(13.0, 26.0)
                } else {
                    (style_font_size * scale * ctx.prefs.font_scale * 0.88).clamp(11.0, 34.0)
                };
                if portrait_docked {
                    // Integrated header band at the top of the docked portrait panel — keeps artwork unobstructed!
                    let header_h = (sp_size * 1.45).clamp(24.0, 38.0).min(text_rect[3] * 0.32);
                    let name_box = [text_rect[0], text_rect[1], text_rect[2], header_h];
                    let push_down = header_h + 6.0;
                    text_rect[1] += push_down;
                    text_rect[3] = (text_rect[3] - push_down).max(28.0);
                    b.pkt.quads.push(Quad::gradient_h(
                        [
                            name_box[0],
                            name_box[1] + header_h + 1.0,
                            (name_box[2] * 0.44).min(150.0),
                            2.0,
                        ],
                        [sc[0], sc[1], sc[2], sc[3] * 0.85],
                        [sc[0], sc[1], sc[2], 0.0],
                    ));
                    b.push_text(TextRun {
                        text: dlg.speaker.clone(),
                        visible_bytes: None,
                        x: name_box[0],
                        y: name_box[1],
                        width: name_box[2],
                        height: name_box[3],
                        size: sp_size,
                        line_height: sp_size * 1.22,
                        color: sc,
                        emphasis: Vec::new(),
                        color_spans: Vec::new(),
                        ruby_spans: Vec::new(),
                        shadow: scaled_shadow,
                        outline: text_outline,
                        scroll: 0.0,
                        clip: Some(name_box),
                        font_assets: b.text_fonts.clone(),
                        align: TextAlign::Left,
                        valign_center: true,
                        auto_fit: true,
                        inline_images: Vec::new(),
                        cache_key: 0,
                    });
                } else if let Some(nr) = style.and_then(|s| s.name_rect) {
                    let name_box = [
                        stage_rect[0] + nr[0] * scale,
                        stage_rect[1] + nr[1] * scale,
                        (nr[2] * scale).max(24.0),
                        (nr[3] * scale).max(16.0),
                    ];
                    b.push_text(TextRun {
                        text: dlg.speaker.clone(),
                        visible_bytes: None,
                        x: name_box[0],
                        y: name_box[1],
                        width: name_box[2],
                        height: name_box[3],
                        size: sp_size,
                        line_height: sp_size * 1.22,
                        color: sc,
                        emphasis: Vec::new(),
                        color_spans: Vec::new(),
                        ruby_spans: Vec::new(),
                        shadow: scaled_shadow,
                        outline: text_outline,
                        scroll: 0.0,
                        clip: Some(name_box),
                        font_assets: b.text_fonts.clone(),
                        align: TextAlign::Left,
                        valign_center: true,
                        auto_fit: true,
                        inline_images: Vec::new(),
                        cache_key: 0,
                    });
                } else {
                    let tab_h = (sp_size * 1.48).clamp(22.0, 44.0);
                    let tab_gap = (4.0 * ui_s).clamp(2.0, 6.0);
                    let tab_y = box_rect[1] - tab_h - tab_gap;
                    if tab_y >= sy + 4.0 {
                        let pad_x = (14.0 * ui_s).clamp(8.0, 20.0);
                        let units = estimate_display_units(&dlg.speaker);
                        let min_w = (76.0 * ui_s).clamp(56.0, 118.0);
                        let max_w = (box_rect[2] * 0.55).max(min_w).min((sw - 16.0).max(80.0));
                        let tab_w = (units * sp_size + pad_x * 2.0 + 6.0).clamp(min_w, max_w);
                        let tab_x =
                            box_rect[0].clamp(sx + 4.0, (sright - tab_w - 4.0).max(sx + 4.0));
                        let tab_rect = [tab_x, tab_y, tab_w, tab_h];
                        let mut accent_bar = sc;
                        accent_bar[3] *= 0.72;
                        b.rounded_panel(
                            tab_rect,
                            [0.06, 0.09, 0.14, 0.90 * alpha * box_opacity.max(0.75)],
                            accent_bar,
                            8.0,
                        );
                        // Crisp character-color accent strip inside the left edge of the nameplate
                        let strip_h = (tab_h - 10.0).max(8.0);
                        b.pkt.quads.push(Quad::rounded(
                            [tab_rect[0] + 5.0, tab_rect[1] + (tab_h - strip_h) * 0.5, 3.0, strip_h],
                            sc,
                            1.5,
                        ));
                        b.push_text(TextRun {
                            text: dlg.speaker.clone(),
                            visible_bytes: None,
                            x: tab_rect[0] + pad_x + 2.0,
                            y: tab_rect[1] + 1.0,
                            width: (tab_rect[2] - pad_x * 2.0 - 2.0).max(20.0),
                            height: (tab_rect[3] - 3.0).max(14.0),
                            size: sp_size,
                            line_height: sp_size * 1.22,
                            color: sc,
                            emphasis: Vec::new(),
                            color_spans: Vec::new(),
                            ruby_spans: Vec::new(),
                            shadow: scaled_shadow,
                            outline: text_outline,
                            scroll: 0.0,
                            clip: Some(tab_rect),
                            font_assets: b.text_fonts.clone(),
                            align: TextAlign::Left,
                            valign_center: true,
                            auto_fit: true,
                            inline_images: Vec::new(),
                            cache_key: 0,
                        });
                        speaker_top_pill_rect = Some(tab_rect);
                    } else {
                        // Box sits at top of viewport (e.g. NVL mode): reserve top band inside text_rect!
                        let header_h = (sp_size * 1.38).min(text_rect[3] * 0.35);
                        let name_box = [text_rect[0], text_rect[1], text_rect[2], header_h];
                        let push_down = header_h + (4.0 * ui_s).clamp(2.0, 6.0);
                        text_rect[1] += push_down;
                        text_rect[3] = (text_rect[3] - push_down).max(20.0);
                        b.push_text(TextRun {
                            text: dlg.speaker.clone(),
                            visible_bytes: None,
                            x: name_box[0],
                            y: name_box[1],
                            width: name_box[2],
                            height: name_box[3],
                            size: sp_size,
                            line_height: sp_size * 1.22,
                            color: sc,
                            emphasis: Vec::new(),
                            color_spans: Vec::new(),
                            ruby_spans: Vec::new(),
                            shadow: scaled_shadow,
                            outline: text_outline,
                            scroll: 0.0,
                            clip: Some(name_box),
                            font_assets: b.text_fonts.clone(),
                            align: TextAlign::Left,
                            valign_center: true,
                            auto_fit: true,
                            inline_images: Vec::new(),
                            cache_key: 0,
                        });
                    }
                }
            }

            // 5. Dialogue text — comfortable mobile size in Portrait Docked Reader,
            //    comfortable minimum floor on compact touch landscape, or faithful stage-scaled typography.
            let (fsize, lh) = if portrait_docked {
                let sz = (17.5 * ctx.prefs.font_scale).clamp(14.0, 30.0);
                (sz, (sz * 1.45).clamp(20.0, 46.0))
            } else {
                let mut sz = (style_font_size * scale * ctx.prefs.font_scale).clamp(10.0, 72.0);
                if touch {
                    let min_touch_sz = (14.5 * ctx.prefs.font_scale).clamp(13.0, 22.0);
                    sz = sz.max(min_touch_sz);
                }
                let l = (style_line_height * scale * ctx.prefs.font_scale)
                    .max(sz * 1.18)
                    .min(sz * 1.85);
                (sz, l)
            };
            let emphasis: Vec<(usize, usize)> = dlg
                .spans
                .iter()
                .filter(|s| s.emphasis)
                .map(|s| (s.byte_start, s.byte_end))
                .collect();
            let color_spans: Vec<(usize, usize, [f32; 4])> = dlg
                .spans
                .iter()
                .filter_map(|s| s.color.map(|c| (s.byte_start, s.byte_end, c)))
                .collect();
            let ruby_spans: Vec<(usize, usize, String)> = dlg
                .spans
                .iter()
                .filter_map(|s| s.ruby.as_ref().map(|rb| (s.byte_start, s.byte_end, rb.clone())))
                .collect();
            let inline_images: Vec<(usize, AssetId)> = dlg
                .spans
                .iter()
                .filter_map(|s| s.inline_image.map(|id| (s.byte_start, id)))
                .collect();
            let line_fade = ease_out_cubic(ctx.dialogue_fade);
            let mut tc = style_text_color;
            tc[3] *= alpha * line_fade;

            // Optional bilingual secondary line when multiple locales exist
            let secondary_text: Option<String> = if ctx.prefs.bilingual && prog.meta.locales.len() >= 2 {
                let sec_loc = if ctx.prefs.text_locale != prog.meta.default_locale {
                    Some(prog.meta.default_locale.as_str())
                } else {
                    prog.meta
                        .locales
                        .iter()
                        .find(|l| l.as_str() != ctx.prefs.text_locale.as_str())
                        .map(String::as_str)
                };
                sec_loc.and_then(|sl| {
                    let entry = prog.text(sl, dlg.text_id)?;
                    let s = entry.to_plain_text();
                    (!s.is_empty() && s != dlg.full_text).then_some(s)
                })
            } else {
                None
            };

            // Give extra top breathing room when ruby annotations are present so ruby never clips!
            let ruby_top_pad = if ruby_spans.is_empty() { 0.0 } else { (fsize * 0.34).clamp(5.0, 12.0) };
            let eff_text_y = text_rect[1] + ruby_top_pad;
            let eff_text_h = (text_rect[3] - ruby_top_pad).max(24.0);
            let (primary_h, sec_rect) = if secondary_text.is_some() && eff_text_h >= 56.0 {
                let ph = eff_text_h * 0.56;
                let sh = (eff_text_h - ph - 4.0).max(18.0);
                (ph, Some([text_rect[0], eff_text_y + ph + 4.0, text_rect[2], sh]))
            } else {
                (eff_text_h, None)
            };

            b.push_text(TextRun {
                text: dlg.full_text.clone(),
                visible_bytes: if dlg.finished {
                    None
                } else {
                    Some(dlg.visible_byte_len())
                },
                x: text_rect[0],
                y: eff_text_y,
                width: text_rect[2],
                height: primary_h,
                size: fsize,
                line_height: if ruby_spans.is_empty() { lh } else { lh.max(fsize * 1.48) },
                color: tc,
                emphasis,
                color_spans,
                ruby_spans,
                shadow: scaled_shadow,
                outline: text_outline,
                scroll: 0.0,
                clip: Some([text_rect[0], text_rect[1], text_rect[2], primary_h + ruby_top_pad]),
                font_assets: b.text_fonts.clone(),
                align: TextAlign::Left,
                valign_center: false,
                auto_fit: true,
                inline_images,
                cache_key: 0,
            });

            if let (Some(stext), Some(sr)) = (secondary_text, sec_rect) {
                let sec_sz = (fsize * 0.82).clamp(10.0, 26.0);
                let mut sc = b.theme.muted;
                sc[3] *= alpha * line_fade * 0.92;
                b.push_text(TextRun {
                    text: stext,
                    visible_bytes: None,
                    x: sr[0],
                    y: sr[1],
                    width: sr[2],
                    height: sr[3],
                    size: sec_sz,
                    line_height: sec_sz * 1.32,
                    color: sc,
                    emphasis: Vec::new(),
                    color_spans: Vec::new(),
                    ruby_spans: Vec::new(),
                    shadow: scaled_shadow,
                    outline: text_outline,
                    scroll: 0.0,
                    clip: Some(sr),
                    font_assets: b.text_fonts.clone(),
                    align: TextAlign::Left,
                    valign_center: false,
                    auto_fit: true,
                    inline_images: Vec::new(),
                    cache_key: 0,
                });
            }

            // Breathing click-to-continue indicator (distinguishes inline click-pause vs end-of-line!)
            let waiting_click = (dlg.finished || dlg.paused_for_click)
                && ctx.vm.choice.is_none()
                && !ctx.auto
                && !ctx.skip
                && !ctx.hold_skip;
            if waiting_click {
                let ind_sz = (fsize * 0.72).clamp(11.0, 18.0);
                let bob = if ctx.prefs.reduced_motion {
                    0.0
                } else {
                    (ctx.ui_time_s * 5.0).sin() * 2.2
                };
                let pulse = if ctx.prefs.reduced_motion {
                    0.92
                } else {
                    0.72 + 0.28 * (ctx.ui_time_s * 5.0).sin().abs()
                };
                let ix = box_rect[0] + box_rect[2] - ind_sz - 10.0;
                let iy = box_rect[1] + box_rect[3] - ind_sz - 7.0 + bob;
                let mut ic = b.theme.accent;
                ic[3] *= alpha * pulse;
                let glyph = if dlg.paused_for_click && !dlg.finished {
                    "…"
                } else {
                    "▼"
                };
                b.push_text(TextRun {
                    text: glyph.into(),
                    visible_bytes: None,
                    x: ix,
                    y: iy,
                    width: ind_sz + 4.0,
                    height: ind_sz + 4.0,
                    size: ind_sz,
                    line_height: ind_sz + 2.0,
                    color: ic,
                    emphasis: Vec::new(),
                    color_spans: Vec::new(),
                    ruby_spans: Vec::new(),
                    shadow: scaled_shadow,
                    outline: None,
                    scroll: 0.0,
                    clip: Some(box_rect),
                    font_assets: b.ui_fonts.clone(),
                    align: TextAlign::Center,
                    valign_center: true,
                    auto_fit: false,
                    inline_images: Vec::new(),
                    cache_key: 0,
                });
            }

            // Optional inline voice-replay pill when the current line has voice (touch-sized to avoid mis-taps!)
            if dlg.voice.is_some() && ctx.vm.choice.is_none() {
                let vw = if touch { 72.0 } else { 48.0 };
                let vh = if touch { 40.0 } else { 22.0 };
                let vx = box_rect[0] + box_rect[2] - vw - 8.0;
                let vy = box_rect[1] + 6.0;
                b.button_styled(
                    [vx, vy, vw, vh],
                    messages.get(loc, "replay-voice"),
                    UiAction::ReplayVoice,
                    true,
                    false,
                    if touch { 0.90 } else { 0.65 },
                );
            }

            // 6. Responsive non-overlapping quick-action toolbar strictly within safe area!
            //    Narrow touch phones (< 460px) condense to 5 wide thumb-friendly buttons incl. 1-tap Q.Save;
            //    landscape phones, tablets & desktop keep the full 7-button strip.
            let compact_touch_bar = touch && sw < 460.0;
            let bar_h = if compact_touch_bar {
                bar_h.max(44.0)
            } else {
                bar_h
            };
            let buttons_storage: Vec<(&str, UiAction, bool)> = if compact_touch_bar {
                vec![
                    ("rollback", UiAction::Rollback, false),
                    ("auto", UiAction::ToggleAuto, ctx.auto),
                    (
                        "skip-compact",
                        UiAction::ToggleSkip,
                        ctx.skip || ctx.hold_skip,
                    ),
                    ("quick-save", UiAction::QuickSave, false),
                    ("menu", UiAction::OpenMenu, false),
                ]
            } else {
                vec![
                    ("auto", UiAction::ToggleAuto, ctx.auto),
                    (
                        "skip-compact",
                        UiAction::ToggleSkip,
                        ctx.skip || ctx.hold_skip,
                    ),
                    ("history-compact", UiAction::OpenHistory, false),
                    ("rollback", UiAction::Rollback, false),
                    (
                        "save",
                        UiAction::OpenSaves { save_mode: true },
                        !ctx.replay_active,
                    ),
                    ("hide-interface", UiAction::ToggleHideUi, false),
                    ("menu", UiAction::OpenMenu, false),
                ]
            };
            let n = buttons_storage.len() as f32;
            let min_btn_w = if compact_touch_bar {
                58.0
            } else if touch {
                44.0
            } else {
                34.0
            };
            let min_comfortable_w = n * min_btn_w + (n - 1.0) * gap;

            let top_y = box_rect[1] - bar_h - margin;
            let top_min_l = speaker_top_pill_rect.map_or(box_rect[0].max(sx + 4.0), |sr| {
                sr[0] + sr[2] + (8.0 * ui_s).clamp(4.0, 12.0)
            });
            let top_max_r = (box_rect[0] + box_rect[2]).min(sright - 4.0);
            let below_y = (box_rect[1] + box_rect[3]).max(stage_rect[1] + stage_rect[3]) + margin;

            let (bar_min_l, bar_max_r, bar_y, bar_centered) = if portrait_docked {
                (
                    sx + 8.0,
                    (sright - 8.0).max(sx + 126.0),
                    sbottom - bar_h - 6.0,
                    true,
                )
            } else if top_y >= sy + 4.0 && (top_max_r - top_min_l) >= min_comfortable_w {
                (top_min_l, top_max_r, top_y, false)
            } else if below_y + bar_h + 4.0 <= sbottom {
                (sx + 6.0, (sright - 6.0).max(sx + 126.0), below_y, true)
            } else {
                (sx + 6.0, (sright - 6.0).max(sx + 126.0), sy + 6.0, false)
            };

            let avail_w = (bar_max_r - bar_min_l).max(120.0);
            let max_btn_w = ((avail_w - gap * (n - 1.0)) / n).max(16.0);
            let cap_btn_w = if compact_touch_bar {
                86.0
            } else if touch {
                76.0
            } else {
                (64.0 * ui_s).clamp(40.0, 74.0)
            };
            let btn_w = max_btn_w.min(cap_btn_w);
            let total_w = btn_w * n + gap * (n - 1.0);
            let mut bx = if bar_centered {
                bar_min_l + ((avail_w - total_w) * 0.5).max(0.0)
            } else {
                (bar_max_r - total_w).max(bar_min_l)
            };

            // Ghost pill bar: subtly receded during quiet reading on desktop, brightens on hover or active mode!
            let toolbar_opacity = if touch || ctx.hud_toolbar_hovered || ctx.auto || ctx.skip || ctx.hold_skip {
                0.94
            } else {
                0.56
            };
            for (key, action, active) in buttons_storage {
                let enabled = !matches!(
                    action,
                    UiAction::OpenSaves { save_mode: true } | UiAction::QuickSave
                ) || !ctx.replay_active;
                b.button_styled(
                    [bx, bar_y, btn_w, bar_h],
                    messages.get(loc, key),
                    action,
                    enabled,
                    active,
                    toolbar_opacity,
                );
                bx += btn_w + gap;
            }

            let mut top_bound = box_rect[1];
            if let Some(sr) = speaker_top_pill_rect {
                top_bound = top_bound.min(sr[1]);
            }
            if bar_y < box_rect[1] && bar_y > sy + 12.0 {
                top_bound = top_bound.min(bar_y);
            }
            dialogue_top_y = Some(top_bound);
        }
    }

    // Choice buttons
    if let Some(choice) = &ctx.vm.choice {
        if let Some(bg) = choice.background {
            b.pkt.quads.push(Quad::image(stage_rect, bg));
        }
        let (mx, my) = match choice.menu_size {
            Some([mw, mh]) if mw > 0 && mh > 0 => {
                (stage_rect[2] / mw as f32, stage_rect[3] / mh as f32)
            }
            _ => (scale, scale),
        };
        let plain_count = choice
            .items
            .iter()
            .filter(|it| it.rect.is_none())
            .count()
            .max(1);
        let cw = (sw * if touch { 0.84 } else { 0.54 })
            .clamp(220.0, 620.0)
            .min((sw - 24.0).max(180.0));
        let avail_top = sy + 16.0;
        let avail_bottom = dialogue_top_y
            .map_or(sbottom - 20.0, |dy| (dy - 12.0).max(sy + sh * 0.35))
            .max(avail_top + 64.0);
        let avail_h = (avail_bottom - avail_top).max(64.0);
        let cgap = (avail_h * 0.04).clamp(6.0, 14.0);
        let min_ch = if touch { 42.0 } else { 32.0 };
        let max_ch = if touch { 56.0 } else { 52.0 };
        let ch = ((avail_h - cgap * (plain_count - 1) as f32) / plain_count as f32)
            .clamp(min_ch, max_ch);
        let total_h = plain_count as f32 * ch + plain_count.saturating_sub(1) as f32 * cgap;
        let mut cy = (avail_top + (avail_h - total_h) * 0.5).max(avail_top);
        let mut hovered_preview: Option<(AssetId, [f32; 2])> = None;
        for (i, item) in choice.items.iter().enumerate() {
            let rect = if let Some(r) = item.rect {
                [
                    stage_rect[0] + r[0] * mx,
                    stage_rect[1] + r[1] * my,
                    r[2] * mx,
                    r[3] * my,
                ]
            } else {
                let r = [sx + (sw - cw) * 0.5, cy, cw, ch];
                cy += ch + cgap;
                r
            };
            if let Some(img) = item.image {
                let id = b.next_hit_id;
                b.next_hit_id += 1;
                let is_hover = !item.disabled && (b.hovered == Some(id) || b.pressed == Some(id));
                let asset = if item.disabled {
                    item.disabled_image.unwrap_or(img)
                } else if is_hover {
                    item.hover_image.unwrap_or(img)
                } else {
                    img
                };
                // Restore SpriteTrim offset for image buttons if present
                let mut q = if let Some(tr) = prog.asset(asset).and_then(|m| m.trim) {
                    let rx = rect[2] / tr.orig_w.max(1) as f32;
                    let ry = rect[3] / tr.orig_h.max(1) as f32;
                    Quad::image(
                        [
                            rect[0] + tr.x as f32 * rx,
                            rect[1] + tr.y as f32 * ry,
                            tr.w as f32 * rx,
                            tr.h as f32 * ry,
                        ],
                        asset,
                    )
                } else {
                    Quad::image(rect, asset)
                };
                if item.disabled && item.disabled_image.is_none() {
                    q.color = [0.55, 0.55, 0.55, 0.75];
                }
                b.pkt.quads.push(q);
                if is_hover {
                    if let Some(prev) = item.preview {
                        hovered_preview = Some(prev);
                    }
                }
                b.pkt.hits.push(HitRegion {
                    id,
                    rect,
                    action: UiAction::Choose(i as u16),
                    label: item.label.clone(),
                    enabled: !item.disabled,
                });
            } else {
                b.button(
                    rect,
                    item.label.clone(),
                    UiAction::Choose(i as u16),
                    !item.disabled,
                    false,
                );
            }
        }
        if let Some((prev_asset, [px, py])) = hovered_preview {
            if let Some(meta) = prog.asset(prev_asset) {
                let (dx, dy, dw, dh) = if let Some(tr) = meta.trim {
                    (tr.x as f32, tr.y as f32, tr.w as f32, tr.h as f32)
                } else {
                    (0.0, 0.0, meta.width as f32, meta.height as f32)
                };
                b.pkt.quads.push(Quad::image(
                    [
                        stage_rect[0] + (px + dx) * mx,
                        stage_rect[1] + (py + dy) * my,
                        dw * mx,
                        dh * my,
                    ],
                    prev_asset,
                ));
            }
        }
    }
}

fn overlay_shell(
    b: &mut Builder<'_>,
    ctx: &UiContext<'_>,
    title: String,
    messages: &Messages,
    loc: &str,
) -> [f32; 4] {
    let w = b.pkt.width;
    let h = b.pkt.height;
    let [sx, sy, sw, sh] = b.safe;
    let touch = b.is_touch();
    let anim = ease_out_cubic(ctx.overlay_anim);

    // True frosted-glass backdrop over blurred live stage when ambient_backdrop is enabled!
    if ctx.prefs.ambient_backdrop && b.pkt.stage_quad_range[1] > b.pkt.stage_quad_range[0] {
        let mut frost = Quad::solid([0.0, 0.0, w, h], [0.24, 0.28, 0.36, 0.96 * anim]);
        frost.uv = cover_uv(b.pkt.stage_rect[2], b.pkt.stage_rect[3], w, h);
        frost.special = SpecialQuad::FrostedBackdrop;
        b.pkt.quads.push(frost);
        b.pkt.needs_blur = true;
        b.pkt
            .quads
            .push(Quad::solid([0.0, 0.0, w, h], [0.02, 0.04, 0.08, 0.54 * anim]));
    } else {
        b.pkt
            .quads
            .push(Quad::solid([0.0, 0.0, w, h], [0.03, 0.05, 0.09, 0.80 * anim]));
    }

    let pw = (sw * if touch { 0.94 } else { 0.86 })
        .clamp(280.0, 960.0)
        .min((sw - 12.0).max(200.0));
    let ph = (sh * if touch { 0.92 } else { 0.86 })
        .clamp(260.0, 680.0)
        .min((sh - 12.0).max(200.0));
    let px = sx + (sw - pw) * 0.5;
    let slide_y = (1.0 - anim) * 14.0;
    let py = sy + (sh - ph) * 0.5 + slide_y;
    let pad = (pw * 0.035).clamp(12.0, 24.0);
    let hdr_h = if touch {
        (ph * 0.11).clamp(46.0, 58.0)
    } else {
        (ph * 0.11).clamp(36.0, 56.0)
    };
    let back_w = (pw * 0.20).clamp(if touch { 76.0 } else { 68.0 }, 108.0);
    let back_h = if touch {
        (hdr_h * 0.82).clamp(38.0, 44.0)
    } else {
        (hdr_h * 0.72).clamp(28.0, 38.0)
    };
    let mut surf = b.theme.surface;
    surf[3] *= anim;
    let border = [
        b.theme.panel_border[0],
        b.theme.panel_border[1],
        b.theme.panel_border[2],
        0.65 * anim,
    ];
    b.soft_shadow([px, py, pw, ph], 14.0, anim);
    b.rounded_panel([px, py, pw, ph], surf, border, 14.0);
    b.pkt.overlay_panel_rect = Some([px, py, pw, ph]);
    b.pkt.quads.push(Quad::gradient_h(
        [px + 16.0, py + 1.0, (pw - 32.0).max(20.0), 2.0],
        [b.theme.accent[0], b.theme.accent[1], b.theme.accent[2], 0.90 * anim],
        [b.theme.accent[0], b.theme.accent[1], b.theme.accent[2], 0.15 * anim],
    ));
    let title_size = (hdr_h * 0.46).clamp(16.0, 26.0);
    let mut tc = b.theme.text;
    tc[3] *= anim;
    b.push_text(TextRun {
        text: title,
        visible_bytes: None,
        x: px + pad,
        y: py + (hdr_h - back_h) * 0.5,
        width: (pw - pad * 3.0 - back_w).max(60.0),
        height: back_h,
        size: title_size,
        line_height: title_size * 1.25,
        color: tc,
        emphasis: Vec::new(),
        color_spans: Vec::new(),
        ruby_spans: Vec::new(),
        shadow: None,
        outline: None,
        scroll: 0.0,
        clip: Some([px + pad, py, (pw - pad * 2.0 - back_w).max(60.0), hdr_h]),
        font_assets: b.ui_fonts.clone(),
        align: TextAlign::Left,
        valign_center: true,
        auto_fit: true,
        inline_images: Vec::new(),
        cache_key: 0,
    });
    b.button(
        [
            px + pw - pad - back_w,
            py + (hdr_h - back_h) * 0.5,
            back_w,
            back_h,
        ],
        messages.get(loc, "back"),
        UiAction::CloseOverlay,
        true,
        false,
    );
    [
        px + pad,
        py + hdr_h + 8.0,
        (pw - pad * 2.0).max(120.0),
        (ph - hdr_h - pad - 8.0).max(100.0),
    ]
}

fn project_menu_overlay(b: &mut Builder<'_>, ctx: &UiContext<'_>, messages: &Messages, loc: &str) {
    let area = overlay_shell(b, ctx, messages.get(loc, "menu"), messages, loc);
    let touch = b.is_touch();
    let has_quick = ctx.quicksave.is_some_and(|s| s.exists);
    let has_voice = ctx.vm.dialogue.as_ref().is_some_and(|d| d.voice.is_some());
    let has_gallery = !ctx.vm.program().ui.gallery.is_empty();

    let mut items: Vec<(String, UiAction, bool, bool)> = vec![
        (messages.get(loc, "close"), UiAction::CloseOverlay, true, true),
        (messages.get(loc, "quick-save"), UiAction::QuickSave, !ctx.replay_active, false),
        (messages.get(loc, "quick-load"), UiAction::QuickLoad, !ctx.replay_active && has_quick, false),
        (messages.get(loc, "save"), UiAction::OpenSaves { save_mode: true }, !ctx.replay_active, false),
        (messages.get(loc, "load"), UiAction::OpenSaves { save_mode: false }, !ctx.replay_active, false),
        (messages.get(loc, "history"), UiAction::OpenHistory, true, false),
    ];
    if has_voice {
        items.push((messages.get(loc, "replay-voice"), UiAction::ReplayVoice, true, false));
    }
    if has_gallery {
        items.push((
            if loc.starts_with("zh") { "鉴赏 / 回想".into() } else { "Gallery".into() },
            UiAction::OpenGallery,
            true,
            false,
        ));
    }
    if touch {
        items.push((
            format!(
                "{}: {}",
                messages.get(loc, "portrait-reader"),
                messages.get(loc, if ctx.portrait_reader { "on" } else { "off" })
            ),
            UiAction::TogglePortraitReader,
            true,
            ctx.portrait_reader,
        ));
    }
    items.push((messages.get(loc, "settings"), UiAction::OpenSettings, true, false));
    items.push((
        messages.get(loc, if ctx.replay_active { "back" } else { "exit" }),
        if ctx.replay_active {
            UiAction::ExitReplay
        } else {
            UiAction::ReturnTitle
        },
        true,
        false,
    ));
    if ctx.host_has_shelf {
        items.push((messages.get(loc, "return-shelf"), UiAction::ReturnShelf, true, false));
    }

    let cols = if area[2] >= 440.0 { 2usize } else { 1usize };
    let rows = items.len().div_ceil(cols).max(1);
    let gap = (area[3] * 0.025).clamp(6.0, 12.0);
    let bw = ((area[2] - gap * (cols - 1) as f32) / cols as f32).min(320.0);
    let total_w = bw * cols as f32 + gap * (cols - 1) as f32;
    let start_x = area[0] + ((area[2] - total_w) * 0.5).max(0.0);
    let min_bh = if touch { 38.0 } else { 30.0 };
    let max_bh = if touch { 50.0 } else { 44.0 };
    let bh = ((area[3] - gap * (rows - 1) as f32) / rows as f32).clamp(min_bh, max_bh);
    let total_h = bh * rows as f32 + gap * (rows - 1) as f32;
    let start_y = area[1] + ((area[3] - total_h) * 0.5).max(0.0);

    for (idx, (label, action, enabled, primary)) in items.into_iter().enumerate() {
        let col = idx % cols;
        let row = idx / cols;
        let x = start_x + col as f32 * (bw + gap);
        let y = start_y + row as f32 * (bh + gap);
        b.button([x, y, bw, bh], label, action, enabled, primary);
    }
}

fn draw_slot_card(
    b: &mut Builder<'_>,
    rect: [f32; 4],
    badge: String,
    timestamp: String,
    summary: String,
    thumb_asset: Option<AssetId>,
    action: UiAction,
    enabled: bool,
    primary: bool,
) {
    let id = b.next_hit_id;
    b.next_hit_id += 1;
    let is_pressed = enabled && b.pressed == Some(id);
    let is_hover = enabled && !b.is_touch() && b.hovered == Some(id);
    let bg = if !enabled {
        [0.12, 0.15, 0.20, 0.50]
    } else if is_pressed {
        [b.theme.accent[0] * 0.65, b.theme.accent[1] * 0.65, b.theme.accent[2] * 0.65, 0.94]
    } else if is_hover {
        [b.theme.accent[0] * 0.32, b.theme.accent[1] * 0.32, b.theme.accent[2] * 0.32, 0.92]
    } else if primary {
        [b.theme.accent[0] * 0.22, b.theme.accent[1] * 0.22, b.theme.accent[2] * 0.22, 0.90]
    } else {
        b.theme.panel
    };
    let border = if is_pressed || is_hover || primary {
        [b.theme.accent[0], b.theme.accent[1], b.theme.accent[2], 0.85]
    } else {
        b.theme.panel_border
    };
    b.rounded_panel(rect, bg, border, 8.0);

    let pad = 6.0;
    let mut tx = rect[0] + pad + 2.0;
    if let Some(asset) = thumb_asset {
        let th = (rect[3] - pad * 2.0).max(24.0);
        let tw = (th * 16.0 / 9.0).min(rect[2] * 0.36);
        b.pkt
            .quads
            .push(Quad::rounded_image([rect[0] + pad, rect[1] + pad, tw, th], asset, 5.0));
        tx = rect[0] + pad + tw + 8.0;
    }
    let tw = (rect[0] + rect[2] - tx - pad).max(40.0);
    let top_h = (rect[3] * 0.40).clamp(15.0, 22.0);
    let top_line = if timestamp.is_empty() {
        badge.clone()
    } else {
        format!("{badge}  ·  {timestamp}")
    };
    b.push_text(TextRun {
        text: top_line,
        visible_bytes: None,
        x: tx,
        y: rect[1] + 4.0,
        width: tw,
        height: top_h,
        size: (top_h * 0.68).clamp(11.0, 14.0),
        line_height: top_h,
        color: if enabled { b.theme.accent } else { b.theme.muted },
        emphasis: Vec::new(),
        color_spans: Vec::new(),
        ruby_spans: Vec::new(),
        shadow: None,
        outline: None,
        scroll: 0.0,
        clip: Some([tx, rect[1] + 2.0, tw, top_h + 2.0]),
        font_assets: b.ui_fonts.clone(),
        align: TextAlign::Left,
        valign_center: true,
        auto_fit: true,
        inline_images: Vec::new(),
        cache_key: 0,
    });
    let sum_y = rect[1] + top_h + 4.0;
    let sum_h = (rect[3] - top_h - 8.0).max(16.0);
    b.push_text(TextRun {
        text: summary.clone(),
        visible_bytes: None,
        x: tx,
        y: sum_y,
        width: tw,
        height: sum_h,
        size: (sum_h * 0.60).clamp(12.0, 16.0),
        line_height: (sum_h * 0.72).clamp(14.0, 20.0),
        color: if enabled { b.theme.text } else { b.theme.muted },
        emphasis: Vec::new(),
        color_spans: Vec::new(),
        ruby_spans: Vec::new(),
        shadow: None,
        outline: None,
        scroll: 0.0,
        clip: Some([tx, sum_y, tw, sum_h]),
        font_assets: b.text_fonts.clone(),
        align: TextAlign::Left,
        valign_center: true,
        auto_fit: true,
        inline_images: Vec::new(),
        cache_key: 0,
    });
    b.pkt.hits.push(HitRegion {
        id,
        rect,
        action,
        label: format!("{badge} {summary}"),
        enabled,
    });
}

fn project_saves_overlay(
    b: &mut Builder<'_>,
    ctx: &UiContext<'_>,
    messages: &Messages,
    loc: &str,
    save_mode: bool,
) {
    let title = messages.get(loc, if save_mode { "save" } else { "load" });
    let mut area = overlay_shell(b, ctx, title, messages, loc);
    let touch = b.is_touch();
    let gap = (area[3] * 0.022).clamp(6.0, 10.0);

    // On Load screen, surface Auto-Save & Quick-Save right at the top for 1-tap resume!
    if !save_mode {
        let has_auto = ctx.autosave.is_some_and(|s| s.exists);
        let has_quick = ctx.quicksave.is_some_and(|s| s.exists);
        if has_auto || has_quick {
            let special_h = (if touch { 54.0_f32 } else { 52.0_f32 }).min(area[3] * 0.22);
            let mut specials: Vec<(&SlotSummary, String, UiAction, AssetId)> = Vec::new();
            if let Some(a) = ctx.autosave.filter(|s| s.exists) {
                specials.push((
                    a,
                    messages.get(loc, "auto-save"),
                    UiAction::LoadAutoSave,
                    AUTOSAVE_THUMB_ASSET_ID,
                ));
            }
            if let Some(q) = ctx.quicksave.filter(|s| s.exists) {
                specials.push((
                    q,
                    messages.get(loc, "quick-save"),
                    UiAction::QuickLoad,
                    QUICKSAVE_THUMB_ASSET_ID,
                ));
            }
            let sw = (area[2] - gap * (specials.len() - 1) as f32) / specials.len() as f32;
            for (i, (slot, badge, action, thumb_id)) in specials.into_iter().enumerate() {
                let sx = area[0] + i as f32 * (sw + gap);
                let ts = format_unix_hm(slot.saved_at_unix, ctx.tz_offset_mins);
                draw_slot_card(
                    b,
                    [sx, area[1], sw, special_h],
                    badge,
                    ts,
                    slot.summary.clone(),
                    slot.has_thumb.then_some(thumb_id),
                    action,
                    true,
                    true,
                );
            }
            area[1] += special_h + gap + 2.0;
            area[3] = (area[3] - special_h - gap - 2.0).max(80.0);
        }
    }

    let cols = if area[2] >= 560.0 { 2 } else { 1 };
    let cell_w = (area[2] - gap * (cols - 1) as f32) / cols as f32;
    let cell_h = if touch {
        (area[3] * 0.16).clamp(52.0, 68.0)
    } else {
        (area[3] * 0.165).clamp(48.0, 68.0)
    };
    let nav_h = if touch { 42.0 } else { 34.0 };
    let total_slots = ctx.slots.len();

    let rows_full = (((area[3] + gap) / (cell_h + gap)).floor() as usize).max(1);
    let need_pages = total_slots > rows_full * cols;
    let list_h = if need_pages {
        (area[3] - nav_h - gap).max(cell_h)
    } else {
        area[3]
    };
    let rows_per_page = (((list_h + gap) / (cell_h + gap)).floor() as usize).max(1);
    let per_page = (rows_per_page * cols).max(1);
    let total_pages = total_slots.div_ceil(per_page).max(1);
    let page = ctx.saves_page.min(total_pages - 1);
    let start = page * per_page;
    let end = (start + per_page).min(total_slots);

    let del_w = if touch { 46.0 } else { 38.0 };
    let del_gap = 6.0;

    for (local_idx, slot) in ctx.slots[start..end].iter().enumerate() {
        let col = local_idx % cols;
        let row = local_idx / cols;
        let x = area[0] + col as f32 * (cell_w + gap);
        let y = area[1] + row as f32 * (cell_h + gap);
        let badge = format!("#{:02}", slot.slot + 1);
        let ts = if slot.exists {
            format_unix_hm(slot.saved_at_unix, ctx.tz_offset_mins)
        } else {
            String::new()
        };
        let summary = if slot.exists {
            slot.summary.clone()
        } else {
            messages.get(loc, "empty-slot")
        };
        let action = if save_mode {
            UiAction::SaveSlot(slot.slot)
        } else {
            UiAction::LoadSlot(slot.slot)
        };
        let enabled = save_mode || slot.exists;
        let show_delete = save_mode && slot.exists && cell_w > del_w + del_gap + 120.0;
        let main_w = if show_delete {
            cell_w - del_w - del_gap
        } else {
            cell_w
        };
        let thumb_asset =
            (slot.exists && slot.has_thumb).then_some(AssetId(SAVE_THUMB_ASSET_BASE + slot.slot));
        draw_slot_card(
            b,
            [x, y, main_w, cell_h],
            badge,
            ts,
            summary,
            thumb_asset,
            action,
            enabled,
            false,
        );
        if show_delete {
            b.button(
                [x + main_w + del_gap, y, del_w, cell_h],
                "×".into(),
                UiAction::DeleteSlot(slot.slot),
                true,
                false,
            );
        }
    }

    if total_pages > 1 {
        let by = area[1] + area[3] - nav_h;
        let nav_w = ((area[2] - 80.0) * 0.5).clamp(76.0, 128.0);
        b.button(
            [area[0], by, nav_w, nav_h],
            messages.get(loc, "scroll-back"),
            UiAction::SavesPage(-1),
            page > 0,
            false,
        );
        b.button(
            [area[0] + area[2] - nav_w, by, nav_w, nav_h],
            messages.get(loc, "scroll-forward"),
            UiAction::SavesPage(1),
            page + 1 < total_pages,
            false,
        );
        let mid_x = area[0] + nav_w + 6.0;
        let mid_w = (area[2] - nav_w * 2.0 - 12.0).max(40.0);
        b.push_text(TextRun {
            text: format!("{} / {}", page + 1, total_pages),
            visible_bytes: None,
            x: mid_x,
            y: by,
            width: mid_w,
            height: nav_h,
            size: 14.0,
            line_height: 18.0,
            color: b.theme.muted,
            emphasis: Vec::new(),
            color_spans: Vec::new(),
            ruby_spans: Vec::new(),
            shadow: None,
            outline: None,
            scroll: 0.0,
            clip: Some([mid_x, by, mid_w, nav_h]),
            font_assets: b.ui_fonts.clone(),
            align: TextAlign::Center,
            valign_center: true,
            auto_fit: true,
            inline_images: Vec::new(),
            cache_key: 0,
        });
    }
}

fn push_slider_row(
    b: &mut Builder<'_>,
    x: f32,
    y: f32,
    w: f32,
    row_h: f32,
    label: String,
    norm: f32,
    kind: SliderKind,
    dec: UiAction,
    inc: UiAction,
) {
    let touch = b.is_touch();
    let btn_w = if touch { 42.0 } else { 34.0 };
    let gap = 8.0;
    let stacked = w < 420.0;
    let clamped_norm = norm.clamp(0.0, 1.0);

    let (bar_x, bar_w, ctrl_y, ctrl_h, minus_x, plus_x) = if stacked {
        let label_h = (row_h * 0.40).clamp(17.0, 24.0);
        let label_size = (label_h * 0.72).clamp(12.5, 16.0);
        b.push_text(TextRun {
            text: label,
            visible_bytes: None,
            x,
            y,
            width: w,
            height: label_h,
            size: label_size,
            line_height: label_h,
            color: b.theme.text,
            emphasis: Vec::new(),
            color_spans: Vec::new(),
            ruby_spans: Vec::new(),
            shadow: None,
            outline: None,
            scroll: 0.0,
            clip: Some([x, y, w, label_h]),
            font_assets: b.ui_fonts.clone(),
            align: TextAlign::Left,
            valign_center: true,
            auto_fit: true,
            inline_images: Vec::new(),
            cache_key: 0,
        });
        let cy = y + label_h + 2.0;
        let ch = (row_h - label_h - 2.0).max(28.0);
        let mx = x;
        let px = x + w - btn_w;
        let bx = mx + btn_w + gap;
        let bw = (px - gap - bx).max(60.0);
        (bx, bw, cy, ch, mx, px)
    } else {
        let right_x = x + w;
        let px = right_x - btn_w;
        let bw = (w * 0.36).clamp(110.0, 240.0);
        let mx = px - gap - bw - gap - btn_w;
        let bx = mx + btn_w + gap;
        let label_w = (mx - x - 8.0).max(60.0);
        let label_size = (row_h * 0.42).clamp(13.0, 17.0);
        b.push_text(TextRun {
            text: label,
            visible_bytes: None,
            x,
            y,
            width: label_w,
            height: row_h,
            size: label_size,
            line_height: label_size * 1.25,
            color: b.theme.text,
            emphasis: Vec::new(),
            color_spans: Vec::new(),
            ruby_spans: Vec::new(),
            shadow: None,
            outline: None,
            scroll: 0.0,
            clip: Some([x, y, label_w, row_h]),
            font_assets: b.ui_fonts.clone(),
            align: TextAlign::Left,
            valign_center: true,
            auto_fit: true,
            inline_images: Vec::new(),
            cache_key: 0,
        });
        (bx, bw, y, row_h, mx, px)
    };

    b.button([minus_x, ctrl_y, btn_w, ctrl_h], "−".into(), dec, true, false);
    // Continuous draggable slider track + glowing thumb pill
    let track_h = if touch { 10.0 } else { 8.0 };
    let track_y = ctrl_y + (ctrl_h - track_h) * 0.5;
    b.pkt.quads.push(Quad::rounded(
        [bar_x, track_y, bar_w, track_h],
        [0.20, 0.25, 0.34, 0.88],
        track_h * 0.5,
    ));
    let fill_w = (bar_w * clamped_norm).max(0.0);
    if fill_w > 2.0 {
        b.pkt.quads.push(Quad::rounded(
            [bar_x, track_y, fill_w, track_h],
            b.theme.accent,
            track_h * 0.5,
        ));
    }
    let thumb_d = (if touch { 22.0_f32 } else { 18.0_f32 })
        .min(ctrl_h - 4.0)
        .max(14.0);
    let thumb_cx = bar_x + fill_w.clamp(thumb_d * 0.45, (bar_w - thumb_d * 0.45).max(thumb_d * 0.45));
    let thumb_rect = [
        thumb_cx - thumb_d * 0.5,
        ctrl_y + (ctrl_h - thumb_d) * 0.5,
        thumb_d,
        thumb_d,
    ];
    b.rounded_panel(
        thumb_rect,
        [0.95, 0.98, 1.0, 0.98],
        [b.theme.accent[0], b.theme.accent[1], b.theme.accent[2], 0.95],
        thumb_d * 0.5,
    );
    b.pkt.sliders.push(SliderTrack {
        rect: [bar_x, ctrl_y, bar_w, ctrl_h],
        kind,
    });
    b.button([plus_x, ctrl_y, btn_w, ctrl_h], "+".into(), inc, true, false);
}

fn push_toggle_row(
    b: &mut Builder<'_>,
    x: f32,
    y: f32,
    w: f32,
    row_h: f32,
    label: String,
    active: bool,
    action: UiAction,
    messages: &Messages,
    loc: &str,
) {
    let toggle_w = (w * 0.26).clamp(84.0, 124.0);
    let toggle_x = x + w - toggle_w;
    let label_w = (toggle_x - x - 8.0).max(60.0);
    let label_size = (row_h * 0.42).clamp(13.0, 17.0);
    b.push_text(TextRun {
        text: label,
        visible_bytes: None,
        x,
        y,
        width: label_w,
        height: row_h,
        size: label_size,
        line_height: label_size * 1.25,
        color: b.theme.text,
        emphasis: Vec::new(),
        color_spans: Vec::new(),
        ruby_spans: Vec::new(),
        shadow: None,
        outline: None,
        scroll: 0.0,
        clip: Some([x, y, label_w, row_h]),
        font_assets: b.ui_fonts.clone(),
        align: TextAlign::Left,
        valign_center: true,
        auto_fit: true,
        inline_images: Vec::new(),
        cache_key: 0,
    });
    b.button(
        [toggle_x, y, toggle_w, row_h],
        messages.get(loc, if active { "on" } else { "off" }),
        action,
        true,
        active,
    );
}

fn project_settings_overlay(
    b: &mut Builder<'_>,
    ctx: &UiContext<'_>,
    messages: &Messages,
    loc: &str,
) {
    let area = overlay_shell(b, ctx, messages.get(loc, "settings"), messages, loc);
    let touch = b.is_touch();
    let tab_h = if touch { 40.0 } else { 34.0 };
    let tab_gap = 8.0;
    let tab_w = ((area[2] - tab_gap * 2.0) / 3.0).min(184.0);
    for (idx, key) in [(0u8, "tab-reading"), (1u8, "tab-visual"), (2u8, "tab-audio")] {
        b.button(
            [area[0] + idx as f32 * (tab_w + tab_gap), area[1], tab_w, tab_h],
            messages.get(loc, key),
            UiAction::SettingsTab(idx),
            true,
            ctx.settings_tab == idx,
        );
    }

    let body_y = area[1] + tab_h + 10.0;
    let body_h = (area[3] - tab_h - 10.0).max(120.0);
    let locales = &ctx.vm.program().meta.locales;
    let stacked_sliders = area[2] < 420.0;
    let (slider_count, toggle_count) = match ctx.settings_tab {
        0 => (
            3usize,
            3 + usize::from(!locales.is_empty()) + usize::from(locales.len() >= 2),
        ),
        1 => (1, 3),
        _ => (4, 2),
    };
    let total_rows = slider_count + toggle_count;
    let gap = (body_h * 0.018).clamp(5.0, 10.0);
    let slider_weight = if stacked_sliders { 1.48_f32 } else { 1.0_f32 };
    let total_weight = slider_count as f32 * slider_weight + toggle_count as f32;
    let usable_h = (body_h - gap * total_rows.saturating_sub(1) as f32).max(100.0);
    let unit_h = usable_h / total_weight.max(1.0);
    let row_h = unit_h.clamp(if touch { 32.0 } else { 28.0 }, if touch { 46.0 } else { 44.0 });
    let slider_h = if stacked_sliders {
        (unit_h * slider_weight).clamp(48.0, 68.0)
    } else {
        row_h
    };
    let mut y = body_y;

    if ctx.settings_tab == 0 {
        // Tab 0: Reading & Language
        push_slider_row(
            b,
            area[0],
            y,
            area[2],
            slider_h,
            format!("{}: {:.1}x", messages.get(loc, "text-speed"), ctx.prefs.text_speed),
            ((ctx.prefs.text_speed - 0.5) / 3.5).clamp(0.0, 1.0),
            SliderKind::TextSpeed,
            UiAction::SetTextSpeed((ctx.prefs.text_speed - 0.5).clamp(0.5, 4.0)),
            UiAction::SetTextSpeed((ctx.prefs.text_speed + 0.5).clamp(0.5, 4.0)),
        );
        y += slider_h + gap;

        push_slider_row(
            b,
            area[0],
            y,
            area[2],
            slider_h,
            format!(
                "{}: {}%",
                messages.get(loc, "font-size"),
                (ctx.prefs.font_scale * 100.0).round() as u32
            ),
            ((ctx.prefs.font_scale - 0.75) / 0.75).clamp(0.0, 1.0),
            SliderKind::FontScale,
            UiAction::SetFontScale((ctx.prefs.font_scale - 0.1).clamp(0.75, 1.50)),
            UiAction::SetFontScale((ctx.prefs.font_scale + 0.1).clamp(0.75, 1.50)),
        );
        y += slider_h + gap;

        push_slider_row(
            b,
            area[0],
            y,
            area[2],
            slider_h,
            format!(
                "{}: {:.1}s",
                messages.get(loc, "auto-wait"),
                ctx.prefs.auto_delay_ms as f32 / 1000.0
            ),
            ((ctx.prefs.auto_delay_ms as f32 - 300.0) / 4700.0).clamp(0.0, 1.0),
            SliderKind::AutoDelay,
            UiAction::SetAutoDelay(ctx.prefs.auto_delay_ms.saturating_sub(300).max(300)),
            UiAction::SetAutoDelay((ctx.prefs.auto_delay_ms + 300).min(5000)),
        );
        y += slider_h + gap;

        push_toggle_row(
            b,
            area[0],
            y,
            area[2],
            row_h,
            messages.get(loc, "skip-unread"),
            ctx.prefs.skip_unread,
            UiAction::ToggleSkipUnread,
            messages,
            loc,
        );
        y += row_h + gap;

        push_toggle_row(
            b,
            area[0],
            y,
            area[2],
            row_h,
            messages.get(loc, "portrait-reader"),
            ctx.portrait_reader,
            UiAction::TogglePortraitReader,
            messages,
            loc,
        );
        y += row_h + gap;

        push_toggle_row(
            b,
            area[0],
            y,
            area[2],
            row_h,
            messages.get(loc, "motion"),
            ctx.prefs.reduced_motion,
            UiAction::ToggleReducedMotion,
            messages,
            loc,
        );
        y += row_h + gap;

        if locales.len() >= 2 && y + row_h <= body_y + body_h + 2.0 {
            push_toggle_row(
                b,
                area[0],
                y,
                area[2],
                row_h,
                messages.get(loc, "bilingual"),
                ctx.prefs.bilingual,
                UiAction::ToggleBilingual,
                messages,
                loc,
            );
            y += row_h + gap;
        }

        if !locales.is_empty() && y + row_h <= body_y + body_h + 2.0 {
            let lgap = 8.0;
            let lw = ((area[2] - lgap * (locales.len().saturating_sub(1)) as f32)
                / locales.len() as f32)
                .clamp(56.0, 120.0);
            let mut lx = area[0];
            for locale in locales {
                if lx + lw > area[0] + area[2] + 1.0 {
                    break;
                }
                let active = locale == &ctx.prefs.text_locale;
                b.button(
                    [lx, y, lw, row_h],
                    locale.clone(),
                    UiAction::SetLocale(locale.clone()),
                    true,
                    active,
                );
                lx += lw + lgap;
            }
        }
    } else if ctx.settings_tab == 1 {
        // Tab 1: Display & Visual Comfort
        push_slider_row(
            b,
            area[0],
            y,
            area[2],
            slider_h,
            format!(
                "{}: {}%",
                messages.get(loc, "dialogue-opacity"),
                (ctx.prefs.dialogue_opacity * 100.0).round() as u32
            ),
            ((ctx.prefs.dialogue_opacity - 0.45) / 0.55).clamp(0.0, 1.0),
            SliderKind::DialogueOpacity,
            UiAction::SetDialogueOpacity((ctx.prefs.dialogue_opacity - 0.08).clamp(0.45, 1.0)),
            UiAction::SetDialogueOpacity((ctx.prefs.dialogue_opacity + 0.08).clamp(0.45, 1.0)),
        );
        y += slider_h + gap;

        push_toggle_row(
            b,
            area[0],
            y,
            area[2],
            row_h,
            messages.get(loc, "ambient-backdrop"),
            ctx.prefs.ambient_backdrop,
            UiAction::ToggleAmbientBackdrop,
            messages,
            loc,
        );
        y += row_h + gap;

        push_toggle_row(
            b,
            area[0],
            y,
            area[2],
            row_h,
            messages.get(loc, "image-sharpening"),
            ctx.prefs.image_sharpening,
            UiAction::ToggleImageSharpening,
            messages,
            loc,
        );
        y += row_h + gap;

        push_toggle_row(
            b,
            area[0],
            y,
            area[2],
            row_h,
            messages.get(loc, "text-outline"),
            ctx.prefs.text_outline,
            UiAction::ToggleTextOutline,
            messages,
            loc,
        );
    } else {
        // Tab 2: Audio & Voice
        push_slider_row(
            b,
            area[0],
            y,
            area[2],
            slider_h,
            format!(
                "{}: {}%",
                messages.get(loc, "master-volume"),
                (ctx.prefs.master_volume * 100.0).round() as u32
            ),
            ctx.prefs.master_volume,
            SliderKind::MasterVolume,
            UiAction::SetMasterVolume((ctx.prefs.master_volume - 0.1).clamp(0.0, 1.0)),
            UiAction::SetMasterVolume((ctx.prefs.master_volume + 0.1).clamp(0.0, 1.0)),
        );
        y += slider_h + gap;

        let sliders: [(&str, AudioBus, f32); 3] = [
            ("music", AudioBus::Bgm, ctx.prefs.bgm_volume),
            ("voice", AudioBus::Voice, ctx.prefs.voice_volume),
            ("sfx", AudioBus::Sfx, ctx.prefs.sfx_volume),
        ];
        for (key, bus, val) in sliders {
            push_slider_row(
                b,
                area[0],
                y,
                area[2],
                slider_h,
                format!("{}: {}%", messages.get(loc, key), (val * 100.0).round() as u32),
                val,
                SliderKind::AudioBus(bus),
                UiAction::SetVolume {
                    bus,
                    value: (val - 0.1).clamp(0.0, 1.0),
                },
                UiAction::SetVolume {
                    bus,
                    value: (val + 0.1).clamp(0.0, 1.0),
                },
            );
            y += slider_h + gap;
        }

        push_toggle_row(
            b,
            area[0],
            y,
            area[2],
            row_h,
            messages.get(loc, "auto-wait-voice"),
            ctx.prefs.auto_wait_voice,
            UiAction::ToggleAutoWaitVoice,
            messages,
            loc,
        );
        y += row_h + gap;

        push_toggle_row(
            b,
            area[0],
            y,
            area[2],
            row_h,
            messages.get(loc, "voice-continue"),
            ctx.prefs.continue_voice_on_advance,
            UiAction::ToggleContinueVoiceOnAdvance,
            messages,
            loc,
        );
    }
}

fn project_history_overlay(
    b: &mut Builder<'_>,
    ctx: &UiContext<'_>,
    messages: &Messages,
    loc: &str,
) {
    let area = overlay_shell(b, ctx, messages.get(loc, "history"), messages, loc);
    let touch = b.is_touch();
    let nav_h = if touch { 44.0 } else { 36.0 };
    let row_h = (area[3] * 0.175).clamp(54.0, 78.0);
    let visible_rows = ((area[3] - nav_h - 10.0) / row_h).max(1.0) as usize;
    let total = ctx.vm.history.len();
    let end = total.saturating_sub(ctx.history_scroll);
    let start = end.saturating_sub(visible_rows);

    let mut y = area[1];
    let fsize = (row_h * 0.25).clamp(13.5, 17.5);
    let voice_btn_w = if touch { 68.0 } else { 58.0 };
    let voice_btn_h = if touch { 36.0 } else { 28.0 };
    for (idx, entry) in ctx.vm.history.range(start..end).enumerate() {
        let card_h = row_h - 6.0;
        let card_rect = [area[0], y, area[2], card_h];
        let has_speaker = !entry.speaker.is_empty();
        let tint = if has_speaker {
            if idx % 2 == 0 {
                [0.12, 0.16, 0.24, 0.62]
            } else {
                [0.10, 0.14, 0.21, 0.48]
            }
        } else {
            [0.08, 0.10, 0.15, 0.34]
        };
        let border = if has_speaker {
            [b.theme.panel_border[0], b.theme.panel_border[1], b.theme.panel_border[2], 0.26]
        } else {
            [0.0; 4]
        };
        b.rounded_panel(card_rect, tint, border, 8.0);

        let has_v = entry.voice.is_some();
        let left_x = area[0] + if has_speaker { 14.0 } else { 10.0 };
        let right_pad = if has_v { voice_btn_w + 16.0 } else { 10.0 };
        let text_w = (area[0] + area[2] - left_x - right_pad).max(60.0);

        if has_speaker {
            // Crisp left character accent bar + dedicated speaker header line!
            b.pkt.quads.push(Quad::rounded(
                [area[0] + 4.0, y + 7.0, 3.0, (card_h - 14.0).max(12.0)],
                b.theme.accent,
                1.5,
            ));
            let sp_h = (card_h * 0.32).clamp(15.0, 20.0);
            b.push_text(TextRun {
                text: entry.speaker.clone(),
                visible_bytes: None,
                x: left_x,
                y: y + 4.0,
                width: text_w,
                height: sp_h,
                size: (sp_h * 0.72).clamp(11.5, 14.0),
                line_height: sp_h,
                color: b.theme.accent,
                emphasis: Vec::new(),
                color_spans: Vec::new(),
                ruby_spans: Vec::new(),
                shadow: None,
                outline: None,
                scroll: 0.0,
                clip: Some([left_x, y + 3.0, text_w, sp_h + 2.0]),
                font_assets: b.ui_fonts.clone(),
                align: TextAlign::Left,
                valign_center: true,
                auto_fit: true,
                inline_images: Vec::new(),
                cache_key: 0,
            });
            let body_y = y + sp_h + 4.0;
            let body_h = (card_h - sp_h - 7.0).max(18.0);
            b.push_text(TextRun {
                text: entry.text.clone(),
                visible_bytes: None,
                x: left_x,
                y: body_y,
                width: text_w,
                height: body_h,
                size: fsize,
                line_height: fsize * 1.32,
                color: b.theme.text,
                emphasis: Vec::new(),
                color_spans: Vec::new(),
                ruby_spans: Vec::new(),
                shadow: None,
                outline: None,
                scroll: 0.0,
                clip: Some([left_x, body_y, text_w, body_h]),
                font_assets: b.text_fonts.clone(),
                align: TextAlign::Left,
                valign_center: false,
                auto_fit: true,
                inline_images: Vec::new(),
                cache_key: 0,
            });
        } else {
            // Narration row: subtly muted tone for immediate visual rhythm vs spoken dialogue
            let nar_col = [
                b.theme.text[0] * 0.86,
                b.theme.text[1] * 0.89,
                b.theme.text[2] * 0.94,
                0.92,
            ];
            b.push_text(TextRun {
                text: entry.text.clone(),
                visible_bytes: None,
                x: left_x,
                y: y + 5.0,
                width: text_w,
                height: card_h - 10.0,
                size: fsize,
                line_height: fsize * 1.36,
                color: nar_col,
                emphasis: Vec::new(),
                color_spans: Vec::new(),
                ruby_spans: Vec::new(),
                shadow: None,
                outline: None,
                scroll: 0.0,
                clip: Some([left_x, y + 3.0, text_w, card_h - 6.0]),
                font_assets: b.text_fonts.clone(),
                align: TextAlign::Left,
                valign_center: true,
                auto_fit: true,
                inline_images: Vec::new(),
                cache_key: 0,
            });
        }
        if let Some(voice_asset) = entry.voice {
            let vx = area[0] + area[2] - voice_btn_w - 6.0;
            let vy = y + (card_h - voice_btn_h) * 0.5;
            b.button(
                [vx, vy, voice_btn_w, voice_btn_h],
                format!("▶ {}", messages.get(loc, "replay-voice")),
                UiAction::ReplayHistoryVoice(voice_asset),
                true,
                false,
            );
        }
        y += row_h;
    }

    let by = area[1] + area[3] - nav_h;
    let nav_w = ((area[2] - 72.0) * 0.5).clamp(80.0, 128.0);
    b.button(
        [area[0], by, nav_w, nav_h],
        messages.get(loc, "scroll-back"),
        UiAction::ScrollHistory(3),
        start > 0,
        false,
    );
    b.button(
        [area[0] + area[2] - nav_w, by, nav_w, nav_h],
        messages.get(loc, "scroll-forward"),
        UiAction::ScrollHistory(-3),
        ctx.history_scroll > 0,
        false,
    );
    if total > 0 {
        let mid_x = area[0] + nav_w + 4.0;
        let mid_w = (area[2] - nav_w * 2.0 - 8.0).max(36.0);
        b.push_text(TextRun {
            text: format!("{}–{}/{}", start + 1, end.max(start + 1), total),
            visible_bytes: None,
            x: mid_x,
            y: by,
            width: mid_w,
            height: nav_h,
            size: 13.0,
            line_height: 17.0,
            color: b.theme.muted,
            emphasis: Vec::new(),
            color_spans: Vec::new(),
            ruby_spans: Vec::new(),
            shadow: None,
            outline: None,
            scroll: 0.0,
            clip: Some([mid_x, by, mid_w, nav_h]),
            font_assets: b.ui_fonts.clone(),
            align: TextAlign::Center,
            valign_center: true,
            auto_fit: true,
            inline_images: Vec::new(),
            cache_key: 0,
        });
    }
}

fn project_gallery_overlay(
    b: &mut Builder<'_>,
    ctx: &UiContext<'_>,
    messages: &Messages,
    loc: &str,
) {
    let prog = ctx.vm.program();
    let area = overlay_shell(
        b,
        ctx,
        if loc.starts_with("zh") {
            "鉴赏 / 回想".into()
        } else {
            "Gallery".into()
        },
        messages,
        loc,
    );
    let touch = b.is_touch();
    let cols = (area[2] / 190.0).floor().clamp(2.0, 4.0) as usize;
    let gap = 12.0;
    let cell_w = (area[2] - gap * (cols - 1) as f32) / cols as f32;
    let cell_h = (cell_w * 0.62).clamp(72.0, 128.0);
    let nav_h = if touch { 42.0 } else { 34.0 };
    let total = prog.ui.gallery.len();
    let rows_full = (((area[3] + gap) / (cell_h + gap)).floor() as usize).max(1);
    let need_pages = total > rows_full * cols;
    let list_h = if need_pages {
        (area[3] - nav_h - gap).max(cell_h)
    } else {
        area[3]
    };
    let rows_per_page = (((list_h + gap) / (cell_h + gap)).floor() as usize).max(1);
    let per_page = (rows_per_page * cols).max(1);
    let total_pages = total.div_ceil(per_page).max(1);
    let page = ctx.gallery_page.min(total_pages - 1);
    let start = page * per_page;
    let end = (start + per_page).min(total);

    for (local_idx, item) in prog.ui.gallery[start..end].iter().enumerate() {
        let abs_idx = start + local_idx;
        let col = local_idx % cols;
        let row = local_idx / cols;
        let x = area[0] + col as f32 * (cell_w + gap);
        let y = area[1] + row as f32 * (cell_h + gap);
        let rect = [x, y, cell_w, cell_h];
        let unlocked = item
            .unlock_flag
            .is_none_or(|f| ctx.vm.profile.flags.contains(prog.str(f)));
        let label = if unlocked {
            prog.str(item.title).to_owned()
        } else {
            "???".into()
        };
        let action = if let Some(func) = item.replay_func {
            Some(UiAction::StartReplay(func))
        } else if !item.cg_assets.is_empty() || item.thumb.is_some() {
            Some(UiAction::ViewCg {
                item: abs_idx as u32,
                variant: 0,
            })
        } else {
            None
        };
        if let Some(act) = action {
            let id = b.next_hit_id;
            b.next_hit_id += 1;
            let is_hover = unlocked && !b.is_touch() && b.hovered == Some(id);
            let border = if is_hover {
                b.theme.accent
            } else {
                b.theme.panel_border
            };
            b.rounded_panel(rect, b.theme.panel, border, 8.0);
            if unlocked {
                if let Some(thumb) = item.thumb.or_else(|| item.cg_assets.first().copied()) {
                    b.pkt.quads.push(Quad::rounded_image(
                        [x + 2.0, y + 2.0, (cell_w - 4.0).max(8.0), (cell_h - 4.0).max(8.0)],
                        thumb,
                        6.0,
                    ));
                }
            }
            let cap_h = 24.0;
            b.pkt.quads.push(Quad::rounded(
                [x + 2.0, y + cell_h - cap_h - 2.0, (cell_w - 4.0).max(8.0), cap_h],
                [0.05, 0.08, 0.13, 0.80],
                5.0,
            ));
            b.push_text(TextRun {
                text: label.clone(),
                visible_bytes: None,
                x: x + 6.0,
                y: y + cell_h - cap_h - 1.0,
                width: (cell_w - 12.0).max(16.0),
                height: cap_h - 2.0,
                size: 13.0,
                line_height: 16.0,
                color: if unlocked { b.theme.text } else { b.theme.muted },
                emphasis: Vec::new(),
                color_spans: Vec::new(),
                ruby_spans: Vec::new(),
                shadow: None,
                outline: None,
                scroll: 0.0,
                clip: Some(rect),
                font_assets: b.ui_fonts.clone(),
                align: TextAlign::Center,
                valign_center: true,
                auto_fit: true,
                inline_images: Vec::new(),
                cache_key: 0,
            });
            b.pkt.hits.push(HitRegion {
                id,
                rect,
                action: act,
                label,
                enabled: unlocked,
            });
        }
    }

    if total_pages > 1 {
        let by = area[1] + area[3] - nav_h;
        let nav_w = ((area[2] - 80.0) * 0.5).clamp(76.0, 128.0);
        b.button(
            [area[0], by, nav_w, nav_h],
            messages.get(loc, "scroll-back"),
            UiAction::GalleryPage(-1),
            page > 0,
            false,
        );
        b.button(
            [area[0] + area[2] - nav_w, by, nav_w, nav_h],
            messages.get(loc, "scroll-forward"),
            UiAction::GalleryPage(1),
            page + 1 < total_pages,
            false,
        );
        let mid_x = area[0] + nav_w + 6.0;
        let mid_w = (area[2] - nav_w * 2.0 - 12.0).max(40.0);
        b.push_text(TextRun {
            text: format!("{} / {}", page + 1, total_pages),
            visible_bytes: None,
            x: mid_x,
            y: by,
            width: mid_w,
            height: nav_h,
            size: 14.0,
            line_height: 18.0,
            color: b.theme.muted,
            emphasis: Vec::new(),
            color_spans: Vec::new(),
            ruby_spans: Vec::new(),
            shadow: None,
            outline: None,
            scroll: 0.0,
            clip: Some([mid_x, by, mid_w, nav_h]),
            font_assets: b.ui_fonts.clone(),
            align: TextAlign::Center,
            valign_center: true,
            auto_fit: true,
            inline_images: Vec::new(),
            cache_key: 0,
        });
    }
}

fn project_cg_viewer(
    b: &mut Builder<'_>,
    ctx: &UiContext<'_>,
    messages: &Messages,
    loc: &str,
    item_idx: usize,
    var_idx: usize,
) {
    let prog = ctx.vm.program();
    let Some(item) = prog.ui.gallery.get(item_idx) else {
        return;
    };
    let assets: Vec<AssetId> = if !item.cg_assets.is_empty() {
        item.cg_assets.clone()
    } else if let Some(t) = item.thumb {
        vec![t]
    } else {
        return;
    };
    let idx = var_idx.min(assets.len() - 1);
    let asset = assets[idx];
    let w = b.pkt.width;
    let h = b.pkt.height;
    let [sx, sy, sw, sh] = b.safe;
    b.pkt
        .quads
        .push(Quad::solid([0.0, 0.0, w, h], [0.02, 0.03, 0.06, 0.96]));

    let bar_h = if b.is_touch() { 44.0 } else { 36.0 };
    let img_area_h = (sh - bar_h - 18.0).max(100.0);
    let (aw, ah) = prog.asset(asset).map_or(
        (prog.meta.stage_width as f32, prog.meta.stage_height as f32),
        |m| (m.width.max(1) as f32, m.height.max(1) as f32),
    );
    let scale = (sw / aw).min(img_area_h / ah) * ctx.cg_zoom.clamp(1.0, 3.0);
    let dw = aw * scale;
    let dh = ah * scale;
    let dx = sx + (sw - dw) * 0.5 + ctx.cg_pan[0];
    let dy = sy + (img_area_h - dh) * 0.5 + ctx.cg_pan[1];
    let mut cg_quad = Quad::image([dx, dy, dw, dh], asset);
    cg_quad.clip = Some([0.0, 0.0, w, sy + img_area_h]);
    b.pkt.quads.push(cg_quad);

    // Subtle zoom badge while magnified so touch users know double-tap resets!
    if ctx.cg_zoom > 1.05 {
        let zlabel = if loc.starts_with("zh") {
            format!("{:.1}x · 双击复位", ctx.cg_zoom)
        } else {
            format!("{:.1}x · Double-tap reset", ctx.cg_zoom)
        };
        let zw = (estimate_display_units(&zlabel) * 12.5 + 22.0).clamp(100.0, 210.0);
        let zh = 28.0;
        let zrect = [sx + (sw - zw) * 0.5, sy + 10.0, zw, zh];
        b.rounded_panel(
            zrect,
            [0.06, 0.09, 0.15, 0.84],
            [b.theme.accent[0], b.theme.accent[1], b.theme.accent[2], 0.65],
            14.0,
        );
        b.push_text(TextRun {
            text: zlabel,
            visible_bytes: None,
            x: zrect[0] + 8.0,
            y: zrect[1] + 1.0,
            width: zw - 16.0,
            height: zh - 2.0,
            size: 12.5,
            line_height: 16.0,
            color: b.theme.text,
            emphasis: Vec::new(),
            color_spans: Vec::new(),
            ruby_spans: Vec::new(),
            shadow: None,
            outline: None,
            scroll: 0.0,
            clip: Some(zrect),
            font_assets: b.ui_fonts.clone(),
            align: TextAlign::Center,
            valign_center: true,
            auto_fit: true,
            inline_images: Vec::new(),
            cache_key: 0,
        });
    }

    // Tap image to advance variant or close (only when unzoomed, so zoomed users can pan/double-tap freely!)
    if ctx.cg_zoom <= 1.05 {
        let img_hit_id = b.next_hit_id;
        b.next_hit_id += 1;
        let next_act = if idx + 1 < assets.len() {
            UiAction::ViewCg {
                item: item_idx as u32,
                variant: (idx + 1) as u16,
            }
        } else {
            UiAction::CloseCgViewer
        };
        b.pkt.hits.push(HitRegion {
            id: img_hit_id,
            rect: [0.0, 0.0, w, sy + img_area_h],
            action: next_act,
            label: String::new(),
            enabled: true,
        });
    }

    let by = sy + sh - bar_h - 6.0;
    let bw = 96.0_f32.min((sw - 36.0) / 3.0);
    if assets.len() > 1 {
        b.button(
            [sx + 8.0, by, bw, bar_h],
            messages.get(loc, "cg-prev"),
            UiAction::ViewCg {
                item: item_idx as u32,
                variant: idx.saturating_sub(1) as u16,
            },
            idx > 0,
            false,
        );
        b.button(
            [sx + bw + 16.0, by, bw, bar_h],
            messages.get(loc, "cg-next"),
            UiAction::ViewCg {
                item: item_idx as u32,
                variant: (idx + 1).min(assets.len() - 1) as u16,
            },
            idx + 1 < assets.len(),
            false,
        );
        b.push_text(TextRun {
            text: format!("{} / {}", idx + 1, assets.len()),
            visible_bytes: None,
            x: sx + bw * 2.0 + 24.0,
            y: by,
            width: (sw - bw * 3.0 - 40.0).max(40.0),
            height: bar_h,
            size: 14.0,
            line_height: 18.0,
            color: b.theme.muted,
            emphasis: Vec::new(),
            color_spans: Vec::new(),
            ruby_spans: Vec::new(),
            shadow: None,
            outline: None,
            scroll: 0.0,
            clip: None,
            font_assets: b.ui_fonts.clone(),
            align: TextAlign::Center,
            valign_center: true,
            auto_fit: true,
            inline_images: Vec::new(),
            cache_key: 0,
        });
    }
    b.button(
        [sx + sw - bw - 8.0, by, bw, bar_h],
        messages.get(loc, "back"),
        UiAction::CloseCgViewer,
        true,
        true,
    );
}

fn project_confirm_modal(
    b: &mut Builder<'_>,
    ctx: &UiContext<'_>,
    messages: &Messages,
    loc: &str,
    confirm: ConfirmKind,
) {
    let w = b.pkt.width;
    let h = b.pkt.height;
    let [sx, sy, sw, sh] = b.safe;
    let anim = ease_out_cubic(ctx.overlay_anim);
    b.pkt
        .quads
        .push(Quad::solid([0.0, 0.0, w, h], [0.02, 0.03, 0.07, 0.72 * anim]));
    // Backdrop hit absorbs accidental clicks behind modal
    let bg_id = b.next_hit_id;
    b.next_hit_id += 1;
    b.pkt.hits.push(HitRegion {
        id: bg_id,
        rect: [0.0, 0.0, w, h],
        action: UiAction::CancelPending,
        label: String::new(),
        enabled: true,
    });

    let cw = (sw * 0.78).clamp(260.0, 420.0).min((sw - 24.0).max(220.0));
    let ch = 152.0;
    let cx = sx + (sw - cw) * 0.5;
    let cy = sy + (sh - ch) * 0.5 + (1.0 - anim) * 10.0;
    b.soft_shadow([cx, cy, cw, ch], 13.0, anim);
    b.rounded_panel(
        [cx, cy, cw, ch],
        b.theme.surface,
        [b.theme.accent[0], b.theme.accent[1], b.theme.accent[2], 0.85],
        13.0,
    );
    let (msg, ok_label) = match confirm {
        ConfirmKind::OverwriteSlot(slot) => (
            format!("{} #{:02}?", messages.get(loc, "overwrite-slot"), slot + 1),
            messages.get(loc, "confirm-overwrite"),
        ),
        ConfirmKind::DeleteSlot(slot) => (
            format!("{} #{:02}?", messages.get(loc, "delete-slot"), slot + 1),
            messages.get(loc, "confirm-delete"),
        ),
    };
    b.push_text(TextRun {
        text: msg,
        visible_bytes: None,
        x: cx + 18.0,
        y: cy + 20.0,
        width: cw - 36.0,
        height: 52.0,
        size: 17.0,
        line_height: 24.0,
        color: b.theme.text,
        emphasis: Vec::new(),
        color_spans: Vec::new(),
        ruby_spans: Vec::new(),
        shadow: None,
        outline: None,
        scroll: 0.0,
        clip: Some([cx + 14.0, cy + 14.0, cw - 28.0, 62.0]),
        font_assets: b.ui_fonts.clone(),
        align: TextAlign::Center,
        valign_center: true,
        auto_fit: true,
        inline_images: Vec::new(),
        cache_key: 0,
    });
    let btn_gap = 12.0;
    let bw = (cw - 36.0 - btn_gap) * 0.5;
    let bh = if b.is_touch() { 44.0 } else { 38.0 };
    let by = cy + ch - bh - 18.0;
    b.button(
        [cx + 18.0, by, bw, bh],
        messages.get(loc, "cancel"),
        UiAction::CancelPending,
        true,
        false,
    );
    b.button(
        [cx + 18.0 + bw + btn_gap, by, bw, bh],
        ok_label,
        UiAction::ConfirmPending,
        true,
        true,
    );
}

fn project_ended_overlay(
    b: &mut Builder<'_>,
    ctx: &UiContext<'_>,
    messages: &Messages,
    loc: &str,
) {
    let area = overlay_shell(b, ctx, messages.get(loc, "ending"), messages, loc);
    let bw = area[2].min(240.0);
    b.button(
        [
            area[0] + (area[2] - bw) * 0.5,
            area[1] + (area[3] - 48.0).max(0.0) * 0.35,
            bw,
            48.0,
        ],
        messages.get(loc, "exit"),
        UiAction::ReturnTitle,
        true,
        true,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn rects_overlap(a: [f32; 4], b: [f32; 4]) -> bool {
        let x_ov = (a[0] + a[2]).min(b[0] + b[2]) - a[0].max(b[0]);
        let y_ov = (a[1] + a[3]).min(b[1] + b[3]) - a[1].max(b[1]);
        x_ov > 0.5 && y_ov > 0.5
    }

    fn livemaker_like_program() -> Arc<NirProgram> {
        let mut ui = UiTheme {
            dialogue_rect: Some([0.0, 440.0, 800.0, 160.0]),
            text_rect: Some([32.0, 456.0, 736.0, 128.0]),
            font_size: 24.0,
            line_height: 34.0,
            ..Default::default()
        };
        ui.dialogue_styles.push(DialogueStyleDef {
            name: StrId(1),
            dialogue_bg: None,
            dialogue_rect: ui.dialogue_rect,
            text_rect: ui.text_rect,
            font_size: Some(24.0),
            line_height: Some(34.0),
            text_color: Some([1.0, 1.0, 1.0, 1.0]),
            face_pos: None,
            name_rect: None,
        });
        Arc::new(NirProgram {
            features: 0,
            meta: GameMeta {
                game_id: "org.nir.lm.test".into(),
                title: "LiveMaker Test".into(),
                version: "1.0".into(),
                stage_width: 800,
                stage_height: 600,
                default_locale: "zh-Hans".into(),
                locales: vec!["zh-Hans".into()],
                entry_func: FuncId(0),
                save_slots: 6,
                default_reveal_us: 0,
                title_bg: None,
                title_bgm: None,
                ui_fonts: vec![],
                text_fonts: vec![],
            },
            strings: vec![
                "main".into(),
                "(標準)".into(),
                "小红".into(),
                "face".into(),
                "btn".into(),
                "end".into(),
            ],
            vars: vec![],
            funcs: vec![FuncMeta {
                name: StrId(0),
                entry_pc: 0,
                param_count: 0,
                local_count: 0,
            }],
            labels: vec![],
            ops: vec![
                Op::SetDialogueStyle(0),
                Op::SetDialoguePortrait(Some(AssetId(0))),
                Op::Say {
                    speaker: Some(StrId(2)),
                    speaker_color: None,
                    text: TextId(0),
                    voice: None,
                    reveal_us: 0,
                    append: false,
                },
                Op::End { outcome: StrId(5) },
            ],
            texts: BTreeMap::from([(
                "zh-Hans".into(),
                vec![TextEntry::plain(
                    "这是一段比较长的测试台词，用来验证在不同屏幕分辨率下字幕不会超出对话框，也不会和人物头像、角色名牌以及底部快捷按钮互相遮挡。",
                )],
            )]),
            ui,
            assets: vec![AssetMeta {
                id: AssetId(0),
                name: StrId(3),
                kind: AssetKind::Image,
                codec: MediaCodec::Png,
                width: 180,
                height: 180,
                trim: None,
                duration_us: 0,
                sample_rate: 0,
                channels: 0,
                loop_region: None,
                offset: 0,
                size: 16,
                delta: None,
            }],
            asset_lookup: BTreeMap::new(),
            timelines: vec![],
            menus: vec![MenuTemplate {
                size: [1280, 720],
                background: None,
                items: vec![MenuItemDef {
                    name: StrId(4),
                    label: StrId(4),
                    rect: [960.0, 540.0, 200.0, 80.0],
                    image: Some(AssetId(0)),
                    hover_image: None,
                    disabled_image: None,
                    preview_image: None,
                    preview_pos: [0.0, 0.0],
                }],
            }],
        })
    }

    #[test]
    fn story_hud_scales_font_indents_face_and_never_overlaps_icons_across_viewports() {
        let prog = livemaker_like_program();
        let mut vm = Vm::new(prog, Some("zh-Hans".into())).unwrap();
        vm.step(1000).unwrap();
        let prefs = Preferences::default();
        let msgs = Messages::default();

        for (vw, vh) in [
            (1280.0, 720.0),
            (800.0, 600.0),
            (640.0, 360.0),
            (390.0, 844.0),
        ] {
            let ctx = UiContext {
                vm: &vm,
                screen: Screen::Story,
                prefs: &prefs,
                auto: false,
                skip: false,
                hold_skip: false,
                interface_hidden: false,
                replay_active: false,
                slots: &[],
                autosave: None,
                quicksave: None,
                saves_page: 0,
                history_scroll: 0,
                gallery_page: 0,
                viewing_cg: None,
                cg_zoom: 1.0,
                cg_pan: [0.0, 0.0],
                settings_tab: 0,
                pending_confirm: None,
                host_has_shelf: false,
                loading_visible: false,
                tz_offset_mins: 0,
                hovered_hit: None,
                pressed_hit: None,
                safe_insets: SafeInsets::default(),
                input_mode: InputMode::Pointer,
                portrait_reader: false,
                toast: None,
                overlay_anim: 1.0,
                toast_anim: 1.0,
                dialogue_fade: 1.0,
                ui_time_s: 0.0,
                hud_toolbar_hovered: false,
                host_top_left_pad: 0.0,
                host_top_right_pad: 0.0,
            };
            let pkt = project(&ctx, vw, vh, &msgs);
            let (_, scale) = fit_stage(800.0, 600.0, vw, vh);

            // 1. Dialogue font size scales cleanly with stage scale (no fixed unscaled 24px!).
            let dlg_run = pkt
                .texts
                .iter()
                .find(|t| t.text.contains("测试台词"))
                .expect("dialogue text run missing");
            let expected_size = (24.0 * scale).clamp(10.0, 72.0);
            assert!(
                (dlg_run.size - expected_size).abs() < 0.05,
                "viewport {vw}x{vh}: expected font size {expected_size}, got {}",
                dlg_run.size
            );
            assert!(dlg_run.auto_fit, "dialogue text must enable auto_fit");
            let dlg_rect = [dlg_run.x, dlg_run.y, dlg_run.width, dlg_run.height];

            // 2. Face portrait fits inside box and does NOT overlap dialogue text!
            let face_quad = pkt
                .quads
                .iter()
                .find(|q| q.asset == Some(AssetId(0)))
                .expect("face portrait quad missing");
            assert!(
                !rects_overlap(face_quad.rect, dlg_rect),
                "viewport {vw}x{vh}: face {:?} overlaps dialogue {:?}",
                face_quad.rect,
                dlg_rect
            );

            // 3. Speaker nameplate does NOT overlap dialogue text or face!
            let sp_run = pkt
                .texts
                .iter()
                .find(|t| t.text == "小红")
                .expect("speaker text missing");
            let sp_rect = [sp_run.x, sp_run.y, sp_run.width, sp_run.height];
            assert!(
                !rects_overlap(sp_rect, dlg_rect),
                "viewport {vw}x{vh}: speaker {:?} overlaps dialogue {:?}",
                sp_rect,
                dlg_rect
            );

            // 4. All 7 toolbar buttons fit inside viewport, do NOT overlap each other,
            //    and do NOT overlap dialogue text or speaker nameplate!
            assert_eq!(
                pkt.hits.len(),
                7,
                "viewport {vw}x{vh}: expected 7 HUD buttons"
            );
            for (i, h) in pkt.hits.iter().enumerate() {
                assert!(
                    h.rect[0] >= -0.5 && h.rect[0] + h.rect[2] <= vw + 0.5,
                    "viewport {vw}x{vh}: button {i} ({}) overflows horizontally: {:?}",
                    h.label,
                    h.rect
                );
                assert!(
                    h.rect[1] >= -0.5 && h.rect[1] + h.rect[3] <= vh + 0.5,
                    "viewport {vw}x{vh}: button {i} ({}) overflows vertically: {:?}",
                    h.label,
                    h.rect
                );
                assert!(
                    !rects_overlap(h.rect, dlg_rect),
                    "viewport {vw}x{vh}: button {} {:?} overlaps dialogue {:?}",
                    h.label,
                    h.rect,
                    dlg_rect
                );
                assert!(
                    !rects_overlap(h.rect, sp_rect),
                    "viewport {vw}x{vh}: button {} {:?} overlaps speaker {:?}",
                    h.label,
                    h.rect,
                    sp_rect
                );
            }
        }
    }

    #[test]
    fn lpm_choice_menu_hides_dialogue_hud_and_scales_by_menu_size() {
        let prog = livemaker_like_program();
        let mut vm = Vm::new(prog, Some("zh-Hans".into())).unwrap();
        vm.step(1000).unwrap();
        // Activate a 1280x720 LPM menu on top of an 800x600 stage!
        vm.choice = Some(nir_vm::ChoiceState {
            items: vec![nir_vm::OfferedChoice {
                index: 0,
                label: "btn".into(),
                image: Some(AssetId(0)),
                hover_image: None,
                disabled_image: None,
                preview: None,
                rect: Some([960.0, 540.0, 200.0, 80.0]),
                target_pc: 0,
                value: None,
                choice_index: None,
                disabled: false,
            }],
            result_var: None,
            index_var: None,
            cancel_pc: None,
            timeout_us: None,
            elapsed_us: 0,
            default_idx: None,
            background: None,
            menu_size: Some([1280, 720]),
            hover_se: None,
            click_se: None,
            fade_out_us: 0,
        });
        let prefs = Preferences::default();
        let msgs = Messages::default();
        let ctx = UiContext {
            vm: &vm,
            screen: Screen::Story,
            prefs: &prefs,
            auto: false,
            skip: false,
            hold_skip: false,
            interface_hidden: false,
            replay_active: false,
            slots: &[],
            autosave: None,
            quicksave: None,
            saves_page: 0,
            history_scroll: 0,
            gallery_page: 0,
            viewing_cg: None,
            cg_zoom: 1.0,
            cg_pan: [0.0, 0.0],
            settings_tab: 0,
            pending_confirm: None,
            host_has_shelf: false,
            loading_visible: false,
            tz_offset_mins: 0,
            hovered_hit: None,
            pressed_hit: None,
            safe_insets: SafeInsets::default(),
            input_mode: InputMode::Pointer,
            portrait_reader: true,
            toast: None,
            overlay_anim: 1.0,
            toast_anim: 1.0,
            dialogue_fade: 1.0,
            ui_time_s: 0.0,
            hud_toolbar_hovered: false,
            host_top_left_pad: 0.0,
            host_top_right_pad: 0.0,
        };
        let pkt = project(&ctx, 800.0, 600.0, &msgs);
        // Only the LPM button hit exists — dialogue HUD & 7 buttons are suppressed!
        assert_eq!(pkt.hits.len(), 1);
        let r = pkt.hits[0].rect;
        // 960/1280 * 800 = 600, 540/720 * 600 = 450, 200/1280*800 = 125, 80/720*600 = 66.67 -> fits inside 800x600!
        assert!((r[0] - 600.0).abs() < 0.5 && (r[1] - 450.0).abs() < 0.5);
        assert!(r[0] + r[2] <= 800.5 && r[1] + r[3] <= 600.5);
    }

    #[test]
    fn mobile_portrait_docked_reader_and_safe_insets_deliver_comfortable_targets() {
        let prog = livemaker_like_program();
        let mut vm = Vm::new(prog, Some("zh-Hans".into())).unwrap();
        vm.step(1000).unwrap();
        let prefs = Preferences::default();
        let msgs = Messages::default();

        // Realistic iPhone 15 portrait (390x844) with Dynamic Island (59px) & Home Indicator (34px)
        let insets = SafeInsets {
            top: 59.0,
            right: 0.0,
            bottom: 34.0,
            left: 0.0,
        };
        let ctx = UiContext {
            vm: &vm,
            screen: Screen::Story,
            prefs: &prefs,
            auto: false,
            skip: false,
            hold_skip: false,
            interface_hidden: false,
            replay_active: false,
            slots: &[],
            autosave: None,
            quicksave: None,
            saves_page: 0,
            history_scroll: 0,
            gallery_page: 0,
            viewing_cg: None,
            cg_zoom: 1.0,
            cg_pan: [0.0, 0.0],
            settings_tab: 0,
            pending_confirm: None,
            host_has_shelf: false,
            loading_visible: false,
            tz_offset_mins: 0,
            hovered_hit: None,
            pressed_hit: None,
            safe_insets: insets,
            input_mode: InputMode::Touch,
            portrait_reader: true,
            toast: None,
            overlay_anim: 1.0,
            toast_anim: 1.0,
            dialogue_fade: 1.0,
            ui_time_s: 0.0,
            hud_toolbar_hovered: false,
            host_top_left_pad: 0.0,
            host_top_right_pad: 0.0,
        };
        let pkt = project(&ctx, 390.0, 844.0, &msgs);
        // Stage docked at safe top (59px)
        assert!((pkt.stage_rect[1] - 59.0).abs() < 0.5);
        let stage_bottom = pkt.stage_rect[1] + pkt.stage_rect[3];

        // Dialogue text sits below stage at comfortable >= 15px mobile reading size
        let dlg_run = pkt
            .texts
            .iter()
            .find(|t| t.text.contains("测试台词"))
            .expect("dialogue text run missing");
        assert!(
            dlg_run.size >= 16.0,
            "portrait reader font should be readable, got {}",
            dlg_run.size
        );
        assert!(
            dlg_run.y >= stage_bottom,
            "dialogue should sit below stage in portrait reader"
        );

        // Narrow portrait touch phones condense toolbar to 5 wide thumb-friendly buttons (>= 44px tall, >= 60px wide, incl. QuickSave)!
        assert_eq!(pkt.hits.len(), 5);
        assert!(pkt.hits.iter().any(|h| h.action == UiAction::QuickSave));
        for h in &pkt.hits {
            assert!(
                h.rect[2] >= 60.0 && h.rect[3] >= 42.0,
                "touch button {} too small: {:?}",
                h.label,
                h.rect
            );
            assert!(h.rect[1] >= insets.top - 0.5);
            assert!(h.rect[1] + h.rect[3] <= 844.0 - insets.bottom + 0.5);
        }

        // Narrow portrait Settings keeps draggable slider tracks visible even at 390px width!
        let settings_ctx = UiContext {
            screen: Screen::Settings,
            ..ctx
        };
        let set_pkt = project(&settings_ctx, 390.0, 844.0, &msgs);
        assert_eq!(set_pkt.sliders.len(), 3, "narrow portrait Settings tab 0 must render all 3 draggable sliders");
        assert!(set_pkt.overlay_panel_rect.is_some(), "modal overlay must expose panel bounds for backdrop tap dismiss");

        // Landscape phone (844x390) with horizontal notches (47px left/right, 21px bottom)
        let land_insets = SafeInsets {
            top: 0.0,
            right: 47.0,
            bottom: 21.0,
            left: 47.0,
        };
        let slots: Vec<SlotSummary> = (0..24)
            .map(|i| SlotSummary {
                slot: i,
                exists: i < 4,
                summary: format!("Line {i}"),
                saved_at_unix: 1000 + i as u64,
                has_thumb: false,
            })
            .collect();
        let saves_ctx = UiContext {
            vm: &vm,
            screen: Screen::Saves { save_mode: true },
            prefs: &prefs,
            auto: false,
            skip: false,
            hold_skip: false,
            interface_hidden: false,
            replay_active: false,
            slots: &slots,
            autosave: None,
            quicksave: None,
            saves_page: 1,
            history_scroll: 0,
            gallery_page: 0,
            viewing_cg: None,
            cg_zoom: 1.0,
            cg_pan: [0.0, 0.0],
            settings_tab: 0,
            pending_confirm: None,
            host_has_shelf: false,
            loading_visible: false,
            tz_offset_mins: 0,
            hovered_hit: None,
            pressed_hit: None,
            safe_insets: land_insets,
            input_mode: InputMode::Touch,
            portrait_reader: true,
            toast: None,
            overlay_anim: 1.0,
            toast_anim: 1.0,
            dialogue_fade: 1.0,
            ui_time_s: 0.0,
            hud_toolbar_hovered: false,
            host_top_left_pad: 0.0,
            host_top_right_pad: 0.0,
        };
        let saves_pkt = project(&saves_ctx, 844.0, 390.0, &msgs);
        // Must have pagination buttons and stay inside safe horizontal/bottom insets!
        assert!(saves_pkt
            .hits
            .iter()
            .any(|h| h.action == UiAction::SavesPage(-1)));
        for h in &saves_pkt.hits {
            assert!(
                h.rect[0] >= land_insets.left - 0.5,
                "button {} enters left notch: {:?}",
                h.label,
                h.rect
            );
            assert!(
                h.rect[0] + h.rect[2] <= 844.0 - land_insets.right + 0.5,
                "button {} enters right notch: {:?}",
                h.label,
                h.rect
            );
            assert!(
                h.rect[1] + h.rect[3] <= 390.0 - land_insets.bottom + 0.5,
                "button {} enters home indicator: {:?}",
                h.label,
                h.rect
            );
        }
    }

    #[test]
    fn visual_upgrades_ambient_frosted_sharpen_ruby_outline_and_chrome_clearance_work() {
        let prog = livemaker_like_program();
        let mut vm = Vm::new(prog, Some("zh-Hans".into())).unwrap();
        vm.step(1000).unwrap();
        // Inject a visible stage layer + rich colored/ruby span to exercise every visual path
        vm.layers.push(LayerState {
            id: LayerId(1),
            parent: None,
            asset: Some(AssetId(0)),
            transform: Transform2D::default(),
            shake_offset: [0.0, 0.0],
            text: None,
        });
        if let Some(dlg) = &mut vm.dialogue {
            dlg.spans.push(nir_vm::ResolvedSpan {
                byte_start: 0,
                byte_end: 6,
                emphasis: false,
                color: Some([1.0, 0.4, 0.5, 1.0]),
                ruby: Some("テスト".into()),
                speed: 1.0,
                inline_image: None,
            });
        }
        let prefs = Preferences::default();
        let msgs = Messages::default();

        // 1. Widescreen 1920x1080 over 800x600 stage (scale = 1.8 > 1.22):
        //    AmbientBackdrop + CAS sharpening + rich color + ruby + outline + chrome clearance!
        let story_ctx = UiContext {
            vm: &vm,
            screen: Screen::Story,
            prefs: &prefs,
            auto: true,
            skip: false,
            hold_skip: false,
            interface_hidden: false,
            replay_active: false,
            slots: &[],
            autosave: None,
            quicksave: None,
            saves_page: 0,
            history_scroll: 0,
            gallery_page: 0,
            viewing_cg: None,
            cg_zoom: 1.0,
            cg_pan: [0.0, 0.0],
            settings_tab: 0,
            pending_confirm: None,
            host_has_shelf: true,
            loading_visible: true,
            tz_offset_mins: 0,
            hovered_hit: None,
            pressed_hit: None,
            safe_insets: SafeInsets::default(),
            input_mode: InputMode::Pointer,
            portrait_reader: false,
            toast: Some("Saved"),
            overlay_anim: 1.0,
            toast_anim: 1.0,
            dialogue_fade: 1.0,
            ui_time_s: 0.5,
            hud_toolbar_hovered: false,
            host_top_left_pad: 92.0,
            host_top_right_pad: 52.0,
        };
        let spkt = project(&story_ctx, 1920.0, 1080.0, &msgs);
        assert!(spkt.needs_blur, "widescreen margins should enable blurred ambient backdrop");
        assert!(spkt.quads.iter().any(|q| q.special == SpecialQuad::AmbientBackdrop));
        assert!(
            spkt.quads.iter().any(|q| q.asset == Some(AssetId(0)) && q.sharpen > 0.15),
            "upscaled 800x600 stage art on 1080p should enable CAS-lite sharpening"
        );
        let dlg_run = spkt
            .texts
            .iter()
            .find(|t| t.text.contains("测试台词"))
            .expect("dialogue text missing");
        assert_eq!(dlg_run.color_spans.len(), 1);
        assert_eq!(dlg_run.ruby_spans.len(), 1);
        assert!(dlg_run.outline.is_some());
        // Loading pill clears left host shelf button (>= 92px)
        let load_run = spkt
            .texts
            .iter()
            .find(|t| t.text.contains("加载画面"))
            .expect("loading pill missing");
        assert!(load_run.x >= 96.0);

        // 2. Settings Tab 1 (Display) exposes frosted backdrop & all 4 visual controls!
        let settings_ctx = UiContext {
            screen: Screen::Settings,
            settings_tab: 1,
            ..story_ctx
        };
        let epkt = project(&settings_ctx, 1280.0, 720.0, &msgs);
        assert!(epkt.needs_blur);
        assert!(epkt.quads.iter().any(|q| q.special == SpecialQuad::FrostedBackdrop));
        assert!(epkt
            .hits
            .iter()
            .any(|h| matches!(h.action, UiAction::ToggleAmbientBackdrop)));
        assert!(epkt
            .hits
            .iter()
            .any(|h| matches!(h.action, UiAction::ToggleImageSharpening)));
        assert!(epkt
            .hits
            .iter()
            .any(|h| matches!(h.action, UiAction::ToggleTextOutline)));
        assert!(epkt
            .hits
            .iter()
            .any(|h| matches!(h.action, UiAction::SetDialogueOpacity(_))));
    }
}
