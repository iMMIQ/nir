use nir_ir::{AssetId, BlendMode, TextAlign, TextShadow, Transition, UiAction};
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SpecialQuad {
    #[default]
    None,
    StageTransition,
    /// Blurred, dimmed stage backdrop filling letterbox margins & portrait gaps.
    AmbientBackdrop,
    /// Blurred stage backdrop behind modal overlays (frosted glass depth).
    FrostedBackdrop,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Quad {
    pub rect: [f32; 4],
    /// Normalized UV rectangle `[u0, v0, u1, v1]` for atlases & sub-rects.
    pub uv: [f32; 4],
    /// Optional 4 transformed corners `[TL, TR, BL, BR]` for rotated / affine layers.
    pub corners: Option<[[f32; 2]; 4]>,
    pub color: [f32; 4],
    /// Optional secondary gradient color (bottom edge when `gradient_horizontal == false`,
    /// right edge when `gradient_horizontal == true`). Zero extra shader branches!
    pub color_end: Option<[f32; 4]>,
    pub gradient_horizontal: bool,
    pub asset: Option<AssetId>,
    pub blend: BlendMode,
    pub clip: Option<[f32; 4]>,
    pub special: SpecialQuad,
    /// Optional SDF corner radius in logical pixels (0 = sharp rectangle).
    pub corner_radius: f32,
    /// Optional single-pass SDF border width in logical pixels (0 = no border).
    pub border_width: f32,
    /// Single-pass SDF border RGBA (straight sRGB, ignored when `border_width <= 0`).
    pub border_color: [f32; 4],
    /// Optional CAS-lite edge sharpening strength (`0.0..=1.0`) for upscaled classic CGs.
    pub sharpen: f32,
}

impl Quad {
    pub fn solid(rect: [f32; 4], color: [f32; 4]) -> Self {
        Self {
            rect,
            uv: [0.0, 0.0, 1.0, 1.0],
            corners: None,
            color,
            color_end: None,
            gradient_horizontal: false,
            asset: None,
            blend: BlendMode::Normal,
            clip: None,
            special: SpecialQuad::None,
            corner_radius: 0.0,
            border_width: 0.0,
            border_color: [0.0; 4],
            sharpen: 0.0,
        }
    }

    pub fn gradient_v(rect: [f32; 4], top: [f32; 4], bottom: [f32; 4]) -> Self {
        Self {
            color_end: Some(bottom),
            gradient_horizontal: false,
            ..Self::solid(rect, top)
        }
    }

    pub fn gradient_h(rect: [f32; 4], left: [f32; 4], right: [f32; 4]) -> Self {
        Self {
            color_end: Some(right),
            gradient_horizontal: true,
            ..Self::solid(rect, left)
        }
    }

    pub fn rounded(rect: [f32; 4], color: [f32; 4], corner_radius: f32) -> Self {
        Self {
            corner_radius,
            ..Self::solid(rect, color)
        }
    }

    pub fn panel(
        rect: [f32; 4],
        fill_top: [f32; 4],
        fill_bottom: Option<[f32; 4]>,
        border: [f32; 4],
        border_width: f32,
        corner_radius: f32,
    ) -> Self {
        Self {
            color_end: fill_bottom,
            corner_radius,
            border_width: if border[3] > 0.01 { border_width } else { 0.0 },
            border_color: border,
            ..Self::solid(rect, fill_top)
        }
    }

    pub fn image(rect: [f32; 4], asset: AssetId) -> Self {
        Self {
            asset: Some(asset),
            ..Self::solid(rect, [1.0, 1.0, 1.0, 1.0])
        }
    }

    pub fn rounded_image(rect: [f32; 4], asset: AssetId, corner_radius: f32) -> Self {
        Self {
            corner_radius,
            ..Self::image(rect, asset)
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    pub text: String,
    pub visible_bytes: Option<usize>,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub size: f32,
    pub line_height: f32,
    pub color: [f32; 4],
    pub emphasis: Vec<(usize, usize)>,
    /// Rich per-span colors `(byte_start, byte_end, rgba)` from script markup.
    pub color_spans: Vec<(usize, usize, [f32; 4])>,
    /// Ruby phonetic annotations `(byte_start, byte_end, ruby_text)` rendered above base glyphs.
    pub ruby_spans: Vec<(usize, usize, String)>,
    pub shadow: Option<TextShadow>,
    /// Optional 4-way soft outline RGBA for crisp legibility over bright/detailed CGs.
    pub outline: Option<[f32; 4]>,
    pub scroll: f32,
    pub clip: Option<[f32; 4]>,
    pub font_assets: Vec<AssetId>,
    pub align: TextAlign,
    pub valign_center: bool,
    pub auto_fit: bool,
    /// Inline image slots `(byte_offset_in_text, asset_id)` rendered inside `\u{3000}` gaps.
    pub inline_images: Vec<(usize, AssetId)>,
    /// Precomputed 64-bit shaping cache key — zero String allocation per frame!
    pub cache_key: u64,
}

impl TextRun {
    pub fn compute_key(&mut self) {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.text.hash(&mut h);
        self.width.to_bits().hash(&mut h);
        if self.auto_fit {
            self.height.to_bits().hash(&mut h);
        }
        self.size.to_bits().hash(&mut h);
        self.line_height.to_bits().hash(&mut h);
        self.emphasis.hash(&mut h);
        for &(s, e, c) in &self.color_spans {
            s.hash(&mut h);
            e.hash(&mut h);
            for ch in c {
                ch.to_bits().hash(&mut h);
            }
        }
        self.font_assets.hash(&mut h);
        (self.align as u8).hash(&mut h);
        self.auto_fit.hash(&mut h);
        self.cache_key = h.finish();
    }

    /// Cache key for shadow/outline passes: reuses `self.cache_key` directly when no rich span colors
    /// exist (zero extra shaping!), or hashes monochrome so shadow/outline stays uniformly dark.
    pub fn shadow_cache_key(&self) -> u64 {
        if self.emphasis.is_empty() && self.color_spans.is_empty() {
            return self.cache_key;
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.text.hash(&mut h);
        self.width.to_bits().hash(&mut h);
        if self.auto_fit {
            self.height.to_bits().hash(&mut h);
        }
        self.size.to_bits().hash(&mut h);
        self.line_height.to_bits().hash(&mut h);
        let empty: &[(usize, usize)] = &[];
        empty.hash(&mut h);
        self.font_assets.hash(&mut h);
        (self.align as u8).hash(&mut h);
        self.auto_fit.hash(&mut h);
        h.finish()
    }

    pub fn has_rich_colors(&self) -> bool {
        !self.emphasis.is_empty() || !self.color_spans.is_empty()
    }

    pub fn shadow_run(&self) -> Option<Self> {
        let s = self.shadow?;
        let mut r = self.clone();
        r.shadow = None;
        r.outline = None;
        r.x += s.offset[0];
        r.y += s.offset[1];
        r.color = [
            s.color[0],
            s.color[1],
            s.color[2],
            s.color[3] * self.color[3],
        ];
        r.emphasis.clear();
        r.color_spans.clear();
        r.ruby_spans.clear();
        r.compute_key();
        Some(r)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HitRegion {
    pub id: u32,
    pub rect: [f32; 4],
    pub action: UiAction,
    pub label: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DrawPacket {
    pub width: f32,
    pub height: f32,
    pub stage_size: [u32; 2],
    pub stage_rect: [f32; 4],
    /// Index slice `[start, end)` inside `quads` containing the pure stage scene (for GPU blur).
    pub stage_quad_range: [usize; 2],
    /// True when any `AmbientBackdrop` or `FrostedBackdrop` quad requires the blurred stage texture.
    pub needs_blur: bool,
    pub quads: Vec<Quad>,
    pub texts: Vec<TextRun>,
    pub hits: Vec<HitRegion>,
    pub transition_from: Vec<Quad>,
    pub transition_to: Vec<Quad>,
    pub transition: Transition,
    pub transition_progress: f32,
}

impl DrawPacket {
    pub fn hit_test(&self, x: f32, y: f32) -> Option<&HitRegion> {
        // Reverse order: topmost UI control wins!
        self.hits.iter().rev().find(|h| {
            h.enabled
                && x >= h.rect[0]
                && x <= h.rect[0] + h.rect[2]
                && y >= h.rect[1]
                && y <= h.rect[1] + h.rect[3]
        })
    }

    pub fn visual_eq(&self, other: &Self) -> bool {
        self.width == other.width
            && self.height == other.height
            && self.stage_rect == other.stage_rect
            && self.needs_blur == other.needs_blur
            && self.quads == other.quads
            && self.texts == other.texts
            && self.transition == other.transition
            && self.transition_progress == other.transition_progress
    }
}
