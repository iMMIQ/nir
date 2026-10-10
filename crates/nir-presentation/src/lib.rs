//! Read-only presentation projection. UI actions do not mutate the story here.
#![forbid(unsafe_code)]
pub use cosmic_text;
use fluent_bundle::{FluentBundle, FluentResource};
use nir_format::*;
use serde::Serialize;
pub mod audio_recovery;
pub mod history;
mod history_controls;
mod input;
mod reading;
mod scrollbar;
pub mod storage_recovery;
pub use input::{
    control_value_action, pointer_action, primary_action, value_action, KeyboardFocus,
};
pub use reading::{MenuScrollIdentity, ReadingState, ScrollView};
pub use scrollbar::{HistoryBar, HistoryBarPart};

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
    fn index(self) -> usize {
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
    pub theme: Theme,
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
}
pub struct Messages {
    en: FluentBundle<FluentResource>,
    zh: FluentBundle<FluentResource>,
}
impl Default for Messages {
    fn default() -> Self {
        fn bundle(loc: &str, src: &str) -> FluentBundle<FluentResource> {
            let mut b = FluentBundle::new(vec![loc.parse().unwrap()]);
            b.add_resource(
                FluentResource::try_new(src.to_owned()).expect("bundled messages parse"),
            )
            .expect("unique bundled messages");
            b
        }
        Self {
            en: bundle("en", include_str!("../messages/en.ftl")),
            zh: bundle("zh-Hans", include_str!("../messages/zh-Hans.ftl")),
        }
    }
}
impl Messages {
    pub fn preflight(&self, locale: &str) -> Vec<String> {
        [
            "new-game",
            "continue",
            "menu",
            "close",
            "settings",
            "history",
            "history-compact",
            "saves",
            "save",
            "load",
            "rollback",
            "exit",
            "auto",
            "skip",
            "hide-interface",
            "show-interface",
            "language",
            "language-zh",
            "language-en",
            "ui-language",
            "text-language",
            "language-active",
            "language-pending",
            "language-failed",
            "language-cancel",
            "language-applied",
            "font-size",
            "text-speed",
            "auto-wait",
            "auto-wait-voice",
            "voice-continue",
            "voice-continue-hint",
            "reading-preferences-hint",
            "music",
            "voice",
            "sfx",
            "motion",
            "export",
            "import",
            "retry",
            "sound-unavailable",
            "sound-reconnecting",
            "sound-recovery-hint",
            "storage-unavailable",
            "storage-recovery-hint",
            "storage-retrying",
            "storage-save-unconfirmed",
            "loading",
            "paused",
            "ending",
            "empty-slot",
            "unavailable-slot",
            "saved",
            "saving",
            "save-pending",
            "read-failed",
            "error-prepare",
            "error-storage",
            "error-render",
            "error-content",
            "error-host",
            "scroll-back",
            "scroll-forward",
            "scroll-back-compact",
            "scroll-forward-compact",
            "title-hint",
            "gate-hint",
            "advance-hint",
            "reveal-hint",
            "history-back",
            "history-forward",
            "history-voice",
            "history-voice-stop",
            "history-voice-failed",
            "history-voice-stop-compact",
            "history-choice",
            "history-choice-timeout",
            "history-choice-cancelled",
        ]
        .into_iter()
        .map(|id| self.text(locale, id))
        .collect()
    }
    pub fn diagnostic(&self, diagnostic: &Diagnostic, locale: &str) -> String {
        if diagnostic.code == "E_STORAGE_UNCERTAIN" && diagnostic.location == "save" {
            return self.text(locale, "save-pending");
        }
        let key = match diagnostic.details.as_ref().map(|d| &d.domain) {
            Some(ErrorDomain::Prepare) => "error-prepare",
            Some(ErrorDomain::Storage) => "error-storage",
            Some(ErrorDomain::Render) => "error-render",
            Some(ErrorDomain::Core | ErrorDomain::Content) => "error-content",
            _ => "error-host",
        };
        format!("{}: {}", diagnostic.code, self.text(locale, key))
    }
    pub fn text(&self, locale: &str, id: &str) -> String {
        let b = if locale == "zh-Hans" {
            &self.zh
        } else {
            &self.en
        };
        let b = if b.has_message(id) { b } else { &self.en };
        b.get_message(id)
            .and_then(|m| m.value())
            .map(|p| b.format_pattern(p, None, &mut vec![]).into_owned())
            .unwrap_or_else(|| id.into())
    }
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

