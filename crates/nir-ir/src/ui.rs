use crate::{AssetId, FuncId, StrId};
use serde::{Deserialize, Serialize};

/// Unified visual theme & declarative UI overrides.
/// Authors need zero config to get the built-in modern Galgame UI, or can override
/// colors, box art, custom title/gallery screens, and widget trees declaratively.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiTheme {
    pub surface: [f32; 4],
    pub panel: [f32; 4],
    pub panel_border: [f32; 4],
    pub text: [f32; 4],
    pub muted: [f32; 4],
    pub accent: [f32; 4],
    pub dialogue_bg: Option<AssetId>,
    pub dialogue_rect: Option<[f32; 4]>,
    pub text_rect: Option<[f32; 4]>,
    pub font_size: f32,
    pub line_height: f32,
    pub text_shadow: Option<TextShadow>,
    pub title_screen: Option<UiScreenDef>,
    pub gallery: Vec<GalleryItem>,
}

impl Default for UiTheme {
    fn default() -> Self {
        Self {
            surface: [0.07, 0.09, 0.13, 0.94],
            panel: [0.10, 0.13, 0.19, 0.88],
            panel_border: [0.42, 0.56, 0.72, 0.45],
            text: [0.95, 0.96, 0.98, 1.0],
            muted: [0.68, 0.74, 0.82, 1.0],
            accent: [0.45, 0.74, 0.96, 1.0],
            dialogue_bg: None,
            dialogue_rect: None,
            text_rect: None,
            font_size: 26.0,
            line_height: 38.0,
            text_shadow: Some(TextShadow {
                offset: [1.5, 1.5],
                color: [0.0, 0.0, 0.0, 0.75],
            }),
            title_screen: None,
            gallery: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TextShadow {
    pub offset: [f32; 2],
    pub color: [f32; 4],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GalleryItem {
    pub id: StrId,
    pub title: StrId,
    pub thumb: Option<AssetId>,
    pub unlock_flag: Option<StrId>,
    pub replay_func: Option<FuncId>,
    pub cg_assets: Vec<AssetId>,
}

/// Declarative screen layout composed of orthogonal responsive widgets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiScreenDef {
    pub background: Option<AssetId>,
    pub bgm: Option<AssetId>,
    pub nodes: Vec<UiNode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiNode {
    pub id: StrId,
    pub rect: [f32; 4],
    pub opacity: f32,
    pub visible_flag: Option<StrId>,
    pub widget: UiWidget,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UiWidget {
    Container {
        bg_color: [f32; 4],
        children: Vec<UiNode>,
    },
    Stack {
        vertical: bool,
        gap: f32,
        children: Vec<UiNode>,
    },
    Image {
        asset: AssetId,
    },
    Text {
        text: StrId,
        size: f32,
        color: [f32; 4],
    },
    Button {
        label: StrId,
        image: Option<AssetId>,
        hover_image: Option<AssetId>,
        action: UiAction,
    },
}

/// Unified typed UI actions shared across Native, Web, and Mobile hosts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UiAction {
    NewGame,
    Continue,
    Advance,
    Choose(u16),
    CancelChoice,
    ToggleAuto,
    ToggleSkip,
    HoldSkip(bool),
    ToggleHideUi,
    Rollback,
    OpenMenu,
    CloseOverlay,
    OpenSaves { save_mode: bool },
    OpenSettings,
    OpenHistory,
    OpenGallery,
    SaveSlot(u32),
    LoadSlot(u32),
    DeleteSlot(u32),
    StartReplay(FuncId),
    ExitReplay,
    ReturnTitle,
    SetVolume { bus: crate::AudioBus, value: f32 },
    SetTextSpeed(f32),
    SetAutoDelay(u32),
    ToggleSkipUnread,
    ToggleReducedMotion,
    SetLocale(String),
    ScrollHistory(i32),
    Quit,
}
