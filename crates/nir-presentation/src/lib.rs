//! Read-only presentation projection. UI actions do not mutate the story here.
#![forbid(unsafe_code)]
pub use cosmic_text;
use fluent_bundle::{FluentBundle, FluentResource};
use nir_format::*;
use serde::Serialize;
mod reading;
pub use reading::{ReadingState, ScrollView};

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
}
#[derive(Debug, Clone)]
pub struct ChoiceView {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub locale: String,
    pub font_plan_digest: String,
    pub font_assets: Vec<String>,
}
#[derive(Debug, Clone)]
pub struct HistoryView {
    pub speaker: String,
    pub text: String,
    pub locale: String,
    pub font_plan_digest: String,
    pub font_assets: Vec<String>,
}
#[derive(Debug, Clone, Default)]
pub struct SlotView {
    pub slot: u32,
    pub label: String,
    pub exists: bool,
}
#[derive(Debug, Clone)]
pub struct UiModel {
    pub title: String,
    pub screen: Screen,
    pub nodes: Vec<Node>,
    pub transition: Option<(Vec<Node>, f32)>,
    pub stage: [f32; 2],
    pub dialogue: Option<DialogueView>,
    pub choices: Vec<ChoiceView>,
    pub prefs: Preferences,
    pub ui_locale: String,
    pub ui_fonts: Vec<String>,
    pub ui_font_plan_digest: String,
    pub text_locale: String,
    pub text_fonts: Vec<String>,
    pub text_font_plan_digest: String,
    pub locale_pending: bool,
    pub locale_error: Option<String>,
    pub preflight_texts: Vec<TextRun>,
    pub theme: Theme,
    pub history: Vec<HistoryView>,
    pub slots: Vec<SlotView>,
    pub paused: bool,
    pub loading: bool,
    pub status: String,
    pub fault: Option<String>,
    pub fault_recovery: Vec<Recovery>,
    pub auto: bool,
    pub skip: bool,
    pub outcome: Option<String>,
    pub history_offset: usize,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Quad {
    pub rect: [f32; 4],
    pub color: [f32; 4],
    pub asset: Option<String>,
    pub clip: Option<[f32; 4]>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    pub text: String,
    pub visible: Option<usize>,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub size: f32,
    pub color: [f32; 4],
    pub emphasis: Vec<(usize, usize)>,
    pub scroll: f32,
    pub clip: Option<[f32; 4]>,
    pub region: Option<ScrollRegion>,
    pub locale: String,
    pub font_assets: Vec<String>,
    pub font_plan_digest: String,
    pub preflight_only: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct SemanticNode {
    pub id: u32,
    pub label: String,
    pub action: UiAction,
    pub enabled: bool,
    pub rect: [f32; 4],
    pub locale: String,
}
#[derive(Debug, Default)]
pub struct DrawPacket {
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
    pub scrolls: Vec<ScrollView>,
    pub(crate) dialogue_hint: Option<usize>,
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
            "saves",
            "save",
            "load",
            "rollback",
            "exit",
            "auto",
            "skip",
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
            "music",
            "voice",
            "sfx",
            "motion",
            "export",
            "import",
            "retry",
            "loading",
            "paused",
            "ending",
            "empty-slot",
            "saved",
            "saving",
            "read-failed",
            "error-prepare",
            "error-storage",
            "error-render",
            "error-content",
            "error-host",
            "scroll-back",
            "scroll-forward",
            "title-hint",
            "gate-hint",
            "advance-hint",
            "reveal-hint",
            "history-back",
            "history-forward",
        ]
        .into_iter()
        .map(|id| self.text(locale, id))
        .collect()
    }
    pub fn diagnostic(&self, diagnostic: &Diagnostic, locale: &str) -> String {
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
        self.quads == other.quads
            && self.texts == other.texts
            && self.locale == other.locale
            && self.font_assets == other.font_assets
            && self.font_plan_digest == other.font_plan_digest
            && self.width == other.width
            && self.height == other.height
            && self.stage_size == other.stage_size
            && self.transition_layers == other.transition_layers
    }