    fn rect(&mut self, r: [f32; 4], c: [f32; 4]) {
        self.quads.push(Quad {
            corners: None,
            rect: r,
            color: c,
            asset: None,
            clip: None,
        });
    }
    fn text(&mut self, t: impl Into<String>, x: f32, y: f32, w: f32, size: f32, c: [f32; 4]) {
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
    fn button(
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
        self.semantics.push(SemanticNode {
            value: None,
            id: self
                .semantics
                .iter()
                .map(|n| n.id)
                .max()
                .map_or(0, |id| id + 1),
            label,
            action,
            enabled: true,
            rect: r,
            locale: self.locale.clone(),
        });
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
fn scene_layout(
    packet: &DrawPacket,
    nodes: &[Node],
    stage: [f32; 2],
    alpha: f32,
) -> Vec<(String, Quad)> {
    // Each sibling list is ordered separately: a group subtree is contiguous.
    #[derive(Clone, Copy)]
    struct Transform {
        x: f32,
        y: f32,
        scale: f32,
        opacity: f32,
        clip: Option<[f32; 4]>,
    }
    fn intersect(a: Option<[f32; 4]>, b: Option<[f32; 4]>) -> Option<[f32; 4]> {
        match (a, b) {
            (Some(a), Some(b)) => {
                let x = a[0].max(b[0]);
                let y = a[1].max(b[1]);
                Some([
                    x,
                    y,
                    (a[0] + a[2]).min(b[0] + b[2]).max(x) - x,
                    (a[1] + a[3]).min(b[1] + b[3]).max(y) - y,
                ])
            }
            (a, b) => a.or(b),
        }
    }
    fn visit(out: &mut Vec<(String, Quad)>, nodes: &[Node], parent: Option<&str>, t: Transform) {
        let mut siblings: Vec<_> = nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.parent.as_deref() == parent)
            .collect();
        siblings.sort_by_key(|(i, n)| (n.order, *i));
        for (_, n) in siblings {
            let x = t.x + (n.x + n.offset[0]) * t.scale;
            let y = t.y + (n.y + n.offset[1]) * t.scale;
            let scale = t.scale * n.scale;
            let opacity = t.opacity * n.opacity;
            let clip = intersect(
                t.clip,
                n.clip.map(|r| {
                    [
                        x + r[0] * scale,
                        y + r[1] * scale,
                        r[2] * scale,
                        r[3] * scale,
                    ]
                }),
            );
            if n.width > 0. && n.height > 0. {
                let mut color = n.color;
                color[3] *= opacity;
                out.push((
                    n.id.clone(),
                    Quad {
                        rect: [x, y, n.width * scale, n.height * scale],
                        corners: n.sprite_transform.map(|transform| {
                            transform
                                .corners(n.width, n.height)
                                .map(|[cx, cy]| [x + cx * scale, y + cy * scale])
                        }),
                        color,
                        asset: n.asset.clone(),
                        clip,
                    },
                ));
            }
            visit(
                out,
                nodes,
                Some(&n.id),
                Transform {
                    x,
                    y,
                    scale,
                    opacity,
                    clip,
                },
            );
        }
    }
    let scale = (packet.width / stage[0]).min(packet.height / stage[1]);
    let mut out = vec![];
    visit(
        &mut out,
        nodes,
        None,
        Transform {
            x: (packet.width - stage[0] * scale) / 2.,
            y: (packet.height - stage[1] * scale) / 2.,
            scale,
            opacity: alpha,
            clip: None,
        },
    );
    out
}
fn scene(packet: &mut DrawPacket, nodes: &[Node], stage: [f32; 2], alpha: f32) {
    let quads = scene_layout(packet, nodes, stage, alpha);
    packet.quads.extend(quads.into_iter().map(|(_, quad)| quad));
}
fn menu_service_enabled(
    action: &ImageMenuAction,
    m: &UiModel,
    capacities: &std::collections::BTreeMap<String, (u32, usize)>,
) -> bool {
    if m.menu_navigation_pending {
        return false;
    }
    match action {
        ImageMenuAction::Back => m.menu_depth > 0,
        ImageMenuAction::PushMenu { .. } => m.menu_depth < nir_format::MAX_MENU_PARENTS,
        ImageMenuAction::Reading { mode } => m.menu_reading_modes.contains(mode),
        ImageMenuAction::HistoryPage { window, delta } => {
            m.theme.image_menus.get(&m.image_menu).is_some_and(|menu| {
                menu.element_state(
                    window,
                    &m.menu_locals,
                    &m.profile,
                    &m.menu_reading_modes,
                    &m.menu_story,
                    m.history_total > 0,
                )
                .0 && menu
                    .history_page_with_capacity(
                        window,
                        *delta,
                        &m.menu_locals,
                        m.history_total,
                        capacities.get(window).map(|(_, limit)| *limit),
                    )
                    .is_some()
            })
        }
        ImageMenuAction::SaveSlot { slot } => {
            m.can_save
                && !m.replay_active
                && slot.resolve(&m.menu_locals).is_some_and(|slot| {
                    !m.busy_slots.contains(&slot)
                        && !m
                            .slots
                            .iter()
                            .any(|row| row.slot == slot && row.error.is_some())
                })
        }
        ImageMenuAction::LoadSlot { slot } => {
            !m.replay_active
                && slot.resolve(&m.menu_locals).is_some_and(|slot| {
                    m.slots
                        .iter()
                        .any(|row| row.slot == slot && (row.exists || row.error.is_some()))
                        && !m.busy_slots.contains(&slot)
                })
        }
        // Nested replays never nest: the frozen session is the only one.
        ImageMenuAction::Replay { .. } => !m.replay_active,
        ImageMenuAction::ExitReplay => m.replay_active,
        _ => true,
    }
}
fn menu_value_visual(
    packet: &mut DrawPacket,
    e: &MenuElement,
    quad: &Quad,
    m: &UiModel,
    enabled: bool,
) {
    let [x, y, w, h] = quad.rect;
    let scale = w / e.rect[2];
    let alpha = quad.color[3] * if enabled { 1. } else { 0.35 };
    let paint = |p: &mut DrawPacket, rect, color: [f32; 4], asset: Option<String>| {
        p.menu_paint.push(MenuPaint::Quad(p.quads.len()));
        let mut color = color;
        color[3] *= alpha;
        p.quads.push(Quad {
            corners: None,
            rect,
            color,
            asset,
            clip: quad.clip,
        });
    };
    let mut label = None;
    match &e.content {
        MenuContent::Toggle {
            label: text,
            binding,
            on_asset,
            off_asset,
        } => {
            let checked = binding.value(&m.menu_locals, &m.prefs).unwrap_or(false);
            let asset = if checked { on_asset } else { off_asset };
            if let Some(asset) = asset {
                paint(packet, quad.rect, [1.; 4], Some(asset.clone()));
            } else {
                let side = h.min(w) * 0.6;
                paint(
                    packet,
                    [x, y + (h - side) / 2., side, side],
                    m.theme.panel,
                    None,
                );
                if checked {
                    paint(
                        packet,
                        [
                            x + side * 0.2,
                            y + (h - side) / 2. + side * 0.2,
                            side * 0.6,
                            side * 0.6,
                        ],
                        m.theme.accent,
                        None,
                    );
                }
                if w > side + 12. * scale {
                    label = Some((text, x + side + 12. * scale, y, w - side - 12. * scale, h));
                }
            }
        }
        MenuContent::Range {
            label: text,
            binding,
            min,
            max,
            thumb_width,
            track_asset,
            thumb_asset,
            ..
        } => {
            let value = binding
                .value(&m.menu_locals, &m.prefs)
                .unwrap_or(*min)
                .clamp(*min, *max);
            let tw = thumb_width * scale;
            let fraction = (value - min) / (max - min);
            let cx = x + tw / 2. + fraction * (w - tw);
            if let Some(asset) = track_asset {
                paint(packet, quad.rect, [1.; 4], Some(asset.clone()));
            } else {
                paint(
                    packet,
                    [x + tw / 2., y + h * 0.65, w - tw, (4. * scale).max(1.)],
                    m.theme.panel,
                    None,
                );
                paint(
                    packet,
                    [
                        x + tw / 2.,
                        y + h * 0.65,
                        fraction * (w - tw),
                        (4. * scale).max(1.),
                    ],
                    m.theme.accent,
                    None,
                );
                label = Some((text, x, y, w, h * 0.45));
            }
            if let Some(asset) = thumb_asset {
                paint(
                    packet,
                    [cx - tw / 2., y, tw, h],
                    [1.; 4],
                    Some(asset.clone()),
                );
            } else {
                paint(
                    packet,
                    [cx - tw / 2., y + h * 0.4, tw, h * 0.5],
                    m.theme.accent,
                    None,
                );
            }
        }
        _ => {}
    }
    if let Some((label, x, y, w, h)) = label {
        packet.menu_paint.push(MenuPaint::Text(packet.texts.len()));
        let mut color = m.theme.text;
        color[3] *= alpha;
        packet.text(label, x, y, w, 16. * scale * m.prefs.font_scale, color);
        let run = packet.texts.last_mut().unwrap();
        run.height = h;
        run.clip = quad.clip;
        run.locale = m.text_locale.clone();
        run.font_assets = m.text_fonts.clone();
        run.font_plan_digest = m.text_font_plan_digest.clone();
    }
}
fn menu_node(
    e: &MenuElement,
    position: [f32; 2],
    m: &UiModel,
    asset: Option<String>,
    color: [f32; 4],
) -> Node {
    let anim = m
        .menu_element_animations
        .get(&e.id)
        .copied()
        .unwrap_or_default();
    Node {
        id: e.id.clone(),
        parent: e.parent.clone(),
        asset,
        x: position[0] + anim.offset[0],
        y: position[1] + anim.offset[1],
        width: if e.content.is_group() { 0. } else { e.rect[2] },
        height: if e.content.is_group() { 0. } else { e.rect[3] },
        scale: anim.scale.unwrap_or(e.scale),
        opacity: anim.opacity.unwrap_or(e.opacity),
        color,
        order: 0,
        clip: e.clip,
        timeline_binding: None,
        inherit_existence: false,
        sprite_transform: None,
        bitmap_text: None,
        preserve_pose: vec![],
        offset: [0.; 2],
    }
}
fn menu_elements(packet: &mut DrawPacket, menu: &ImageMenu, m: &UiModel, messages: &Messages) {
    let capacities = packet.menu_history_capacity.clone();
    let positions = menu.element_positions(
        &m.menu_locals,
        &m.profile,
        &m.menu_reading_modes,
        &m.menu_story,
        m.history_total > 0,
    );
    let enabled = |e: &MenuElement| {
        menu.element_state(
            &e.id,
            &m.menu_locals,
            &m.profile,
            &m.menu_reading_modes,
            &m.menu_story,
            m.history_total > 0,
        )
        .1 && e
            .control()
            .is_none_or(|(_, action, _)| menu_service_enabled(action, m, &capacities))
    };
    let nodes: Vec<_> = menu
        .elements
        .iter()
        .filter(|e| {
            menu.element_state(
                &e.id,
                &m.menu_locals,
                &m.profile,
                &m.menu_reading_modes,
                &m.menu_story,
                m.history_total > 0,
            )
            .0
        })
        .map(|e| {
            let mut color = [1.; 4];
            let asset = match &e.content {
                MenuContent::Image { asset } => Some(asset.clone()),
                MenuContent::Button {
                    asset,
                    hover_asset,
                    locked_asset,
                    ..
                } => {
                    let selected = m.hovered_image.as_deref() == Some(e.id.as_str());
                    if !enabled(e) && locked_asset.is_none() {
                        color = [0.35, 0.35, 0.35, 1.];
                    }
                    Some(
                        if !enabled(e) {
                            locked_asset.as_ref()
                        } else if selected {
                            hover_asset.as_ref()
                        } else {
                            None
                        }
                        .unwrap_or(asset)
                        .clone(),
                    )
                }
                MenuContent::TextButton {
                    color: normal,
                    hover_color,
                    disabled_color,
                    ..
                } => {
                    color = if !enabled(e) {
                        *disabled_color
                    } else if m.hovered_image.as_deref() == Some(e.id.as_str()) {
                        *hover_color
                    } else {
                        *normal
                    };
                    None
                }
                MenuContent::Text { color: c, .. }
                | MenuContent::HistoryWindow { color: c, .. }
                | MenuContent::HistoryFlow { color: c, .. } => {
                    color = *c;
                    None
                }
                _ => None,
            };
            menu_node(e, positions[&e.id], m, asset, color)
        })
        .collect();
    let start = packet.quads.len();
    for (order, (id, quad)) in scene_layout(packet, &nodes, m.stage, 1.)
        .into_iter()
        .enumerate()
    {
        let e = menu.elements.iter().find(|e| e.id == id).unwrap();
        let [x, y, w, h] = quad.rect;
        match &e.content {
            MenuContent::Toggle { .. } | MenuContent::Range { .. } => {
                menu_value_visual(packet, e, &quad, m, enabled(e))
            }
            MenuContent::HistoryScrollbar { .. } => {
                packet.history_bar_view = Some(scrollbar::BarView {
                    element: e.clone(),
                    quad: quad.clone(),
                    enabled: enabled(e),
                    order,
                    paint_index: packet.menu_paint.len(),
                    semantic_index: packet.semantics.len(),
                    base_id: 65536
                        + (menu.buttons.len()
                            + menu.elements.iter().position(|v| v.id == e.id).unwrap())
                            as u32
                            * 4,
                });
            }
            MenuContent::HistoryFlow {
                voice_controls,
                size,
                line_height,
                gap,
                wheel_step,
                page_step,
                max_visible,
                ..
            } => {
                let scale = w / e.rect[2];
                packet.history_flow = Some(reading::HistoryFlowView {
                    id: e.id.clone(),
                    order,
                    rect: quad.rect,
                    clip: quad.clip,
                    color: quad.color,
                    style: history::HistoryStyle {
                        width: w,
                        height: h,
                        size: size * scale * m.prefs.font_scale,
                        line_height: line_height * scale * m.prefs.font_scale,
                        gap: gap * scale,
                    },
                    wheel_step: wheel_step * scale * m.prefs.font_scale,
                    page_step: page_step * scale,
                    max_visible: *max_visible as usize,
                    paint_index: packet.menu_paint.len(),
                    semantic_index: packet.semantics.len(),
                    enabled: enabled(e),
                    voice_controls: *voice_controls,
                    voice_base: 131072
                        + menu.elements.iter().position(|v| v.id == e.id).unwrap() as u32 * 1024,
                });
            }
            MenuContent::HistoryWindow {
                row_height,
                size,
                voice_controls,
                limit,
                ..
            } => {
                let scale = w / e.rect[2];
                let viewport = [x, y, w, h];
                let parent = quad.clip.unwrap_or(viewport);
                let left = x.max(parent[0]).max(0.);
                let top = y.max(parent[1]).max(0.);
                let right = (x + w).min(parent[0] + parent[2]).min(packet.width);
                let bottom = (y + h).min(parent[1] + parent[3]).min(packet.height);
                if let Some(rows) = m.menu_history.get(&e.id) {
                    let row_height = (row_height * scale).max(if *voice_controls {
                        if w >= 280. {
                            52.
                        } else {
                            76.
                        }
                    } else {
                        0.
                    });
                    let capacity = if *voice_controls {
                        (((bottom - top).max(0.) / row_height).floor() as usize)
                            .clamp(1, *limit as usize)
                    } else {
                        *limit as usize
                    };
                    if *voice_controls {
                        packet
                            .menu_history_capacity
                            .insert(e.id.clone(), (m.menu_instance, capacity));
                    }
                    let skip = if *voice_controls {
                        rows.len().saturating_sub(capacity)
                    } else {
                        0
                    };
                    for (index, row) in rows.iter().skip(skip).enumerate() {
                        let ry =
                            (if *voice_controls { top } else { y }) + index as f32 * row_height;
                        let rh = row_height.min(bottom - ry);
                        if right <= left || rh <= 0. || ry + rh <= top {
                            continue;
                        }
                        let entry = &row.entry;
                        let (text_width, voice_top, voice_rect) =
                            history_controls::voice_layout(w, *voice_controls, entry.voice_count);
                        let heading_height = if let Some(choice) = entry.choice {
                            packet.menu_paint.push(MenuPaint::Text(packet.texts.len()));
                            packet.text(
                                messages.text(&m.ui_locale, choice.message()),
                                x,
                                ry + voice_top,
                                text_width,
                                size * scale * m.prefs.font_scale * 0.8,
                                quad.color,
                            );
                            let heading = packet.texts.last_mut().unwrap();
                            let heading_height = heading.line_height.min(rh);
                            heading.height = heading_height;
                            heading.clip = Some([
                                left,
                                top.max(ry),
                                right - left,
                                (ry + heading_height - top.max(ry)).max(0.),
                            ]);
                            heading_height
                        } else {
                            0.
                        };
                        let text = if entry.speaker.is_empty() {
                            entry.text.clone()
                        } else {
                            format!("{}  /  {}", entry.speaker, entry.text)
                        };
                        packet.menu_paint.push(MenuPaint::Text(packet.texts.len()));
                        packet.text(
                            text,
                            x,
                            ry + voice_top + heading_height,
                            text_width,
                            size * scale * m.prefs.font_scale,
                            quad.color,
                        );
                        let run = packet.texts.last_mut().unwrap();
                        run.height = (rh - voice_top - heading_height).max(1.);
                        run.clip = Some([
                            left,
                            top.max(ry + voice_top + heading_height),
                            ((x + text_width).min(right) - left).max(0.),
                            (ry + rh - top.max(ry + voice_top + heading_height)).max(0.),
                        ]);
                        run.locale = entry.locale.clone();
                        run.font_assets = entry.font_assets.clone();
                        run.font_plan_digest = entry.font_plan_digest.clone();
                        run.images = entry
                            .images
                            .iter()
                            .cloned()
                            .map(|mut i| {
                                if !entry.speaker.is_empty() {
                                    i.offset += (entry.speaker.len() + 5) as u32;
                                }
                                i
                            })
                            .collect();
                        run.image_scale = scale;
                        if let Some(mut rect) = voice_rect {
                            rect[0] += x;
                            rect[1] += ry;
                            history_controls::button(
                                packet,
                                m,
                                messages,
                                history_controls::VoiceButton {
                                    window: &e.id,
                                    layout: None,
                                    key: row.key,
                                    id: 131072
                                        + menu.elements.iter().position(|v| v.id == e.id).unwrap()
                                            as u32
                                            * 1024
                                        + row.key as u32,
                                    rect,
                                    clip: [left, top, right - left, (bottom - top).max(0.)],
                                    enabled: enabled(e),
                                    alpha: quad.color[3],
                                },
                            );
                        }
                    }
                }
            }
            MenuContent::Image { .. } | MenuContent::Button { .. } => {
                packet.menu_paint.push(MenuPaint::Quad(packet.quads.len()));
                packet.quads.push(quad.clone());
            }
            MenuContent::Text { text, size, .. }
            | MenuContent::TextButton {
                label: text, size, ..
            } => {
                let scale = w / e.rect[2];
                packet.menu_paint.push(MenuPaint::Text(packet.texts.len()));
                let bound = e
                    .text_local
                    .as_ref()
                    .and_then(|name| m.menu_locals.get(name))
                    .map(MenuValue::display)
                    .or_else(|| {
                        e.text_preference
                            .map(|field| format!("{:.2}", field.value(&m.prefs)))
                    })
                    .or_else(|| {
                        e.text_slot
                            .as_ref()
                            .and_then(|slot| slot.resolve(&m.menu_locals))
                            .and_then(|slot| m.slots.iter().find(|row| row.slot == slot))
                            .filter(|row| row.exists || row.error.is_some())
                            .map(|row| {
                                if row.error.is_some() {
                                    messages.text(&m.ui_locale, "unavailable-slot")
                                } else {
                                    row.label.clone()
                                }
                            })
                    });
                packet.text(
                    bound.as_deref().unwrap_or(text),
                    x,
                    y,
                    w,
                    size * scale * m.prefs.font_scale,
                    quad.color,
                );
                let run = packet.texts.last_mut().unwrap();
                run.height = h;
                run.clip = quad.clip;
                let unavailable_slot = e
                    .text_local
                    .as_ref()
                    .and_then(|name| m.menu_locals.get(name))
                    .is_none()
                    && e.text_preference.is_none()
                    && e.text_slot
                        .as_ref()
                        .and_then(|slot| slot.resolve(&m.menu_locals))
                        .and_then(|slot| m.slots.iter().find(|row| row.slot == slot))
                        .is_some_and(|row| row.error.is_some());
                if unavailable_slot {
                    run.locale = m.ui_locale.clone();
                    run.font_assets = m.ui_fonts.clone();
                    run.font_plan_digest = m.ui_font_plan_digest.clone();
                } else {
                    run.locale = m.text_locale.clone();
                    run.font_assets = m.text_fonts.clone();
                    run.font_plan_digest = m.text_font_plan_digest.clone();
                }
            }
            _ => {}
        }
        if let MenuContent::Button { label, .. }
        | MenuContent::TextButton { label, .. }
        | MenuContent::HitRegion { label, .. }
        | MenuContent::Toggle { label, .. }
        | MenuContent::Range { label, .. } = &e.content
        {
            let clip = quad.clip.unwrap_or([0., 0., packet.width, packet.height]);
            let left = x.max(clip[0]).max(0.);
            let top = y.max(clip[1]).max(0.);
            let right = (x + w).min(clip[0] + clip[2]).min(packet.width);
            let bottom = (y + h).min(clip[1] + clip[3]).min(packet.height);
            if right > left && bottom > top {
                // Full declaration index remains stable when siblings become hidden.
                let id = (menu.buttons.len()
                    + menu
                        .elements
                        .iter()
                        .position(|item| item.id == e.id)
                        .unwrap()) as u32;
                packet.menu_controls.insert(id, e.id.clone());
                let (value, action) = match &e.content {
                    MenuContent::Toggle { binding, .. } => {
                        let checked = binding.value(&m.menu_locals, &m.prefs).unwrap_or(false);
                        (
                            Some(SemanticValue::Toggle { checked }),
                            UiAction::MenuValue {
                                instance: m.menu_instance,
                                revision: m.menu_revision,
                                control: e.id.clone(),
                                value: MenuValueInput::Bool(!checked),
                            },
                        )
                    }
                    MenuContent::Range {
                        binding,
                        min,
                        max,
                        step,
                        thumb_width,
                        ..
                    } => {
                        let value = binding
                            .value(&m.menu_locals, &m.prefs)
                            .unwrap_or(*min)
                            .clamp(*min, *max);
                        let half = thumb_width * w / e.rect[2] / 2.;
                        (
                            Some(SemanticValue::Range {
                                min: *min,
                                max: *max,
                                step: *step,
                                value,
                                from: x + half,
                                to: x + w - half,
                            }),
                            UiAction::MenuValue {
                                instance: m.menu_instance,
                                revision: m.menu_revision,
                                control: e.id.clone(),
                                value: MenuValueInput::Number(value),
                            },
                        )
                    }
                    _ => (
                        None,
                        UiAction::MenuControl {
                            instance: m.menu_instance,
                            revision: m.menu_revision,
                            control: e.id.clone(),
                        },
                    ),
                };
                packet.semantics.push(SemanticNode {
                    value,
                    id,
                    label: label.clone(),
                    action,
                    enabled: enabled(e),
                    rect: [left, top, right - left, bottom - top],
                    locale: m.text_locale.clone(),
                });
            }
        }
    }
    if !packet.menu_paint.is_empty()
        || packet.history_flow.is_some()
        || packet.history_bar_view.is_some()
    {
        packet.menu_quad_range = Some((start, packet.quads.len()));
    }
}

pub fn project(m: &UiModel, width: f32, height: f32, messages: &Messages) -> DrawPacket {
    let mut p = project_measured(
        m,
        width,
        height,
        messages,
        &[],
        0.,
        &PanelOffsets::default(),
    );
    divert_menu_page(&mut p, m);
    apply_dialogue_offset(&mut p, m);
    p
}

fn apply_dialogue_offset(packet: &mut DrawPacket, model: &UiModel) {
    let offset = model.dialogue_appearance.text_offset;
    if model.screen != Screen::Story || offset == [0.; 2] {
        return;
    }
    let scale = (packet.width / model.stage[0]).min(packet.height / model.stage[1]);
    for run in packet
        .texts
        .iter_mut()
        .filter(|run| run.region == Some(ScrollRegion::Dialogue))
    {
        // The surface is stationary. Only content moves inside its original
        // clip; scrolling, shaping, controls and the background keep geometry.
        run.clip
            .get_or_insert([run.x, run.y, run.width, run.height]);
        run.x += offset[0] * scale;
        run.y += offset[1] * scale;
    }
}

/// Diverts the authored menu page into the offscreen page root while a
/// spatial reveal is in flight. Must run after history flow/bar projection:
/// the bar splices four quads inside the page span, so splitting earlier
/// would invalidate its index fixups.
pub(crate) fn divert_menu_page(p: &mut DrawPacket, m: &UiModel) {
    let Some((style, to_visible, progress)) = m.menu_transition.as_ref() else {
        return;
    };
    let Some((start, end)) = p.menu_page_range else {
        return;
    };
    // Page texts are the branch's own runs plus every page-marked text
    // appended after it (history flow rows).
    let mut texts: Vec<usize> = p
        .menu_page_texts
        .map(|(from, to)| from..to)
        .into_iter()
        .flatten()
        .collect();
    texts.extend(p.menu_paint.iter().filter_map(|paint| match paint {
        MenuPaint::Text(i) => Some(*i),
        _ => None,
    }));
    // Authored texts are already in the branch range; dynamic flow texts
    // arrive through menu_paint. Preserve order without painting either twice.
    let mut seen = std::collections::BTreeSet::new();
    texts.retain(|i| seen.insert(*i));
    let tail = p.quads.split_off(end);
    let quads = p.quads.split_off(start);
    p.quads.push(Quad {
        corners: None,
        rect: [0., 0., p.width, p.height],
        color: [1., 1., 1., 1.],
        asset: Some("@menu".into()),
        clip: None,
    });
    let position = p.quads.len() - 1;
    p.quads.extend(tail);
    // Quads after the page shift down by the page length minus the sentinel.
    let shift = end - start - 1;
    if let Some(index) = &mut p.dialogue_hint_quad {
        if *index >= end {
            *index -= shift;
        }
    }
    // Interleaving is moot while the page lives in its own root.
    p.menu_paint.clear();
    p.menu_quad_range = None;
    p.menu_page_range = None;
    p.menu_page_texts = None;
    p.menu_layers = Some(MenuLayers {
        quads,
        texts,
        style: style.clone(),
        to_visible: *to_visible,
        progress: *progress,
        position,
    });
}
#[derive(Debug, Default)]
struct PanelOffsets {
    menu: f32,
    saves: f32,
    settings: f32,
}

// Paint and hit testing share the viewport. Partially visible controls may be
// painted as context, but cannot be activated until their whole target fits.
// A narrow settings row puts its value above the controls so neither the
// value nor the hit target is squeezed to make room for the other.
fn setting_range(
    p: &mut DrawPacket,
    label: String,
    actions: [UiAction; 2],
    labels: [String; 2],
    [x, y, w]: [f32; 3],
    theme: &Theme,
) -> f32 {
    let stacked = w < 360.;
    p.text(
        label,
        x,
        y,
        if stacked { w } else { w - 145. },
        18.,
        theme.text,
    );
    let button_width = if stacked { (w - 12.) / 2. } else { 58. };
    let button_y = y + if stacked { 36. } else { -7. };
    let button_x = if stacked { x } else { x + w - 128. };
    for (i, (action, label)) in actions.into_iter().zip(labels).enumerate() {
        p.button(
            label,
            action,
            [
                button_x + i as f32 * (button_width + 12.),
                button_y,
                button_width,
                44.,
            ],
            false,
            theme,
        );
    }
    if stacked {
        92.
    } else {
        56.
    }
}

fn scroll_panel(
    p: &mut DrawPacket,
    first: [usize; 3],
    view: ScrollView,
    m: &UiModel,
    messages: &Messages,
) {
    let [x, y, w, h] = view.rect;
    for q in &mut p.quads[first[0]..] {
        q.rect[1] -= view.offset;
        q.clip = Some(view.rect);
    }
    for r in &mut p.texts[first[1]..] {
        r.y -= view.offset;
        r.clip = r
            .clip
            .map(|[rx, ry, rw, rh]| {
                let top = (ry - view.offset).max(y);
                let bottom = (ry - view.offset + rh).min(y + h);
                [
                    rx.max(x),
                    top,
                    ((rx + rw).min(x + w) - rx.max(x)).max(0.),
                    (bottom - top).max(0.),
                ]
            })
            .or(Some(view.rect));
    }
    for n in &mut p.semantics[first[2]..] {
        let top = n.rect[1] - view.offset;
        let bottom = top + n.rect[3];
        n.enabled &=
            top >= y && bottom <= y + h && n.rect[0] >= x && n.rect[0] + n.rect[2] <= x + w;
        n.rect[1] = top.clamp(y, y + h);
        n.rect[3] = (bottom.min(y + h) - n.rect[1]).max(0.);
    }
    if view.max > 0. {
        reading::controls(p, view, m, messages);
    }
}

fn project_measured(
    m: &UiModel,
    width: f32,
    height: f32,
    messages: &Messages,
    choice_heights: &[f32],
    choice_offset: f32,
    panels: &PanelOffsets,
) -> DrawPacket {
    let mut p = DrawPacket {
        width,
        height,
        stage_size: [m.stage[0] as u32, m.stage[1] as u32],
        locale: m.ui_locale.clone(),
        font_assets: m.ui_fonts.clone(),
        font_plan_digest: m.ui_font_plan_digest.clone(),
        ..Default::default()
    };
    let t = &m.theme;
    let msg = |id| messages.text(&m.ui_locale, id);
    let narrow = width < 650.;
    let margin = if narrow { 20. } else { 48. };
    p.rect([0., 0., width, height], t.background);
    if let Some((src, progress)) = &m.transition {
        let mut a = DrawPacket {
            width,
            height,
            ..Default::default()
        };
        let mut b = DrawPacket {
            width,
            height,
            ..Default::default()
        };
        scene(&mut a, src, m.stage, 1.);
        scene(&mut b, &m.nodes, m.stage, 1.);
        p.transition_layers = Some((a.quads, b.quads, *progress));
        p.transition_style = m.transition_style.clone();
        p.quads.push(Quad {
            corners: None,
            rect: [0., 0., width, height],
            color: [1., 1., 1., 1.],
            asset: Some("@transition".into()),
            clip: None,
        });
    } else {
        scene(&mut p, &m.nodes, m.stage, 1.);
    }
    match m.screen {
        Screen::Title | Screen::Menu if m.authored_menu => {
            let menu = &m.theme.image_menus[&m.image_menu];
            p.menu_history_capacity = history_controls::window_capacities(&p, menu, m);
            let scale = (width / m.stage[0]).min(height / m.stage[1]);
            let ox = (width - m.stage[0] * scale) / 2.;
            let oy = (height - m.stage[1] * scale) / 2.;
            // The fade owns everything this branch paints, never the story
            // scene already projected underneath the overlay.
            let fade_quads = p.quads.len();
            let fade_texts = p.texts.len();
            let fade_flows = p.history_flow.is_some();
            let fade_bars = p.history_bar_view.is_some();
            p.quads.push(Quad {
                corners: None,
                rect: [ox, oy, m.stage[0] * scale, m.stage[1] * scale],
                color: [1.; 4],
                asset: Some(menu.background.clone()),
                clip: None,
            });
            for button in &menu.buttons {
                let enabled = button
                    .requires
                    .as_ref()
                    .is_none_or(|key| m.profile.contains(key))
                    && menu_service_enabled(&button.action, m, &p.menu_history_capacity);
                let selected = m.hovered_image.as_deref() == Some(button.id.as_str());
                let asset = if !enabled {
                    button.locked_asset.as_ref()
                } else if selected {
                    button.hover_asset.as_ref()
                } else {
                    None
                }
                .unwrap_or(&button.asset);
                let r = button.rect;
                let rect = [
                    ox + r[0] * scale,
                    oy + r[1] * scale,
                    r[2] * scale,
                    r[3] * scale,
                ];
                let dim = if !enabled && button.locked_asset.is_none() {
                    0.35
                } else {
                    1.
                };
                p.quads.push(Quad {
                    corners: None,
                    rect,
                    color: [dim, dim, dim, 1.],
                    asset: Some(asset.clone()),
                    clip: None,
                });
                let id = p
                    .semantics
                    .iter()
                    .map(|n| n.id)
                    .max()
                    .map_or(0, |id| id + 1);
                p.menu_controls.insert(id, button.id.clone());
                p.semantics.push(SemanticNode {
                    value: None,
                    id,
                    label: button.label.clone(),
                    action: if menu.uses_state() || menu.uses_services() || m.screen == Screen::Menu
                    {
                        UiAction::MenuControl {
                            instance: m.menu_instance,
                            revision: m.menu_revision,
                            control: button.id.clone(),
                        }
                    } else {
                        button
                            .action
                            .ui_action()
                            .expect("validated legacy menu action")
                    },
                    enabled,
                    rect,
                    locale: m.text_locale.clone(),
                });
            }
            menu_elements(&mut p, menu, m, messages);
            if menu.builtin_navigation {
                p.button(
                    msg(if m.menu_depth > 0 {
                        "back"
                    } else if m.screen == Screen::Menu {
                        "close"
                    } else {
                        "menu"
                    }),
                    if m.screen == Screen::Menu || m.menu_depth > 0 {
                        UiAction::Close
                    } else {
                        UiAction::Menu
                    },
                    [width - 104., 12., 92., 36.],
                    false,
                    t,
                );
            }
            if menu.builtin_navigation && m.screen == Screen::Title && m.image_menu != "title" {
                p.button(
                    msg("exit"),
                    UiAction::Title,
                    [width - 180., height - 54., 168., 42.],
                    false,
                    t,
                );
            }
            // The page owns everything this branch paints, including builtin
            // navigation; a spatial reveal diverts exactly this span.
            p.menu_page_range = Some((fade_quads, p.quads.len()));
            p.menu_page_texts = Some((fade_texts, p.texts.len()));
            let opacity = m.menu_opacity.clamp(0., 1.);
            if opacity < 1. {
                for quad in &mut p.quads[fade_quads..] {
                    quad.color[3] *= opacity;
                }
                for text in &mut p.texts[fade_texts..] {
                    text.color[3] *= opacity;
                }
                if fade_flows {
                    p.history_flow.as_mut().unwrap().color[3] *= opacity;
                }
                if fade_bars {
                    p.history_bar_view.as_mut().unwrap().quad.color[3] *= opacity;
                }
            }
        }
        Screen::Title => {
            p.rect([0., 0., width, height], [0.015, 0.035, 0.04, 0.57]);
            let w = width - 2. * margin;
            let stacked = w < 280.;
            let tiny = stacked && height < 280.;
            if !tiny {
                p.rect([margin, 32., 28., 2.], t.accent);
                p.text(
                    if w < 360. {
                        "N I R / STORIES"
                    } else {
                        "N I R   /   I N T E R A C T I V E   S T O R I E S"
                    },
                    margin + 42.,
                    22.,
                    width - margin * 2. - 42.,
                    11.,
                    t.muted,
                );
            }
            let short = height < 400.;
            let y = if tiny {
                8.
            } else if short {
                50.
            } else {
                (height * 0.36).max(120.)
            };
            let controls_height = if stacked { 162. } else { 110. };
            let by = (y + 184.).min(height - controls_height - if tiny { 12. } else { 42. });
            let title = if m.ui_locale == "zh-Hans" {
                m.title.split('·').next().unwrap_or(&m.title).trim()
            } else {
                m.title.split('·').nth(1).unwrap_or(&m.title).trim()
            };
            let title_height = (by - y - 12.).clamp(0., 114.);
            let size = (if short {
                32_f32
            } else if narrow {
                48.
            } else {
                76.
            })
            .min((title_height / 1.5).max(1.));
            p.text(title, margin, y, w, size, t.text);
            p.texts.last_mut().unwrap().height = title_height;
            p.texts.last_mut().unwrap().clip = Some([margin, y, w, title_height]);
            if !short && y + 132. <= by - 12. {
                p.text("NIR / VISUAL NOVEL", margin, y + 110., w, 13., t.accent);
            }
            p.button(
                msg("new-game"),
                UiAction::NewGame,
                [margin, by, if narrow { w } else { 240. }, 54.],
                true,
                t,
            );
            p.button(
                msg("saves"),
                UiAction::Saves,
                [margin, by + 66., if stacked { w } else { 150. }, 44.],
                false,
                t,
            );
            p.button(
                msg("settings"),
                UiAction::Settings,
                [
                    if stacked { margin } else { margin + 162. },
                    by + if stacked { 118. } else { 66. },
                    if stacked { w } else { 110. },
                    44.,
                ],
                false,
                t,
            );
            if !tiny {
                p.text(
                    "01   /   WEB EDITION",
                    margin,
                    height - 34.,
                    w.min(250.),
                    11.,
                    t.muted,
                );
            }
            if !narrow {
                p.text(
                    msg("title-hint"),
                    width - 365.,
                    height - 34.,
                    330.,
                    11.,
                    t.muted,
                );
            }
        }
        Screen::Story if m.interface_hidden => {
            p.announcement = msg("show-interface");
            p.semantics.push(SemanticNode {
                value: None,
                id: 0,
                label: msg("show-interface"),
                action: UiAction::RestoreInterface,
                enabled: true,
                rect: [0., 0., width, height],
                locale: m.ui_locale.clone(),
            });
        }
        Screen::Story => {
            let toolbar_bottom = 16. + if narrow { 52. } else { 0. } + 44.;
            if m.hidden_dialogue || m.advance_wait {
                p.semantics.push(SemanticNode {
                    value: None,
                    id: p.semantics.len() as u32,
                    label: msg("continue"),
                    action: UiAction::Advance,
                    enabled: true,
                    rect: [0., 0., width, height],
                    locale: m.ui_locale.clone(),
                });
            }
            if let Some(d) = &m.dialogue {
                let first_quad = p.quads.len();
                let first_text = p.texts.len();
                let mut h = if narrow {
                    (height * 0.40).max(t.dialogue.height).min(330.)
                } else {
                    t.dialogue.height
                }
                .min((height - 88.).max(80.));
                // Builtin windows share the viewport with the fixed reader
                // toolbar. Compact layouts keep a name, one text line and an
                // overflow-control row visible without reducing the font.
                let compact = height < 320. && t.dialogue.rect.is_none();
                let minimum_top = toolbar_bottom + if narrow && !compact { 8. } else { 4. };
                let bottom = height - if compact { 4. } else { margin * 0.6 };
                if t.dialogue.rect.is_none() {
                    h = h.min((bottom - minimum_top).max(0.));
                }
                let mut top = match t.slots.dialogue {
                    DialogueComponent::Bottom => bottom - h,
                    DialogueComponent::Top => minimum_top,
                };
                let mut left = margin;
                let mut box_width = width - margin * 2.;
                let mut scale = 1.;
                if let Some(rect) = t.dialogue.rect {
                    scale = (width / m.stage[0]).min(height / m.stage[1]);
                    left = (width - m.stage[0] * scale) / 2. + rect[0] * scale;
                    top = (height - m.stage[1] * scale) / 2. + rect[1] * scale;
                    box_width = rect[2] * scale;
                    h = rect[3] * scale;
                }
                let padding = t.dialogue.padding * scale;
                if let Some(asset) = &t.dialogue.background {
                    p.quads.push(Quad {
                        corners: None,
                        rect: [left, top, box_width, h],
                        color: [1., 1., 1., t.dialogue.opacity],
                        asset: Some(asset.clone()),
                        clip: None,
                    });
                } else {
                    let mut panel = t.panel;
                    panel[3] *= t.dialogue.opacity;
                    p.rect([left, top, box_width, h], panel);
                    let mut accent = t.accent;
                    accent[3] *= t.dialogue.opacity;
                    p.rect([left, top, 3., h], accent);
                }
                if !d.speaker.is_empty() {
                    p.text(
                        &d.speaker,
                        left + padding,
                        top + if compact { 4. } else { 18. },
                        box_width - padding * 2.,
                        16.,
                        t.accent,
                    );
                }
                let ty = top
                    + if t.dialogue.rect.is_some() {
                        padding
                    } else if compact {
                        if d.speaker.is_empty() {
                            6.
                        } else {
                            30.
                        }
                    } else if d.speaker.is_empty() {
                        24.
                    } else {
                        52.
                    };
                let size = (t.dialogue.font_size
                    - if narrow && t.dialogue.rect.is_none() {
                        4.
                    } else {
                        0.
                    })
                    * m.prefs.font_scale
                    * scale;
                p.texts.push(TextRun {
                    text: d.full_text.clone(),
                    visible: Some(d.visible_text.len()),
                    x: left + padding,
                    y: ty,
                    width: box_width - padding * 2.,
                    height: (top + h
                        - if t.dialogue.rect.is_some() {
                            padding
                        } else {
                            43.
                        }
                        - ty)
                        .max(30.),
                    size,
                    line_height: size * t.dialogue.line_height,
                    color: t.text,
                    emphasis: d.emphasis.clone(),
                    images: d.images.clone(),
                    image_scale: scale,
                    scroll: 0.,
                    clip: None,
                    region: Some(ScrollRegion::Dialogue),
                    locale: d.locale.clone(),
                    font_assets: d.font_assets.clone(),
                    font_plan_digest: d.font_plan_digest.clone(),
                    preflight_only: false,
                    shadow: None,
                    monochrome: false,
                });
                p.texts.last_mut().unwrap().shadow = t.dialogue.shadow.map(|mut shadow| {
                    shadow.offset[0] *= scale;
                    shadow.offset[1] *= scale;
                    shadow
                });
                if let Some(rect) = t.dialogue.text_rect {
                    let run = p.texts.last_mut().unwrap();
                    run.x = (width - m.stage[0] * scale) / 2. + rect[0] * scale;
                    run.y = (height - m.stage[1] * scale) / 2. + rect[1] * scale;
                    run.width = rect[2] * scale;
                    run.height = rect[3] * scale;
                }
                if !d.ruby.is_empty() {
                    let run = p.texts.last_mut().unwrap();
                    let annotation_height = run.size * 0.6;
                    run.y += annotation_height;
                    run.height = (run.height - annotation_height).max(0.);
                    run.line_height = run.line_height.max(run.size * 1.6);
                }
                p.announcement = d.full_text.clone();
                p.announcement_locale = d.locale.clone();
                {
                    let (hint_x, hint_y) = if t.dialogue.rect.is_some() {
                        let hx = (left + box_width - 130.).clamp(4., (width - 120.).max(4.));
                        let hy = if top + h + 24. <= height {
                            top + h + 4.
                        } else if top >= 24. {
                            top - 24.
                        } else {
                            (height - 22.).max(0.)
                        };
                        p.dialogue_hint_quad = Some(p.quads.len());
                        p.rect([hx - 4., hy - 3., 118., 22.], t.panel);
                        (hx, hy)
                    } else {
                        (width - margin - 130., top + h - 33.)
                    };
                    p.dialogue_hint = Some(p.texts.len());
                    p.text(
                        msg(if d.gate {
                            "gate-hint"
                        } else if d.ready {
                            "advance-hint"
                        } else {
                            "reveal-hint"
                        }),
                        hint_x,
                        hint_y,
                        110.,
                        11.,
                        t.muted,
                    );
                }
                let appearance = m.dialogue_appearance;
                for quad in &mut p.quads[first_quad..] {
                    quad.color[3] *= appearance.opacity * appearance.background_opacity;
                }
                for text in &mut p.texts[first_text..] {
                    text.color[3] *= appearance.opacity * appearance.text_opacity;
                }
                // Fixed stage geometry belongs to the dialogue visibility
                // root, independent of text scrolling and font preferences.
                let scale = (width / m.stage[0]).min(height / m.stage[1]);
                let origin = [
                    (width - m.stage[0] * scale) / 2.,
                    (height - m.stage[1] * scale) / 2.,
                ];
                for image in m.dialogue_decorations.values() {
                    p.quads.push(Quad {
                        corners: None,
                        rect: [
                            origin[0] + image.rect[0] * scale,
                            origin[1] + image.rect[1] * scale,
                            image.rect[2] * scale,
                            image.rect[3] * scale,
                        ],
                        color: [1., 1., 1., appearance.opacity],
                        asset: Some(image.asset.clone()),
                        clip: None,
                    });
                }
                match m.window_transition.as_ref() {
                    // Uniform coverage is the dissolve ramp: fold it into the
                    // existing per-item opacity multiply.
                    Some(w) if matches!(w.style, StageTransition::Dissolve) => {
                        let coverage = if w.to_visible {
                            w.progress
                        } else {
                            1. - w.progress
                        };
                        for quad in &mut p.quads[first_quad..] {
                            quad.color[3] *= coverage;
                        }
                        for text in &mut p.texts[first_text..] {
                            text.color[3] *= coverage;
                        }
                    }
                    // Spatial coverage composites through the mix pass: divert
                    // the window quads into the offscreen root, leave a
                    // full-surface sentinel at their z-position. Window texts
                    // stay in the packet for layout; their glyph areas route
                    // to the window pass instead of the shared renderer.
                    Some(w) => {
                        let quads = p.quads.split_off(first_quad);
                        p.dialogue_hint_quad = None;
                        p.quads.push(Quad {
                            corners: None,
                            rect: [0., 0., width, height],
                            color: [1., 1., 1., 1.],
                            asset: Some("@window".into()),
                            clip: None,
                        });
                        p.window_layers = Some(WindowLayers {
                            quads,
                            texts: (first_text..p.texts.len()).collect(),
                            style: w.style.clone(),
                            to_visible: w.to_visible,
                            progress: w.progress,
                        });
                    }
                    None => {}
                }
                p.semantics.push(SemanticNode {
                    value: None,
                    id: 0,
                    label: msg("continue"),
                    action: UiAction::Advance,
                    enabled: !d.gate || m.advance_wait,
                    rect: [left, top, box_width, h],
                    locale: m.ui_locale.clone(),
                });
            }
            if !m.choices.is_empty() && m.choices.iter().all(|c| c.image.is_some()) {
                let scale = (width / m.stage[0]).min(height / m.stage[1]);
                let left = (width - m.stage[0] * scale) / 2.;
                let top = (height - m.stage[1] * scale) / 2.;
                for c in &m.choices {
                    let image = c.image.as_ref().unwrap();
                    let [x, y, w, h] = image.rect;
                    let rect = [left + x * scale, top + y * scale, w * scale, h * scale];
                    p.quads.push(Quad {
                        corners: None,
                        rect,
                        color: [1.; 4],
                        clip: None,
                        asset: Some(
                            if !c.enabled {
                                image.disabled_asset.as_ref().unwrap_or(&image.asset)
                            } else if c.selected
                                || m.hovered_image.as_deref() == Some(c.id.as_str())
                            {
                                image.hover_asset.as_ref().unwrap_or(&image.asset)
                            } else {
                                &image.asset
                            }
                            .clone(),
                        ),
                    });
                    p.semantics.push(SemanticNode {
                        value: None,
                        id: 0,
                        label: c.label.clone(),
                        action: UiAction::Choose {
                            option: c.id.clone(),
                        },
                        enabled: c.enabled,
                        rect,
                        locale: c.locale.clone(),
                    });
                }
                if m.choice_cancellable {
                    let w = 160f32.min((width - 40.).max(80.));
                    p.button(
                        msg("cancel"),
                        UiAction::CancelChoice,
                        [(width - w) / 2., (height - 112.).max(0.), w, 44.],
                        false,
                        t,
                    );
                }
            } else if !m.choices.is_empty() {
                let w = t.choice.width.min((width - 40.).max(80.));
                let gap = match t.slots.choice {
                    ChoiceComponent::Standard => 14.,
                    ChoiceComponent::Compact => 6.,
                };
                let heights: Vec<_> = m
                    .choices
                    .iter()
                    .enumerate()
                    .map(|(i, _)| {
                        choice_heights
                            .get(i)
                            .copied()
                            .unwrap_or(t.choice.item_height)
                    })
                    .collect();
                let total = heights.iter().sum::<f32>() + gap * (heights.len() - 1) as f32;
                let view_height = total.min((height - 176.).max(48.));
                let x = (width - w) / 2.;
                let y = ((height - view_height - 32.) / 2. - 40.).max(80.);
                let viewport = [x, y, w, view_height];
                let max = (total - view_height).max(0.);
                let offset = choice_offset.clamp(0., max);
                p.rect([x - 12., y - 16., w + 24., view_height + 32.], t.panel);
                let mut row_y = y - offset;
                for (c, h) in m.choices.iter().zip(heights) {
                    if row_y + h > y && row_y < y + view_height {
                        p.button(
                            c.label.clone(),
                            UiAction::Choose {
                                option: c.id.clone(),
                            },
                            [x, row_y, w, h],
                            c.selected,
                            t,
                        );
                        p.quads.last_mut().unwrap().clip = Some(viewport);
                        let text = p.texts.last_mut().unwrap();
                        text.size = 16. * m.prefs.font_scale;
                        text.line_height = text.size * 1.5;
                        text.clip = Some(viewport);
                        text.locale = c.locale.clone();
                        text.font_assets = c.font_assets.clone();
                        text.font_plan_digest = c.font_plan_digest.clone();
                        if !c.enabled {
                            text.color = t.muted;
                        }
                        let node = p.semantics.last_mut().unwrap();
                        node.enabled = c.enabled;
                        node.locale = c.locale.clone();
                        node.rect[1] = row_y.max(y);
                        node.rect[3] = (row_y + h).min(y + view_height) - node.rect[1];
                    }
                    row_y += h + gap;
                }
                if max > 0. {
                    reading::controls(
                        &mut p,
                        ScrollView {
                            menu: None,
                            region: ScrollRegion::Choices,
                            rect: viewport,
                            offset,
                            max,
                            step: (view_height - 32.).max(32.),
                        },
                        m,
                        messages,
                    );
                }
                if m.choice_cancellable {
                    p.button(
                        msg("cancel"),
                        UiAction::CancelChoice,
                        [x, y + view_height + 14., w, 36.],
                        false,
                        t,
                    );
                }
            }
            let items = [
                ("menu", UiAction::Menu),
                ("history", UiAction::History),
                ("auto", UiAction::ToggleAuto),
                ("skip", UiAction::ToggleSkip),
                ("hide-interface", UiAction::ToggleInterface),
            ];
            let item_count = items.len();
            let columns = if narrow { 3 } else { item_count };
            for (i, (label, action)) in items.into_iter().enumerate() {
                if m.menu_disabled && matches!(action, UiAction::Menu | UiAction::History) {
                    continue;
                }
                let row = i / columns;
                let column = i % columns;
                let row_count = (item_count - row * columns).min(columns);
                let w = ((width - margin * 2. - (row_count - 1) as f32 * 8.) / row_count as f32)
                    .min(115.);
                let full_label = msg(label);
                let painted_label = if label == "history" && w < 90. {
                    msg("history-compact")
                } else {
                    full_label.clone()
                };
                p.button(
                    painted_label,
                    action,
                    [
                        width
                            - margin
                            - (row_count - column) as f32 * w
                            - (row_count - column - 1) as f32 * 8.,
                        16. + row as f32 * 52.,
                        w,
                        44.,
                    ],
                    (label == "auto" && m.auto) || (label == "skip" && m.skip),
                    t,
                );
                p.semantics.last_mut().unwrap().label = full_label;
                if w < 90. {
                    let run = p.texts.last_mut().unwrap();
                    run.x -= 8.;
                    run.width = w - 16.;
                }
                if label == "hide-interface"
                    && (m.loading
                        || m.paused
                        || !m.choices.is_empty()
                        || (m.dialogue.is_none() && !m.hidden_dialogue))
                {
                    p.semantics.last_mut().unwrap().enabled = false;
                    p.texts.last_mut().unwrap().color = t.muted;
                }
            }
            if m.can_continue && !m.loading {
                p.button(
                    msg("continue"),
                    UiAction::Continue,
                    [(width - 240.) / 2., height * 0.33, 240., 52.],
                    true,
                    t,
                );
            }
        }
        Screen::Ended => {
            p.rect([0., 0., width, height], [0.02, 0.045, 0.05, 0.75]);
            p.text(
                msg("ending"),
                margin,
                height * 0.31,
                width - 2. * margin,
                38.,
                t.accent,
            );
            p.text(
                m.history
                    .last()
                    .map(|entry| entry.text.clone())
                    .unwrap_or_default(),
                margin,
                height * 0.31 + 64.,
                width - 2. * margin,
                20.,
                t.text,
            );
            p.button(
                msg("new-game"),
                UiAction::NewGame,
                [margin, height * 0.31 + 124., 220., 50.],
                true,
                t,
            );
            p.button(
                msg("history"),
                UiAction::History,
                [margin, height * 0.31 + 188., 220., 44.],
                false,
                t,
            );
        }
        screen => {
            p.rect([0., 0., width, height], [0.015, 0.035, 0.04, 0.83]);
            let w = (width - 2. * margin).min(680.);
            let x = (width - w) / 2.;
            let short_panel =
                height < 400. && matches!(screen, Screen::Menu | Screen::Saves | Screen::Settings);
            let tiny_settings = screen == Screen::Settings && height < 280.;
            let y = if tiny_settings {
                8.
            } else if short_panel {
                12.
            } else if height < 600. {
                24.
            } else {
                60.
            };
            let heading = match screen {
                Screen::Menu => "menu",
                Screen::Settings => "settings",
                Screen::History => "history",
                Screen::Saves => "saves",
                _ => "menu",
            };
            p.text(
                msg(heading),
                x,
                y,
                w,
                if tiny_settings {
                    20.
                } else if short_panel {
                    24.
                } else {
                    30.
                },
                t.text,
            );
            p.rect(
                [
                    x,
                    y + if tiny_settings {
                        32.
                    } else if short_panel {
                        35.
                    } else {
                        53.
                    },
                    w,
                    1.,
                ],
                t.accent,
            );
            match screen {
                Screen::Menu => {
                    let first = [p.quads.len(), p.texts.len(), p.semantics.len()];
                    let yy = y + if short_panel { 48. } else { 76. };
                    for (i, (k, a)) in [
                        ("saves", UiAction::Saves),
                        ("settings", UiAction::Settings),
                        ("history", UiAction::History),
                        ("rollback", UiAction::Rollback),
                        ("exit", UiAction::Title),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        p.button(msg(k), a, [x, yy + i as f32 * 59., w, 46.], false, t);
                    }
                    let view_height = (height - 128. - yy).max(1.);
                    let max = (4. * 59. + 46. - view_height).max(0.);
                    scroll_panel(
                        &mut p,
                        first,
                        ScrollView {
                            menu: None,
                            region: ScrollRegion::Menu,
                            rect: [x, yy, w, view_height],
                            offset: panels.menu.clamp(0., max),
                            max,
                            step: (view_height - 46.).max(1.),
                        },
                        m,
                        messages,
                    );
                }
                Screen::Settings => {
                    let first = [p.quads.len(), p.texts.len(), p.semantics.len()];
                    let yy = y + if tiny_settings {
                        40.
                    } else if short_panel {
                        48.
                    } else {
                        74.
                    };
                    let stacked = w < 360.;
                    let mut cursor = yy;
                    for (key, locales, active, is_ui) in [
                        (
                            "ui-language",
                            &m.available_ui_locales,
                            &m.prefs.ui_locale,
                            true,
                        ),
                        (
                            "text-language",
                            &m.available_text_locales,
                            &m.prefs.text_locale,
                            false,
                        ),
                    ] {
                        p.text(msg(key), x, cursor, w, 16., t.muted);
                        let columns = if stacked {
                            1
                        } else {
                            (((w + 12.) / 108.).floor() as usize)
                                .max(1)
                                .min(locales.len().max(1))
                        };
                        let button_width = (w - 12. * (columns - 1) as f32) / columns as f32;
                        for (i, locale) in locales.iter().enumerate() {
                            let label = match locale.as_str() {
                                "zh-Hans" => msg("language-zh"),
                                "ja" => msg("language-ja"),
                                _ => msg("language-en"),
                            };
                            let action = if is_ui {
                                UiAction::UiLocale {
                                    locale: locale.clone(),
                                }
                            } else {
                                UiAction::TextLocale {
                                    locale: locale.clone(),
                                }
                            };
                            p.button(
                                label,
                                action,
                                [
                                    x + (i % columns) as f32 * (button_width + 12.),
                                    cursor + 28. + (i / columns) as f32 * 52.,
                                    button_width,
                                    44.,
                                ],
                                active == locale,
                                t,
                            );
                        }
                        cursor += 28. + locales.len().div_ceil(columns) as f32 * 52.;
                    }
                    let state_message = if let Some(error) = &m.locale_error {
                        format!("{}: {error}", msg("language-failed"))
                    } else if m.locale_pending {
                        msg("language-pending")
                    } else {
                        format!(
                            "{}: {} / {}",
                            msg("language-active"),
                            m.ui_locale,
                            m.text_locale
                        )
                    };
                    let status_height = if m.locale_error.is_some() { 60. } else { 40. };
                    p.text(state_message, x, cursor, w, 13., t.muted);
                    p.texts.last_mut().unwrap().height = status_height;
                    p.texts.last_mut().unwrap().clip = Some([x, cursor, w, status_height]);
                    cursor += status_height + 8.;
                    if m.locale_error.is_some() || m.locale_pending {
                        let has_retry = m.locale_error.is_some();
                        if has_retry {
                            p.button(
                                msg("retry"),
                                UiAction::LocaleRetry,
                                [x, cursor, if stacked { w } else { (w - 12.) / 2. }, 44.],
                                true,
                                t,
                            );
                        }
                        p.button(
                            msg("language-cancel"),
                            UiAction::LocaleCancel,
                            [
                                if has_retry && !stacked {
                                    x + (w + 12.) / 2.
                                } else {
                                    x
                                },
                                cursor + if has_retry && stacked { 52. } else { 0. },
                                if has_retry && !stacked {
                                    (w - 12.) / 2.
                                } else {
                                    w
                                },
                                44.,
                            ],
                            false,
                            t,
                        );
                        cursor += if has_retry && stacked { 104. } else { 52. };
                    }
                    for (key, value, down, up) in [
                        (
                            "font-size",
                            m.prefs.font_scale,
                            UiAction::FontSize { delta: -0.1 },
                            UiAction::FontSize { delta: 0.1 },
                        ),
                        (
                            "music",
                            m.prefs.bgm_volume,
                            UiAction::Volume {
                                bus: AudioBus::Bgm,
                                delta: -0.1,
                            },
                            UiAction::Volume {
                                bus: AudioBus::Bgm,
                                delta: 0.1,
                            },
                        ),
                        (
                            "voice",
                            m.prefs.voice_volume,
                            UiAction::Volume {
                                bus: AudioBus::Voice,
                                delta: -0.1,
                            },
                            UiAction::Volume {
                                bus: AudioBus::Voice,
                                delta: 0.1,
                            },
                        ),
                        (
                            "sfx",
                            m.prefs.sfx_volume,
                            UiAction::Volume {
                                bus: AudioBus::Sfx,
                                delta: -0.1,
                            },
                            UiAction::Volume {
                                bus: AudioBus::Sfx,
                                delta: 0.1,
                            },
                        ),
                    ] {
                        cursor += setting_range(
                            &mut p,
                            format!("{}   {:.0}%", msg(key), value * 100.),
                            [down, up],
                            [msg("decrease"), msg("increase")],
                            [x, cursor, w],
                            t,
                        );
                    }
                    p.button(
                        format!(
                            "{}   {}",
                            msg("motion"),
                            if m.prefs.reduced_motion { "ON" } else { "OFF" }
                        ),
                        UiAction::ReducedMotion,
                        [x, cursor, w, 44.],
                        m.prefs.reduced_motion,
                        t,
                    );
                    cursor += 52.;
                    for (key, value, down, up) in [
                        (
                            "text-speed",
                            m.prefs.text_speed,
                            UiAction::TextSpeed { delta: -0.25 },
                            UiAction::TextSpeed { delta: 0.25 },
                        ),
                        (
                            "auto-wait",
                            m.prefs.auto_wait_scale,
                            UiAction::AutoWait { delta: -0.25 },
                            UiAction::AutoWait { delta: 0.25 },
                        ),
                    ] {
                        cursor += setting_range(
                            &mut p,
                            format!("{}   {:.0}%", msg(key), value * 100.),
                            [down, up],
                            [msg("decrease"), msg("increase")],
                            [x, cursor, w],
                            t,
                        );
                    }
                    let toggle_height = if stacked { 60. } else { 44. };
                    p.button(
                        format!(
                            "{}   {}",
                            msg("auto-wait-voice"),
                            if m.prefs.auto_wait_voice { "ON" } else { "OFF" }
                        ),
                        UiAction::AutoWaitVoice {
                            enabled: !m.prefs.auto_wait_voice,
                        },
                        [x, cursor, w, toggle_height],
                        m.prefs.auto_wait_voice,
                        t,
                    );
                    cursor += toggle_height + 8.;
                    p.button(
                        format!(
                            "{}   {}",
                            msg("voice-continue"),
                            if m.prefs.voice_continue { "ON" } else { "OFF" }
                        ),
                        UiAction::VoiceContinue {
                            enabled: !m.prefs.voice_continue,
                        },
                        [x, cursor, w, toggle_height],
                        m.prefs.voice_continue,
                        t,
                    );
                    cursor += toggle_height + 14.;
                    p.text(msg("voice-continue-hint"), x, cursor, w, 13., t.muted);
                    p.texts.last_mut().unwrap().height = if stacked { 100. } else { 60. };
                    cursor += if stacked { 108. } else { 68. };
                    p.text(msg("reading-preferences-hint"), x, cursor, w, 13., t.muted);
                    p.texts.last_mut().unwrap().height = if stacked { 100. } else { 60. };
                    cursor += if stacked { 118. } else { 78. };
                    let view_height = (height - 128. - yy).max(1.);
                    let viewport = [x, yy, w, view_height];
                    let roles_y = cursor;
                    let roles_height = if m.character_voices.is_empty() {
                        0.
                    } else {
                        48. + m.character_voices.len() as f32 * 116.
                    };
                    let max = (roles_y + roles_height - yy - view_height).max(0.);
                    let offset = panels.settings.clamp(0., max);
                    if !m.character_voices.is_empty() {
                        p.text(msg("character-voices"), x, roles_y, w, 20., t.text);
                        for (i, role) in m.character_voices.iter().enumerate() {
                            let ry = roles_y + 48. + i as f32 * 116.;
                            // Project only visible role rows; long casts do
                            // not create hundreds of hidden controls/glyphs.
                            if ry + 104. - offset < yy || ry - offset > yy + view_height {
                                continue;
                            }
                            let value = m
                                .prefs
                                .character_voices
                                .get(&role.id)
                                .copied()
                                .unwrap_or_default();
                            let first_role_control = p.semantics.len();
                            let editable = m.prefs.character_voices.contains_key(&role.id)
                                || m.prefs.character_voices.len() < MAX_CHARACTER_VOICES;
                            p.text(
                                format!("{}   {:.0}%", role.name, value.volume * 100.),
                                x,
                                ry,
                                w,
                                20.,
                                t.text,
                            );
                            let text = p.texts.last_mut().unwrap();
                            text.locale = role.locale.clone();
                            text.font_assets = role.fonts.clone();
                            text.height = 44.;
                            p.button(
                                msg("decrease"),
                                UiAction::CharacterVolume {
                                    character: role.id.clone(),
                                    delta: -0.1,
                                },
                                [x, ry + 52., 44., 44.],
                                false,
                                t,
                            );
                            p.button(
                                msg(if value.muted {
                                    "character-unmute"
                                } else {
                                    "character-mute"
                                }),
                                UiAction::CharacterMute {
                                    character: role.id.clone(),
                                    muted: !value.muted,
                                },
                                [x + 56., ry + 52., w - 112., 44.],
                                value.muted,
                                t,
                            );
                            p.semantics.last_mut().unwrap().value = Some(SemanticValue::Toggle {
                                checked: value.muted,
                            });
                            p.button(
                                msg("increase"),
                                UiAction::CharacterVolume {
                                    character: role.id.clone(),
                                    delta: 0.1,
                                },
                                [x + w - 44., ry + 52., 44., 44.],
                                false,
                                t,
                            );
                            for control in &mut p.semantics[first_role_control..] {
                                control.label = format!("{} · {}", role.name, control.label);
                                control.enabled &= editable;
                            }
                        }
                    }
                    scroll_panel(
                        &mut p,
                        first,
                        ScrollView {
                            menu: None,
                            region: ScrollRegion::Settings,
                            rect: viewport,
                            offset,
                            max,
                            step: (view_height - toggle_height).max(1.),
                        },
                        m,
                        messages,
                    );
                }
                Screen::Saves => {
                    let first = [p.quads.len(), p.texts.len(), p.semantics.len()];
                    let yy = y + if short_panel { 48. } else { 74. };
                    let stacked = w < 360.;
                    for (i, slot) in m.slots.iter().enumerate().take(3) {
                        let sy = yy + i as f32 * if stacked { 112. } else { 85. };
                        p.text(
                            format!(
                                "0{}   {}",
                                slot.slot + 1,
                                if slot.error.is_some() {
                                    msg("unavailable-slot")
                                } else if slot.exists {
                                    slot.label.clone()
                                } else {
                                    msg("empty-slot")
                                }
                            ),
                            x,
                            sy,
                            if stacked { w } else { w - 190. },
                            16.,
                            t.text,
                        );
                        if stacked {
                            p.texts.last_mut().unwrap().height = 48.;
                        }
                        let button_y = sy + if stacked { 52. } else { 0. };
                        let save_rect = if stacked {
                            [x, button_y, (w - 12.) / 2., 44.]
                        } else {
                            [x + w - 176., button_y, 82., 44.]
                        };
                        let load_rect = if stacked {
                            [x + (w + 12.) / 2., button_y, (w - 12.) / 2., 44.]
                        } else {
                            [x + w - 86., button_y, 86., 44.]
                        };
                        p.button(
                            msg("save"),
                            UiAction::Save { slot: slot.slot },
                            save_rect,
                            false,
                            t,
                        );
                        p.semantics.last_mut().unwrap().enabled = m.can_save
                            && slot.error.is_none()
                            && !m.busy_slots.contains(&slot.slot);
                        p.button(
                            msg(if slot.error.is_some() {
                                "retry"
                            } else {
                                "load"
                            }),
                            UiAction::Load { slot: slot.slot },
                            load_rect,
                            false,
                            t,
                        );
                        p.semantics.last_mut().unwrap().enabled = (slot.exists
                            || slot.error.is_some())
                            && !m.busy_slots.contains(&slot.slot);
                    }
                    let transfer_y = yy + if stacked { 336. } else { 273. };
                    p.button(
                        msg("export"),
                        UiAction::Export,
                        [x, transfer_y, if stacked { w } else { (w - 12.) / 2. }, 44.],
                        false,
                        t,
                    );
                    p.button(
                        msg("import"),
                        UiAction::Import,
                        [
                            if stacked { x } else { x + (w + 12.) / 2. },
                            transfer_y + if stacked { 52. } else { 0. },
                            if stacked { w } else { (w - 12.) / 2. },
                            44.,
                        ],
                        false,
                        t,
                    );
                    let view_height = (height - 128. - yy).max(1.);
                    let content_height = transfer_y - yy + if stacked { 96. } else { 44. };
                    let max = (content_height - view_height).max(0.);
                    scroll_panel(
                        &mut p,
                        first,
                        ScrollView {
                            menu: None,
                            region: ScrollRegion::Saves,
                            rect: [x, yy, w, view_height],
                            offset: panels.saves.clamp(0., max),
                            max,
                            step: (view_height - 44.).max(1.),
                        },
                        m,
                        messages,
                    );
                }
                Screen::History => {
                    if !m.status.is_empty() {
                        p.text(&m.status, x, y + 55., w, 13., t.accent);
                        p.texts.last_mut().unwrap().clip = Some([x, y + 55., w, 21.]);
                    }
                    let entries = &m.history;
                    let scrollable_entry = entries
                        .iter()
                        .enumerate()
                        .max_by_key(|(_, entry)| entry.text.len())
                        .map(|(index, _)| index);
                    let row_height = ((height - y - 276.).max(60.) / 3.).max(48.);
                    for (i, entry) in entries.iter().enumerate() {
                        let has_voice = entry.voice_count > 0;
                        let playback = m
                            .history_voice
                            .as_ref()
                            .filter(|voice| voice.entry == entry.key);
                        let active = playback.is_some_and(|voice| !voice.failed);
                        let text_width = if has_voice { (w - 126.).max(32.) } else { w };
                        let row_y = y + 76. + i as f32 * row_height;
                        let heading_height = if let Some(choice) = entry.choice {
                            p.text(msg(choice.message()), x, row_y, text_width, 13., t.muted);
                            22.
                        } else {
                            0.
                        };
                        let text = if entry.speaker.is_empty() {
                            entry.text.clone()
                        } else {
                            format!("{}  /  {}", entry.speaker, entry.text)
                        };
                        p.texts.push(TextRun {
                            text,
                            visible: None,
                            x,
                            y: row_y + heading_height,
                            width: text_width,
                            height: (row_height - heading_height - 8.).max(1.),
                            size: if narrow { 16. } else { 18. },
                            line_height: if narrow { 24. } else { 27. },
                            color: t.text,
                            emphasis: vec![],
                            images: entry
                                .images
                                .iter()
                                .cloned()
                                .map(|mut i| {
                                    if !entry.speaker.is_empty() {
                                        i.offset += (entry.speaker.len() + 5) as u32;
                                    }
                                    i
                                })
                                .collect(),
                            image_scale: 1.,
                            scroll: 0.,
                            clip: (scrollable_entry == Some(i)).then_some([
                                x,
                                row_y + heading_height,
                                text_width,
                                (row_height - heading_height - 8.).max(1.),
                            ]),
                            region: (scrollable_entry == Some(i)).then_some(ScrollRegion::History),
                            locale: entry.locale.clone(),
                            font_assets: entry.font_assets.clone(),
                            font_plan_digest: entry.font_plan_digest.clone(),
                            preflight_only: false,
                            shadow: None,
                            monochrome: false,
                        });
                        if has_voice {
                            let label = if playback.is_some_and(|voice| voice.failed) {
                                msg("history-voice-failed")
                            } else if active {
                                msg("history-voice-stop")
                            } else {
                                msg("history-voice")
                            };
                            p.button(
                                label,
                                if active {
                                    UiAction::StopHistoryVoice
                                } else {
                                    UiAction::HistoryVoice { entry: entry.key }
                                },
                                [x + w - 118., y + 76. + i as f32 * row_height, 118., 44.],
                                active,
                                t,
                            );
                        }
                    }
                    p.button(
                        msg("history-back"),
                        UiAction::HistoryPage { delta: 3 },
                        [x, height - 135., 70., 40.],
                        false,
                        t,
                    );
                    p.button(
                        msg("history-forward"),
                        UiAction::HistoryPage { delta: -3 },
                        [x + 82., height - 135., 70., 40.],
                        false,
                        t,
                    );
                }
                _ => {}
            }
            {
                p.button(
                    msg("close"),
                    UiAction::Close,
                    [x, height - 76., w, 44.],
                    true,
                    t,
                );
            }
        }
    }
    if m.loading && (m.fault.is_none() || m.retrying) {
        let footer_height = if matches!(m.screen, Screen::Saves | Screen::Settings)
            || m.screen == Screen::Menu && !m.authored_menu
        {
            32.
        } else {
            38.
        };
        p.rect(
            [0., height - footer_height, width, footer_height],
            t.background,
        );
        p.text(
            msg("loading"),
            margin,
            height - footer_height + 7.,
            width - 2. * margin,
            13.,
            t.accent,
        );
    }
    let builtin_panel = m.screen == Screen::Saves || m.screen == Screen::Menu && !m.authored_menu;
    if !m.status.is_empty()
        && m.fault.is_none()
        && !matches!(m.screen, Screen::Settings | Screen::History)
        && (!builtin_panel || !m.loading)
    {
        let status_y = if builtin_panel {
            // Keep feedback below the fixed Close button. Short panel headers
            // reserve the remaining height for content and scroll controls.
            height - 25.
        } else if m.screen == Screen::Story && narrow {
            120.
        } else {
            65.
        };
        p.text(
            &m.status,
            margin,
            status_y,
            width - 2. * margin,
            13.,
            t.accent,
        );
    }
    if let Some(error) = &m.fault {
        let button_width = ((width - 2. * margin - 54.) / 2.).clamp(0., 140.);
        p.rect([margin, height * 0.3, width - 2. * margin, 152.], t.panel);
        p.texts.push(TextRun {
            text: error.clone(),
            visible: None,
            x: margin + 20.,
            y: height * 0.3 + 16.,
            width: width - 2. * margin - 40.,
            height: 65.,
            size: 15.,
            line_height: 22.5,
            color: t.text,
            emphasis: vec![],
            images: vec![],
            image_scale: 1.,
            scroll: 0.,
            clip: None,
            region: None,
            locale: m.ui_locale.clone(),
            font_assets: m.ui_fonts.clone(),
            font_plan_digest: m.ui_font_plan_digest.clone(),
            preflight_only: false,
            shadow: None,
            monochrome: false,
        });
        if m.fault_recovery.contains(&Recovery::Retry) {
            p.button(
                msg(if m.retrying { "retrying" } else { "retry" }),
                UiAction::Retry,
                [margin + 20., height * 0.3 + 90., button_width, 44.],
                !m.retrying,
                t,
            );
            p.semantics.last_mut().unwrap().enabled = !m.retrying;
        }
        p.button(
            msg("exit"),
            UiAction::Title,
            [
                margin + 34. + button_width,
                height * 0.3 + 90.,
                button_width,
                44.,
            ],
            false,
            t,
        );
    }
    if m.menu_navigation_pending {
        // Keep the previous page visible, but its controls no longer accept
        // work until navigation commits or is explicitly abandoned.
        for node in &mut p.semantics {
            if !matches!(node.action, UiAction::Retry | UiAction::Title) {
                node.enabled = false;
            }
        }
        p.button(
            msg("cancel-navigation"),
            UiAction::Close,
            [(width - margin - 140.).max(margin), height - 92., 140., 44.],
            true,
            t,
        );
    }
    if let Some((token, slot)) = m.save_confirmation {
        p.quads.clear();
        p.texts.clear();
        p.semantics.clear();
        p.scrolls.clear();
        p.history_flow = None;
        p.history_bar_view = None;
        p.history_bar = None;
        p.menu_paint.clear();
        p.menu_controls.clear();
        p.menu_quad_range = None;
        p.menu_page_range = None;
        p.menu_page_texts = None;
        p.menu_layers = None;
        p.transition_layers = None;
        p.rect([0., 0., width, height], t.background);
        let w = (width - 2. * margin).min(520.);
        let x = (width - w) / 2.;
        let y = (height * 0.35).max(24.);
        p.text(
            format!("{} {}", msg("overwrite-slot"), slot + 1),
            x,
            y,
            w,
            24.,
            t.text,
        );
        p.button(
            msg("confirm-overwrite"),
            UiAction::ConfirmSave { token },
            [x, y + 64., w, 44.],
            true,
            t,
        );
        p.button(
            msg("cancel-overwrite"),
            UiAction::CancelSave { token },
            [x, y + 120., w, 44.],
            false,
            t,
        );
    }
    p.texts.extend(m.preflight_texts.clone());
    p
}

/// CPU shaping cache, kept independent of the GPU atlas and the VM.
pub struct TextEngine {
    pub fonts: cosmic_text::FontSystem,
    pub buffers: std::collections::BTreeMap<String, cosmic_text::Buffer>,
    pub shapes: u64,
    families: std::collections::BTreeMap<String, &'static str>,
    configured_plan: String,
    pub missing_font: Option<String>,
    cache_recency: std::collections::BTreeMap<String, u64>,
    cache_clock: u64,
    cache_hits: u64,
    cache_misses: u64,
    cache_evictions: u64,
}

/// A snapshot of the CPU text-shaping cache.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextCacheStats {
    /// Number of successful lookups of a buffer already in the cache.
    pub hits: u64,
    /// Number of lookups whose buffer was not in the cache.
    pub misses: u64,
    /// Number of buffers removed to enforce the cache target.
    pub evictions: u64,
    /// Number of buffers currently in the cache.
    pub entries: usize,
}
struct ExplicitFallback {
    families: Vec<&'static str>,
}
impl cosmic_text::Fallback for ExplicitFallback {
    fn common_fallback(&self) -> &[&'static str] {
        &self.families
    }
    fn forbidden_fallback(&self) -> &[&'static str] {
        &[]
    }
    fn script_fallback(&self, _script: unicode_script::Script, _locale: &str) -> &[&'static str] {
        &self.families
    }
}
impl Default for TextEngine {
    fn default() -> Self {
        let db = cosmic_text::fontdb::Database::new();
        Self {
            fonts: cosmic_text::FontSystem::new_with_locale_and_db("zh-Hans".into(), db),
            buffers: Default::default(),
            shapes: 0,
            families: Default::default(),
            configured_plan: String::new(),
            missing_font: None,
            cache_recency: Default::default(),
            cache_clock: 0,
            cache_hits: 0,
            cache_misses: 0,
            cache_evictions: 0,
        }
    }
}
impl TextEngine {
    pub fn add_font_asset(
        &mut self,
        asset: &str,
        bytes: Vec<u8>,
    ) -> std::result::Result<(), String> {
        let before: std::collections::BTreeSet<_> = self.fonts.db().faces().map(|f| f.id).collect();
        self.fonts.db_mut().load_font_data(bytes);
        let family = self
            .fonts
            .db()
            .faces()
            .find(|face| !before.contains(&face.id))
            .and_then(|face| face.families.first().map(|f| f.0.clone()))
            .ok_or_else(|| format!("E_FONT: no font face in asset {asset}"))?;
        if self
            .families
            .iter()
            .any(|(other, name)| other != asset && *name == family)
        {
            return Err(format!(
                "E_FONT_FAMILY_AMBIGUOUS: duplicate family {family}"
            ));
        }
        let static_family: &'static str = Box::leak(family.clone().into_boxed_str());
        self.families.insert(asset.into(), static_family);
        // This default is only used by isolated presentation tests with no plan.
        if self.families.len() == 1 {
            self.fonts.db_mut().set_sans_serif_family(family);
        }
        self.buffers.clear();
        self.cache_recency.clear();
        self.configured_plan.clear();
        Ok(())
    }

