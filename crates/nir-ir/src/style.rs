//! Adaptive visual style DNA extraction & OKLCH perceptual theming.
//!
//! Pure-Rust, zero-unsafe, zero-dependency perceptual color & composition analyzer
//! shared across importers (`nir-import-*`), runtime (`nir-runtime`), and UI (`nir-ui`).
//! Automatically derives a harmonious, WCAG-contrasted palette, style archetype, and
//! non-occluding title-screen layout from each game's original artwork (`dialogue_bg`
//! and `title_bg`), working both at import time and dynamically at runtime for legacy `.nirpak`s.

use crate::UiTheme;
use serde::{Deserialize, Serialize};

/// Player-selectable UI presentation mode (persisted in JSON `Preferences`, backward-compatible).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiStyleMode {
    /// Preserves original `dialogue_bg` in landscape with auto-matched nameplate & contrast backing,
    /// while rendering portrait reader & system overlays in adaptive modern frosted glass.
    #[default]
    Hybrid,
    /// HD-remastered modern frosted-glass dialogue box & overlays using the game's extracted palette.
    Modern,
    /// Faithfully displays the original `dialogue_bg` and layout with minimal modern embellishment.
    Classic,
}

impl UiStyleMode {
    pub fn next(self) -> Self {
        match self {
            Self::Hybrid => Self::Modern,
            Self::Modern => Self::Classic,
            Self::Classic => Self::Hybrid,
        }
    }

    pub fn ftl_key(self) -> &'static str {
        match self {
            Self::Hybrid => "ui-style-hybrid",
            Self::Modern => "ui-style-modern",
            Self::Classic => "ui-style-classic",
        }
    }
}

/// Visual personality inferred automatically from the game's original artwork.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StyleArchetype {
    /// Clean contemporary visual novel look: medium radius (`10..12px`), crisp 1px glass rim.
    #[default]
    Sleek,
    /// Warm/pastel/romantic tone: softer radii (`14..16px`), pill nameplates, luminous warm glow.
    Soft,
    /// Literary/classic/warm-paper aesthetic: restrained radii (`6..8px`), double-hairline frames.
    ClassicPaper,
    /// Moody/dark/high-contrast aesthetic: crisp compact radii (`5..6px`), metallic hairline accents.
    Noir,
}

impl StyleArchetype {
    pub fn panel_radius(self) -> f32 {
        match self {
            Self::Sleek => 12.0,
            Self::Soft => 16.0,
            Self::ClassicPaper => 7.0,
            Self::Noir => 5.0,
        }
    }

    pub fn card_radius(self) -> f32 {
        match self {
            Self::Sleek => 10.0,
            Self::Soft => 13.0,
            Self::ClassicPaper => 6.0,
            Self::Noir => 4.5,
        }
    }

    pub fn button_radius(self, height: f32) -> f32 {
        match self {
            Self::Sleek => (height * 0.24).clamp(6.0, 11.0),
            Self::Soft => (height * 0.38).clamp(9.0, 18.0),
            Self::ClassicPaper => (height * 0.16).clamp(4.0, 7.0),
            Self::Noir => (height * 0.14).clamp(3.5, 6.0),
        }
    }

    pub fn glass_rim_strength(self) -> f32 {
        match self {
            Self::Sleek => 0.24,
            Self::Soft => 0.28,
            Self::ClassicPaper => 0.14,
            Self::Noir => 0.18,
        }
    }
}

/// Best unobstructed placement for the built-in title screen menu, derived from `title_bg` saliency.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TitleAnchor {
    #[default]
    Left,
    Right,
    BottomBar,
}

/// Extracted features from an original `dialogue_bg` skin texture.
#[derive(Debug, Clone, PartialEq)]
pub struct SkinSample {
    pub avg_l: f32,
    pub avg_c: f32,
    pub avg_alpha: f32,
    pub dominant_rgb: [f32; 3],
    pub accent_rgb: [f32; 3],
    pub accent_chroma: f32,
    pub is_light: bool,
    pub soft_corners: bool,
}

/// Extracted color & visual-saliency composition from a game's `title_bg`.
#[derive(Debug, Clone, PartialEq)]
pub struct TitleAnalysis {
    pub avg_l: f32,
    pub avg_c: f32,
    pub dominant_rgb: [f32; 3],
    pub accent_rgb: [f32; 3],
    pub accent_chroma: f32,
    pub anchor: TitleAnchor,
    pub has_embedded_logo: bool,
}

