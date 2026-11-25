use nir_ir::{AssetId, BlendMode, TextAlign, TextShadow, Transition, UiAction};
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SpecialQuad {
    #[default]
    None,
    StageTransition,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Quad {
    pub rect: [f32; 4],
    /// Normalized UV rectangle `[u0, v0, u1, v1]` for atlases & sub-rects.
    pub uv: [f32; 4],
    /// Optional 4 transformed corners `[TL, TR, BL, BR]` for rotated / affine layers.
    pub corners: Option<[[f32; 2]; 4]>,
    pub color: [f32; 4],
    pub asset: Option<AssetId>,
    pub blend: BlendMode,
    pub clip: Option<[f32; 4]>,
    pub special: SpecialQuad,
}

impl Quad {
    pub fn solid(rect: [f32; 4], color: [f32; 4]) -> Self {
        Self {
            rect,
            uv: [0.0, 0.0, 1.0, 1.0],
            corners: None,
            color,
            asset: None,
            blend: BlendMode::Normal,
            clip: None,
            special: SpecialQuad::None,
        }
    }

    pub fn image(rect: [f32; 4], asset: AssetId) -> Self {
        Self {
            rect,
            uv: [0.0, 0.0, 1.0, 1.0],
            corners: None,
            color: [1.0, 1.0, 1.0, 1.0],
            asset: Some(asset),
            blend: BlendMode::Normal,
            clip: None,
            special: SpecialQuad::None,
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
    pub shadow: Option<TextShadow>,
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
        self.font_assets.hash(&mut h);
        (self.align as u8).hash(&mut h);
        self.auto_fit.hash(&mut h);
        self.cache_key = h.finish();
    }

    /// Cache key for the shadow pass: reuses `self.cache_key` directly when `emphasis` is empty
    /// (zero extra shaping!), or hashes without `emphasis` so shadow text stays shadow-colored.
    pub fn shadow_cache_key(&self) -> u64 {
        if self.emphasis.is_empty() {
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

    pub fn shadow_run(&self) -> Option<Self> {
        let s = self.shadow?;
        let mut r = self.clone();
        r.shadow = None;
        r.x += s.offset[0];
        r.y += s.offset[1];
        r.color = [
            s.color[0],
            s.color[1],
            s.color[2],
            s.color[3] * self.color[3],
        ];
        r.emphasis.clear();
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
            && self.quads == other.quads
            && self.texts == other.texts
            && self.transition == other.transition
            && self.transition_progress == other.transition_progress
    }
}
