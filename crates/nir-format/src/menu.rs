use crate::*;

/// Maximum suspended parent pages in one authored navigation context.
pub const MAX_MENU_PARENTS: usize = 8;
/// Finite declarative menu content. All geometry uses the authored stage space.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MenuElement {
    pub id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub visible_when: Vec<MenuCondition>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enabled_when: Vec<MenuCondition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_local: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_preference: Option<MenuPreference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_slot: Option<MenuSlot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub rect: [f32; 4],
    #[serde(default = "one")]
    pub scale: f32,
    #[serde(default = "one")]
    pub opacity: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<[f32; 4]>,
    pub content: MenuContent,
}
/// Explicit image states for a finite scrollbar part. No runtime sprite slicing.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MenuImageStates {
    pub asset: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hover_asset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pressed_asset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_asset: Option<String>,
}
impl MenuImageStates {
    pub fn assets(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.asset.as_str())
            .chain(self.hover_asset.as_deref())
            .chain(self.pressed_asset.as_deref())
            .chain(self.disabled_asset.as_deref())
    }
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MenuContent {
    HistoryScrollbar {
        window: String,
        label: String,
        thumb_height: f32,
        arrow_height: f32,
        line_step: f32,
        track: Box<MenuImageStates>,
        thumb: Box<MenuImageStates>,
        decrease: Box<MenuImageStates>,
        increase: Box<MenuImageStates>,
    },
    Toggle {
        label: String,
        binding: MenuToggleBinding,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_asset: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        off_asset: Option<String>,
    },
    Range {
        label: String,
        binding: MenuRangeBinding,
        min: f32,
        max: f32,
        step: f32,
        #[serde(default = "default_thumb_width")]
        thumb_width: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        track_asset: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thumb_asset: Option<String>,
    },
    HistoryFlow {
        size: f32,
        line_height: f32,
        gap: f32,
        wheel_step: f32,
        page_step: f32,
        max_visible: u32,
        color: [f32; 4],
    },
    HistoryWindow {
        offset_local: String,
        limit: u32,
        row_height: f32,
        size: f32,
        color: [f32; 4],
    },
    Group,
    /// Finite vertical layout of declared, visible direct children.
    Stack {
        gap: f32,
    },
    Image {
        asset: String,
    },
    Text {
        text: String,
        size: f32,
        color: [f32; 4],
    },
    TextButton {
        label: String,
        size: f32,
        color: [f32; 4],
        hover_color: [f32; 4],
        disabled_color: [f32; 4],
        action: ImageMenuAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        requires: Option<String>,
    },
    Button {
        label: String,
        asset: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hover_asset: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        locked_asset: Option<String>,
        action: ImageMenuAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        requires: Option<String>,
    },
    HitRegion {
        label: String,
        action: ImageMenuAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        requires: Option<String>,
    },
}
impl MenuContent {
    pub fn is_group(&self) -> bool {
        matches!(self, Self::Group | Self::Stack { .. })
    }
}
impl MenuElement {
    pub fn control(&self) -> Option<(&str, &ImageMenuAction, Option<&str>)> {
        match &self.content {
            MenuContent::Button {
                action, requires, ..
            }
            | MenuContent::TextButton {
                action, requires, ..
            }
            | MenuContent::HitRegion {
                action, requires, ..
            } => Some((&self.id, action, requires.as_deref())),
            _ => None,
        }
    }
    pub fn assets(&self) -> Vec<&str> {
        match &self.content {
            MenuContent::HistoryScrollbar {
                track,
                thumb,
                decrease,
                increase,
                ..
            } => track
                .assets()
                .chain(thumb.assets())
                .chain(decrease.assets())
                .chain(increase.assets())
                .collect(),
            MenuContent::Toggle {
                on_asset,
                off_asset,
                ..
            } => on_asset
                .iter()
                .chain(off_asset)
                .map(String::as_str)
                .collect(),
            MenuContent::Range {
                track_asset,
                thumb_asset,
                ..
            } => track_asset
                .iter()
                .chain(thumb_asset)
                .map(String::as_str)
                .collect(),
            MenuContent::Image { asset } => vec![asset],
            MenuContent::Button {
                asset,
                hover_asset,
                locked_asset,
                ..
            } => std::iter::once(asset.as_str())
                .chain(hover_asset.as_deref())
                .chain(locked_asset.as_deref())
                .collect(),
            _ => vec![],
        }
    }
}
impl ImageMenu {
    pub fn controls(&self) -> impl Iterator<Item = (&str, &ImageMenuAction, Option<&str>)> {
        self.buttons
            .iter()
            .map(|b| (b.id.as_str(), &b.action, b.requires.as_deref()))
            .chain(self.elements.iter().filter_map(MenuElement::control))
    }
    pub fn validate_elements(&self) -> Result<()> {
        self.validate_state()?;
        if let Some(effects) = &self.effects {
            effects.validate()?;
        }
        let fail = || {
            Diagnostic::new(
                "E_VIEW",
                "theme.image_menus.elements",
                "invalid or excessive menu geometry, content, or hierarchy",
            )
        };
        if self
            .elements
            .iter()
            .map(|e| match e.content {
                MenuContent::Text { .. }
                | MenuContent::TextButton { .. }
                | MenuContent::Toggle { .. }
                | MenuContent::Range { .. } => 1,
                MenuContent::HistoryWindow { limit, .. } => limit.min(65) as usize,
                MenuContent::HistoryFlow { max_visible, .. } => max_visible.min(65) as usize,
                _ => 0,
            })
            .sum::<usize>()
            > 64
            || self.elements.len()
                + self.buttons.len()
                + self
                    .elements
                    .iter()
                    .filter(|e| matches!(e.content, MenuContent::HistoryScrollbar { .. }))
                    .count()
                    * 3
                > 256
        {
            return Err(fail());
        }
        if self
            .elements
            .iter()
            .filter(|e| matches!(e.content, MenuContent::HistoryFlow { .. }))
            .count()
            > 1
        {
            return Err(fail());
        }
        if self
            .elements
            .iter()
            .filter(|e| matches!(e.content, MenuContent::HistoryScrollbar { .. }))
            .count()
            > 1
        {
            return Err(fail());
        }
        let mut ids: BTreeSet<&str> = self.buttons.iter().map(|b| b.id.as_str()).collect();
        let geometry = |r: [f32; 4]| {
            r.iter().all(|v| v.is_finite() && v.abs() <= 8192.) && r[2] >= 0. && r[3] >= 0.
        };
        for e in &self.elements {
            if e.id.is_empty()
                || e.id.len() > 128
                || !ids.insert(&e.id)
                || !geometry(e.rect)
                || !e.scale.is_finite()
                || !(0.01..=8.).contains(&e.scale)
                || !e.opacity.is_finite()
                || !(0. ..=1.).contains(&e.opacity)
                || e.clip.is_some_and(|r| !geometry(r))
            {
                return Err(fail());
            }
            if !e.content.is_group() && (e.rect[2] <= 0. || e.rect[3] <= 0.) {
                return Err(fail());
            }
            match &e.content {
                MenuContent::TextButton {
                    label,
                    size,
                    color,
                    hover_color,
                    disabled_color,
                    ..
                } => {
                    if label.is_empty()
                        || label.len() > 1024
                        || !size.is_finite()
                        || !(8. ..=128.).contains(size)
                        || color
                            .iter()
                            .chain(hover_color)
                            .chain(disabled_color)
                            .any(|c| !c.is_finite() || !(0. ..=1.).contains(c))
                    {
                        return Err(fail());
                    }
                }
                MenuContent::Stack { gap } => {
                    if !gap.is_finite() || !(0. ..=1024.).contains(gap) {
                        return Err(fail());
                    }
                    let mut extent = 0.;
                    for child in self
                        .elements
                        .iter()
                        .filter(|c| c.parent.as_deref() == Some(&e.id))
                    {
                        // Rows reserve a finite authored height, including group
                        // rows. Hidden rows reserve neither height nor gap.
                        if child.rect[1] != 0. || child.rect[3] <= 0. {
                            return Err(fail());
                        }
                        extent += child.rect[3] * child.scale + gap;
                    }
                    if !extent.is_finite() || extent > 8192. {
                        return Err(fail());
                    }
                }
                MenuContent::Toggle { label, binding, .. } => {
                    if label.is_empty() || label.len() > 1024 || !binding.valid(&self.locals) {
                        return Err(fail());
                    }
                }
                MenuContent::Range {
                    label,
                    binding,
                    min,
                    max,
                    step,
                    thumb_width,
                    ..
                } => {
                    if label.is_empty()
                        || label.len() > 1024
                        || !min.is_finite()
                        || !max.is_finite()
                        || !step.is_finite()
                        || min >= max
                        || *step <= 0.
                        || *step > max - min
                        || (max - min) / step > 1000.
                        || !thumb_width.is_finite()
                        || *thumb_width < 1.
                        || *thumb_width >= e.rect[2]
                        || !binding.valid(&self.locals, *min, *max, *step)
                    {
                        return Err(fail());
                    }
                }
                MenuContent::HistoryScrollbar {
                    window,
                    label,
                    thumb_height,
                    arrow_height,
                    line_step,
                    ..
                } => {
                    if label.is_empty()
                        || label.len() > 1024
                        || !thumb_height.is_finite()
                        || *thumb_height < 1.
                        || !arrow_height.is_finite()
                        || *arrow_height < 1.
                        || *thumb_height + 2. * arrow_height >= e.rect[3]
                        || !line_step.is_finite()
                        || !(1. ..=8192.).contains(line_step)
                        || !self.elements.iter().any(|target| {
                            target.id == *window
                                && matches!(target.content, MenuContent::HistoryFlow { .. })
                        })
                    {
                        return Err(fail());
                    }
                }
                MenuContent::HistoryFlow {
                    size,
                    line_height,
                    gap,
                    wheel_step,
                    page_step,
                    max_visible,
                    color,
                } => {
                    if !size.is_finite() || !(8. ..=128.).contains(size)
                        || !line_height.is_finite() || !(*size..=512.).contains(line_height)
                        || !gap.is_finite() || !(0. ..=1024.).contains(gap)
                        || !wheel_step.is_finite() || !(1. ..=8192.).contains(wheel_step)
                        || !page_step.is_finite() || !(1. ..=8192.).contains(page_step)
                        || !(3..=64).contains(max_visible)
                        // Worst-case minimum reader scale, plus both partial edge records.
                        || e.rect[3] > line_height * 0.8 * max_visible.saturating_sub(2) as f32
                        || color.iter().any(|c| !c.is_finite() || !(0. ..=1.).contains(c))
                    {
                        return Err(fail());
                    }
                }
                MenuContent::HistoryWindow {
                    offset_local,
                    limit,
                    row_height,
                    size,
                    color,
                } => {
                    if !(1..=16).contains(limit)
                        || !row_height.is_finite()
                        || !(16. ..=1024.).contains(row_height)
                        || !size.is_finite()
                        || !(8. ..=128.).contains(size)
                        || color
                            .iter()
                            .any(|c| !c.is_finite() || !(0. ..=1.).contains(c))
                        || !matches!(self.locals.get(offset_local),Some(MenuLocal::Int {min,max,..}) if *min == 0 && *max <= 999)
                    {
                        return Err(fail());
                    }
                }
                MenuContent::Text { text, size, color }
                    if text.is_empty()
                        || text.len() > 4096
                        || !size.is_finite()
                        || !(8. ..=128.).contains(size)
                        || color
                            .iter()
                            .any(|c| !c.is_finite() || !(0. ..=1.).contains(c)) =>
                {
                    return Err(fail())
                }
                MenuContent::Button { label, .. } | MenuContent::HitRegion { label, .. }
                    if label.is_empty() || label.len() > 1024 =>
                {
                    return Err(fail())
                }
                _ => {}
            }
            if e.assets().iter().any(|a| a.is_empty()) {
                return Err(fail());
            }
            let mut parent = e.parent.as_deref();
            let mut seen = BTreeSet::from([e.id.as_str()]);
            let mut scale = e.scale;
            while let Some(id) = parent {
                if seen.len() >= 8 || !seen.insert(id) {
                    return Err(fail());
                }
                let p = self.elements.iter().find(|p| p.id == id).ok_or_else(fail)?;
                if !p.content.is_group() {
                    return Err(fail());
                }
                scale *= p.scale;
                if !(0.001..=16.).contains(&scale) {
                    return Err(fail());
                }
                parent = p.parent.as_deref();
            }
        }
        Ok(())
    }
}