    /// Return cumulative cache counters and the current number of buffers.
    ///
    /// Hit, miss, and eviction counters are cumulative for the lifetime of
    /// this engine. Loading a font invalidates all cached buffers without
    /// resetting the counters or counting the invalidated buffers as
    /// evictions; `entries` always reports the live buffer count.
    pub fn cache_stats(&self) -> TextCacheStats {
        TextCacheStats {
            hits: self.cache_hits,
            misses: self.cache_misses,
            evictions: self.cache_evictions,
            entries: self.buffers.len(),
        }
    }

    fn touch_cache_key(&mut self, key: &str) {
        if self.cache_clock == u64::MAX {
            let mut oldest_first: Vec<_> = self
                .cache_recency
                .iter()
                .map(|(key, age)| (key.clone(), *age))
                .collect();
            oldest_first.sort_by_key(|(_, age)| *age);
            for (index, (key, _)) in oldest_first.into_iter().enumerate() {
                self.cache_recency.insert(key, index as u64 + 1);
            }
            self.cache_clock = self.cache_recency.len() as u64;
        }
        self.cache_clock += 1;
        self.cache_recency.insert(key.to_owned(), self.cache_clock);
    }

    fn sync_cache_recency(&mut self) {
        self.cache_recency
            .retain(|key, _| self.buffers.contains_key(key));
        let untracked: Vec<_> = self
            .buffers
            .keys()
            .filter(|key| !self.cache_recency.contains_key(*key))
            .cloned()
            .collect();
        for key in untracked {
            self.touch_cache_key(&key);
        }
    }

