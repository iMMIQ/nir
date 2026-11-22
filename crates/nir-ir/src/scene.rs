use crate::{AssetId, ChannelId, LayerId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum BlendMode {
    #[default]
    Normal = 0,
    Add = 1,
    Multiply = 2,
    Screen = 3,
}

/// Unified 2D transform for any persistent scene layer or UI sprite.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transform2D {
    pub x: f32,
    pub y: f32,
    /// Explicit draw dimensions (0 = inherit asset's original width/height).
    pub width: f32,
    pub height: f32,
    /// Normalized anchor point in `[0..1, 0..1]` (e.g., `[0.5, 1.0]` for standing sprites).
    pub anchor: [f32; 2],
    pub scale: [f32; 2],
    pub rotation_deg: f32,
    pub opacity: f32,
    /// Multiplicative RGBA tint in sRGB `[0..1]`.
    pub color: [f32; 4],
    pub z_order: i32,
    pub blend: BlendMode,
    pub clip: Option<[f32; 4]>,
}

impl Default for Transform2D {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
            anchor: [0.0, 0.0],
            scale: [1.0, 1.0],
            rotation_deg: 0.0,
            opacity: 1.0,
            color: [1.0, 1.0, 1.0, 1.0],
            z_order: 0,
            blend: BlendMode::Normal,
            clip: None,
        }
    }
}

/// Metadata written by the auto-optimizer when trimming transparent margins from sprites.
/// Renderers place the trimmed texture at `(orig_x + trim.x, orig_y + trim.y)` with zero visual change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpriteTrim {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub orig_w: u32,
    pub orig_h: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum WipeDir {
    LeftToRight = 0,
    RightToLeft = 1,
    TopToBottom = 2,
    BottomToTop = 3,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub enum Transition {
    #[default]
    Cut,
    Dissolve,
    Wipe {
        dir: WipeDir,
        softness: f32,
    },
    RuleMask {
        mask: AssetId,
        invert: bool,
        softness: f32,
    },
}

impl Transition {
    pub fn mask_asset(&self) -> Option<AssetId> {
        match *self {
            Self::RuleMask { mask, .. } => Some(mask),
            _ => None,
        }
    }

    /// Encodes transition shader uniforms into `[mode, softness, invert, progress]`.
    pub fn shader_params(&self, progress: f32) -> [f32; 4] {
        let p = progress.clamp(0.0, 1.0);
        match *self {
            Self::Cut => [0.0, 0.0, 0.0, if p >= 1.0 { 1.0 } else { 0.0 }],
            Self::Dissolve => [0.0, 0.0, 0.0, p],
            Self::Wipe { dir, softness } => {
                let mode = match dir {
                    WipeDir::LeftToRight => 1.0,
                    WipeDir::RightToLeft => 2.0,
                    WipeDir::TopToBottom => 3.0,
                    WipeDir::BottomToTop => 4.0,
                };
                [mode, softness.clamp(0.0, 1.0), 0.0, p]
            }
            Self::RuleMask {
                invert, softness, ..
            } => [
                5.0,
                softness.clamp(0.0, 1.0),
                if invert { 1.0 } else { 0.0 },
                p,
            ],
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Easing {
    #[default]
    Linear = 0,
    Smooth = 1,
    EaseIn = 2,
    EaseOut = 3,
    EaseInOut = 4,
}

pub fn interpolate(from: f32, to: f32, progress: f32, easing: Easing) -> f32 {
    let t = progress.clamp(0.0, 1.0) as f64;
    let p = match easing {
        Easing::Linear => t,
        Easing::Smooth => t * t * (3.0 - 2.0 * t),
        Easing::EaseIn => t * t * t,
        Easing::EaseOut => {
            let u = 1.0 - t;
            1.0 - u * u * u
        }
        Easing::EaseInOut => {
            if t < 0.5 {
                4.0 * t * t * t
            } else {
                let u = -2.0 * t + 2.0;
                1.0 - (u * u * u) * 0.5
            }
        }
    };
    if p <= 0.0 {
        from
    } else if p >= 1.0 {
        to
    } else {
        ((1.0 - p) * from as f64 + p * to as f64) as f32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum TweenProp {
    X = 0,
    Y = 1,
    ScaleX = 2,
    ScaleY = 3,
    Rotation = 4,
    Opacity = 5,
    Volume = 6,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TweenTarget {
    Layer(LayerId),
    DialogueBox,
    Audio(ChannelId),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum ShakeMode {
    #[default]
    Quake = 0,
    Wave = 1,
    Bound = 2,
}

/// Unified oscillation modifier replacing DialogueShake + SpriteWave + SpriteShake.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ShakeSpec {
    pub amplitude: [f32; 2],
    pub freq_hz: f32,
    pub duration_us: u64,
    pub decay: bool,
    pub mode: ShakeMode,
}

impl ShakeSpec {
    pub fn sample(&self, elapsed_us: u64, seed: u32) -> [f32; 2] {
        if self.duration_us == 0 || elapsed_us >= self.duration_us {
            return [0.0, 0.0];
        }
        let t = elapsed_us as f64 / 1_000_000.0;
        let progress = (elapsed_us as f64 / self.duration_us as f64).clamp(0.0, 1.0);
        let env = if self.decay { 1.0 - progress } else { 1.0 };
        let phase = t * self.freq_hz.max(0.5) as f64 * std::f64::consts::TAU;
        match self.mode {
            ShakeMode::Wave => [
                (phase.sin() * self.amplitude[0] as f64 * env) as f32,
                (phase.cos() * self.amplitude[1] as f64 * env) as f32,
            ],
            ShakeMode::Bound => [
                (phase.sin() * self.amplitude[0] as f64 * env) as f32,
                (-(phase.sin().abs()) * self.amplitude[1] as f64 * env) as f32,
            ],
            ShakeMode::Quake => {
                // Deterministic pseudo-random impulse stepped at `freq_hz * 2` ticks/sec.
                let step = (t * (self.freq_hz.max(1.0) as f64 * 2.0)) as u64;
                let h = step
                    .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                    .wrapping_add(seed as u64);
                let hx = ((h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9)) as i32 as f64
                    / i32::MAX as f64;
                let hy = ((h.wrapping_add(1) ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB))
                    as i32 as f64
                    / i32::MAX as f64;
                [
                    (hx * self.amplitude[0] as f64 * env) as f32,
                    (hy * self.amplitude[1] as f64 * env) as f32,
                ]
            }
        }
    }
}
