use super::*;

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Image,
    Audio,
    Font,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub menu_overlay: Option<String>,
    pub background: [f32; 4],
    pub panel: [f32; 4],
    pub accent: [f32; 4],
    pub text: [f32; 4],
    pub muted: [f32; 4],
    #[serde(default)]
    pub slots: ThemeSlots,
    #[serde(default)]
    pub dialogue: DialogueProps,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dialogue_styles: BTreeMap<String, DialogueStyle>,
    #[serde(default)]
    pub choice: ChoiceProps,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub image_menus: BTreeMap<String, ImageMenu>,
    #[serde(default)]
    pub return_to_title: bool,
}
impl Default for Theme {
    fn default() -> Self {
        Self {
            background: [0.04, 0.075, 0.09, 1.],
            panel: [0.06, 0.105, 0.12, 0.97],
            accent: [0.83, 0.73, 0.48, 1.],
            text: [0.93, 0.94, 0.88, 1.],
            muted: [0.58, 0.69, 0.69, 1.],
            slots: ThemeSlots::default(),
            dialogue: DialogueProps::default(),
            dialogue_styles: BTreeMap::new(),
            choice: ChoiceProps::default(),
            menu_overlay: None,
            image_menus: BTreeMap::new(),
            return_to_title: false,
        }
    }
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageMenu {
    /// Authors can reserve the complete page for source controls. Shared
    /// Escape/right-click and preparation-failure exits remain available.
    #[serde(
        default = "builtin_navigation_default",
        skip_serializing_if = "is_builtin_navigation_default"
    )]
    pub builtin_navigation: bool,
    /// Explicit read-only aliases of bounded story scalars. Never UI write targets.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub story_exports: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub locals: BTreeMap<String, MenuLocal>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub elements: Vec<MenuElement>,
    pub background: String,
    pub buttons: Vec<ImageButton>,
    /// Finite page presentation effects; absent keeps legacy behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effects: Option<MenuEffects>,
}
fn builtin_navigation_default() -> bool {
    true
}
fn is_builtin_navigation_default(value: &bool) -> bool {
    *value
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageButton {
    pub id: String,
    pub label: String,
    pub asset: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hover_asset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked_asset: Option<String>,
    pub rect: [f32; 4],
    pub action: ImageMenuAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<String>,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ImageMenuAction {
    PushMenu {
        menu: String,
    },
    Back,
    Reading {
        mode: MenuReadingMode,
    },
    HistoryPage {
        window: String,
        delta: i32,
    },
    SaveSlot {
        slot: MenuSlot,
    },
    LoadSlot {
        slot: MenuSlot,
    },
    Close,
    AdjustPreference {
        field: MenuPreference,
        delta: f32,
    },
    ToggleReducedMotion,
    SetLocal {
        local: String,
        value: MenuValue,
    },
    NewGame,
    Saves,
    Settings,
    Title,
    Menu {
        menu: String,
    },
    Entry {
        function: String,
    },
    /// Isolated replay: freeze the current session, run the function as a
    /// temporary one, and return to the frozen session afterwards. Unlike
    /// Entry this never destroys the launching session.
    Replay {
        function: String,
    },
    /// Manual return from an active replay to the frozen session. Only
    /// enabled while a replay session is live.
    ExitReplay,
}
impl ImageMenuAction {
    pub fn ui_action(&self) -> Option<UiAction> {
        Some(match self {
            Self::PushMenu { .. }
            | Self::Back
            | Self::SetLocal { .. }
            | Self::Reading { .. }
            | Self::SaveSlot { .. }
            | Self::LoadSlot { .. }
            | Self::HistoryPage { .. } => return None,
            Self::Close => UiAction::Close,
            Self::AdjustPreference { field, delta } => field.adjust(*delta),
            Self::ToggleReducedMotion => UiAction::ReducedMotion,
            Self::NewGame => UiAction::NewGame,
            Self::Saves => UiAction::Saves,
            Self::Settings => UiAction::Settings,
            Self::Title => UiAction::Title,
            Self::Menu { menu } => UiAction::ImageMenu { menu: menu.clone() },
            Self::Entry { function } => UiAction::ImageMenuEntry {
                function: function.clone(),
            },
            Self::Replay { function } => UiAction::ImageMenuReplay {
                function: function.clone(),
            },
            Self::ExitReplay => UiAction::ExitReplay,
        })
    }
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MenuReadingMode {
    Auto,
    SkipRead,
    PeekStory,
}
impl Theme {
    pub fn image_assets(&self) -> BTreeSet<String> {
        let mut assets = BTreeSet::new();
        if let Some(asset) = &self.dialogue.background {
            assets.insert(asset.clone());
        }
        assets.extend(
            self.dialogue_styles
                .values()
                .filter_map(|style| style.dialogue.background.clone()),
        );
        for menu in self.image_menus.values() {
            assets.extend(menu.image_assets());
        }
        assets
    }
}
/// Closed component registry implemented by the locked player SDK. Components
/// only project state; their actions and focus semantics remain player-owned.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeSlots {
    #[serde(default, rename = "dialogue.main")]
    pub dialogue: DialogueComponent,
    #[serde(default, rename = "choice.main")]
    pub choice: ChoiceComponent,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DialogueComponent {
    #[default]
    #[serde(rename = "builtin.dialogue")]
    Bottom,
    #[serde(rename = "builtin.dialogue.top")]
    Top,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChoiceComponent {
    #[default]
    #[serde(rename = "builtin.choice")]
    Standard,
    #[serde(rename = "builtin.choice.compact")]
    Compact,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextShadow {
    pub offset: [f32; 2],
    pub color: [f32; 4],
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DialogueProps {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadow: Option<TextShadow>,
    pub height: f32,
    pub padding: f32,
    pub font_size: f32,
    pub line_height: f32,
    pub opacity: f32,
    pub background: Option<String>,
    pub rect: Option<[f32; 4]>,
    /// Absolute stage coordinates for the text surface, independent of the
    /// decorative window. Requires an explicit window rectangle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_rect: Option<[f32; 4]>,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DialogueStyle {
    pub dialogue: DialogueProps,
    pub text: [f32; 4],
}
impl Default for DialogueProps {
    fn default() -> Self {
        Self {
            shadow: None,
            height: 220.,
            padding: 24.,
            font_size: 23.,
            line_height: 1.5,
            opacity: 1.,
            background: None,
            rect: None,
            text_rect: None,
        }
    }
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ChoiceProps {
    pub width: f32,
    pub item_height: f32,
}
impl Default for ChoiceProps {
    fn default() -> Self {
        Self {
            width: 520.,
            item_height: 58.,
        }
    }
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HidePolicy {
    #[default]
    ContinueStory,
    PauseStory,
}
fn default_hide_policy(value: &HidePolicy) -> bool {
    *value == HidePolicy::ContinueStory
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoDelayPolicy {
    #[default]
    LengthScaled,
    Fixed,
}
fn default_auto_delay_policy(value: &AutoDelayPolicy) -> bool {
    *value == AutoDelayPolicy::LengthScaled
}
pub const DEFAULT_SAVE_SLOTS: u32 = 3;
pub const MAX_SAVE_SLOTS: u32 = 64;
fn default_save_slots() -> u32 {
    DEFAULT_SAVE_SLOTS
}
fn is_default_save_slots(value: &u32) -> bool {
    *value == DEFAULT_SAVE_SLOTS
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PlayerDefaults {
    #[serde(skip_serializing_if = "default_auto_delay_policy")]
    pub auto_delay_policy: AutoDelayPolicy,
    #[serde(skip_serializing_if = "default_hide_policy")]
    pub hide_policy: HidePolicy,
    #[serde(
        default = "default_save_slots",
        skip_serializing_if = "is_default_save_slots"
    )]
    pub save_slots: u32,
    pub font_scale: f32,
    pub bgm_volume: f32,
    pub voice_volume: f32,
    pub sfx_volume: f32,
    pub reduced_motion: bool,
    pub auto_delay_us: Micros,
    pub prefetch_content: bool,
    pub prefetch_media: bool,
}
impl Default for PlayerDefaults {
    fn default() -> Self {
        Self {
            auto_delay_policy: AutoDelayPolicy::LengthScaled,
            hide_policy: HidePolicy::ContinueStory,
            save_slots: DEFAULT_SAVE_SLOTS,
            font_scale: 1.,
            bgm_volume: 0.3,
            voice_volume: 0.8,
            sfx_volume: 0.5,
            reduced_motion: false,
            auto_delay_us: Micros(1_200_000),
            prefetch_content: true,
            prefetch_media: false,
        }
    }
}
impl PlayerDefaults {
    pub fn auto_delay(&self, character_count: usize, scale: f32) -> u64 {
        let extra = if self.auto_delay_policy == AutoDelayPolicy::LengthScaled {
            (character_count as u64).saturating_mul(20_000)
        } else {
            0
        };
        let base = self.auto_delay_us.0.saturating_add(extra);
        (base as f64 * scale as f64).round() as u64
    }
    pub fn preferences(&self, ui_locale: String, text_locale: String) -> Preferences {
        Preferences {
            ui_locale,
            text_locale,
            font_scale: self.font_scale,
            text_speed: 1.,
            auto_wait_scale: 1.,
            auto_wait_voice: true,
            voice_continue: true,
            character_voices: BTreeMap::new(),
            bgm_volume: self.bgm_volume,
            voice_volume: self.voice_volume,
            sfx_volume: self.sfx_volume,
            reduced_motion: self.reduced_motion,
        }
    }
}
fn validate_dialogue_props(dialogue: &DialogueProps) -> Result<()> {
    let range = |value: f32, lo: f32, hi: f32| value.is_finite() && (lo..=hi).contains(&value);
    let rect_ok = |r: &[f32; 4]| {
        r.iter().all(|v| v.is_finite() && v.abs() <= 8192.) && r[2] > 0. && r[3] > 0.
    };
    if dialogue.rect.as_ref().is_some_and(|r| !rect_ok(r))
        || dialogue.text_rect.as_ref().is_some_and(|r| !rect_ok(r))
        || (dialogue.text_rect.is_some() && dialogue.rect.is_none())
    {
        return Err(Diagnostic::new(
            "E_THEME_PROPS",
            "theme",
            "invalid image menu or dialogue rectangle",
        ));
    }
    for (name, value, lo, hi) in [
        (
            "dialogue.height",
            dialogue.height,
            if dialogue.rect.is_some() { 32. } else { 220. },
            if dialogue.rect.is_some() { 8192. } else { 320. },
        ),
        (
            "dialogue.padding",
            dialogue.padding,
            if dialogue.rect.is_some() { 0. } else { 12. },
            32.,
        ),
        ("dialogue.line_height", dialogue.line_height, 1., 2.),
        ("dialogue.opacity", dialogue.opacity, 0., 1.),
        (
            "dialogue.font_size",
            dialogue.font_size,
            18.,
            if dialogue.rect.is_some() { 64. } else { 28. },
        ),
    ] {
        if !range(value, lo, hi) {
            return Err(Diagnostic::new(
                "E_THEME_PROPS",
                format!("theme.{name}"),
                format!("expected {lo}..{hi}"),
            ));
        }
    }
    if let Some(shadow) = dialogue.shadow {
        if shadow.offset.iter().any(|v| !range(*v, -16., 16.))
            || shadow.color.iter().any(|v| !range(*v, 0., 1.))
        {
            return Err(Diagnostic::new(
                "E_THEME_PROPS",
                "theme.dialogue.shadow",
                "expected finite offset -16..16 and RGBA 0..1",
            ));
        }
    }
    Ok(())
}

/// Also enforced at runtime: a hand-written executable cannot bypass author checks.
pub fn validate_ui_config(theme: &Theme, player: &PlayerDefaults) -> Result<()> {
    if theme.dialogue_styles.len() > 32 {
        return Err(Diagnostic::new(
            "E_THEME_PROPS",
            "theme.dialogue_styles",
            "at most 32 styles",
        ));
    }
    for (name, style) in &theme.dialogue_styles {
        if name.is_empty()
            || name.len() > 256
            || style
                .text
                .iter()
                .any(|value| !value.is_finite() || !(0. ..=1.).contains(value))
        {
            return Err(Diagnostic::new(
                "E_THEME_PROPS",
                name,
                "invalid dialogue style identity or color",
            ));
        }
        validate_dialogue_props(&style.dialogue)?;
    }
    validate_dialogue_props(&theme.dialogue)?;
    let range = |value: f32, lo: f32, hi: f32| value.is_finite() && (lo..=hi).contains(&value);
    for (name, color) in [
        ("background", theme.background),
        ("panel", theme.panel),
        ("accent", theme.accent),
        ("text", theme.text),
        ("muted", theme.muted),
    ] {
        if !color.iter().all(|v| range(*v, 0., 1.)) || color[3] < 0.5 {
            return Err(Diagnostic::new(
                "E_THEME_PROPS",
                format!("theme.{name}"),
                "RGBA must be finite in 0..1; alpha must be at least 0.5",
            ));
        }
    }
    let rect_ok = |r: &[f32; 4]| {
        r.iter().all(|v| v.is_finite() && v.abs() <= 8192.) && r[2] > 0. && r[3] > 0.
    };
    if theme.image_menus.len() > 4096
        || (!theme.image_menus.is_empty()
            && !theme.image_menus.contains_key("title")
            && theme.menu_overlay.is_none())
    {
        return Err(Diagnostic::new(
            "E_THEME_PROPS",
            "theme",
            "invalid image menu or dialogue rectangle",
        ));
    }
    if theme
        .menu_overlay
        .as_ref()
        .is_some_and(|id| !theme.image_menus.contains_key(id))
    {
        return Err(Diagnostic::new(
            "E_VIEW",
            "theme.menu_overlay",
            "unknown overlay menu",
        ));
    }
    if !(1..=MAX_SAVE_SLOTS).contains(&player.save_slots) {
        return Err(Diagnostic::new(
            "E_PLAYER_CONFIG",
            "player.save_slots",
            format!("expected 1..={MAX_SAVE_SLOTS}"),
        ));
    }
    for (id, menu) in &theme.image_menus {
        menu.validate_elements_with_slots(player.save_slots)?;
        if menu.controls().any(|(_,action,_)|matches!(action,ImageMenuAction::Menu {menu} | ImageMenuAction::PushMenu {menu} if !theme.image_menus.contains_key(menu))) {return Err(Diagnostic::new("E_VIEW","theme.image_menus","unknown menu target"));}
        let mut ids = BTreeSet::new();
        if id.is_empty()
            || (menu.uses_navigation() && id.len() > 128)
            || menu.background.is_empty()
            || menu.buttons.len() > 256
        {
            return Err(Diagnostic::new(
                "E_THEME_PROPS",
                "theme.image_menus",
                "invalid menu",
            ));
        }
        for button in &menu.buttons {
            if button.id.is_empty()
                || !ids.insert(&button.id)
                || button.label.is_empty()
                || button.asset.is_empty()
                || !rect_ok(&button.rect)
                || matches!(&button.action, ImageMenuAction::Menu { menu } | ImageMenuAction::PushMenu {menu} if !theme.image_menus.contains_key(menu))
            {
                return Err(Diagnostic::new(
                    "E_THEME_PROPS",
                    "theme.image_menus",
                    "invalid button or menu target",
                ));
            }
        }
    }
    // System overlays cannot launch an unisolated title-only replay entry.
    if let Some(root) = &theme.menu_overlay {
        let mut pending = vec![root.as_str()];
        let mut visited = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if !visited.insert(id) {
                continue;
            }
            for (_, action, _) in theme.image_menus[id].controls() {
                match action {
                    ImageMenuAction::Entry { .. } => return Err(Diagnostic::new(
                        "E_VIEW_SERVICE",
                        "theme.menu_overlay",
                        "entry requires the title context; isolated replay is not available here",
                    )),
                    ImageMenuAction::Menu { menu } | ImageMenuAction::PushMenu { menu } => {
                        pending.push(menu.as_str())
                    }
                    _ => {}
                }
            }
        }
    }
    let luminance = |c: [f32; 4]| {
        let channel = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        channel(c[0]) * 0.2126 + channel(c[1]) * 0.7152 + channel(c[2]) * 0.0722
    };
    for (name, foreground, background) in [
        ("text/panel", theme.text, theme.panel),
        ("accent/background", theme.accent, theme.background),
    ] {
        let a = luminance(foreground);
        let b = luminance(background);
        if (a.max(b) + 0.05) / (a.min(b) + 0.05) < 4.5 {
            return Err(Diagnostic::new(
                "E_THEME_CONTRAST",
                format!("theme.{name}"),
                "base color contrast must be at least 4.5:1",
            ));
        }
    }
    for (name, value, lo, hi) in [
        ("choice.width", theme.choice.width, 360., 680.),
        ("choice.item_height", theme.choice.item_height, 48., 72.),
    ] {
        if !range(value, lo, hi) {
            return Err(Diagnostic::new(
                "E_THEME_PROPS",
                format!("theme.{name}"),
                format!("expected {lo}..{hi}"),
            ));
        }
    }
    for (name, value, lo, hi) in [
        ("font_scale", player.font_scale, 0.8, 1.5),
        ("bgm_volume", player.bgm_volume, 0., 1.),
        ("voice_volume", player.voice_volume, 0., 1.),
        ("sfx_volume", player.sfx_volume, 0., 1.),
    ] {
        if !range(value, lo, hi) {
            return Err(Diagnostic::new(
                "E_PLAYER_CONFIG",
                format!("player.{name}"),
                format!("expected {lo}..{hi}"),
            ));
        }
    }
    let minimum = if player.auto_delay_policy == AutoDelayPolicy::Fixed {
        0
    } else {
        100_000
    };
    if !(minimum..=30_000_000).contains(&player.auto_delay_us.0) {
        return Err(Diagnostic::new(
            "E_PLAYER_CONFIG",
            "player.auto_delay_us",
            format!("expected {minimum}..30000000 microseconds"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_ui_config_enforces_save_slots_bounds_and_menu_slots() {
        let theme = Theme::default();
        let mut player = PlayerDefaults::default();
        assert_eq!(player.save_slots, DEFAULT_SAVE_SLOTS);
        assert!(validate_ui_config(&theme, &player).is_ok());

        // Zero and > MAX_SAVE_SLOTS fail closed.
        player.save_slots = 0;
        assert_eq!(
            validate_ui_config(&theme, &player).unwrap_err().code,
            "E_PLAYER_CONFIG"
        );
        player.save_slots = MAX_SAVE_SLOTS + 1;
        assert_eq!(
            validate_ui_config(&theme, &player).unwrap_err().code,
            "E_PLAYER_CONFIG"
        );

        // Configurable 12 slots allows MenuSlot::Fixed { slot: 11 } and rejects slot 12.
        player.save_slots = 12;
        assert!(validate_ui_config(&theme, &player).is_ok());
        let locals = BTreeMap::from([("s".into(), MenuValue::Int(11))]);
        assert_eq!(MenuSlot::Fixed { slot: 11 }.resolve(&locals), None);
        assert_eq!(
            MenuSlot::Fixed { slot: 11 }.resolve_in(&locals, 12),
            Some(11)
        );
        assert_eq!(MenuSlot::Fixed { slot: 12 }.resolve_in(&locals, 12), None);
        assert_eq!(
            MenuSlot::Local { name: "s".into() }.resolve_in(&locals, 12),
            Some(11)
        );
    }

    #[test]
    fn validate_ui_config_rejects_low_contrast_and_invalid_shadow() {
        let player = PlayerDefaults::default();
        let mut theme = Theme::default();
        // Text equal to panel violates WCAG 4.5:1 contrast.
        theme.text = theme.panel;
        assert_eq!(
            validate_ui_config(&theme, &player).unwrap_err().code,
            "E_THEME_CONTRAST"
        );

        let mut theme = Theme::default();
        theme.dialogue.shadow = Some(TextShadow {
            offset: [20., 0.],
            color: [0., 0., 0., 1.],
        });
        assert_eq!(
            validate_ui_config(&theme, &player).unwrap_err().code,
            "E_THEME_PROPS"
        );

        let mut theme = Theme::default();
        theme.dialogue.text_rect = Some([10., 10., 100., 50.]);
        // text_rect without rect is forbidden.
        assert_eq!(
            validate_ui_config(&theme, &player).unwrap_err().code,
            "E_THEME_PROPS"
        );
    }

    #[test]
    fn player_defaults_auto_delay_respects_policy_and_scale() {
        let mut p = PlayerDefaults::default();
        assert_eq!(p.auto_delay(10, 1.0), 1_400_000);
        assert_eq!(p.auto_delay(10, 0.5), 700_000);
        p.auto_delay_policy = AutoDelayPolicy::Fixed;
        assert_eq!(p.auto_delay(50, 1.0), 1_200_000);
    }
}