    fn evict_unprotected(&mut self, protected: &std::collections::BTreeSet<String>) {
        const TARGET_ENTRIES: usize = 128;
        // `buffers` stays public for compatibility, so reconcile direct map
        // edits only on the uncommon over-capacity path.
        self.sync_cache_recency();
        let remove_count = self.buffers.len().saturating_sub(TARGET_ENTRIES);
        if remove_count == 0 {
            return;
        }

        let mut candidates: Vec<_> = self
            .cache_recency
            .iter()
            .filter(|(key, _)| !protected.contains(*key))
            .map(|(key, age)| (key.clone(), *age))
            .collect();
        candidates.sort_by_key(|(_, age)| *age);
        for (key, _) in candidates.into_iter().take(remove_count) {
            if self.buffers.remove(&key).is_some() {
                self.cache_recency.remove(&key);
                self.cache_evictions = self.cache_evictions.saturating_add(1);
            }
        }
    }
    pub fn key(run: &TextRun) -> String {
        format!(
            "{}:{}:{}:{}:{}:{}:{:?}:{:?}:{}:{}:{:?}:{}",
            run.size.to_bits(),
            run.line_height.to_bits(),
            run.width.to_bits(),
            run.height.to_bits(),
            run.locale,
            run.font_plan_digest,
            run.font_assets,
            run.emphasis,
            run.text,
            run.monochrome,
            run.images,
            run.image_scale.to_bits()
        )
    }
    /// Original UTF-8 offsets for the exact paragraph splitter used by shaping.
    pub fn line_offsets(run: &TextRun) -> Vec<usize> {
        let mut offsets: Vec<_> = if run.emphasis.is_empty() && run.images.is_empty() {
            cosmic_text::LineIter::new(&run.text)
                .map(|(range, _)| range.start)
                .collect()
        } else {
            cosmic_text::BidiParagraphs::new(&run.text)
                .map(|line| line.as_ptr() as usize - run.text.as_ptr() as usize)
                .collect()
        };
        // cosmic-text keeps one empty BufferLine for an empty string.
        if offsets.is_empty() {
            offsets.push(0);
        }
        offsets
    }
    pub fn layout(&mut self, packet: &DrawPacket) {
        self.layout_texts(&packet.texts);
    }
    pub fn layout_texts(&mut self, texts: &[TextRun]) {
        self.missing_font = None;
        // Retain each formatted key for lookup and possible end-of-layout
        // eviction, avoiding a second key allocation for active runs.
        let packet_keys: Vec<_> = texts.iter().map(Self::key).collect();
        for (r, key) in texts.iter().zip(&packet_keys) {
            if self.buffers.contains_key(key) {
                self.cache_hits = self.cache_hits.saturating_add(1);
                self.touch_cache_key(key);
                continue;
            }
            self.cache_misses = self.cache_misses.saturating_add(1);
            {
                let explicit: Vec<_> = r
                    .font_assets
                    .iter()
                    .filter_map(|id| self.families.get(id).copied())
                    .collect();
                if !r.font_assets.is_empty() && explicit.len() != r.font_assets.len() {
                    let missing = r
                        .font_assets
                        .iter()
                        .find(|id| !self.families.contains_key(*id))
                        .cloned()
                        .unwrap_or_default();
                    self.missing_font = Some(format!(
                        "E_FONT_PLAN: font asset {missing} was not prepared"
                    ));
                    // A missing face must never reach cosmic-text shaping: an
                    // empty font database panics instead of reporting an error.
                    continue;
                }
                if explicit.is_empty() {
                    self.missing_font =
                        Some("E_FONT_PLAN: text run has no explicit font plan".into());
                    continue;
                }
                let signature = format!("{}:{}:{:?}", r.locale, r.font_plan_digest, explicit);
                if self.configured_plan != signature && !explicit.is_empty() {
                    let old = std::mem::replace(
                        &mut self.fonts,
                        cosmic_text::FontSystem::new_with_locale_and_db(
                            "en".into(),
                            cosmic_text::fontdb::Database::new(),
                        ),
                    );
                    let (_, db) = old.into_locale_and_db();
                    self.fonts = cosmic_text::FontSystem::new_with_locale_and_db_and_fallback(
                        r.locale.clone(),
                        db,
                        ExplicitFallback {
                            families: explicit.clone(),
                        },
                    );
                    self.configured_plan = signature;
                }
                let primary = explicit.first().copied();
                let mut b = cosmic_text::Buffer::new(
                    &mut self.fonts,
                    cosmic_text::Metrics::new(r.size, r.line_height),
                );
                b.set_size(&mut self.fonts, Some(r.width), None);
                let attrs = match primary {
                    Some(name) => cosmic_text::Attrs::new().family(cosmic_text::Family::Name(name)),
                    None => cosmic_text::Attrs::new().family(cosmic_text::Family::SansSerif),
                };
                if !r.images.is_empty() {
                    // The object-replacement character is a layout slot, never
                    // a font glyph. Measure its advance in this exact font plan
                    // and reserve the authored image width with rich metrics.
                    let mut probe = cosmic_text::Buffer::new(
                        &mut self.fonts,
                        cosmic_text::Metrics::new(1., 1.),
                    );
                    probe.set_text(
                        &mut self.fonts,
                        "\u{fffc}",
                        &attrs,
                        cosmic_text::Shaping::Advanced,
                    );
                    probe.shape_until_scroll(&mut self.fonts, false);
                    let unit_width = probe
                        .layout_runs()
                        .flat_map(|line| line.glyphs.iter())
                        .map(|glyph| glyph.w)
                        .sum::<f32>();
                    let mut cuts = vec![0, r.text.len()];
                    for &(start, end) in &r.emphasis {
                        cuts.extend([start, end]);
                    }
                    for image in &r.images {
                        cuts.extend([image.offset as usize, image.offset as usize + 3]);
                    }
                    cuts.sort_unstable();
                    cuts.dedup();
                    let mut spans = vec![];
                    for range in cuts.windows(2) {
                        let [start, end] = [range[0], range[1]];
                        let Some(value) = r.text.get(start..end) else {
                            continue;
                        };
                        let attr = if let Some(placement) = r
                            .images
                            .iter()
                            .find(|i| i.offset as usize == start && end == start + 3)
                        {
                            let image = &placement.image;
                            let width = (image.width + image.margins[0] + image.margins[1]) as f32
                                * r.image_scale;
                            let height = (image.height + image.margins[2] + image.margins[3])
                                as f32
                                * r.image_scale;
                            attrs
                                .clone()
                                .metrics(cosmic_text::Metrics::new(1., height.max(r.line_height)))
                                .letter_spacing(width - unit_width)
                                .color(cosmic_text::Color::rgba(0, 0, 0, 0))
                        } else if !r.monochrome
                            && r.emphasis.iter().any(|&(a, b)| a <= start && end <= b)
                        {
                            attrs.clone().color(cosmic_text::Color::rgb(224, 202, 153))
                        } else {
                            attrs.clone()
                        };
                        spans.push((value, attr));
                    }
                    b.set_rich_text(
                        &mut self.fonts,
                        spans,
                        &attrs,
                        cosmic_text::Shaping::Advanced,
                        None,
                    );
                } else if r.emphasis.is_empty() {
                    b.set_text(
                        &mut self.fonts,
                        &r.text,
                        &attrs,
                        cosmic_text::Shaping::Advanced,
                    );
                } else {
                    let mut spans = vec![];
                    let mut pos = 0;
                    for &(start, end) in &r.emphasis {
                        if start >= pos
                            && end <= r.text.len()
                            && r.text.is_char_boundary(start)
                            && r.text.is_char_boundary(end)
                        {
                            spans.push((&r.text[pos..start], attrs.clone()));
                            spans.push((
                                &r.text[start..end],
                                if r.monochrome {
                                    attrs.clone()
                                } else {
                                    attrs.clone().color(cosmic_text::Color::rgb(224, 202, 153))
                                },
                            ));
                            pos = end;
                        }
                    }
                    spans.push((&r.text[pos..], attrs.clone()));
                    b.set_rich_text(
                        &mut self.fonts,
                        spans,
                        &attrs,
                        cosmic_text::Shaping::Advanced,
                        None,
                    );
                }
                b.shape_until_scroll(&mut self.fonts, false);
                self.buffers.insert(key.clone(), b);
                self.touch_cache_key(key);
                self.shapes = self.shapes.saturating_add(1);
            }
        }
        if self.buffers.len() > 128 {
            // Protect all runs in this packet before the first eviction. If
            // the working set itself is oversized, it remains intact until
            // a later layout offers less protection and the cache can shrink.
            let protected = packet_keys.into_iter().collect();
            self.evict_unprotected(&protected);
        }
    }
}

#[cfg(test)]
mod scene_tests {
    use super::*;

