use super::*;

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub ui_locale: String,
    pub text_locale: String,
    pub font_scale: f32,
    #[serde(default = "one", skip_serializing_if = "is_unit_gain")]
    pub text_speed: f32,
    #[serde(default = "one", skip_serializing_if = "is_unit_gain")]
    pub auto_wait_scale: f32,
    #[serde(default = "wait_voice_by_default")]
    pub auto_wait_voice: bool,
    /// Allow authored voice lifetimes across a completed reading interaction.
    #[serde(default = "wait_voice_by_default")]
    pub voice_continue: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub character_voices: BTreeMap<String, CharacterVoicePreference>,
    pub bgm_volume: f32,
    pub voice_volume: f32,
    pub sfx_volume: f32,
    pub reduced_motion: bool,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            ui_locale: "zh-Hans".into(),
            text_locale: "zh-Hans".into(),
            font_scale: 1.,
            text_speed: 1.,
            auto_wait_scale: 1.,
            auto_wait_voice: true,
            voice_continue: true,
            character_voices: BTreeMap::new(),
            bgm_volume: 0.3,
            voice_volume: 0.8,
            sfx_volume: 0.5,
            reduced_motion: false,
        }
    }
}
fn wait_voice_by_default() -> bool {
    true
}

pub const MAX_CHARACTER_VOICES: usize = 128;
pub const MAX_CHARACTER_ID_BYTES: usize = 256;
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CharacterVoicePreference {
    pub volume: f32,
    pub muted: bool,
}
impl Default for CharacterVoicePreference {
    fn default() -> Self {
        Self {
            volume: 1.,
            muted: false,
        }
    }
}
impl Preferences {
    pub fn character_voice_gain(&self, character: &str) -> f32 {
        self.character_voices.get(character).map_or(1., |voice| {
            if voice.muted {
                0.
            } else if voice.volume.is_finite() {
                voice.volume.clamp(0., 1.)
            } else {
                1.
            }
        })
    }
    pub fn normalize_character_voices(&mut self) {
        self.character_voices.retain(|id, value| {
            !id.is_empty() && id.len() <= MAX_CHARACTER_ID_BYTES && value.volume.is_finite()
        });
        for value in self.character_voices.values_mut() {
            value.volume = value.volume.clamp(0., 1.);
        }
        while self.character_voices.len() > MAX_CHARACTER_VOICES {
            self.character_voices.pop_last();
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum UiAction {
    MenuHistoryVoice {
        instance: u32,
        revision: u32,
        window: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        layout: Option<u32>,
        entry: usize,
        #[serde(default)]
        stop: bool,
    },
    MenuHistoryScroll {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        control: Option<String>,
        instance: u32,
        revision: u32,
        window: String,
        layout: u32,
        input: HistoryScrollInput,
    },
    MenuValue {
        instance: u32,
        revision: u32,
        control: String,
        value: MenuValueInput,
    },
    ConfirmSave {
        token: u32,
    },
    CancelSave {
        token: u32,
    },
    MenuControl {
        instance: u32,
        revision: u32,
        control: String,
    },
    NewGame,
    ImageMenu {
        menu: String,
    },
    ImageMenuEntry {
        function: String,
    },
    ImageMenuReplay {
        function: String,
    },
    ExitReplay,
    HoverImage {
        id: Option<String>,
    },
    Continue,
    Advance,
    Choose {
        option: String,
    },
    /// Move the semantic selection cursor of the pending typed-result
    /// interaction. An observation: no input identity, no story progress.
    SelectChoice {
        option: String,
    },
    /// Cancel the pending typed-result interaction through its declared
    /// cancel target. Hosts only offer this while a cancel affordance exists.
    CancelChoice,
    Menu,
    Close,
    Settings,
    History,
    Saves,
    Save {
        slot: u32,
    },
    Load {
        slot: u32,
    },
    Rollback,
    Title,
    ToggleAuto,
    ToggleSkip,
    ToggleInterface,
    RestoreInterface,
    HoldSkip {
        pressed: bool,
    },
    UiLocale {
        locale: String,
    },
    TextLocale {
        locale: String,
    },
    LocaleRetry,
    LocaleCancel,
    FontSize {
        delta: f32,
    },
    TextSpeed {
        delta: f32,
    },
    AutoWait {
        delta: f32,
    },
    AutoWaitVoice {
        enabled: bool,
    },
    VoiceContinue {
        enabled: bool,
    },
    CharacterVolume {
        character: String,
        delta: f32,
    },
    CharacterMute {
        character: String,
        muted: bool,
    },
    Volume {
        bus: AudioBus,
        delta: f32,
    },
    ReducedMotion,
    HistoryPage {
        delta: i32,
    },
    Scroll {
        region: ScrollRegion,
        delta: i32,
    },
    Export,
    Import,
    Retry,
    HistoryVoice {
        entry: usize,
    },
    StopHistoryVoice,
}

impl UiAction {
    /// Only focus retention may ignore a menu model revision. Dispatch must not.
    pub fn same_pointer_target(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::MenuValue {
                    instance: a,
                    revision: b,
                    control: c,
                    ..
                },
                Self::MenuValue {
                    instance: d,
                    revision: e,
                    control: f,
                    ..
                },
            ) => a == d && b == e && c == f,
            _ => self == other,
        }
    }
    pub fn same_focus_target(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::MenuHistoryVoice {
                    instance: a,
                    window: b,
                    entry: c,
                    ..
                },
                Self::MenuHistoryVoice {
                    instance: d,
                    window: e,
                    entry: f,
                    ..
                },
            ) => a == d && b == e && c == f,
            (
                Self::MenuHistoryScroll {
                    instance: a,
                    window: wa,
                    control: Some(b),
                    input: c,
                    ..
                },
                Self::MenuHistoryScroll {
                    instance: d,
                    window: wd,
                    control: Some(e),
                    input: f,
                    ..
                },
            ) => {
                a == d
                    && wa == wd
                    && b == e
                    && match (c, f) {
                        (
                            HistoryScrollInput::Position { .. },
                            HistoryScrollInput::Position { .. },
                        ) => true,
                        (
                            HistoryScrollInput::Line { delta: x },
                            HistoryScrollInput::Line { delta: y },
                        ) => x == y,
                        _ => false,
                    }
            }
            (
                Self::MenuValue {
                    instance: a,
                    control: b,
                    ..
                },
                Self::MenuValue {
                    instance: c,
                    control: d,
                    ..
                },
            ) => a == c && b == d,
            (
                Self::MenuControl {
                    instance: a,
                    control: b,
                    ..
                },
                Self::MenuControl {
                    instance: c,
                    control: d,
                    ..
                },
            ) => a == c && b == d,
            _ => self == other,
        }
    }
}

/// View-local history navigation; never a story or saved-state mutation.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum HistoryScrollInput {
    Line { delta: i32 },
    Step { delta: i32 },
    Page { delta: i32 },
    Position { ratio: f32 },
}

/// Viewport navigation, never a VM instruction or a snapshot cursor.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollRegion {
    Menu,
    Saves,
    Settings,
    Dialogue,
    Choices,
    History,
}