/// Fully resolved runtime style tokens uniting original game DNA with modern legibility.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedStyle {
    pub surface: [f32; 4],
    pub panel: [f32; 4],
    pub panel_border: [f32; 4],
    pub text: [f32; 4],
    pub muted: [f32; 4],
    pub accent: [f32; 4],
    pub accent_soft: [f32; 4],
    pub nameplate_bg: [f32; 4],
    pub nameplate_text: [f32; 4],
    pub dialogue_is_light: bool,
    pub dialogue_alpha_avg: f32,
    pub archetype: StyleArchetype,
    pub title_anchor: TitleAnchor,
    pub title_has_embedded_logo: bool,
    pub scene_tint: Option<[f32; 3]>,
    pub derived_from_art: bool,
}

impl ResolvedStyle {
    pub fn from_theme(theme: &UiTheme) -> Self {
        let accent = theme.accent;
        let accent_soft = [accent[0], accent[1], accent[2], 0.24];
        let nameplate_bg = [
            theme.surface[0] * 0.92,
            theme.surface[1] * 0.92,
            theme.surface[2] * 0.92,
            0.90,
        ];
        Self {
            surface: theme.surface,
            panel: theme.panel,
            panel_border: theme.panel_border,
            text: theme.text,
            muted: theme.muted,
            accent,
            accent_soft,
            nameplate_bg,
            nameplate_text: theme.text,
            dialogue_is_light: false,
            dialogue_alpha_avg: 0.90,
            archetype: infer_archetype_from_colors(theme.surface, theme.accent, false, false),
            title_anchor: TitleAnchor::Left,
            title_has_embedded_logo: false,
            scene_tint: None,
            derived_from_art: false,
        }
    }

    /// Returns true if a `UiTheme` still carries the untouched built-in default colors.
    pub fn theme_uses_default_palette(theme: &UiTheme) -> bool {
        let def = UiTheme::default();
        let eq = |a: [f32; 4], b: [f32; 4]| {
            (a[0] - b[0]).abs() < 0.015
                && (a[1] - b[1]).abs() < 0.015
                && (a[2] - b[2]).abs() < 0.015
        };
        eq(theme.surface, def.surface) && eq(theme.panel, def.panel) && eq(theme.accent, def.accent)
    }

    /// Formats a lightweight JSON object of CSS custom properties so Web host chrome matches the game.
    pub fn to_web_css_vars_json(&self) -> String {
        let to_255 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let ar = to_255(self.accent[0]);
        let ag = to_255(self.accent[1]);
        let ab = to_255(self.accent[2]);
        let sr = to_255(self.surface[0]);
        let sg = to_255(self.surface[1]);
        let sb = to_255(self.surface[2]);
        let pr = to_255(self.panel[0]);
        let pg = to_255(self.panel[1]);
        let pb = to_255(self.panel[2]);
        let tr = to_255(self.text[0]);
        let tg = to_255(self.text[1]);
        let tb = to_255(self.text[2]);
        let br = to_255(self.panel_border[0]);
        let bg = to_255(self.panel_border[1]);
        let bb = to_255(self.panel_border[2]);
        let radius = self.archetype.card_radius().round() as u32;
        format!(
            concat!(
                "{{\"--nir-accent\":\"rgb({},{},{})\",",
                "\"--nir-accent-bg\":\"rgba({},{},{},0.82)\",",
                "\"--nir-accent-border\":\"rgba({},{},{},0.62)\",",
                "\"--nir-surface\":\"rgba({},{},{},0.74)\",",
                "\"--nir-surface-hover\":\"rgba({},{},{},0.92)\",",
                "\"--nir-border\":\"rgba({},{},{},0.38)\",",
                "\"--nir-text\":\"rgb({},{},{})\",",
                "\"--nir-radius\":\"{}px\"}}"
            ),
            ar, ag, ab, ar, ag, ab, ar, ag, ab, sr, sg, sb, pr, pg, pb, br, bg, bb, tr, tg, tb,
            radius
        )
    }
}

// ---------------------------------------------------------------------------
// OKLCH Perceptual Color Math (Björn Ottosson OKLab reference matrices)
// ---------------------------------------------------------------------------