    fn text_packet(texts: impl IntoIterator<Item = String>) -> DrawPacket {
        let mut packet = DrawPacket {
            locale: "zh-Hans".into(),
            font_assets: vec!["font.reader".into()],
            font_plan_digest: "plan-a".into(),
            ..Default::default()
        };
        for text in texts {
            packet.text(text, 0., 0., 300., 18., [1.; 4]);
        }
        packet
    }

    fn reader_text_engine() -> TextEngine {
        let mut text = TextEngine::default();
        text.add_font_asset(
            "font.reader",
            include_bytes!("../../../examples/rain-letters/assets/source/reader.otf").to_vec(),
        )
        .unwrap();
        text
    }

    fn visual_packet() -> DrawPacket {
        let mut packet = DrawPacket {
            locale: "en".into(),
            font_assets: vec!["font.ui".into()],
            font_plan_digest: "ui-plan-a".into(),
            width: 640.,
            height: 360.,
            stage_size: [1280, 720],
            transition_layers: Some((
                vec![Quad {
                    corners: None,
                    rect: [1., 2., 30., 40.],
                    color: [0.1, 0.2, 0.3, 1.],
                    asset: Some("old-bg".into()),
                    clip: None,
                }],
                vec![Quad {
                    corners: None,
                    rect: [4., 5., 60., 70.],
                    color: [0.4, 0.5, 0.6, 1.],
                    asset: Some("new-bg".into()),
                    clip: Some([0., 0., 640., 360.]),
                }],
                0.25,
            )),
            ..Default::default()
        };
        packet.rect([0., 0., 640., 360.], [0.01, 0.02, 0.03, 1.]);
        packet.text("hello", 10., 20., 300., 18., [1.; 4]);
        packet
    }

