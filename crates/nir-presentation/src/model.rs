use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Title,
    Story,
    Menu,
    Settings,
    History,
    Saves,
    Ended,
}
#[derive(Debug, Clone)]
pub struct DialogueView {
    pub full_text: String,
    pub visible_text: String,
    pub speaker: String,
    pub ready: bool,
    pub gate: bool,
    pub locale: String,
    pub font_plan_digest: String,
    pub font_assets: Vec<String>,
    pub emphasis: Vec<(usize, usize)>,
    pub ruby: Vec<(usize, usize, String)>,
    pub images: Vec<InlineImagePlacement>,
}
#[derive(Debug, Clone)]
pub struct ChoiceView {
    pub id: String,
    pub image: Option<nir_format::ChoiceImage>,
    pub label: String,
    pub enabled: bool,
    /// Semantic selection cursor of a typed-result interaction. Hover and
    /// keyboard focus are transients and never set this.
    pub selected: bool,
    pub locale: String,
    pub font_plan_digest: String,
    pub font_assets: Vec<String>,
}
#[derive(Debug, Clone)]
pub struct HistoryView {
    pub key: usize,
    pub voice_count: usize,
    pub choice: Option<HistoryChoiceKind>,
    pub speaker: String,
    pub text: String,
    pub images: Vec<InlineImagePlacement>,
    pub locale: String,
    pub font_plan_digest: String,
    pub font_assets: Vec<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryChoiceKind {
    Selected,
    TimedOut,
    Cancelled,
}
impl HistoryChoiceKind {
    pub fn message(self) -> &'static str {
        match self {
            Self::Selected => "history-choice",
            Self::TimedOut => "history-choice-timeout",
            Self::Cancelled => "history-choice-cancelled",
        }
    }
    pub(super) fn index(self) -> usize {
        match self {
            Self::Selected => 0,
            Self::TimedOut => 1,
            Self::Cancelled => 2,
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct HistoryVoiceView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    pub entry: usize,
    pub failed: bool,
    pub preparing: bool,
}
#[derive(Debug, Clone)]
pub struct CharacterVoiceView {
    pub id: String,
    pub name: String,
    pub locale: String,
    pub fonts: Vec<String>,
}
#[derive(Debug, Clone)]
pub struct MenuHistoryRow {
    pub key: usize,
    pub entry: HistoryView,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlotView {
    pub slot: u32,
    pub label: String,
    pub exists: bool,
    /// A read failure is distinct from an empty slot. Keep its file intact
    /// and block writes until a subsequent listing can establish its revision.
    pub error: Option<String>,
}
/// In-flight overrides for one menu element's enter animation. `None`
/// properties keep the authored value; `offset` is added displacement that
/// rests at zero. A finished animation leaves no entry at all.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ElementAnimation {
    pub opacity: Option<f32>,
    pub scale: Option<f32>,
    pub offset: [f32; 2],
}
#[derive(Debug, Clone)]
pub struct UiModel {
    pub transition_style: StageTransition,
    pub image_menu: String,
    pub authored_menu: bool,
    pub menu_instance: u32,
    pub menu_revision: u32,
    pub menu_depth: usize,
    pub menu_navigation_pending: bool,
    pub menu_locals: std::collections::BTreeMap<String, MenuValue>,
    pub hovered_image: Option<String>,
    pub profile: std::collections::BTreeSet<String>,
    pub title: String,
    pub screen: Screen,
    pub nodes: Vec<Node>,
    pub transition: Option<(Vec<Node>, f32)>,
    pub stage: [f32; 2],
    pub dialogue: Option<DialogueView>,
    pub hidden_dialogue: bool,
    pub advance_wait: bool,
    /// In-flight message-window reveal; `None` (including under reduced
    /// motion) keeps the committed hidden/visible state with no interpolation.
    pub window_transition: Option<WindowTransition>,
    /// In-flight spatial menu-page reveal. Dissolve coverage (and no style at
    /// all) rides the legacy `menu_opacity` ramp instead; `None` keeps the
    /// page on the shared surface.
    pub menu_transition: Option<(StageTransition, bool, f32)>,
    /// In-flight element enter animations, keyed by element id. Entries are
    /// transient overrides projected before layout so parent transforms
    /// propagate; the map is empty whenever nothing animates.
    pub menu_element_animations: std::collections::BTreeMap<String, ElementAnimation>,
    pub interface_hidden: bool,
    pub dialogue_appearance: nir_format::DialogueAppearance,
    pub dialogue_decorations: std::collections::BTreeMap<
        nir_format::DialogueDecorationSlot,
        nir_format::DialogueDecoration,
    >,
    pub choices: Vec<ChoiceView>,
    /// The pending interaction declares an explicit cancel target.
    pub choice_cancellable: bool,
    pub prefs: Preferences,
    pub ui_locale: String,
    pub ui_fonts: Vec<String>,
    pub ui_font_plan_digest: String,
    pub text_locale: String,
    pub available_ui_locales: Vec<String>,
    pub available_text_locales: Vec<String>,
    pub text_fonts: Vec<String>,
    pub text_font_plan_digest: String,
    pub locale_pending: bool,
    pub locale_error: Option<String>,
    pub preflight_texts: Vec<TextRun>,
    pub theme: std::sync::Arc<Theme>,
    pub history: Vec<HistoryView>,
    pub history_total: usize,
    pub history_voice: Option<HistoryVoiceView>,
    pub character_voices: Vec<CharacterVoiceView>,
    pub menu_history: std::collections::BTreeMap<String, Vec<MenuHistoryRow>>,
    pub menu_history_flow: Option<std::sync::Arc<[MenuHistoryRow]>>,
    /// Authored menu page opacity from finite enter/close fades; 1 when no
    /// fade is active. Applies to the whole menu layer, never the story
    /// scene below it.
    pub menu_opacity: f32,
    pub slots: Vec<SlotView>,
    pub save_confirmation: Option<(u32, u32)>,
    pub busy_slots: std::collections::BTreeSet<u32>,
    pub can_save: bool,
    pub can_import: bool,
    pub menu_disabled: bool,
    /// A live replay owns the session: replay entries close, the exit opens,
    /// and storage services hide until the frozen session returns.
    pub replay_active: bool,
    pub menu_story: std::collections::BTreeMap<String, MenuValue>,
    pub menu_reading_modes: std::collections::BTreeSet<MenuReadingMode>,
    pub paused: bool,
    /// Continue can release the restore pause only when no other owner blocks Story.
    pub can_continue: bool,
    pub loading: bool,
    pub status: String,
    pub fault: Option<String>,
    pub retrying: bool,
    pub fault_recovery: Vec<Recovery>,
    pub auto: bool,
    pub skip: bool,
    pub outcome: Option<String>,
    pub history_offset: usize,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Quad {
    pub rect: [f32; 4],
    pub corners: Option<[[f32; 2]; 4]>,
    pub color: [f32; 4],
    pub asset: Option<String>,
    pub clip: Option<[f32; 4]>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    pub shadow: Option<TextShadow>,
    pub monochrome: bool,
    pub text: String,
    pub visible: Option<usize>,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub size: f32,
    pub line_height: f32,
    pub color: [f32; 4],
    pub emphasis: Vec<(usize, usize)>,
    pub images: Vec<InlineImagePlacement>,
    pub image_scale: f32,
    pub scroll: f32,
    pub clip: Option<[f32; 4]>,
    pub region: Option<ScrollRegion>,
    pub locale: String,
    pub font_assets: Vec<String>,
    pub font_plan_digest: String,
    pub preflight_only: bool,
}
impl TextRun {
    /// Paint-only duplicate: retain shaping, visibility and scroll, but never
    /// introduce a second reading region or semantic node.
    pub fn shadow_run(&self) -> Option<Self> {
        let shadow = self.shadow?;
        let mut run = self.clone();
        run.shadow = None;
        run.monochrome = true;
        run.region = None;
        run.x += shadow.offset[0];
        run.y += shadow.offset[1];
        run.color = shadow.color;
        run.color[3] *= self.color[3];
        let [x, y, w, h] = self
            .clip
            .unwrap_or([self.x, self.y, self.width, self.height]);
        let left = x.max(self.x);
        let top = y.max(self.y);
        let right = (x + w).min(self.x + self.width);
        let bottom = (y + h).min(self.y + self.height);
        run.clip = Some([left, top, (right - left).max(0.), (bottom - top).max(0.)]);
        Some(run)
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SemanticValue {
    Scrollbar {
        value: f32,
        max: f32,
        step: f32,
        from: f32,
        to: f32,
        thumb_top: f32,
        thumb_bottom: f32,
    },
    Toggle {
        checked: bool,
    },
    Range {
        min: f32,
        max: f32,
        step: f32,
        value: f32,
        from: f32,
        to: f32,
    },
}
#[derive(Debug, Clone, Serialize)]
pub struct SemanticNode {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<SemanticValue>,
    pub id: u32,
    pub label: String,
    pub action: UiAction,
    pub enabled: bool,
    pub rect: [f32; 4],
    pub locale: String,
}
#[derive(Debug, Clone, PartialEq)]
pub enum MenuPaint {
    Quad(usize),
    Text(usize),
}
/// In-flight message-window reveal as seen by projection. Dissolve folds into
/// the existing per-item opacity multiply; wipe/mask divert the window into
/// the offscreen root and return through the `@window` sentinel.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowTransition {
    pub style: StageTransition,
    pub to_visible: bool,
    pub progress: f32,
}
/// The diverted window root: quads render into the offscreen root texture;
/// `texts` indexes `DrawPacket::texts` (kept in the packet for layout) whose
/// glyph areas route to the window pass instead of the shared text renderer.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowLayers {
    pub quads: Vec<Quad>,
    pub texts: Vec<usize>,
    pub style: StageTransition,
    pub to_visible: bool,
    pub progress: f32,
}
/// The diverted menu page root. `position` is the `@menu` sentinel's index in
/// `DrawPacket::quads` — the underlying frame keeps every quad around it;
/// `texts` index `DrawPacket::texts` (kept in the packet for layout) whose
/// glyph areas route to the page pass instead of the shared renderer.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuLayers {
    pub quads: Vec<Quad>,
    pub texts: Vec<usize>,
    pub style: StageTransition,
    pub to_visible: bool,
    pub progress: f32,
    pub position: usize,
}
#[derive(Debug, Default)]
pub struct DrawPacket {
    pub(crate) stage_viewport: Option<[f32; 4]>,
    pub(crate) builtin_dialogue: Option<[f32; 4]>,
    pub(crate) history_flow: Option<reading::HistoryFlowView>,
    pub(crate) history_bar_view: Option<scrollbar::BarView>,
    pub history_bar: Option<HistoryBar>,
    pub transition_style: StageTransition,
    pub menu_paint: Vec<MenuPaint>,
    pub menu_controls: std::collections::BTreeMap<u32, String>,
    /// Projection authority for responsive fixed history pages, not story state.
    pub menu_history_capacity: std::collections::BTreeMap<String, (u32, usize)>,
    pub menu_quad_range: Option<(usize, usize)>,
    pub(crate) menu_page_range: Option<(usize, usize)>,
    pub(crate) menu_page_texts: Option<(usize, usize)>,
    pub quads: Vec<Quad>,
    pub texts: Vec<TextRun>,
    pub semantics: Vec<SemanticNode>,
    pub announcement: String,
    pub announcement_locale: String,
    pub locale: String,
    pub font_assets: Vec<String>,
    pub font_plan_digest: String,
    pub width: f32,
    pub height: f32,
    pub stage_size: [u32; 2],
    pub transition_layers: Option<(Vec<Quad>, Vec<Quad>, f32)>,
    pub window_layers: Option<WindowLayers>,
    pub menu_layers: Option<MenuLayers>,
    pub scrolls: Vec<ScrollView>,
    pub(crate) dialogue_hint: Option<usize>,
    pub(crate) dialogue_hint_quad: Option<usize>,
    pub(crate) next_semantic_id: u32,
}

impl DrawPacket {
    /// Compare the fields that affect drawing this packet.
    ///
    /// Accessibility announcements, hit targets, and scroll metadata are
    /// intentionally excluded because callers can update them without
    /// changing the rendered frame.
    pub fn visual_eq(&self, other: &Self) -> bool {
        self.menu_paint == other.menu_paint
            && self.menu_quad_range == other.menu_quad_range
            && self.quads == other.quads
            && self.texts == other.texts
            && self.locale == other.locale
            && self.font_assets == other.font_assets
            && self.font_plan_digest == other.font_plan_digest
            && self.width == other.width
            && self.height == other.height
            && self.stage_size == other.stage_size
            && self.transition_layers == other.transition_layers
            && self.transition_style == other.transition_style
            && self.window_layers == other.window_layers
            && self.menu_layers == other.menu_layers
    }

    pub(super) fn rect(&mut self, r: [f32; 4], c: [f32; 4]) {
        self.quads.push(Quad {
            corners: None,
            rect: r,
            color: c,
            asset: None,
            clip: None,
        });
    }
    pub(super) fn text(
        &mut self,
        t: impl Into<String>,
        x: f32,
        y: f32,
        w: f32,
        size: f32,
        c: [f32; 4],
    ) {
        self.texts.push(TextRun {
            text: t.into(),
            visible: None,
            x,
            y,
            width: w,
            height: size * 1.55,
            size,
            line_height: size * 1.5,
            color: c,
            emphasis: vec![],
            images: vec![],
            image_scale: 1.,
            scroll: 0.,
            clip: None,
            region: None,
            locale: self.locale.clone(),
            font_assets: self.font_assets.clone(),
            font_plan_digest: self.font_plan_digest.clone(),
            preflight_only: false,
            shadow: None,
            monochrome: false,
        });
    }
    pub(super) fn button(
        &mut self,
        label: String,
        action: UiAction,
        r: [f32; 4],
        active: bool,
        theme: &Theme,
    ) {
        self.rect(r, if active { theme.accent } else { theme.panel });
        self.text(
            label.clone(),
            r[0] + 16.,
            r[1] + (r[3] - 24.) / 2. - 1.,
            r[2] - 24.,
            16.,
            if active { theme.background } else { theme.text },
        );
        if let Some(run) = self.texts.last_mut() {
            run.height = (r[3] - 12.).max(20.);
            run.y = r[1] + 6.;
        }
        let id = self.alloc_semantic_id();
        self.semantics.push(SemanticNode {
            value: None,
            id,
            label,
            action,
            enabled: true,
            rect: r,
            locale: self.locale.clone(),
        });
    }
    pub fn alloc_semantic_id(&mut self) -> u32 {
        let floor = self
            .semantics
            .iter()
            .filter(|n| n.id < u32::MAX - 16)
            .map(|n| n.id)
            .max()
            .map_or(0, |id| id.saturating_add(1));
        let id = self.next_semantic_id.max(floor);
        self.next_semantic_id = id.saturating_add(1);
        id
    }
    pub fn hit_node(&self, x: f32, y: f32) -> Option<&SemanticNode> {
        self.semantics.iter().rev().find(|n| {
            x >= n.rect[0]
                && y >= n.rect[1]
                && x <= n.rect[0] + n.rect[2]
                && y <= n.rect[1] + n.rect[3]
        })
    }
    pub fn hit(&self, x: f32, y: f32) -> Option<UiAction> {
        self.hit_node(x, y)
            .filter(|n| n.enabled)
            .map(|n| n.action.clone())
    }
}
