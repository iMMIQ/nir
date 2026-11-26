use nir_ir::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerText {
    pub text: String,
    pub font_size: f32,
    pub color: [f32; 4],
    pub align: TextAlign,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerState {
    pub id: LayerId,
    pub parent: Option<LayerId>,
    pub asset: Option<AssetId>,
    pub transform: Transform2D,
    pub shake_offset: [f32; 2],
    #[serde(default)]
    pub text: Option<LayerText>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActiveTimeline {
    pub timeline: TimelineId,
    pub layer: LayerId,
    pub elapsed_us: u64,
    pub scrap_on_end: bool,
    #[serde(default)]
    pub triggered_sounds: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActiveTween {
    pub target: TweenTarget,
    pub prop: TweenProp,
    pub from: f32,
    pub to: f32,
    pub elapsed_us: u64,
    pub duration_us: u64,
    pub easing: Easing,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActiveShake {
    pub target: TweenTarget,
    pub spec: ShakeSpec,
    pub elapsed_us: u64,
    pub seed: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StageTransitionState {
    pub from_layers: Vec<LayerState>,
    pub transition: Transition,
    pub elapsed_us: u64,
    pub duration_us: u64,
}

impl StageTransitionState {
    pub fn progress(&self) -> f32 {
        if self.duration_us == 0 {
            1.0
        } else {
            (self.elapsed_us as f64 / self.duration_us as f64).clamp(0.0, 1.0) as f32
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowRevealState {
    pub to_visible: bool,
    pub transition: Transition,
    pub from_alpha: f32,
    pub elapsed_us: u64,
    pub duration_us: u64,
}

impl WindowRevealState {
    pub fn alpha(&self) -> f32 {
        let t = if self.duration_us == 0 {
            1.0
        } else {
            (self.elapsed_us as f64 / self.duration_us as f64).clamp(0.0, 1.0) as f32
        };
        let to = if self.to_visible { 1.0 } else { 0.0 };
        self.from_alpha + (to - self.from_alpha) * t
    }
}

/// Flattened span inside an instantiated dialogue (variables already interpolated).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedSpan {
    pub byte_start: usize,
    pub byte_end: usize,
    pub emphasis: bool,
    pub color: Option<[f32; 4]>,
    pub ruby: Option<String>,
    pub speed: f32,
    #[serde(default)]
    pub inline_image: Option<AssetId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DialoguePausePoint {
    pub grapheme_index: usize,
    pub duration_us: u64,
    pub click: bool,
    pub triggered: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DialogueState {
    pub text_id: TextId,
    pub speaker: String,
    pub speaker_color: Option<[f32; 4]>,
    pub full_text: String,
    pub spans: Vec<ResolvedSpan>,
    pub pauses: Vec<DialoguePausePoint>,
    pub voice: Option<AssetId>,
    pub reveal_us: u64,
    pub elapsed_us: u64,
    pub visible_graphemes: usize,
    /// Cached byte offset in `full_text` matching `visible_graphemes` (O(1) per frame!).
    #[serde(default)]
    pub visible_bytes: usize,
    pub total_graphemes: usize,
    pub paused_for_click: bool,
    pub pause_remaining_us: u64,
    pub finished: bool,
    pub already_read: bool,
}

impl DialogueState {
    /// Updates cached `visible_bytes` incrementally after advancing `visible_graphemes` by `delta`.
    pub fn advance_visible_bytes(&mut self, delta_graphemes: usize) {
        if self.finished || self.visible_graphemes >= self.total_graphemes {
            self.visible_bytes = self.full_text.len();
            return;
        }
        if self.visible_graphemes == 0 {
            self.visible_bytes = 0;
            return;
        }
        use unicode_segmentation::UnicodeSegmentation;
        let base = self.visible_bytes.min(self.full_text.len());
        if self.full_text.is_char_boundary(base) && delta_graphemes > 0 {
            let tail = &self.full_text[base..];
            self.visible_bytes = tail
                .grapheme_indices(true)
                .nth(delta_graphemes)
                .map_or(self.full_text.len(), |(idx, _)| base + idx);
        } else {
            self.recompute_visible_bytes();
        }
    }

    pub fn recompute_visible_bytes(&mut self) {
        if self.finished || self.visible_graphemes >= self.total_graphemes {
            self.visible_bytes = self.full_text.len();
            return;
        }
        if self.visible_graphemes == 0 {
            self.visible_bytes = 0;
            return;
        }
        use unicode_segmentation::UnicodeSegmentation;
        self.visible_bytes = self
            .full_text
            .grapheme_indices(true)
            .nth(self.visible_graphemes)
            .map_or(self.full_text.len(), |(idx, _)| idx);
    }

    /// Byte offset in `full_text` corresponding to `visible_graphemes` (O(1) cached!).
    pub fn visible_byte_len(&self) -> usize {
        if self.finished || self.visible_graphemes >= self.total_graphemes {
            return self.full_text.len();
        }
        if self.visible_graphemes == 0 {
            return 0;
        }
        if self.visible_bytes > 0
            && self.visible_bytes <= self.full_text.len()
            && self.full_text.is_char_boundary(self.visible_bytes)
        {
            return self.visible_bytes;
        }
        use unicode_segmentation::UnicodeSegmentation;
        self.full_text
            .grapheme_indices(true)
            .nth(self.visible_graphemes)
            .map_or(self.full_text.len(), |(idx, _)| idx)
    }

    pub fn visible_text(&self) -> &str {
        &self.full_text[..self.visible_byte_len()]
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OfferedChoice {
    pub index: u16,
    pub label: String,
    pub image: Option<AssetId>,
    pub hover_image: Option<AssetId>,
    #[serde(default)]
    pub disabled_image: Option<AssetId>,
    #[serde(default)]
    pub preview: Option<(AssetId, [f32; 2])>,
    pub rect: Option<[f32; 4]>,
    pub target_pc: u32,
    pub value: Option<Value>,
    #[serde(default)]
    pub choice_index: Option<Value>,
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChoiceState {
    pub items: Vec<OfferedChoice>,
    pub result_var: Option<VarId>,
    #[serde(default)]
    pub index_var: Option<VarId>,
    pub cancel_pc: Option<u32>,
    pub timeout_us: Option<u64>,
    pub elapsed_us: u64,
    pub default_idx: Option<u16>,
    #[serde(default)]
    pub background: Option<AssetId>,
    #[serde(default)]
    pub menu_size: Option<[u32; 2]>,
    #[serde(default)]
    pub hover_se: Option<AssetId>,
    #[serde(default)]
    pub click_se: Option<AssetId>,
    #[serde(default)]
    pub fade_out_us: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioChannelState {
    pub channel: ChannelId,
    pub bus: AudioBus,
    pub asset: AssetId,
    pub gain: f32,
    pub envelope: f32,
    pub looped: bool,
    pub loop_region: Option<AudioLoopRegion>,
    pub character: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub text_id: TextId,
    pub speaker: String,
    pub text: String,
    pub voice: Option<AssetId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallFrame {
    pub func: FuncId,
    pub return_pc: u32,
    pub locals: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WaitReason {
    None,
    Dialogue,
    Choice,
    Timer { remaining_us: u64, skippable: bool },
    StageTransition,
    Tween,
    Shake,
    Video { asset: AssetId, skippable: bool },
    Timeline { layer: LayerId, skippable: bool },
    Modal(ModalKind),
}

/// Typed host commands emitted by `Vm::step` / `Vm::tick` — zero JSON serialization!
#[derive(Debug, Clone, PartialEq)]
pub enum VmIntent {
    PlayAudio {
        channel: ChannelId,
        bus: AudioBus,
        asset: AssetId,
        gain: f32,
        looped: bool,
        fade_in_us: u64,
        loop_region: Option<AudioLoopRegion>,
        character: Option<String>,
    },
    StopAudio {
        channel: ChannelId,
        fade_out_us: u64,
    },
    SetAudioGain {
        channel: ChannelId,
        gain: f32,
    },
    PauseAudioBus {
        bus: AudioBus,
        paused: bool,
    },
    PlayVideo {
        asset: AssetId,
        skippable: bool,
    },
    StopVideo,
    OpenModal(ModalKind),
    ProfileChanged,
    Checkpoint,
}

/// Lightweight, version-resilient save snapshot (~2–4 KB typical).
/// Stores variables by name and tracks `last_label` so saves survive minor script edits & hot-reload!
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SaveSnapshot {
    pub format: u32,
    pub game_id: String,
    pub version: String,
    pub saved_at_unix: u64,
    pub summary: String,
    pub pc: u32,
    pub last_label: Option<String>,
    pub vars: BTreeMap<String, Value>,
    pub frames: Vec<CallFrame>,
    pub layers: Vec<LayerState>,
    pub dialogue_visible: bool,
    #[serde(default)]
    pub dialogue_style: u16,
    #[serde(default)]
    pub dialogue_portrait: Option<AssetId>,
    #[serde(default)]
    pub timelines: Vec<ActiveTimeline>,
    pub dialogue: Option<DialogueState>,
    pub choice: Option<ChoiceState>,
    pub wait: WaitReason,
    pub audio: Vec<AudioChannelState>,
    pub audio_paused: [bool; 3],
    pub history: Vec<HistoryEntry>,
    pub locale: String,
    pub rng_word_pos: u128,
    pub outcome: Option<String>,
    /// Optional small base64 data URL (e.g. 160×90 WebP ~3 KB) captured by the host at save time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
}

pub const SAVE_FORMAT_VERSION: u32 = 2;

/// Zero-history-clone in-memory checkpoint for instant rollback without duplicating 500 strings per line.
#[derive(Debug, Clone)]
pub struct RollbackCheckpoint {
    pub pc: u32,
    pub last_label_id: Option<StrId>,
    pub last_label_fallback: Option<String>,
    pub vars: Vec<Value>,
    pub frames: Vec<CallFrame>,
    pub layers: Vec<LayerState>,
    pub dialogue_visible: bool,
    pub dialogue_style: u16,
    pub dialogue_portrait: Option<AssetId>,
    pub timelines: Vec<ActiveTimeline>,
    pub dialogue: Option<DialogueState>,
    pub choice: Option<ChoiceState>,
    pub wait: WaitReason,
    pub audio: Vec<AudioChannelState>,
    pub audio_paused: [bool; 3],
    pub history_seq: u64,
    pub locale: String,
    pub rng_word_pos: u128,
    pub outcome: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProfileData {
    pub flags: BTreeSet<String>,
    pub values: BTreeMap<String, Value>,
    pub read_texts: BTreeSet<u32>,
}

impl ProfileData {
    /// Non-destructively merges another profile (e.g. from cross-platform backup/sync)
    /// by taking the union of read texts & unlocked flags and filling/updating values.
    pub fn merge_from(&mut self, other: &ProfileData) {
        self.flags.extend(other.flags.iter().cloned());
        self.read_texts.extend(other.read_texts.iter().copied());
        for (k, v) in &other.values {
            self.values.insert(k.clone(), v.clone());
        }
    }
}

/// Canonical cross-platform portable save backup format identifier shared by Desktop, Android, and Web.
pub const SAVE_BACKUP_FORMAT: &str = "nir-backup-v1";

/// Portable single-file save backup bundle (`nir-backup-v1`) interoperable across Desktop, Android, and Web.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SaveBackupBundle {
    pub format: String,
    pub game_id: String,
    #[serde(default)]
    pub preferences: Option<Preferences>,
    #[serde(default)]
    pub profile: Option<ProfileData>,
    #[serde(default)]
    pub saves: BTreeMap<String, SaveSnapshot>,
}

impl SaveBackupBundle {
    pub fn new(
        game_id: impl Into<String>,
        preferences: Option<Preferences>,
        profile: Option<ProfileData>,
        saves: BTreeMap<String, SaveSnapshot>,
    ) -> Self {
        Self {
            format: SAVE_BACKUP_FORMAT.to_owned(),
            game_id: game_id.into(),
            preferences,
            profile,
            saves,
        }
    }

    pub fn validate_for(&self, expected_game_id: &str) -> std::result::Result<(), String> {
        if !self.format.is_empty() && self.format != SAVE_BACKUP_FORMAT {
            return Err(format!("unsupported backup format `{}`", self.format));
        }
        if !self.game_id.is_empty() && self.game_id != expected_game_id {
            return Err(format!(
                "backup game_id `{}` does not match `{expected_game_id}`",
                self.game_id
            ));
        }
        Ok(())
    }
}
