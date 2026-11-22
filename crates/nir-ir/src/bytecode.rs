use crate::{
    AudioBus, AudioLoopRegion, BinOp, Easing, ShakeSpec, Transform2D, Transition, TweenProp,
    TweenTarget, UnaryOp, Value,
};
use serde::{Deserialize, Serialize};

macro_rules! id_type {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[repr(transparent)]
        pub struct $name(pub u32);
    };
}

id_type!(StrId);
id_type!(VarId);
id_type!(FuncId);
id_type!(AssetId);
id_type!(TextId);
id_type!(LayerId);
id_type!(ChannelId);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChoiceItem {
    pub text: TextId,
    pub image: Option<AssetId>,
    pub hover_image: Option<AssetId>,
    pub rect: Option<[f32; 4]>,
    pub target_pc: u32,
    /// Optional condition expression entry PC (evaluates on stack before offering choice).
    /// When `None`, the option is unconditionally visible & enabled.
    pub cond_var: Option<VarId>,
    pub value: Option<Value>,
}

/// Flat numeric bytecode instruction. Zero string lookups at runtime!
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Op {
    // --- Stack & Variables ---
    PushNull,
    PushBool(bool),
    PushInt(i64),
    PushFloat(f64),
    PushStr(StrId),
    LoadVar(VarId),
    StoreVar(VarId),
    LoadLocal(u16),
    StoreLocal(u16),
    Pop,
    BinOp(BinOp),
    UnaryOp(UnaryOp),
    RandInt {
        dst: VarId,
        min: i64,
        max: i64,
    },

    // --- Control Flow ---
    Jump(u32),
    JumpIfFalse(u32),
    Call {
        func: FuncId,
        argc: u8,
    },
    Ret {
        has_value: bool,
    },
    End {
        outcome: StrId,
    },

    // --- Persistent Scene Graph (replaces static Scene templates + 5 inherit_* hacks!) ---
    LayerSet {
        layer: LayerId,
        parent: Option<LayerId>,
        asset: Option<AssetId>,
        transform: Box<Transform2D>,
    },
    LayerAsset {
        layer: LayerId,
        asset: Option<AssetId>,
    },
    LayerPatch {
        layer: LayerId,
        prop: TweenProp,
        value: f32,
    },
    LayerRemove {
        layer: LayerId,
    },
    LayerClear,
    Tween {
        target: TweenTarget,
        prop: TweenProp,
        to: f32,
        duration_us: u64,
        easing: Easing,
        wait: bool,
    },
    Shake {
        target: TweenTarget,
        spec: ShakeSpec,
        wait: bool,
    },
    StageCommit {
        transition: Transition,
        duration_us: u64,
        wait: bool,
    },

    // --- Narrative & Choices ---
    Say {
        speaker: Option<StrId>,
        speaker_color: Option<[f32; 4]>,
        text: TextId,
        voice: Option<AssetId>,
        /// Microseconds per grapheme cluster (0 = use project default).
        reveal_us: u64,
        /// Append to current dialogue page rather than clearing.
        append: bool,
    },
    TextWindow {
        visible: bool,
        transition: Transition,
        duration_us: u64,
    },
    Choice {
        items: Vec<ChoiceItem>,
        result_var: Option<VarId>,
        cancel_pc: Option<u32>,
        timeout_us: Option<u64>,
        default_idx: Option<u16>,
    },
    Wait {
        duration_us: u64,
        skippable: bool,
    },

    // --- Audio & Video ---
    AudioPlay {
        channel: ChannelId,
        bus: AudioBus,
        asset: AssetId,
        gain: f32,
        looped: bool,
        fade_in_us: u64,
        loop_region: Option<AudioLoopRegion>,
    },
    AudioStop {
        channel: ChannelId,
        fade_out_us: u64,
    },
    AudioPause {
        bus: AudioBus,
        paused: bool,
    },
    VideoPlay {
        asset: AssetId,
        skippable: bool,
    },

    // --- Persistent Profile / Gallery Unlocks ---
    ProfileSetFlag(StrId),
    ProfileGetFlag {
        dst: VarId,
        key: StrId,
    },
    ProfileSetVal {
        key: StrId,
    },
    ProfileGetVal {
        dst: VarId,
        key: StrId,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuncMeta {
    pub name: StrId,
    pub entry_pc: u32,
    pub param_count: u16,
    pub local_count: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VarMeta {
    pub name: StrId,
    pub default: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabelMeta {
    pub name: StrId,
    pub pc: u32,
    pub file: StrId,
    pub line: u32,
}