#[inline]
fn srgb_to_linear_ch(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[inline]
fn linear_to_srgb_ch(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Converts sRGB `[r, g, b]` in `0..=1` into perceptual OKLCH `[L, C, H_rad]`.
pub fn srgb_to_oklch(rgb: [f32; 3]) -> [f32; 3] {
    let r = srgb_to_linear_ch(rgb[0]);
    let g = srgb_to_linear_ch(rgb[1]);
    let b = srgb_to_linear_ch(rgb[2]);

    let l = (0.41222147 * r + 0.53633254 * g + 0.05144599 * b).max(0.0).cbrt();
    let m = (0.21190350 * r + 0.68069955 * g + 0.10739696 * b).max(0.0).cbrt();
    let s = (0.08830246 * r + 0.28171884 * g + 0.62997870 * b).max(0.0).cbrt();

    let ok_l = 0.21045426 * l + 0.79361779 * m - 0.00407205 * s;
    let ok_a = 1.97799850 * l - 2.42859221 * m + 0.45059371 * s;
    let ok_b = 0.02590404 * l + 0.78277177 * m - 0.80867577 * s;

    let c = (ok_a * ok_a + ok_b * ok_b).sqrt();
    let h = ok_b.atan2(ok_a);
    [ok_l.clamp(0.0, 1.0), c.max(0.0), h]
}

/// Converts OKLCH `[L, C, H_rad]` back into clamped sRGB `[r, g, b]` in `0..=1`.
pub fn oklch_to_srgb(lch: [f32; 3]) -> [f32; 3] {
    let ok_l = lch[0].clamp(0.0, 1.0);
    let mut c = lch[1].max(0.0);
    let (sin_h, cos_h) = lch[2].sin_cos();

    // Soft chroma bisection if out of sRGB gamut to avoid ugly hue clipping.
    for _ in 0..4 {
        let a = c * cos_h;
        let b = c * sin_h;
        let l_ = ok_l + 0.39633778 * a + 0.21580376 * b;
        let m_ = ok_l - 0.10556135 * a - 0.06385417 * b;
        let s_ = ok_l - 0.08948418 * a - 1.29148555 * b;
        let l = l_ * l_ * l_;
        let m = m_ * m_ * m_;
        let s = s_ * s_ * s_;
        let r = 4.07674166 * l - 3.30771159 * m + 0.23096993 * s;
        let g = -1.26843800 * l + 2.60975740 * m - 0.34131940 * s;
        let bl = -0.00419609 * l - 0.70341861 * m + 1.70761470 * s;
        if r >= -0.005 && r <= 1.005 && g >= -0.005 && g <= 1.005 && bl >= -0.005 && bl <= 1.005 {
            return [
                linear_to_srgb_ch(r),
                linear_to_srgb_ch(g),
                linear_to_srgb_ch(bl),
            ];
        }
        c *= 0.80;
    }

    let a = c * cos_h;
    let b = c * sin_h;
    let l_ = ok_l + 0.39633778 * a + 0.21580376 * b;
    let m_ = ok_l - 0.10556135 * a - 0.06385417 * b;
    let s_ = ok_l - 0.08948418 * a - 1.29148555 * b;
    let l = l_ * l_ * l_;
    let m = m_ * m_ * m_;
    let s = s_ * s_ * s_;
    let r = 4.07674166 * l - 3.30771159 * m + 0.23096993 * s;
    let g = -1.26843800 * l + 2.60975740 * m - 0.34131940 * s;
    let bl = -0.00419609 * l - 0.70341861 * m + 1.70761470 * s;
    [
        linear_to_srgb_ch(r),
        linear_to_srgb_ch(g),
        linear_to_srgb_ch(bl),
    ]
}

/// WCAG 2.1 relative luminance for sRGB `[r, g, b]`.
pub fn relative_luminance(rgb: [f32; 3]) -> f32 {
    0.2126 * srgb_to_linear_ch(rgb[0])
        + 0.7152 * srgb_to_linear_ch(rgb[1])
        + 0.0722 * srgb_to_linear_ch(rgb[2])
}

/// WCAG 2.1 contrast ratio (`1.0..=21.0`).
pub fn contrast_ratio(a: [f32; 3], b: [f32; 3]) -> f32 {
    let la = relative_luminance(a);
    let lb = relative_luminance(b);
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Adjusts text lightness in OKLCH so it achieves at least `min_ratio` contrast against `bg`.
pub fn ensure_contrast_text(bg: [f32; 3], candidate: [f32; 3], min_ratio: f32) -> [f32; 3] {
    if contrast_ratio(bg, candidate) >= min_ratio {
        return candidate;
    }
    let bg_lum = relative_luminance(bg);
    let mut lch = srgb_to_oklch(candidate);
    let lighten = bg_lum < 0.34;
    for _ in 0..10 {
        lch[0] = if lighten {
            (lch[0] + 0.08).min(0.98)
        } else {
            (lch[0] - 0.08).max(0.14)
        };
        lch[1] *= 0.92;
        let next = oklch_to_srgb(lch);
        if contrast_ratio(bg, next) >= min_ratio {
            return next;
        }
    }
    if lighten {
        [0.96, 0.97, 0.99]
    } else {
        [0.10, 0.11, 0.15]
    }
}

/// Deterministic, harmonious per-speaker color in OKLCH for backlog timelines & unnamed speaker hues.
pub fn speaker_palette_color(name: &str, base_accent: [f32; 4], light_surface: bool) -> [f32; 4] {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in name.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    let base_lch = srgb_to_oklch([base_accent[0], base_accent[1], base_accent[2]]);
    // Spread across 10 curated perceptual hue offsets around the game's base accent hue.
    let slot = (h % 10) as f32;
    let hue = base_lch[2] + slot * (std::f32::consts::TAU / 10.0);
    let target_l = if light_surface { 0.44 } else { 0.80 };
    let target_c = if light_surface { 0.15 } else { 0.12 };
    let rgb = oklch_to_srgb([target_l, target_c, hue]);
    [rgb[0], rgb[1], rgb[2], 1.0]
}

// ---------------------------------------------------------------------------
// Image Pixel Analyzers (Pure RGBA slice, handles arbitrary sizes in < 0.5ms)
// ---------------------------------------------------------------------------

struct HueBucket {
    weight: f32,
    sum_l: f32,
    sum_c: f32,
    sum_cos: f32,
    sum_sin: f32,
}

impl Default for HueBucket {
    fn default() -> Self {
        Self {
            weight: 0.0,
            sum_l: 0.0,
            sum_c: 0.0,
            sum_cos: 0.0,
            sum_sin: 0.0,
        }
    }
}

fn sample_rgba_buckets(
    w: u32,
    h: u32,
    rgba: &[u8],
    max_samples: usize,
) -> Option<(f32, f32, f32, [f32; 3], [f32; 3], f32)> {
    if w == 0 || h == 0 || rgba.len() < (w as usize * h as usize * 4) {
        return None;
    }
    let total_px = (w as usize) * (h as usize);
    let step = ((total_px as f32 / max_samples.max(64) as f32).sqrt().floor() as usize).max(1);

    let mut dom_buckets: [HueBucket; 18] = std::array::from_fn(|_| HueBucket::default());
    let mut vib_buckets: [HueBucket; 18] = std::array::from_fn(|_| HueBucket::default());
    let mut total_w = 0.0_f32;
    let mut sum_l = 0.0_f32;
    let mut sum_c = 0.0_f32;
    let mut sum_r = 0.0_f32;
    let mut sum_g = 0.0_f32;
    let mut sum_b = 0.0_f32;
    let mut alpha_sum = 0.0_f32;
    let mut sampled_px = 0usize;

    let wu = w as usize;
    let hu = h as usize;
    // Detect straight vs premultiplied RGBA automatically: straight RGBA frequently has RGB > A on translucent pixels.
    let mut has_translucent = false;
    let mut rgb_exceeds_a = false;
    {
        let mut py = 0usize;
        while py < hu && !rgb_exceeds_a {
            let mut px = 0usize;
            while px < wu {
                let idx = (py * wu + px) * 4;
                let a = rgba[idx + 3];
                if a > 8 && a < 248 {
                    has_translucent = true;
                    if rgba[idx] > a.saturating_add(2)
                        || rgba[idx + 1] > a.saturating_add(2)
                        || rgba[idx + 2] > a.saturating_add(2)
                    {
                        rgb_exceeds_a = true;
                        break;
                    }
                }
                px += step;
            }
            py += step;
        }
    }
    let is_premul = has_translucent && !rgb_exceeds_a;

    let mut y = 0usize;
    while y < hu {
        let mut x = 0usize;
        while x < wu {
            let idx = (y * wu + x) * 4;
            let a = rgba[idx + 3] as f32 / 255.0;
            alpha_sum += a;
            sampled_px += 1;
            if a > 0.14 {
                let inv_a = if is_premul { 1.0 / a.max(0.20) } else { 1.0 };
                let r = (rgba[idx] as f32 / 255.0 * inv_a).min(1.0);
                let g = (rgba[idx + 1] as f32 / 255.0 * inv_a).min(1.0);
                let b = (rgba[idx + 2] as f32 / 255.0 * inv_a).min(1.0);
                let lch = srgb_to_oklch([r, g, b]);
                let [ok_l, ok_c, ok_h] = lch;

                total_w += a;
                sum_l += ok_l * a;
                sum_c += ok_c * a;
                sum_r += r * a;
                sum_g += g * a;
                sum_b += b * a;

                let norm_h = ok_h.rem_euclid(std::f32::consts::TAU);
                let bi = ((norm_h / std::f32::consts::TAU) * 18.0) as usize % 18;
                let (sh, ch) = ok_h.sin_cos();

                let dw = a * (0.25 + ok_c * 3.0);
                let db = &mut dom_buckets[bi];
                db.weight += dw;
                db.sum_l += ok_l * dw;
                db.sum_c += ok_c * dw;
                db.sum_cos += ch * dw;
                db.sum_sin += sh * dw;

                // Vibrant & pastel accents: capture both rich hues and delicate sakura/cream pastels!
                if ok_c > 0.018 && ok_l > 0.14 && ok_l < 0.985 {
                    let mid_l = 1.0 - (ok_l - 0.62).abs().min(0.62);
                    let vw = a * ok_c * ok_c * (0.35 + mid_l);
                    let vb = &mut vib_buckets[bi];
                    vb.weight += vw;
                    vb.sum_l += ok_l * vw;
                    vb.sum_c += ok_c * vw;
                    vb.sum_cos += ch * vw;
                    vb.sum_sin += sh * vw;
                }
            }
            x += step;
        }
        y += step;
    }

    if total_w <= 0.01 || sampled_px == 0 {
        return None;
    }
    let avg_l = sum_l / total_w;
    let avg_c = sum_c / total_w;
    let avg_alpha = alpha_sum / sampled_px as f32;
    let fallback_rgb = [sum_r / total_w, sum_g / total_w, sum_b / total_w];

    let bucket_to_rgb = |b: &HueBucket| -> Option<([f32; 3], f32)> {
        if b.weight <= 0.0001 {
            return None;
        }
        let l = b.sum_l / b.weight;
        let c = b.sum_c / b.weight;
        let h = b.sum_sin.atan2(b.sum_cos);
        Some((oklch_to_srgb([l, c, h]), c))
    };

    let best_dom = dom_buckets
        .iter()
        .max_by(|a, b| a.weight.total_cmp(&b.weight))
        .and_then(bucket_to_rgb)
        .map_or(fallback_rgb, |(rgb, _)| rgb);

    let (best_acc, acc_chroma) = vib_buckets
        .iter()
        .max_by(|a, b| a.weight.total_cmp(&b.weight))
        .and_then(bucket_to_rgb)
        .unwrap_or((best_dom, avg_c));

    Some((avg_l, avg_c, avg_alpha, best_dom, best_acc, acc_chroma))
}

/// Analyzes an original dialogue box skin (`dialogue_bg`) for color DNA, lightness & corner softness.
pub fn analyze_dialogue_skin_rgba(w: u32, h: u32, rgba: &[u8]) -> Option<SkinSample> {
    let (avg_l, avg_c, avg_alpha, dominant_rgb, accent_rgb, accent_chroma) =
        sample_rgba_buckets(w, h, rgba, 1600)?;

    // Probe the 4 outer corners to detect rounded/soft/feathered original boxes vs sharp rectangular ones
    let corner_alpha = |cx: u32, cy: u32| -> f32 {
        let idx = (cy.min(h - 1) as usize * w as usize + cx.min(w - 1) as usize) * 4;
        rgba[idx + 3] as f32 / 255.0
    };
    let c_avg = (corner_alpha(0, 0)
        + corner_alpha(w - 1, 0)
        + corner_alpha(0, h - 1)
        + corner_alpha(w - 1, h - 1))
        * 0.25;
    let soft_corners = c_avg < 0.35;

    Some(SkinSample {
        avg_l,
        avg_c,
        avg_alpha,
        dominant_rgb,
        accent_rgb,
        accent_chroma,
        is_light: avg_l > 0.68,
        soft_corners,
    })
}

/// Analyzes a game's `title_bg` for perceptual palette, visual-saliency menu anchor, and embedded logo hints.
pub fn analyze_title_rgba(w: u32, h: u32, rgba: &[u8]) -> Option<TitleAnalysis> {
    let (avg_l, avg_c, _avg_alpha, dominant_rgb, accent_rgb, accent_chroma) =
        sample_rgba_buckets(w, h, rgba, 2048)?;

    // Build a fast 16x9 luma + chroma grid to measure visual busyness (Sobel energy + focal contrast)
    const GW: usize = 16;
    const GH: usize = 9;
    let mut luma = [[0.0_f32; GW]; GH];
    let mut chroma = [[0.0_f32; GW]; GH];

    let wu = w as usize;
    let hu = h as usize;
    for gy in 0..GH {
        let py = ((gy * 2 + 1) * hu / (GH * 2)).min(hu - 1);
        for gx in 0..GW {
            let px = ((gx * 2 + 1) * wu / (GW * 2)).min(wu - 1);
            let idx = (py * wu + px) * 4;
            let r = rgba[idx] as f32 / 255.0;
            let g = rgba[idx + 1] as f32 / 255.0;
            let b = rgba[idx + 2] as f32 / 255.0;
            luma[gy][gx] = 0.299 * r + 0.587 * g + 0.114 * b;
            let mx = r.max(g).max(b);
            let mn = r.min(g).min(b);
            chroma[gy][gx] = mx - mn;
        }
    }

    let mut energy = [[0.0_f32; GW]; GH];
    for gy in 0..GH {
        let ym = gy.saturating_sub(1);
        let yp = (gy + 1).min(GH - 1);
        for gx in 0..GW {
            let xm = gx.saturating_sub(1);
            let xp = (gx + 1).min(GW - 1);
            let dx = (luma[gy][xp] - luma[gy][xm]).abs();
            let dy = (luma[yp][gx] - luma[ym][gx]).abs();
            energy[gy][gx] = dx + dy + chroma[gy][gx] * 0.45;
        }
    }

    let region_score = |x0: usize, x1: usize, y0: usize, y1: usize| -> f32 {
        let mut s = 0.0_f32;
        let mut n = 0usize;
        for gy in y0..y1 {
            for gx in x0..x1 {
                s += energy[gy][gx];
                n += 1;
            }
        }
        if n > 0 { s / n as f32 } else { 1.0 }
    };

    let left_busy = region_score(0, 6, 1, 8);
    let right_busy = region_score(10, 16, 1, 8);
    let bottom_busy = region_score(2, 14, 6, 9);
    let upper_center_busy = region_score(3, 13, 0, 4);

    // Slight natural bias toward Left (classic VN reading flow), switching to Right or BottomBar
    // whenever another region is meaningfully calmer and avoids occluding characters.
    let anchor = if bottom_busy + 0.05 < left_busy.min(right_busy) {
        TitleAnchor::BottomBar
    } else if right_busy + 0.045 < left_busy {
        TitleAnchor::Right
    } else {
        TitleAnchor::Left
    };

    // High-contrast upper band energy is a reliable signal that the cover art already contains a title logo.
    let has_embedded_logo = upper_center_busy > 0.34;

    Some(TitleAnalysis {
        avg_l,
        avg_c,
        dominant_rgb,
        accent_rgb,
        accent_chroma,
        anchor,
        has_embedded_logo,
    })
}

/// Fast low-frequency ambient light color extractor for active story backgrounds (< 0.1ms).
pub fn analyze_scene_tint_rgba(w: u32, h: u32, rgba: &[u8]) -> Option<[f32; 3]> {
    let (_avg_l, _avg_c, avg_alpha, dominant_rgb, accent_rgb, accent_chroma) =
        sample_rgba_buckets(w, h, rgba, 256)?;
    if avg_alpha < 0.70 {
        // Ignore transparent character sprites; only sample full/opaque scene backgrounds.
        return None;
    }
    let src = if accent_chroma > 0.06 {
        accent_rgb
    } else {
        dominant_rgb
    };
    let lch = srgb_to_oklch(src);
    let tinted = oklch_to_srgb([0.52, lch[1].clamp(0.03, 0.13), lch[2]]);
    Some(tinted)
}

fn infer_archetype_from_colors(
    surface: [f32; 4],
    accent: [f32; 4],
    soft_corners: bool,
    light_skin: bool,
) -> StyleArchetype {
    let slch = srgb_to_oklch([surface[0], surface[1], surface[2]]);
    let alch = srgb_to_oklch([accent[0], accent[1], accent[2]]);
    let hue_deg = alch[2].rem_euclid(std::f32::consts::TAU).to_degrees();

    // Warm rose / sakura / peach / golden-pink hues or soft rounded light skins -> Soft romantic feel
    let is_rose_or_pastel = (hue_deg >= 325.0 || hue_deg <= 42.0) && alch[1] > 0.09;
    if soft_corners || light_skin || (is_rose_or_pastel && slch[0] > 0.14) {
        return StyleArchetype::Soft;
    }
    // Warm amber / sepia / earth / muted gold -> Classic literary / Japanese paper feel
    if (42.0..=110.0).contains(&hue_deg) && alch[1] <= 0.15 {
        return StyleArchetype::ClassicPaper;
    }
    // Deep crimson / moody violet with dark low-lightness atmosphere -> Noir / Gothic
    if slch[0] < 0.13 && ((280.0..335.0).contains(&hue_deg) || hue_deg <= 25.0) {
        return StyleArchetype::Noir;
    }
    StyleArchetype::Sleek
}

/// Synthesizes a harmonious, WCAG-verified `ResolvedStyle` combining `UiTheme` overrides
/// with artwork-derived DNA (`SkinSample`, `TitleAnalysis`, and optional live `scene_tint`).
pub fn synthesize_resolved_style(
    base: &UiTheme,
    title: Option<&TitleAnalysis>,
    skin: Option<&SkinSample>,
    scene_tint: Option<[f32; 3]>,
) -> ResolvedStyle {
    let mut out = ResolvedStyle::from_theme(base);
    out.scene_tint = scene_tint;

    if let Some(t) = title {
        out.title_anchor = t.anchor;
        out.title_has_embedded_logo = t.has_embedded_logo;
    }
    if let Some(s) = skin {
        out.dialogue_is_light = s.is_light;
        out.dialogue_alpha_avg = s.avg_alpha;
    }

    let custom_author_theme = !ResolvedStyle::theme_uses_default_palette(base);
    let primary_art = skin
        .map(|s| (s.dominant_rgb, s.accent_rgb, s.accent_chroma, s.avg_l, s.soft_corners, s.is_light))
        .or_else(|| {
            title.map(|t| (t.dominant_rgb, t.accent_rgb, t.accent_chroma, t.avg_l, false, false))
        });

    if let Some((dom_rgb, acc_rgb, acc_chroma, art_l, soft_corners, is_light_skin)) = primary_art {
        out.derived_from_art = true;
        if !custom_author_theme {
            let dom_lch = srgb_to_oklch(dom_rgb);
            let mut acc_lch = srgb_to_oklch(acc_rgb);
            // If artwork was nearly grayscale, fall back to a calm refined sky/champagne hue.
            if acc_chroma < 0.015 && dom_lch[1] < 0.015 {
                acc_lch = [0.76, 0.08, 235.0_f32.to_radians()];
            } else {
                acc_lch[0] = acc_lch[0].clamp(0.70, 0.84);
                acc_lch[1] = acc_lch[1].clamp(0.10, 0.17);
            }
            let accent_rgb = oklch_to_srgb(acc_lch);

            // Subtle tinted dark glass surfaces inspired by the game's dominant hue!
            let base_hue = if dom_lch[1] > 0.025 {
                dom_lch[2]
            } else {
                acc_lch[2]
            };
            let surf_c = dom_lch[1].clamp(0.014, 0.036);
            let surf_l = if art_l > 0.65 { 0.19 } else { 0.155 };
            let panel_l = surf_l + 0.045;
            let surface_rgb = oklch_to_srgb([surf_l, surf_c, base_hue]);
            let panel_rgb = oklch_to_srgb([panel_l, (surf_c * 1.15).min(0.045), base_hue]);
            let border_rgb = oklch_to_srgb([0.64, (acc_lch[1] * 0.65).clamp(0.04, 0.10), acc_lch[2]]);

            let text_rgb = ensure_contrast_text(panel_rgb, [0.96, 0.96, 0.98], 7.0);
            let muted_candidate = oklch_to_srgb([0.74, (surf_c * 0.9).min(0.03), base_hue]);
            let muted_rgb = ensure_contrast_text(panel_rgb, muted_candidate, 4.5);

            out.surface = [surface_rgb[0], surface_rgb[1], surface_rgb[2], 0.93];
            out.panel = [panel_rgb[0], panel_rgb[1], panel_rgb[2], 0.86];
            out.panel_border = [border_rgb[0], border_rgb[1], border_rgb[2], 0.44];
            out.text = [text_rgb[0], text_rgb[1], text_rgb[2], 1.0];
            out.muted = [muted_rgb[0], muted_rgb[1], muted_rgb[2], 1.0];
            out.accent = [accent_rgb[0], accent_rgb[1], accent_rgb[2], 1.0];
            out.accent_soft = [accent_rgb[0], accent_rgb[1], accent_rgb[2], 0.24];
        }

        // Pair nameplate surface cleanly with the original dialogue skin lightness!
        if is_light_skin {
            let np_lch = srgb_to_oklch(dom_rgb);
            let np_bg = oklch_to_srgb([0.94, np_lch[1].min(0.035), np_lch[2]]);
            let np_txt = ensure_contrast_text(np_bg, [0.16, 0.14, 0.20], 7.0);
            out.nameplate_bg = [np_bg[0], np_bg[1], np_bg[2], 0.92];
            out.nameplate_text = [np_txt[0], np_txt[1], np_txt[2], 1.0];
        } else {
            out.nameplate_bg = [
                out.surface[0] * 0.92,
                out.surface[1] * 0.92,
                out.surface[2] * 0.92,
                0.90,
            ];
            out.nameplate_text = out.text;
        }

        out.archetype = infer_archetype_from_colors(out.surface, out.accent, soft_corners, is_light_skin);
    }

    // Apply subtle optical scene tinting (10-14%) to frosted panel/border tones when active
    if let Some(st) = scene_tint {
        let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
        out.panel[0] = mix(out.panel[0], st[0] * 0.36, 0.14);
        out.panel[1] = mix(out.panel[1], st[1] * 0.36, 0.14);
        out.panel[2] = mix(out.panel[2], st[2] * 0.36, 0.14);
        out.panel_border[0] = mix(out.panel_border[0], st[0], 0.16);
        out.panel_border[1] = mix(out.panel_border[1], st[1], 0.16);
        out.panel_border[2] = mix(out.panel_border[2], st[2], 0.16);
    }

    out
}

/// Applies artwork-derived 6-color tokens directly into existing `UiTheme` fields (zero IR schema change!).
pub fn apply_derived_palette_to_theme(theme: &mut UiTheme, resolved: &ResolvedStyle) {
    if !resolved.derived_from_art || !ResolvedStyle::theme_uses_default_palette(theme) {
        return;
    }
    theme.surface = resolved.surface;
    theme.panel = resolved.panel;
    theme.panel_border = resolved.panel_border;
    theme.muted = resolved.muted;
    theme.accent = resolved.accent;
    // Preserve explicit author text_color if customized; otherwise apply WCAG-verified text color
    let def = UiTheme::default();
    if (theme.text[0] - def.text[0]).abs() < 0.02
        && (theme.text[1] - def.text[1]).abs() < 0.02
        && (theme.text[2] - def.text[2]).abs() < 0.02
    {
        theme.text = resolved.text;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oklch_roundtrip_and_wcag_contrast_hold() {
        for rgb in [
            [0.92, 0.48, 0.62],
            [0.24, 0.56, 0.88],
            [0.82, 0.64, 0.32],
            [0.14, 0.12, 0.18],
        ] {
            let lch = srgb_to_oklch(rgb);
            let back = oklch_to_srgb(lch);
            for i in 0..3 {
                assert!(
                    (rgb[i] - back[i]).abs() < 0.02,
                    "roundtrip drift {:?} -> {:?}",
                    rgb,
                    back
                );
            }
        }

        let dark_bg = [0.09, 0.08, 0.12];
        let fixed = ensure_contrast_text(dark_bg, [0.22, 0.22, 0.28], 4.5);
        assert!(contrast_ratio(dark_bg, fixed) >= 4.5);
    }

    #[test]
    fn pink_skin_and_left_busy_title_derive_soft_theme_and_right_anchor() {
        // 32x16 light sakura dialogue skin with transparent rounded corners
        let mut skin_rgba = vec![0u8; 32 * 16 * 4];
        for y in 0..16 {
            for x in 0..32 {
                let idx = (y * 32 + x) * 4;
                let corner = (x < 2 || x >= 30) && (y < 2 || y >= 14);
                if !corner {
                    skin_rgba[idx] = 248;
                    skin_rgba[idx + 1] = 212;
                    skin_rgba[idx + 2] = 224;
                    skin_rgba[idx + 3] = 230;
                }
            }
        }
        let skin = analyze_dialogue_skin_rgba(32, 16, &skin_rgba).expect("skin sample");
        assert!(skin.is_light);
        assert!(skin.soft_corners);

        // 32x18 title image: busy high-contrast figure on LEFT, calm dark sky on RIGHT
        let mut title_rgba = vec![0u8; 32 * 18 * 4];
        for y in 0..18 {
            for x in 0..32 {
                let idx = (y * 32 + x) * 4;
                if x < 14 {
                    let stripe = if (x + y) % 2 == 0 { 240 } else { 20 };
                    title_rgba[idx] = stripe;
                    title_rgba[idx + 1] = 40;
                    title_rgba[idx + 2] = 180;
                } else {
                    title_rgba[idx] = 28;
                    title_rgba[idx + 1] = 24;
                    title_rgba[idx + 2] = 36;
                }
                title_rgba[idx + 3] = 255;
            }
        }
        let title = analyze_title_rgba(32, 18, &title_rgba).expect("title analysis");
        assert_eq!(title.anchor, TitleAnchor::Right);

        let theme = UiTheme::default();
        let resolved = synthesize_resolved_style(&theme, Some(&title), Some(&skin), None);
        assert!(resolved.derived_from_art);
        assert_eq!(resolved.archetype, StyleArchetype::Soft);
        assert!(resolved.dialogue_is_light);
        assert!(
            resolved.accent[0] > resolved.accent[2],
            "sakura skin should derive a warm rosy accent, got {:?}",
            resolved.accent
        );
        assert!(contrast_ratio(
            [resolved.panel[0], resolved.panel[1], resolved.panel[2]],
            [resolved.text[0], resolved.text[1], resolved.text[2]]
        ) >= 7.0);
        assert!(resolved.to_web_css_vars_json().contains("--nir-accent"));
    }
}