/// Finite page presentation effects. Sounds and music run in the foreground
/// UI audio domain; fades multiply the whole page's draw alpha. The state is
/// transient per menu instance and never enters story snapshots, and restore
/// projections never replay these effects.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MenuEffects {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enter: Option<MenuTransition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close: Option<MenuTransition>,
    /// One-shot sound when a control action is accepted. Restore-driven
    /// projections do not fire it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub click: Option<String>,
    /// Looping page music in the foreground domain. Never part of any
    /// wait-for-completion set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub music: Option<MenuMusic>,
}
/// One page boundary: an optional one-shot sound plus a bounded fade.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MenuTransition {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sound: Option<String>,
    #[serde(default)]
    pub fade_us: Micros,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MenuMusic {
    pub asset: String,
    #[serde(default)]
    pub bus: AudioBus,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub gain: f32,
}
fn one() -> f32 {
    1.
}
fn is_one(value: &f32) -> bool {
    *value == 1.
}
impl MenuEffects {
    pub fn assets(&self) -> Vec<&str> {
        self.enter
            .iter()
            .chain(&self.close)
            .filter_map(|t| t.sound.as_deref())
            .chain(self.click.as_deref())
            .chain(self.music.as_ref().map(|m| m.asset.as_str()))
            .collect()
    }
    fn validate(&self) -> Result<()> {
        let fail = || {
            Diagnostic::new(
                "E_VIEW_EFFECTS",
                "theme.image_menus.effects",
                "invalid menu effect sound, music, gain, or fade duration",
            )
        };
        let asset = |id: &str| !id.is_empty() && id.len() <= 128;
        for transition in [&self.enter, &self.close].into_iter().flatten() {
            if transition.fade_us.0 > 2_000_000
                || transition.sound.as_deref().is_some_and(|s| !asset(s))
            {
                return Err(fail());
            }
        }
        if self.click.as_deref().is_some_and(|s| !asset(s)) {
            return Err(fail());
        }
        if let Some(music) = &self.music {
            if !asset(&music.asset)
                || !music.gain.is_finite()
                || !(0. ..=4.).contains(&music.gain)
            {
                return Err(fail());
            }
        }
        Ok(())
    }
}
impl ImageMenu {
    pub fn uses_effects(&self) -> bool {
        self.effects.is_some()
    }
    /// Menu media closure including effect sounds and page music.
    pub fn effect_assets(&self) -> BTreeSet<String> {
        self.effects
            .iter()
            .flat_map(|e| e.assets().into_iter().map(str::to_owned))
            .collect()
    }
}
/// UI-local data never aliases VM variables or snapshot slots.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MenuValue {
    Bool(bool),
    Int(i32),
    Text(String),
}
impl MenuValue {
    pub fn display(&self) -> String {
        match self {
            Self::Bool(v) => v.to_string(),
            Self::Int(v) => v.to_string(),
            Self::Text(v) => v.clone(),
        }
    }
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MenuLocal {
    Bool {
        initial: bool,
    },
    Int {
        initial: i32,
        min: i32,
        max: i32,
    },
    Enum {
        initial: String,
        values: Vec<String>,
    },
}
impl MenuLocal {
    pub fn initial(&self) -> MenuValue {
        match self {
            Self::Bool { initial } => MenuValue::Bool(*initial),
            Self::Int { initial, .. } => MenuValue::Int(*initial),
            Self::Enum { initial, .. } => MenuValue::Text(initial.clone()),
        }
    }
    pub fn accepts(&self, value: &MenuValue) -> bool {
        match (self, value) {
            (Self::Bool { .. }, MenuValue::Bool(_)) => true,
            (Self::Int { min, max, .. }, MenuValue::Int(v)) => (min..=max).contains(&v),
            (Self::Enum { values, .. }, MenuValue::Text(v)) => values.contains(v),
            _ => false,
        }
    }
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MenuCondition {
    HistoryAvailable {
        available: bool,
    },
    Story {
        name: String,
        equals: MenuValue,
    },
    Local {
        name: String,
        equals: MenuValue,
    },
    Profile {
        key: String,
        present: bool,
    },
    ReadingAvailable {
        mode: MenuReadingMode,
        available: bool,
    },
}
impl MenuCondition {
    fn matches(
        &self,
        locals: &BTreeMap<String, MenuValue>,
        profile: &BTreeSet<String>,
        reading: &BTreeSet<MenuReadingMode>,
        story: &BTreeMap<String, MenuValue>,
        history: bool,
    ) -> bool {
        match self {
            Self::HistoryAvailable { available } => history == *available,
            Self::Story { name, equals } => story.get(name) == Some(equals),
            Self::Local { name, equals } => locals.get(name) == Some(equals),
            Self::Profile { key, present } => profile.contains(key) == *present,
            Self::ReadingAvailable { mode, available } => reading.contains(mode) == *available,
        }
    }
}
impl ImageMenu {
    pub fn uses_state(&self) -> bool {
        !self.locals.is_empty()
            || self.elements.iter().any(|e| {
                !e.visible_when.is_empty() || !e.enabled_when.is_empty() || e.text_local.is_some()
            })
            || self
                .controls()
                .any(|(_, a, _)| matches!(a, ImageMenuAction::SetLocal { .. }))
    }
    pub fn initial_locals(&self) -> BTreeMap<String, MenuValue> {
        self.locals
            .iter()
            .map(|(k, v)| (k.clone(), v.initial()))
            .collect()
    }
    pub fn element_state(
        &self,
        id: &str,
        locals: &BTreeMap<String, MenuValue>,
        profile: &BTreeSet<String>,
        reading: &BTreeSet<MenuReadingMode>,
        story: &BTreeMap<String, MenuValue>,
        history: bool,
    ) -> (bool, bool) {
        let mut visible = true;
        let mut enabled = true;
        let mut current = Some(id);
        for _ in 0..8 {
            let Some(id) = current else {
                return (visible, enabled);
            };
            let Some(e) = self.elements.iter().find(|e| e.id == id) else {
                return (false, false);
            };
            visible &= e
                .visible_when
                .iter()
                .all(|c| c.matches(locals, profile, reading, story, history));
            enabled &= e
                .enabled_when
                .iter()
                .all(|c| c.matches(locals, profile, reading, story, history))
                && e.control()
                    .is_none_or(|(_, _, guard)| guard.is_none_or(|k| profile.contains(k)));
            current = e.parent.as_deref();
        }
        if current.is_some() {
            (false, false)
        } else {
            (visible, enabled)
        }
    }
    pub fn uses_stack(&self) -> bool {
        self.elements
            .iter()
            .any(|e| matches!(e.content, MenuContent::Stack { .. }))
    }
    pub fn uses_text_buttons(&self) -> bool {
        self.elements
            .iter()
            .any(|e| matches!(e.content, MenuContent::TextButton { .. }))
    }
    /// Pure layout in parent coordinates. Declaration order and IDs remain
    /// stable; only the position of visible stack rows changes.
    pub fn element_positions(
        &self,
        locals: &BTreeMap<String, MenuValue>,
        profile: &BTreeSet<String>,
        reading: &BTreeSet<MenuReadingMode>,
        story: &BTreeMap<String, MenuValue>,
        history: bool,
    ) -> BTreeMap<String, [f32; 2]> {
        let mut positions: BTreeMap<_, _> = self
            .elements
            .iter()
            .map(|e| (e.id.clone(), [e.rect[0], e.rect[1]]))
            .collect();
        for parent in &self.elements {
            let MenuContent::Stack { gap } = parent.content else {
                continue;
            };
            let mut y = 0.;
            for child in self
                .elements
                .iter()
                .filter(|e| e.parent.as_deref() == Some(&parent.id))
            {
                if self
                    .element_state(&child.id, locals, profile, reading, story, history)
                    .0
                {
                    positions.insert(child.id.clone(), [child.rect[0], y]);
                    y += child.rect[3] * child.scale + gap;
                }
            }
        }
        positions
    }
    fn validate_state(&self) -> Result<()> {
        let fail = || {
            Diagnostic::new(
                "E_VIEW_STATE",
                "theme.image_menus",
                "invalid local declaration, binding, or assignment",
            )
        };
        let key = |s: &str| !s.is_empty() && s.len() <= 128;
        if self.locals.len() > 32
            || self.story_exports.len() > 32
            || self
                .story_exports
                .iter()
                .any(|(alias, variable)| !key(alias) || !key(variable))
        {
            return Err(fail());
        }
        for (name, local) in &self.locals {
            if !key(name) || !local.accepts(&local.initial()) {
                return Err(fail());
            }
            if let MenuLocal::Enum { values, .. } = local {
                if values.is_empty()
                    || values.len() > 32
                    || values.iter().any(|v| v.is_empty() || v.len() > 256)
                    || values.iter().collect::<BTreeSet<_>>().len() != values.len()
                {
                    return Err(fail());
                }
            }
        }
        for e in &self.elements {
            if let Some(slot) = &e.text_slot {
                if !slot.valid(&self.locals)
                    || e.text_preference.is_some()
                    || e.text_local.is_some()
                    || !matches!(e.content, MenuContent::Text { .. })
                {
                    return Err(fail());
                }
            }
            if e.text_preference.is_some()
                && (e.text_local.is_some() || !matches!(e.content, MenuContent::Text { .. }))
            {
                return Err(fail());
            }
            if e.visible_when.len() > 16 || e.enabled_when.len() > 16 {
                return Err(fail());
            }
            for c in e.visible_when.iter().chain(&e.enabled_when) {
                match c {
                    MenuCondition::Story { name, equals }
                        if !self.story_exports.contains_key(name)
                            || matches!(equals, MenuValue::Text(_)) =>
                    {
                        return Err(fail())
                    }
                    MenuCondition::Local { name, equals }
                        if self.locals.get(name).is_none_or(|l| !l.accepts(equals)) =>
                    {
                        return Err(fail())
                    }
                    MenuCondition::Profile { key: k, .. } if !key(k) => return Err(fail()),
                    _ => {}
                }
            }
            if let Some(local) = &e.text_local {
                if !matches!(e.content, MenuContent::Text { .. })
                    || !self.locals.contains_key(local)
                {
                    return Err(fail());
                }
            }
        }
        for (_, action, _) in self.controls() {
            if let ImageMenuAction::PushMenu { menu } = action {
                if menu.is_empty() || menu.len() > 128 {
                    return Err(fail());
                }
            }
            if let ImageMenuAction::HistoryPage { window, delta } = action {
                if *delta == 0
                    || delta.unsigned_abs() > 16
                    || !self.elements.iter().any(|e| {
                        e.id == *window && matches!(e.content, MenuContent::HistoryWindow { .. })
                    })
                {
                    return Err(fail());
                }
            }
            if let ImageMenuAction::SaveSlot { slot } | ImageMenuAction::LoadSlot { slot } = action
            {
                if !slot.valid(&self.locals) {
                    return Err(fail());
                }
            }
            if let ImageMenuAction::AdjustPreference { delta, .. } = action {
                if !delta.is_finite() || delta.abs() > 4. {
                    return Err(fail());
                }
            }
            if let ImageMenuAction::SetLocal { local, value } = action {
                if self.locals.get(local).is_none_or(|l| !l.accepts(value)) {
                    return Err(fail());
                }
            }
        }
        Ok(())
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MenuPreference {
    BgmVolume,
    VoiceVolume,
    SfxVolume,
    FontScale,
    TextSpeed,
    AutoWaitScale,
}
impl MenuPreference {
    pub fn value(self, p: &Preferences) -> f32 {
        match self {
            Self::BgmVolume => p.bgm_volume,
            Self::VoiceVolume => p.voice_volume,
            Self::SfxVolume => p.sfx_volume,
            Self::FontScale => p.font_scale,
            Self::TextSpeed => p.text_speed,
            Self::AutoWaitScale => p.auto_wait_scale,
        }
    }
    pub fn adjust(self, delta: f32) -> UiAction {
        match self {
            Self::BgmVolume => UiAction::Volume {
                bus: AudioBus::Bgm,
                delta,
            },
            Self::VoiceVolume => UiAction::Volume {
                bus: AudioBus::Voice,
                delta,
            },
            Self::SfxVolume => UiAction::Volume {
                bus: AudioBus::Sfx,
                delta,
            },
            Self::FontScale => UiAction::FontSize { delta },
            Self::TextSpeed => UiAction::TextSpeed { delta },
            Self::AutoWaitScale => UiAction::AutoWait { delta },
        }
    }
}
impl ImageMenu {
    pub fn uses_services(&self) -> bool {
        self.uses_navigation()
            || self.uses_reading()
            || self.uses_storage()
            || self.uses_history()
            || self.uses_history_flow()
            || self.uses_history_availability()
            || self.uses_values()
            || self.elements.iter().any(|e| e.text_preference.is_some())
            || self.controls().any(|(_, a, _)| {
                matches!(
                    a,
                    ImageMenuAction::Close
                        | ImageMenuAction::Reading { .. }
                        | ImageMenuAction::AdjustPreference { .. }
                        | ImageMenuAction::ToggleReducedMotion
                )
            })
    }
}

impl ImageMenu {
    pub fn uses_reading(&self) -> bool {
        self.controls()
            .any(|(_, a, _)| matches!(a, ImageMenuAction::Reading { .. }))
            || self.elements.iter().any(|e| {
                e.visible_when
                    .iter()
                    .chain(&e.enabled_when)
                    .any(|c| matches!(c, MenuCondition::ReadingAvailable { .. }))
            })
    }
    pub fn image_assets(&self) -> BTreeSet<String> {
        std::iter::once(self.background.clone())
            .chain(self.buttons.iter().flat_map(|b| {
                std::iter::once(b.asset.clone())
                    .chain(b.hover_asset.clone())
                    .chain(b.locked_asset.clone())
            }))
            .chain(
                self.elements
                    .iter()
                    .flat_map(|e| e.assets().into_iter().map(str::to_owned)),
            )
            .collect()
    }
    /// Assets a page needs resident before it may present and play its page
    /// effects: every image plus the effect sounds and looping music, so a
    /// prepared page never addresses an undecoded buffer.
    pub fn prepared_assets(&self) -> BTreeSet<String> {
        let mut assets = self.image_assets();
        assets.extend(self.effect_assets());
        assets
    }
}

impl Theme {
    /// Preserve legacy title-page preparation, excluding overlay-only pages.
    /// Effect sounds and music ride along so boot decodes them with the page.
    pub fn title_image_assets(&self) -> BTreeSet<String> {
        let Some(root) = self.menu_overlay.as_deref() else {
            // No overlay: every page is title-reachable, effects included.
            let mut assets = self.image_assets();
            assets.extend(self.image_menus.values().flat_map(ImageMenu::effect_assets));
            return assets;
        };
        let closure = |roots: Vec<String>| {
            let mut pending = roots;
            let mut seen = BTreeSet::new();
            while let Some(id) = pending.pop() {
                if !seen.insert(id.clone()) {
                    continue;
                }
                if let Some(menu) = self.image_menus.get(&id) {
                    pending.extend(menu.controls().filter_map(|(_, a, _)| match a {
                        ImageMenuAction::Menu { menu } | ImageMenuAction::PushMenu { menu } => {
                            Some(menu.clone())
                        }
                        _ => None,
                    }));
                }
            }
            seen
        };
        let overlay = closure(vec![root.into()]);
        let mut roots: Vec<_> = self
            .image_menus
            .keys()
            .filter(|id| !overlay.contains(*id))
            .cloned()
            .collect();
        roots.push("title".into());
        let title = closure(roots);
        self.dialogue
            .background
            .iter()
            .cloned()
            .chain(
                self.image_menus
                    .iter()
                    .filter(|(id, _)| title.contains(*id))
                    .flat_map(|(_, menu)| menu.prepared_assets()),
            )
            .collect()
    }
}

/// A bounded save slot, either literal or selected by a local integer.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MenuSlot {
    Fixed { slot: u32 },
    Local { name: String },
}
impl MenuSlot {
    pub fn resolve(&self, locals: &BTreeMap<String, MenuValue>) -> Option<u32> {
        match self {
            Self::Fixed { slot } => (*slot < 3).then_some(*slot),
            Self::Local { name } => match locals.get(name) {
                Some(MenuValue::Int(value)) if (0..3).contains(value) => Some(*value as u32),
                _ => None,
            },
        }
    }
    fn valid(&self, locals: &BTreeMap<String, MenuLocal>) -> bool {
        match self {
            Self::Fixed { slot } => *slot < 3,
            Self::Local { name } => {
                matches!(locals.get(name), Some(MenuLocal::Int {min,max,..}) if *min >= 0 && *max < 3)
            }
        }
    }
}
impl ImageMenu {
    pub fn uses_storage(&self) -> bool {
        self.elements.iter().any(|e| e.text_slot.is_some())
            || self.controls().any(|(_, a, _)| {
                matches!(
                    a,
                    ImageMenuAction::SaveSlot { .. } | ImageMenuAction::LoadSlot { .. }
                )
            })
    }
}

impl ImageMenu {
    pub fn uses_history_scrollbar(&self) -> bool {
        self.elements
            .iter()
            .any(|e| matches!(e.content, MenuContent::HistoryScrollbar { .. }))
    }
    pub fn uses_history_availability(&self) -> bool {
        self.elements.iter().any(|e| {
            e.visible_when
                .iter()
                .chain(&e.enabled_when)
                .any(|c| matches!(c, MenuCondition::HistoryAvailable { .. }))
        })
    }
    pub fn uses_history_flow(&self) -> bool {
        self.elements
            .iter()
            .any(|e| matches!(e.content, MenuContent::HistoryFlow { .. }))
    }
    pub fn uses_history(&self) -> bool {
        self.elements
            .iter()
            .any(|e| matches!(e.content, MenuContent::HistoryWindow { .. }))
            || self
                .controls()
                .any(|(_, a, _)| matches!(a, ImageMenuAction::HistoryPage { .. }))
    }
}

impl ImageMenu {
    pub fn history_page(
        &self,
        window: &str,
        delta: i32,
        locals: &BTreeMap<String, MenuValue>,
        total: usize,
    ) -> Option<(String, i32)> {
        let element = self.elements.iter().find(|e| e.id == window)?;
        let MenuContent::HistoryWindow {
            offset_local,
            limit,
            ..
        } = &element.content
        else {
            return None;
        };
        let MenuLocal::Int { max, .. } = self.locals.get(offset_local)? else {
            return None;
        };
        let MenuValue::Int(value) = locals.get(offset_local)? else {
            return None;
        };
        let end = total.saturating_sub(*limit as usize).min(*max as usize) as i64;
        let current = (*value as i64).clamp(0, end);
        let next = (current + delta as i64).clamp(0, end);
        (next != current).then(|| (offset_local.clone(), next as i32))
    }
}

fn default_thumb_width() -> f32 {
    20.
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MenuToggleBinding {
    Local { name: String },
    ReducedMotion,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MenuRangeBinding {
    Local { name: String },
    Preference { field: MenuPreference },
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum MenuValueInput {
    Bool(bool),
    Number(f32),
}
impl MenuToggleBinding {
    fn valid(&self, locals: &BTreeMap<String, MenuLocal>) -> bool {
        match self {
            Self::ReducedMotion => true,
            Self::Local { name } => matches!(locals.get(name), Some(MenuLocal::Bool { .. })),
        }
    }
    pub fn value(&self, locals: &BTreeMap<String, MenuValue>, prefs: &Preferences) -> Option<bool> {
        match self {
            Self::ReducedMotion => Some(prefs.reduced_motion),
            Self::Local { name } => match locals.get(name) {
                Some(MenuValue::Bool(value)) => Some(*value),
                _ => None,
            },
        }
    }
}
impl MenuRangeBinding {
    fn valid(&self, locals: &BTreeMap<String, MenuLocal>, min: f32, max: f32, step: f32) -> bool {
        match self {
            Self::Preference { field } => {
                let (lo, hi) = field.limits();
                min >= lo && max <= hi
            }
            Self::Local { name } => {
                min.fract() == 0.
                    && max.fract() == 0.
                    && step.fract() == 0.
                    && min >= -16_777_215.
                    && max <= 16_777_215.
                    && matches!(locals.get(name),Some(MenuLocal::Int{min:lo,max:hi,..}) if min as f64 >= *lo as f64 && max as f64 <= *hi as f64)
            }
        }
    }
    pub fn value(&self, locals: &BTreeMap<String, MenuValue>, prefs: &Preferences) -> Option<f32> {
        match self {
            Self::Preference { field } => Some(field.value(prefs)),
            Self::Local { name } => match locals.get(name) {
                Some(MenuValue::Int(value)) => Some(*value as f32),
                _ => None,
            },
        }
    }
}
impl MenuPreference {
    pub fn limits(self) -> (f32, f32) {
        match self {
            Self::BgmVolume | Self::VoiceVolume | Self::SfxVolume => (0., 1.),
            Self::FontScale => (0.8, 1.5),
            Self::TextSpeed | Self::AutoWaitScale => (0.25, 4.),
        }
    }
}
impl ImageMenu {
    pub fn uses_values(&self) -> bool {
        self.elements.iter().any(|e| {
            matches!(
                e.content,
                MenuContent::Range { .. } | MenuContent::Toggle { .. }
            )
        })
    }
}

impl ImageMenu {
    pub fn uses_story(&self) -> bool {
        !self.story_exports.is_empty()
            || self.elements.iter().any(|e| {
                e.visible_when
                    .iter()
                    .chain(&e.enabled_when)
                    .any(|c| matches!(c, MenuCondition::Story { .. }))
            })
    }
    /// Project only explicitly exported bool/i32 values. Missing values fail closed.
    pub fn story_values(&self, variables: &BTreeMap<String, Value>) -> BTreeMap<String, MenuValue> {
        self.story_exports
            .iter()
            .filter_map(|(alias, variable)| {
                let value = match variables.get(variable)? {
                    Value::Bool(v) => MenuValue::Bool(*v),
                    Value::I32(v) => MenuValue::Int(*v),
                    Value::String(_) => return None,
                };
                Some((alias.clone(), value))
            })
            .collect()
    }
    pub fn validate_story_exports(&self, variables: &BTreeMap<String, Value>) -> Result<()> {
        let fail = || {
            Diagnostic::new(
                "E_VIEW_STORY",
                "theme.image_menus.story_exports",
                "expected declared bool/i32 variables and same-typed comparisons",
            )
        };
        let values = self.story_values(variables);
        if values.len() != self.story_exports.len() {
            return Err(fail());
        }
        for condition in self
            .elements
            .iter()
            .flat_map(|e| e.visible_when.iter().chain(&e.enabled_when))
        {
            if let MenuCondition::Story { name, equals } = condition {
                if !matches!(
                    (values.get(name), equals),
                    (Some(MenuValue::Bool(_)), MenuValue::Bool(_))
                        | (Some(MenuValue::Int(_)), MenuValue::Int(_))
                ) {
                    return Err(fail());
                }
            }
        }
        Ok(())
    }
}

impl ImageMenu {
    pub fn uses_navigation(&self) -> bool {
        self.controls()
            .any(|(_, a, _)| matches!(a, ImageMenuAction::PushMenu { .. } | ImageMenuAction::Back))
    }
    pub fn uses_replay(&self) -> bool {
        self.controls().any(|(_, a, _)| {
            matches!(a, ImageMenuAction::Replay { .. } | ImageMenuAction::ExitReplay)
        })
    }
}