    #[test]
    fn appended_controls_do_not_reuse_retained_semantic_ids() {
        let mut p = DrawPacket::default();
        for _ in 0..6 {
            p.button(
                "control".into(),
                UiAction::Menu,
                [0., 0., 30., 30.],
                false,
                &Theme::default(),
            );
        }
        p.semantics.retain(|n| n.id == 4 || n.id == 5);
        p.button(
            "close".into(),
            UiAction::Close,
            [0., 0., 30., 30.],
            false,
            &Theme::default(),
        );
        let ids: std::collections::BTreeSet<_> = p.semantics.iter().map(|n| n.id).collect();
        assert_eq!(ids.len(), p.semantics.len());
        assert_eq!(p.semantics.last().unwrap().id, 6);
    }
    #[test]
    fn shadow_is_paint_only_and_preserves_reading_geometry() {
        let mut p = DrawPacket::default();
        p.text("a\nb", 10., 20., 100., 18., [1., 1., 1., 0.5]);
        let r = &mut p.texts[0];
        r.shadow = Some(TextShadow {
            offset: [2., 3.],
            color: [0., 0., 0., 0.8],
        });
        r.emphasis = vec![(0, 1)];
        r.visible = Some(1);
        r.scroll = 7.;
        r.region = Some(ScrollRegion::Dialogue);
        let shadow = r.shadow_run().unwrap();
        assert_eq!((shadow.x, shadow.y), (12., 23.));
        assert_eq!(shadow.width, r.width);
        assert_eq!(shadow.emphasis, r.emphasis);
        assert_eq!(
            TextEngine::line_offsets(&shadow),
            TextEngine::line_offsets(r)
        );
        assert_eq!(shadow.visible, r.visible);
        assert_eq!(shadow.scroll, 7.);
        assert_eq!(shadow.color[3], 0.4);
        assert_eq!(shadow.region, None);
        assert!(shadow.shadow.is_none());
        assert!(shadow.monochrome);
        assert_ne!(TextEngine::key(&shadow), TextEngine::key(r));
        assert!(shadow
            .clip
            .unwrap()
            .iter()
            .zip([10., 20., 100., r.height])
            .all(|(a, b)| (*a - b).abs() < 0.00001));
    }
    #[test]
    fn imported_line_height_fits_four_lines_and_has_its_own_shape_key() {
        let mut text = reader_text_engine();
        let mut packet = text_packet(["A\nB\nC\nD".to_owned()]);
        packet.texts[0].size = 32.;
        packet.texts[0].line_height = 40.;
        packet.texts[0].height = 175.;
        text.layout(&packet);
        let key = TextEngine::key(&packet.texts[0]);
        let lines: Vec<_> = text.buffers[&key].layout_runs().collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(
            lines.last().unwrap().line_top + lines.last().unwrap().line_height,
            160.
        );
        packet.texts[0].line_height = 48.;
        assert_ne!(key, TextEngine::key(&packet.texts[0]));
        text.layout(&packet);
        assert_eq!(text.cache_stats().entries, 2);
    }