    fn rect(&mut self, r: [f32; 4], c: [f32; 4]) {
        self.quads.push(Quad {
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
            color: c,
            emphasis: vec![],
            scroll: 0.,
            clip: None,
            region: None,
            locale: self.locale.clone(),
            font_assets: self.font_assets.clone(),
            font_plan_digest: self.font_plan_digest.clone(),
            preflight_only: false,
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
            id: self.semantics.len() as u32,
            label,
            action,
            enabled: true,
            rect: r,
            locale: self.locale.clone(),
        });
    }
    pub fn hit(&self, x: f32, y: f32) -> Option<UiAction> {
        self.semantics
            .iter()
            .rev()
            .find(|n| {
                n.enabled
                    && x >= n.rect[0]
                    && y >= n.rect[1]
                    && x <= n.rect[0] + n.rect[2]
                    && y <= n.rect[1] + n.rect[3]
            })
            .map(|n| n.action.clone())
    }
}
fn scene(packet: &mut DrawPacket, nodes: &[Node], stage: [f32; 2], alpha: f32) {
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
    fn visit(packet: &mut DrawPacket, nodes: &[Node], parent: Option<&str>, t: Transform) {
        let mut siblings: Vec<_> = nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.parent.as_deref() == parent)
            .collect();
        siblings.sort_by_key(|(i, n)| (n.order, *i));
        for (_, n) in siblings {
            let x = t.x + n.x * t.scale;
            let y = t.y + n.y * t.scale;
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
                packet.quads.push(Quad {
                    rect: [x, y, n.width * scale, n.height * scale],
                    color,
                    asset: n.asset.clone(),
                    clip,
                });
            }
            visit(
                packet,
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
    visit(
        packet,
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
}

pub fn project(m: &UiModel, width: f32, height: f32, messages: &Messages) -> DrawPacket {
    project_measured(m, width, height, messages, &[], 0.)
}
fn project_measured(
    m: &UiModel,
    width: f32,
    height: f32,
    messages: &Messages,
    choice_heights: &[f32],
    choice_offset: f32,
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
        p.quads.push(Quad {
            rect: [0., 0., width, height],
            color: [1., 1., 1., 1.],
            asset: Some("@transition".into()),
            clip: None,
        });
    } else {
        scene(&mut p, &m.nodes, m.stage, 1.);
    }
    match m.screen {
        Screen::Title => {
            p.rect([0., 0., width, height], [0.015, 0.035, 0.04, 0.57]);
            p.rect([margin, 32., 28., 2.], t.accent);
            p.text(
                "N I R   /   I N T E R A C T I V E   S T O R I E S",
                margin + 42.,
                22.,
                width - margin * 2. - 42.,
                11.,
                t.muted,
            );
            let y = (height * 0.36).max(120.);
            let title = if m.ui_locale == "zh-Hans" {
                m.title.split('·').next().unwrap_or(&m.title).trim()
            } else {
                m.title.split('·').nth(1).unwrap_or(&m.title).trim()
            };
            p.text(
                title,
                margin,
                y,
                width - margin * 2.,
                if narrow { 48. } else { 76. },
                t.text,
            );
            p.text(
                "NIR / VISUAL NOVEL",
                margin,
                y + 110.,
                width - margin * 2.,
                13.,
                t.accent,
            );
            let by = (y + 184.).min(height - 155.);
            p.button(
                msg("new-game"),
                UiAction::NewGame,
                [
                    margin,
                    by,
                    if narrow { width - margin * 2. } else { 240. },
                    54.,
                ],
                true,
                t,
            );
            p.button(
                msg("saves"),
                UiAction::Saves,
                [margin, by + 66., 150., 42.],
                false,
                t,
            );
            p.button(
                msg("settings"),
                UiAction::Settings,
                [margin + 162., by + 66., 110., 42.],
                false,
                t,
            );
            p.text(
                "01   /   WEB EDITION",
                margin,
                height - 34.,
                250.,
                11.,
                t.muted,
            );
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
        Screen::Story => {
            if let Some(d) = &m.dialogue {
                let h = if narrow {
                    (height * 0.40).max(t.dialogue.height).min(330.)
                } else {
                    t.dialogue.height
                }
                .min((height - 88.).max(80.));
                let top = match t.slots.dialogue {
                    DialogueComponent::Bottom => height - h - margin * 0.6,
                    DialogueComponent::Top => 64.,
                };
                let padding = t.dialogue.padding;
                p.rect([margin, top, width - margin * 2., h], t.panel);
                p.rect([margin, top, 3., h], t.accent);
                if !d.speaker.is_empty() {
                    p.text(
                        &d.speaker,
                        margin + padding,
                        top + 18.,
                        width - margin * 2. - padding * 2.,
                        16.,
                        t.accent,
                    );
                }
                let ty = top + if d.speaker.is_empty() { 24. } else { 52. };
                let size =
                    (t.dialogue.font_size - if narrow { 4. } else { 0. }) * m.prefs.font_scale;
                p.texts.push(TextRun {
                    text: d.full_text.clone(),
                    visible: Some(d.visible_text.len()),
                    x: margin + padding,
                    y: ty,
                    width: width - margin * 2. - padding * 2.,
                    height: (top + h - 43. - ty).max(30.),
                    size,
                    color: t.text,
                    emphasis: d.emphasis.clone(),
                    scroll: 0.,
                    clip: None,
                    region: Some(ScrollRegion::Dialogue),
                    locale: d.locale.clone(),
                    font_assets: d.font_assets.clone(),
                    font_plan_digest: d.font_plan_digest.clone(),
                    preflight_only: false,
                });
                p.announcement = d.full_text.clone();
                p.announcement_locale = d.locale.clone();
                p.dialogue_hint = Some(p.texts.len());
                p.text(
                    msg(if d.gate {
                        "gate-hint"
                    } else if d.ready {
                        "advance-hint"
                    } else {
                        "reveal-hint"
                    }),
                    width - margin - 130.,
                    top + h - 33.,
                    110.,
                    11.,
                    t.muted,
                );
                p.semantics.push(SemanticNode {
                    id: 0,
                    label: msg("continue"),
                    action: UiAction::Advance,
                    enabled: !d.gate,
                    rect: [margin, top, width - margin * 2., h - 42.],
                    locale: m.ui_locale.clone(),
                });
            }
            if !m.choices.is_empty() {
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
                            false,
                            t,
                        );
                        p.quads.last_mut().unwrap().clip = Some(viewport);
                        let text = p.texts.last_mut().unwrap();
                        text.size = 16. * m.prefs.font_scale;
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
            }
            let items = [
                ("menu", UiAction::Menu),
                ("auto", UiAction::ToggleAuto),
                ("skip", UiAction::ToggleSkip),
            ];
            for (i, (label, action)) in items.into_iter().enumerate() {
                let w = if narrow { 95. } else { 115. };
                p.button(
                    msg(label),
                    action,
                    [width - margin - (3 - i) as f32 * (w + 8.), 16., w, 36.],
                    (label == "auto" && m.auto) || (label == "skip" && m.skip),
                    t,
                );
            }
            if m.paused && !m.loading {
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
            let y = if height < 600. { 24. } else { 60. };
            let heading = match screen {
                Screen::Menu => "menu",
                Screen::Settings => "settings",
                Screen::History => "history",
                Screen::Saves => "saves",
                _ => "menu",
            };
            p.text(msg(heading), x, y, w, 30., t.text);
            p.rect([x, y + 53., w, 1.], t.accent);
            match screen {
                Screen::Menu => {
                    for (i, (k, a)) in [
                        ("close", UiAction::Close),
                        ("saves", UiAction::Saves),
                        ("settings", UiAction::Settings),
                        ("history", UiAction::History),
                        ("rollback", UiAction::Rollback),
                        ("exit", UiAction::Title),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        p.button(msg(k), a, [x, y + 76. + i as f32 * 59., w, 46.], i == 0, t);
                    }
                }
                Screen::Settings => {
                    let compact = height < 600.;
                    let yy = if compact { y + 50. } else { y + 74. };
                    let gap = if compact { 52. } else { 66. };
                    p.text(msg("ui-language"), x, yy, w, 16., t.muted);
                    p.button(
                        msg("language-zh"),
                        UiAction::UiLocale {
                            locale: "zh-Hans".into(),
                        },
                        [x, yy + 20., (w - 12.) / 2., if compact { 34. } else { 40. }],
                        m.prefs.ui_locale == "zh-Hans",
                        t,
                    );
                    p.button(
                        msg("language-en"),
                        UiAction::UiLocale {
                            locale: "en".into(),
                        },
                        [
                            x + (w + 12.) / 2.,
                            yy + 20.,
                            (w - 12.) / 2.,
                            if compact { 34. } else { 40. },
                        ],
                        m.prefs.ui_locale == "en",
                        t,
                    );
                    let ty = yy + gap;
                    p.text(msg("text-language"), x, ty, w, 16., t.muted);
                    p.button(
                        msg("language-zh"),
                        UiAction::TextLocale {
                            locale: "zh-Hans".into(),
                        },
                        [x, ty + 20., (w - 12.) / 2., if compact { 34. } else { 40. }],
                        m.prefs.text_locale == "zh-Hans",
                        t,
                    );
                    p.button(
                        msg("language-en"),
                        UiAction::TextLocale {
                            locale: "en".into(),
                        },
                        [
                            x + (w + 12.) / 2.,
                            ty + 20.,
                            (w - 12.) / 2.,
                            if compact { 34. } else { 40. },
                        ],
                        m.prefs.text_locale == "en",
                        t,
                    );
                    let status_y = ty + gap;
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
                    p.text(state_message, x, status_y, w, 13., t.muted);
                    if m.locale_error.is_some() || m.locale_pending {
                        if m.locale_error.is_some() {
                            p.button(
                                msg("retry"),
                                UiAction::LocaleRetry,
                                [x, status_y + 18., (w - 12.) / 2., 34.],
                                true,
                                t,
                            );
                        }
                        p.button(
                            msg("language-cancel"),
                            UiAction::LocaleCancel,
                            [
                                if m.locale_error.is_some() {
                                    x + (w + 12.) / 2.
                                } else {
                                    x
                                },
                                status_y + 18.,
                                if m.locale_error.is_some() {
                                    (w - 12.) / 2.
                                } else {
                                    w
                                },
                                34.,
                            ],
                            false,
                            t,
                        );
                    }
                    let controls_y = status_y + if compact { 45. } else { 58. };
                    p.text(
                        format!("{}   {:.0}%", msg("font-size"), m.prefs.font_scale * 100.),
                        x,
                        controls_y,
                        w - 145.,
                        18.,
                        t.text,
                    );
                    p.button(
                        msg("decrease"),
                        UiAction::FontSize { delta: -0.1 },
                        [
                            x + w - 128.,
                            controls_y - 8.,
                            58.,
                            if compact { 32. } else { 38. },
                        ],
                        false,
                        t,
                    );
                    p.button(
                        msg("increase"),
                        UiAction::FontSize { delta: 0.1 },
                        [
                            x + w - 58.,
                            controls_y - 8.,
                            58.,
                            if compact { 32. } else { 38. },
                        ],
                        false,
                        t,
                    );
                    for (i, (key, bus, v)) in [
                        ("music", AudioBus::Bgm, m.prefs.bgm_volume),
                        ("voice", AudioBus::Voice, m.prefs.voice_volume),
                        ("sfx", AudioBus::Sfx, m.prefs.sfx_volume),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let ry = controls_y
                            + if compact { 43. } else { 61. }
                            + i as f32 * if compact { 37. } else { 52. };
                        p.text(
                            format!("{}   {:.0}%", msg(key), v * 100.),
                            x,
                            ry,
                            w - 145.,
                            18.,
                            t.text,
                        );
                        p.button(
                            msg("decrease"),
                            UiAction::Volume { bus, delta: -0.1 },
                            [x + w - 128., ry - 7., 58., if compact { 32. } else { 38. }],
                            false,
                            t,
                        );
                        p.button(
                            msg("increase"),
                            UiAction::Volume { bus, delta: 0.1 },
                            [x + w - 58., ry - 7., 58., if compact { 32. } else { 38. }],
                            false,
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
                        [
                            x,
                            controls_y + if compact { 160. } else { 214. },
                            w,
                            if compact { 34. } else { 42. },
                        ],
                        m.prefs.reduced_motion,
                        t,
                    );
                }
                Screen::Saves => {
                    for (i, slot) in m.slots.iter().enumerate().take(3) {
                        let sy = y + 78. + i as f32 * 85.;
                        p.text(
                            format!(
                                "0{}   {}",
                                slot.slot + 1,
                                if slot.exists {
                                    slot.label.clone()
                                } else {
                                    msg("empty-slot")
                                }
                            ),
                            x,
                            sy,
                            w - 190.,
                            16.,
                            t.text,
                        );
                        p.button(
                            msg("save"),
                            UiAction::Save { slot: slot.slot },
                            [x + w - 176., sy - 6., 82., 40.],
                            false,
                            t,
                        );
                        p.button(
                            msg("load"),
                            UiAction::Load { slot: slot.slot },
                            [x + w - 86., sy - 6., 86., 40.],
                            false,
                            t,
                        );
                        p.semantics.last_mut().unwrap().enabled = slot.exists;
                    }
                    p.button(
                        msg("export"),
                        UiAction::Export,
                        [x, y + 347., (w - 12.) / 2., 42.],
                        false,
                        t,
                    );
                    p.button(
                        msg("import"),
                        UiAction::Import,
                        [x + (w + 12.) / 2., y + 347., (w - 12.) / 2., 42.],
                        false,
                        t,
                    );
                }
                Screen::History => {
                    let entries = m
                        .history
                        .iter()
                        .rev()
                        .skip(m.history_offset)
                        .take(3)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .cloned()
                        .collect::<Vec<_>>();
                    let scrollable_entry = entries
                        .iter()
                        .enumerate()
                        .max_by_key(|(_, entry)| entry.text.len())
                        .map(|(index, _)| index);
                    let row_height = ((height - y - 276.).max(60.) / 3.).max(40.);
                    for (i, entry) in entries.iter().enumerate() {
                        let text = if entry.speaker.is_empty() {
                            entry.text.clone()
                        } else {
                            format!("{}  /  {}", entry.speaker, entry.text)
                        };
                        p.texts.push(TextRun {
                            text,
                            visible: None,
                            x,
                            y: y + 76. + i as f32 * row_height,
                            width: w,
                            height: row_height - 8.,
                            size: if narrow { 16. } else { 18. },
                            color: t.text,
                            emphasis: vec![],
                            scroll: 0.,
                            clip: (scrollable_entry == Some(i)).then_some([
                                x,
                                y + 76. + i as f32 * row_height,
                                w,
                                row_height - 8.,
                            ]),
                            region: (scrollable_entry == Some(i)).then_some(ScrollRegion::History),
                            locale: entry.locale.clone(),
                            font_assets: entry.font_assets.clone(),
                            font_plan_digest: entry.font_plan_digest.clone(),
                            preflight_only: false,
                        });
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
            if screen != Screen::Menu {
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
    if m.loading {
        p.rect([0., height - 38., width, 38.], t.background);
        p.text(
            msg("loading"),
            margin,
            height - 29.,
            width - 2. * margin,
            13.,
            t.accent,
        );
    }
    if !m.status.is_empty() && !matches!(m.screen, Screen::Settings) {
        p.text(&m.status, margin, 65., width - 2. * margin, 13., t.accent);
    }
    if let Some(error) = &m.fault {
        p.rect([margin, height * 0.3, width - 2. * margin, 140.], t.panel);
        p.texts.push(TextRun {
            text: error.clone(),
            visible: None,
            x: margin + 20.,
            y: height * 0.3 + 16.,
            width: width - 2. * margin - 40.,
            height: 65.,
            size: 15.,
            color: t.text,
            emphasis: vec![],
            scroll: 0.,
            clip: None,
            region: None,
            locale: m.ui_locale.clone(),
            font_assets: m.ui_fonts.clone(),
            font_plan_digest: m.ui_font_plan_digest.clone(),
            preflight_only: false,
        });
        if m.fault_recovery.contains(&Recovery::Retry) {
            p.button(
                msg("retry"),
                UiAction::Retry,
                [margin + 20., height * 0.3 + 90., 140., 38.],
                true,
                t,
            );
        }
        p.button(
            msg("exit"),
            UiAction::Title,
            [margin + 174., height * 0.3 + 90., 140., 38.],
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
    next_compat_font: u32,
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
            next_compat_font: 0,
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
    pub fn add_font(&mut self, bytes: Vec<u8>) {
        let id = format!("@compat-font-{}", self.next_compat_font);
        self.next_compat_font = self.next_compat_font.saturating_add(1);
        self.add_font_asset(&id, bytes)
            .expect("valid explicit font asset");
    }
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
            "{}:{}:{}:{}:{}:{:?}:{:?}:{}",
            run.size.to_bits(),
            run.width.to_bits(),
            run.height.to_bits(),
            run.locale,
            run.font_plan_digest,
            run.font_assets,
            run.emphasis,
            run.text
        )
    }
    /// Original UTF-8 offsets for the exact paragraph splitter used by shaping.
    pub fn line_offsets(run: &TextRun) -> Vec<usize> {
        let mut offsets: Vec<_> = if run.emphasis.is_empty() {
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
        self.missing_font = None;
        // Retain each formatted key for lookup and possible end-of-layout
        // eviction, avoiding a second key allocation for active runs.
        let packet_keys: Vec<_> = packet.texts.iter().map(Self::key).collect();
        for (r, key) in packet.texts.iter().zip(&packet_keys) {
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
                    cosmic_text::Metrics::new(r.size, r.size * 1.5),
                );
                b.set_size(&mut self.fonts, Some(r.width), None);
                let attrs = match primary {
                    Some(name) => cosmic_text::Attrs::new().family(cosmic_text::Family::Name(name)),
                    None => cosmic_text::Attrs::new().family(cosmic_text::Family::SansSerif),
                };
                if r.emphasis.is_empty() {
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
                                attrs.clone().color(cosmic_text::Color::rgb(224, 202, 153)),
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
                    rect: [1., 2., 30., 40.],
                    color: [0.1, 0.2, 0.3, 1.],
                    asset: Some("old-bg".into()),
                    clip: None,
                }],
                vec![Quad {
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
            id: 7,
            label: "continue".into(),
            action: UiAction::Advance,
            enabled: false,
            rect: [1., 2., 3., 4.],
            locale: "zh-Hans".into(),
        });
        semantically_changed.scrolls.push(ScrollView {
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
        }
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
