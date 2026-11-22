use crate::{
    AudioBus, AudioLoopRegion, BinOp, Easing, ModalKind, ShakeSpec, TextAlign, Transform2D,
    Transition, TweenProp, TweenTarget, UnaryOp, Value,
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
id_type!(TimelineId);
id_type!(MenuId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum BuiltinFn {
    ToInt = 0,
    ToStr = 1,
    CharLen = 2,
    StrRepeat = 3,
    AddDelimiter = 4,
    ListPush = 5,
    ListLen = 6,
    ListGet = 7,
    ListSet = 8,
    ListJoin = 9,
    ListSplit = 10,
    ListCompact = 11,
    Min = 12,
    Max = 13,
    RandInt = 14,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChoiceItem {
    pub text: TextId,
    pub image: Option<AssetId>,
    pub hover_image: Option<AssetId>,
    #[serde(default)]
    pub disabled_image: Option<AssetId>,
    #[serde(default)]
    pub preview: Option<(AssetId, [f32; 2])>,
    pub rect: Option<[f32; 4]>,
    pub target_pc: u32,
    /// Optional condition expression entry PC (evaluates on stack before offering choice).
    /// When `None`, the option is unconditionally visible & enabled.
    pub cond_var: Option<VarId>,
    #[serde(default)]
    pub disabled_var: Option<VarId>,
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

    // --- Pure Built-ins, Dynamic Media, Timelines, Dynamic Menus & Modals ---
    CallBuiltin {
        func: BuiltinFn,
        argc: u8,
    },
    /// Pops a string path, normalizes slashes/case, looks up in `NirProgram::asset_lookup`,
    /// pushes `Value::Int(asset_id)` or `Value::Null`.
    ResolveAsset,
    /// Pops `Value` (`Int(asset_id)`, `Str("#RRGGBB"/"$RRGGBB")`, or `Null`) and sets layer.
    LayerSetDyn {
        layer: LayerId,
        parent: Option<LayerId>,
        transform: Box<Transform2D>,
    },
    /// Pops `Value` (`Int(asset_id)`, `Str("#RRGGBB"/"$RRGGBB")`, or `Null`) and updates layer visual.
    LayerAssetDyn {
        layer: LayerId,
    },
    /// Renders dynamic or static text onto a scene layer (when `text` is `None`, pops value from stack).
    LayerSetText {
        layer: LayerId,
        parent: Option<LayerId>,
        transform: Option<Box<Transform2D>>,
        text: Option<StrId>,
        font_size: f32,
        color: [f32; 4],
        align: TextAlign,
    },
    /// Stops any active shake/oscillation on `target`.
    ShakeStop {
        target: TweenTarget,
    },
    /// Plays an audio asset whose `AssetId` (as `Value::Int`) is popped from the stack.
    AudioPlayDyn {
        channel: ChannelId,
        bus: AudioBus,
        gain: f32,
        looped: bool,
        fade_in_us: u64,
    },
    /// Smoothly changes absolute gain (`0.0..=2.0`) on an active audio channel.
    AudioGain {
        channel: ChannelId,
        gain: f32,
        duration_us: u64,
        wait: bool,
    },
    /// Offers dynamic text choices popped from stack: `[labels, id_list]` (`id_list` on top).
    ChoiceList {
        result_var: Option<VarId>,
        index_var: Option<VarId>,
        cancelable: bool,
        click_se: Option<AssetId>,
        timeout_us: Option<u64>,
    },
    /// Offers an interactive `MenuTemplate` with dynamic filter lists popped from stack:
    /// `[hidden_names, disabled_names]` (`disabled_names` on top).
    ChoiceMenu {
        menu: MenuId,
        result_var: Option<VarId>,
        index_var: Option<VarId>,
        cancelable: bool,
        hover_se: Option<AssetId>,
        click_se: Option<AssetId>,
        fade_in_us: u64,
        fade_out_us: u64,
        timeout_us: Option<u64>,
    },
    /// Starts a keyframed `TimelineDef` bound to `layer`.
    TimelinePlay {
        timeline: TimelineId,
        layer: LayerId,
        scrap_on_end: bool,
        wait: bool,
        click_skippable: bool,
    },
    /// Waits until the timeline bound to `layer` finishes.
    WaitPlay {
        layer: LayerId,
        click_skippable: bool,
    },
    /// Suspends VM and opens a built-in host modal (Load/Save/Gallery/Replay).
    OpenModal(ModalKind),
    /// Switches active dialogue box style preset (`UiTheme::dialogue_styles[index]`).
    SetDialogueStyle(u16),
    /// Sets or clears the dialogue-bound portrait decoration.
    SetDialoguePortrait(Option<AssetId>),
    /// Pops `Value::Int(asset_id)` or `Value::Null` for the dialogue-bound portrait.
    SetDialoguePortraitDyn,
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