    #[test]
    fn shadow_has_identical_glyph_geometry_without_emphasis_colors() {
        let mut engine = reader_text_engine();
        let mut packet = text_packet(["rain after\nhello world".to_owned()]);
        let run = &mut packet.texts[0];
        run.emphasis = vec![(0, 4)];
        run.shadow = Some(TextShadow {
            offset: [2., 2.],
            color: [0., 0., 0., 1.],
        });
        let shadow = run.shadow_run().unwrap();
        let key = TextEngine::key(run);
        let shadow_key = TextEngine::key(&shadow);
        packet.texts.push(shadow);
        engine.layout(&packet);
        let geometry = |key: &str| {
            engine.buffers[key]
                .layout_runs()
                .flat_map(|r| {
                    r.glyphs
                        .iter()
                        .map(|g| (g.start, g.end, g.x, g.y, g.w, g.glyph_id))
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(geometry(&key), geometry(&shadow_key));
        assert!(engine.buffers[&shadow_key]
            .layout_runs()
            .all(|r| r.glyphs.iter().all(|g| g.color_opt.is_none())));
    }

    #[test]
    fn text_cache_lru_keeps_a_frequently_used_entry_hot() {
        let mut text = reader_text_engine();
        let hot = text_packet(["hot entry".to_owned()]);
        let hot_key = TextEngine::key(&hot.texts[0]);
        text.layout(&hot);

        for index in 0..200 {
            text.layout(&hot);
            let cold = text_packet([format!("cold entry {index}")]);
            text.layout(&cold);
        }

        let stats = text.cache_stats();
        assert!(text.buffers.contains_key(&hot_key));
        assert_eq!(stats.entries, 128);
        assert_eq!(stats.hits, 200);
        assert_eq!(stats.misses, 201);
        assert_eq!(stats.evictions, 73);
    }

    #[test]
    fn text_cache_protects_the_full_current_packet_then_converges() {
        let mut text = reader_text_engine();
        let packet = text_packet((0..140).map(|index| format!("packet entry {index}")));
        let keys: Vec<_> = packet.texts.iter().map(TextEngine::key).collect();
        text.layout(&packet);

        assert_eq!(text.cache_stats().entries, 140);
        assert_eq!(text.cache_stats().evictions, 0);
        assert!(keys.iter().all(|key| text.buffers.contains_key(key)));

        let mut next_packet = DrawPacket::default();
        next_packet.texts.push(packet.texts[139].clone());
        text.layout(&next_packet);

        let stats = text.cache_stats();
        assert_eq!(stats.entries, 128);
        assert_eq!(stats.evictions, 12);
        assert!(text.buffers.contains_key(&keys[139]));
        assert!(!text.buffers.contains_key(&keys[0]));
    }

    #[test]
    fn font_load_invalidates_buffers_but_keeps_cumulative_stats() {
        let mut text = reader_text_engine();
        let packet = text_packet(["font invalidation".to_owned()]);
        text.layout(&packet);
        assert_eq!(text.cache_stats().misses, 1);
        assert_eq!(text.cache_stats().entries, 1);

        text.add_font_asset(
            "font.abe",
            include_bytes!("../../../examples/rain-letters/assets/fonts/ABeeZee-Regular.ttf")
                .to_vec(),
        )
        .unwrap();
        assert_eq!(text.cache_stats().entries, 0);

        text.layout(&packet);
        let stats = text.cache_stats();
        assert_eq!(stats.misses, 2);
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.evictions, 0);
        assert_eq!(stats.entries, 1);
    }

    #[test]
    fn visual_equality_ignores_semantic_only_changes() {
        let packet = visual_packet();
        let mut semantically_changed = visual_packet();
        semantically_changed.announcement = "accessible description".into();
        semantically_changed.announcement_locale = "zh-Hans".into();
        semantically_changed.semantics.push(SemanticNode {
            value: None,
            id: 7,
            label: "continue".into(),
            action: UiAction::Advance,
            enabled: false,
            rect: [1., 2., 3., 4.],
            locale: "zh-Hans".into(),
        });
        semantically_changed.scrolls.push(ScrollView {
            menu: None,
            region: ScrollRegion::Dialogue,
            rect: [0., 0., 10., 10.],
            offset: 4.,
            max: 12.,
            step: 8.,
        });
        semantically_changed.dialogue_hint = Some(0);

        assert!(packet.visual_eq(&semantically_changed));
    }

    #[test]
    fn visual_equality_detects_geometry_fonts_text_stage_and_transition_changes() {
        let packet = visual_packet();

        let mut changed = visual_packet();
        changed.quads[0].color[0] += 0.1;
        assert!(!packet.visual_eq(&changed));

        let mut changed = visual_packet();
        changed.texts[0].text.push('!');
        assert!(!packet.visual_eq(&changed));

        let mut changed = visual_packet();
        changed.texts[0].size += 1.;
        assert!(!packet.visual_eq(&changed));

        let mut changed = visual_packet();
        changed.font_plan_digest.push_str("-b");
        assert!(!packet.visual_eq(&changed));

        let mut changed = visual_packet();
        changed.stage_size[0] += 1;
        assert!(!packet.visual_eq(&changed));

        let mut changed = visual_packet();
        changed.transition_layers.as_mut().unwrap().2 = 0.5;
        assert!(!packet.visual_eq(&changed));
    }

    #[test]
    fn missing_explicit_font_reports_error_before_shaping() {
        let mut text = TextEngine::default();
        let mut packet = DrawPacket {
            locale: "zh-Hans".into(),
            font_assets: vec!["font.reader".into()],
            font_plan_digest: "plan-a".into(),
            ..Default::default()
        };
        packet.text("雨后", 0., 0., 300., 18., [1.; 4]);
        text.layout(&packet);
        assert!(text
            .missing_font
            .as_deref()
            .unwrap()
            .contains("font.reader"));
        assert!(text.buffers.is_empty());
        text.add_font_asset(
            "font.reader",
            include_bytes!("../../../examples/rain-letters/assets/source/reader.otf").to_vec(),
        )
        .unwrap();
        text.layout(&packet);
        assert!(text.missing_font.is_none());
        assert_eq!(text.shapes, 1);
    }

    #[test]
    fn text_shape_cache_is_scoped_to_locale_and_font_plan() {
        let mut text = TextEngine::default();
        text.add_font_asset(
            "font.reader",
            include_bytes!("../../../examples/rain-letters/assets/source/reader.otf").to_vec(),
        )
        .unwrap();
        let mut packet = DrawPacket {
            width: 320.,
            height: 180.,
            locale: "zh-Hans".into(),
            font_assets: vec!["font.reader".into()],
            font_plan_digest: "plan-a".into(),
            ..Default::default()
        };
        packet.text("雨后", 0., 0., 300., 18., [1.; 4]);
        text.layout(&packet);
        assert_eq!(text.shapes, 1);
        packet.texts[0].font_plan_digest = "plan-b".into();
        text.layout(&packet);
        assert_eq!(text.shapes, 2);
        packet.texts[0].locale = "en".into();
        text.layout(&packet);
        assert_eq!(text.shapes, 3);
    }

    fn node(id: &str, parent: Option<&str>, order: i32) -> Node {
        Node {
            id: id.into(),
            parent: parent.map(str::to_owned),
            asset: Some(id.into()),
            x: 0.,
            y: 0.,
            width: 10.,
            height: 10.,
            scale: 1.,
            opacity: 1.,
            color: [1.; 4],
            order,
            clip: None,
            timeline_binding: None,
            inherit_existence: false,
            sprite_transform: None,
            bitmap_text: None,
            preserve_pose: vec![],
            offset: [0.; 2],
        }
    }
    #[test]
    fn leaf_sprite_rotation_composes_with_ancestor_pose_and_fixed_parent_clip() {
        let mut group = node("group", None, 0);
        group.width = 0.;
        group.height = 0.;
        group.x = 10.;
        group.y = 20.;
        group.scale = 2.;
        group.clip = Some([0., 0., 50., 50.]);
        let mut child = node("sprite", Some("group"), 0);
        child.width = 32.;
        child.height = 24.;
        child.x = 3.;
        child.y = 4.;
        child.offset = [5., -2.];
        child.sprite_transform = Some(nir_format::SpriteTransform {
            origin: [24., 0.],
            basis_x: [0., 1.],
            basis_y: [-1., 0.],
        });
        let packet = DrawPacket {
            width: 1280.,
            height: 720.,
            ..Default::default()
        };
        let quads = scene_layout(&packet, &[group, child], [1280., 720.], 1.);
        assert_eq!(quads.len(), 1);
        assert_eq!(
            quads[0].1.corners,
            Some([[74., 24.], [74., 88.], [26., 24.], [26., 88.]])
        );
        assert_eq!(quads[0].1.clip, Some([10., 20., 100., 100.]));
        let changed = DrawPacket {
            width: 1280.,
            height: 720.,
            quads: quads.iter().map(|(_, q)| q.clone()).collect(),
            ..Default::default()
        };
        let mut other = DrawPacket {
            width: 1280.,
            height: 720.,
            quads: changed.quads.clone(),
            ..Default::default()
        };
        other.quads[0].corners.as_mut().unwrap()[0][0] += 1.;
        assert!(!changed.visual_eq(&other));
    }
    #[test]
    fn group_order_and_nested_clip_are_composed() {
        let mut group = node("group", None, 0);
        group.width = 0.;
        group.x = 10.;
        group.scale = 2.;
        group.clip = Some([0., 0., 8., 8.]);
        let mut child = node("child", Some("group"), 99);
        child.x = 3.;
        child.clip = Some([0., 0., 10., 10.]);
        let front = node("front", None, 1);
        let mut p = DrawPacket {
            width: 100.,
            height: 100.,
            ..Default::default()
        };
        scene(&mut p, &[group, front, child], [100., 100.], 1.);
        assert_eq!(p.quads[0].asset.as_deref(), Some("child"));
        assert_eq!(p.quads[1].asset.as_deref(), Some("front"));
        assert_eq!(p.quads[0].rect, [16., 0., 20., 20.]);
        assert_eq!(p.quads[0].clip, Some([16., 0., 10., 16.]));
    }
}

#[cfg(test)]
mod story_layout;
