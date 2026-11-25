use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Native 64-bit runtime value. Fast, orthogonal, no software x87 Float80.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Value>),
}

impl Value {
    pub fn as_bool(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Bool(b) => *b,
            Self::Int(n) => *n != 0,
            Self::Float(f) => *f != 0.0 && !f.is_nan(),
            Self::Str(s) => !s.is_empty(),
            Self::List(v) => !v.is_empty(),
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Int(n) => Some(*n as f64),
            Self::Float(f) => Some(*f),
            Self::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Self::Str(s) => s.trim().parse::<f64>().ok(),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(n) => Some(*n),
            Self::Float(f) if f.is_finite() => Some(*f as i64),
            Self::Bool(b) => Some(i64::from(*b)),
            Self::Str(s) => s.trim().parse::<i64>().ok(),
            _ => None,
        }
    }

    pub fn to_display_string(&self) -> String {
        match self {
            Self::Null => String::new(),
            Self::Bool(b) => if *b { "true" } else { "false" }.into(),
            Self::Int(n) => n.to_string(),
            Self::Float(f) => {
                if f.fract() == 0.0 && f.abs() < 1e15 {
                    format!("{:.0}", f)
                } else {
                    format!("{f}")
                }
            }
            Self::Str(s) => s.clone(),
            Self::List(v) => v
                .iter()
                .map(Self::to_display_string)
                .collect::<Vec<_>>()
                .join("\r\n"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum BinOp {
    Add = 0,
    Sub = 1,
    Mul = 2,
    Div = 3,
    Rem = 4,
    Eq = 5,
    Ne = 6,
    Lt = 7,
    Le = 8,
    Gt = 9,
    Ge = 10,
    And = 11,
    Or = 12,
    Concat = 13,
    BitAnd = 14,
    BitOr = 15,
    BitXor = 16,
    Shl = 17,
    Shr = 18,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum UnaryOp {
    Neg = 0,
    Not = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum AudioBus {
    Bgm = 0,
    Voice = 1,
    Sfx = 2,
}

/// Sample-accurate audio loop region: plays intro `[0, end_us)` once, then loops `[start_us, end_us)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioLoopRegion {
    pub start_us: u64,
    pub end_us: u64,
}

impl AudioLoopRegion {
    pub fn valid(self, duration_us: u64) -> bool {
        self.start_us < self.end_us && self.end_us <= duration_us
    }

    pub fn frame_bounds(self, rate: u32, frames: u64) -> Option<(u64, u64)> {
        if rate == 0 || self.start_us >= self.end_us {
            return None;
        }
        let frame = |us: u64| (us as u128 * rate as u128 + 500_000) / 1_000_000;
        let start = frame(self.start_us);
        let end = frame(self.end_us);
        (start < end && end <= frames as u128).then_some((start as u64, end as u64))
    }

    pub fn playback_frame(self, position_us: u64, rate: u32, frames: u64) -> Option<u64> {
        let (start, end) = self.frame_bounds(rate, frames)?;
        let pos = (position_us as u128 * rate as u128 + 500_000) / 1_000_000;
        Some(if pos < end as u128 {
            pos as u64
        } else {
            start + ((pos - end as u128) % (end - start) as u128) as u64
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub ui_locale: String,
    pub text_locale: String,
    /// Relative multiplier on dialogue reveal speed (0.25..=8.0, 0 = instant).
    pub text_speed: f32,
    /// Auto-advance wait delay in milliseconds (0..=10000).
    pub auto_delay_ms: u32,
    /// Whether Auto waits for the current voice clip to finish.
    pub auto_wait_voice: bool,
    /// Whether manual advance keeps the current voice clip playing.
    pub continue_voice_on_advance: bool,
    /// Allow skipping unread text (Ctrl / Skip button).
    pub skip_unread: bool,
    pub font_scale: f32,
    pub master_volume: f32,
    pub bgm_volume: f32,
    pub voice_volume: f32,
    pub sfx_volume: f32,
    pub reduced_motion: bool,
    pub fullscreen: bool,
    /// Comfortable docked bottom dialogue layout on narrow portrait screens (phones).
    #[serde(default = "default_true")]
    pub portrait_reader: bool,
    /// Per-character voice volume override (0.0 = muted, 1.0 = full).
    pub character_volumes: BTreeMap<String, f32>,
}

fn default_true() -> bool {
    true
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            ui_locale: "zh-Hans".into(),
            text_locale: "zh-Hans".into(),
            text_speed: 1.0,
            auto_delay_ms: 1500,
            auto_wait_voice: true,
            continue_voice_on_advance: false,
            skip_unread: false,
            font_scale: 1.0,
            master_volume: 1.0,
            bgm_volume: 0.7,
            voice_volume: 0.9,
            sfx_volume: 0.8,
            reduced_motion: false,
            fullscreen: false,
            portrait_reader: true,
            character_volumes: BTreeMap::new(),
        }
    }
}

impl Preferences {
    pub fn sanitize(&mut self) {
        let clamp = |v: f32, lo: f32, hi: f32, def: f32| {
            if v.is_finite() {
                v.clamp(lo, hi)
            } else {
                def
            }
        };
        self.text_speed = clamp(self.text_speed, 0.0, 8.0, 1.0);
        self.auto_delay_ms = self.auto_delay_ms.min(15_000);
        self.font_scale = clamp(self.font_scale, 0.75, 1.75, 1.0);
        self.master_volume = clamp(self.master_volume, 0.0, 1.0, 1.0);
        self.bgm_volume = clamp(self.bgm_volume, 0.0, 1.0, 0.7);
        self.voice_volume = clamp(self.voice_volume, 0.0, 1.0, 0.9);
        self.sfx_volume = clamp(self.sfx_volume, 0.0, 1.0, 0.8);
        for v in self.character_volumes.values_mut() {
            *v = clamp(*v, 0.0, 1.0, 1.0);
        }
    }

    pub fn effective_bus_gain(&self, bus: AudioBus, character: Option<&str>) -> f32 {
        let base = self.master_volume
            * match bus {
                AudioBus::Bgm => self.bgm_volume,
                AudioBus::Voice => self.voice_volume,
                AudioBus::Sfx => self.sfx_volume,
            };
        if bus == AudioBus::Voice {
            if let Some(ch) = character {
                if let Some(&mult) = self.character_volumes.get(ch) {
                    return base * mult;
                }
            }
        }
        base
    }
}
