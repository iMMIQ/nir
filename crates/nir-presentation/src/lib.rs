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
    pub emphasis: Vec<(usize, usize)>,
}
#[derive(Debug, Clone)]
pub struct ChoiceView {
    pub id: String,
    pub label: String,
    pub enabled: bool,
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
    pub theme: Theme,
    pub history: Vec<(String, String)>,
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
#[derive(Debug, Clone)]
pub struct Quad {
    pub rect: [f32; 4],
    pub color: [f32; 4],
    pub asset: Option<String>,
    pub clip: Option<[f32; 4]>,
}
#[derive(Debug, Clone)]
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
}
#[derive(Debug, Clone, Serialize)]
pub struct SemanticNode {
    pub id: u32,
    pub label: String,
    pub action: UiAction,
    pub enabled: bool,
    pub rect: [f32; 4],
}
#[derive(Debug, Default)]
pub struct DrawPacket {
    pub quads: Vec<Quad>,
    pub texts: Vec<TextRun>,
    pub semantics: Vec<SemanticNode>,
    pub announcement: String,
    pub locale: String,
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
        locale: m.prefs.locale.clone(),
        ..Default::default()
    };
    let t = &m.theme;
    let msg = |id| messages.text(&m.prefs.locale, id);
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
            let title = if m.prefs.locale == "zh-Hans" {
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
                    "SPACE  →  READ     ·     ESC  →  MENU",
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
                });
                p.announcement = d.full_text.clone();
                p.dialogue_hint = Some(p.texts.len());
                p.text(
                    if d.gate {
                        "…"
                    } else if d.ready {
                        "SPACE / ↗"
                    } else {
                        "· · ·"
                    },
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
                        if !c.enabled {
                            text.color = t.muted;
                        }
                        let node = p.semantics.last_mut().unwrap();
                        node.enabled = c.enabled;
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
                    .map(|(_, text)| text.clone())
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
                    let yy = y + 74.;
                    p.text(msg("language"), x, yy, w, 16., t.muted);
                    p.button(
                        "简体中文".into(),
                        UiAction::Locale {
                            locale: "zh-Hans".into(),
                        },
                        [x, yy + 31., (w - 12.) / 2., 42.],
                        m.prefs.locale == "zh-Hans",
                        t,
                    );
                    p.button(
                        "English".into(),
                        UiAction::Locale {
                            locale: "en".into(),
                        },
                        [x + (w + 12.) / 2., yy + 31., (w - 12.) / 2., 42.],
                        m.prefs.locale == "en",
                        t,
                    );
                    p.text(
                        format!("{}   {:.0}%", msg("font-size"), m.prefs.font_scale * 100.),
                        x,
                        yy + 94.,
                        w - 145.,
                        18.,
                        t.text,
                    );
                    p.button(
                        "−".into(),
                        UiAction::FontSize { delta: -0.1 },
                        [x + w - 128., yy + 86., 58., 38.],
                        false,
                        t,
                    );
                    p.button(
                        "+".into(),
                        UiAction::FontSize { delta: 0.1 },
                        [x + w - 58., yy + 86., 58., 38.],
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
                        let ry = yy + 147. + i as f32 * 52.;
                        p.text(
                            format!("{}   {:.0}%", msg(key), v * 100.),
                            x,
                            ry,
                            w - 145.,
                            18.,
                            t.text,
                        );
                        p.button(
                            "−".into(),
                            UiAction::Volume { bus, delta: -0.1 },
                            [x + w - 128., ry - 7., 58., 38.],
                            false,
                            t,
                        );
                        p.button(
                            "+".into(),
                            UiAction::Volume { bus, delta: 0.1 },
                            [x + w - 58., ry - 7., 58., 38.],
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
                        [x, yy + 300., w, 42.],
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
                    let text = m
                        .history
                        .iter()
                        .rev()
                        .skip(m.history_offset)
                        .take(3)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .map(|(s, t)| {
                            if s.is_empty() {
                                t.clone()
                            } else {
                                format!("{s}  /  {t}")
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    p.texts.push(TextRun {
                        text,
                        visible: None,
                        x,
                        y: y + 76.,
                        width: w,
                        height: (height - y - 276.).max(40.),
                        size: if narrow { 16. } else { 18. },
                        color: t.text,
                        emphasis: vec![],
                        scroll: 0.,
                        clip: None,
                        region: Some(ScrollRegion::History),
                    });
                    p.button(
                        "←".into(),
                        UiAction::HistoryPage { delta: 3 },
                        [x, height - 135., 70., 40.],
                        false,
                        t,
                    );
                    p.button(
                        "→".into(),
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
    if !m.status.is_empty() {
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
    p
}

/// CPU shaping cache, kept independent of the GPU atlas and the VM.
pub struct TextEngine {
    pub fonts: cosmic_text::FontSystem,
    pub buffers: std::collections::BTreeMap<String, cosmic_text::Buffer>,
    pub shapes: u64,
}
impl Default for TextEngine {
    fn default() -> Self {
        let db = cosmic_text::fontdb::Database::new();
        Self {
            fonts: cosmic_text::FontSystem::new_with_locale_and_db("zh-Hans".into(), db),
            buffers: Default::default(),
            shapes: 0,
        }
    }
}
impl TextEngine {
    pub fn add_font(&mut self, bytes: Vec<u8>) {
        self.fonts.db_mut().load_font_data(bytes);
        let name = self
            .fonts
            .db()
            .faces()
            .next()
            .and_then(|face| face.families.first().map(|f| f.0.clone()));
        if let Some(name) = name {
            self.fonts.db_mut().set_sans_serif_family(name);
        }
        self.buffers.clear();
    }
    pub fn key(run: &TextRun) -> String {
        format!(
            "{}:{}:{}:{:?}:{}",
            run.size.to_bits(),
            run.width.to_bits(),
            run.height.to_bits(),
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
        for r in &packet.texts {
            let key = Self::key(r);
            if !self.buffers.contains_key(&key) {
                let mut b = cosmic_text::Buffer::new(
                    &mut self.fonts,
                    cosmic_text::Metrics::new(r.size, r.size * 1.5),
                );
                b.set_size(&mut self.fonts, Some(r.width), None);
                let attrs = cosmic_text::Attrs::new().family(cosmic_text::Family::SansSerif);
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
                self.buffers.insert(key, b);
                self.shapes += 1;
            }
        }
        if self.buffers.len() > 128 {
            let keys: std::collections::BTreeSet<_> = packet.texts.iter().map(Self::key).collect();
            self.buffers.retain(|k, _| keys.contains(k));
        }
    }
}

#[cfg(test)]
mod scene_tests {
    use super::*;
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
