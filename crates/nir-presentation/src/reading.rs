use super::*;
#[path = "reading_flow.rs"]
mod flow;
pub(super) use flow::HistoryFlowView;

#[derive(Debug, Clone, Serialize)]
pub struct ScrollView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub menu: Option<MenuScrollIdentity>,
    pub region: ScrollRegion,
    pub rect: [f32; 4],
    pub offset: f32,
    pub max: f32,
    pub step: f32,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MenuScrollIdentity {
    pub instance: u32,
    pub revision: u32,
    pub window: String,
    pub layout: u32,
}
impl ScrollView {
    pub fn action(&self, delta: i32, page: bool) -> UiAction {
        if let Some(menu) = &self.menu {
            UiAction::MenuHistoryScroll {
                control: None,
                instance: menu.instance,
                revision: menu.revision,
                window: menu.window.clone(),
                layout: menu.layout,
                input: if page {
                    HistoryScrollInput::Page { delta }
                } else {
                    HistoryScrollInput::Step { delta }
                },
            }
        } else {
            UiAction::Scroll {
                region: self.region,
                delta,
            }
        }
    }
}
#[derive(Debug, Default)]
struct TextView {
    offset: f32,
    follow: bool,
    shape: String,
    anchor: usize,
}
#[derive(Debug, Default)]
pub struct ReadingState {
    touch_input: bool,
    pub(crate) bar_pointer: Option<[f32; 2]>,
    pub(crate) bar_pressed: Option<(MenuScrollIdentity, String, HistoryBarPart)>,
    pub(crate) bar_gesture: Option<crate::scrollbar::BarGesture>,
    flow: Option<flow::State>,
    flow_pending: bool,
    identity: Option<(u32, u32)>,
    history_identity: Option<(u32, usize, usize)>,
    dialogue: TextView,
    history: TextView,
    choice_offset: f32,
    panels: PanelOffsets,
}
impl ReadingState {
    pub fn set_touch_input(&mut self, touch: bool) -> bool {
        let changed = self.touch_input != touch;
        self.touch_input = touch;
        changed
    }
    pub fn matches(&self, identity: (u32, u32)) -> bool {
        self.identity == Some(identity)
    }
    pub fn hold_dialogue(&mut self) {
        self.dialogue.follow = false;
    }
    pub fn scroll(&mut self, region: ScrollRegion, delta: i32, packet: &DrawPacket) -> bool {
        let Some(view) = packet
            .scrolls
            .iter()
            .find(|v| v.region == region && v.menu.is_none())
        else {
            return false;
        };
        let offset = (view.offset + delta.clamp(-1, 1) as f32 * view.step).clamp(0., view.max);
        match region {
            ScrollRegion::Dialogue => {
                self.dialogue.offset = offset;
                self.dialogue.follow = delta > 0 && offset >= view.max;
            }
            ScrollRegion::Choices => self.choice_offset = offset,
            ScrollRegion::History => self.history.offset = offset,
            ScrollRegion::Menu => self.panels.menu = offset,
            ScrollRegion::Saves => self.panels.saves = offset,
            ScrollRegion::Settings => self.panels.settings = offset,
        }
        true
    }
    pub fn project(
        &mut self,
        m: &UiModel,
        identity: (u32, u32),
        width: f32,
        height: f32,
        messages: &Messages,
        text: &mut TextEngine,
    ) -> DrawPacket {
        if self.identity != Some(identity) {
            self.identity = Some(identity);
            self.dialogue = TextView {
                follow: true,
                ..Default::default()
            };
            self.choice_offset = 0.;
        }
        if m.auto || m.skip {
            self.dialogue.follow = true;
        }
        let history_key = (identity.0, m.history_offset, m.history.len());
        if self.history_identity != Some(history_key) {
            self.history_identity = Some(history_key);
            self.history = TextView::default();
        }
        // Measure full labels using the same font/width as the final glyph renderer.
        let mut labels = DrawPacket::default();
        if m.screen == Screen::Story {
            let w = m.theme.choice.width.min((width - 40.).max(80.));
            for c in &m.choices {
                labels.text(
                    &c.label,
                    0.,
                    0.,
                    w - 24.,
                    16. * m.prefs.font_scale,
                    m.theme.text,
                );
                let run = labels.texts.last_mut().unwrap();
                run.locale = c.locale.clone();
                run.font_assets = c.font_assets.clone();
                run.font_plan_digest = c.font_plan_digest.clone();
            }
        }
        text.layout(&labels);
        let heights: Vec<_> = labels
            .texts
            .iter()
            .map(|r| {
                text.buffers[&TextEngine::key(r)]
                    .layout_runs()
                    .map(|l| l.line_top + l.line_height)
                    .fold(0., f32::max)
                    .max(m.theme.choice.item_height - 12.)
                    + 12.
            })
            .collect();
        let mut measured = LayoutMeasurements {
            choices: &heights,
            touch_input: self.touch_input,
            ..Default::default()
        };
        let mut p = project_measured(
            m,
            width,
            height,
            messages,
            &measured,
            self.choice_offset,
            &self.panels,
        );
        if width < 650. && height >= 320. {
            if let (Some(rect), Some(run)) = (
                p.builtin_dialogue,
                p.texts
                    .iter()
                    .find(|r| r.region == Some(ScrollRegion::Dialogue)),
            ) {
                // Shape the complete sentence, never its currently revealed
                // prefix, so the panel does not move as characters appear.
                text.layout_texts(std::slice::from_ref(run));
                let bottom = text.buffers[&TextEngine::key(run)]
                    .layout_runs()
                    .map(|line| line.line_top + line.line_height)
                    .fold(0., f32::max);
                let needed = bottom.max(run.line_height * 2.) + rect[3] - run.height;
                let fitted = needed.max(140.).min(rect[3]);
                if fitted < rect[3] - 0.5 {
                    measured.dialogue_height = Some(fitted);
                    p = project_measured(
                        m,
                        width,
                        height,
                        messages,
                        &measured,
                        self.choice_offset,
                        &self.panels,
                    );
                }
            }
        }
        let paints = p.menu_paint.len();
        self.project_history_flow(&mut p, m, identity.0, messages, text);
        let added = p.menu_paint.len() - paints;
        self.project_history_bar(&mut p, m, added);
        // The page-root divert runs after the bar splice fixed its indices.
        super::divert_menu_page(&mut p, m);
        text.layout(&p);
        let mut views = vec![];
        for r in &mut p.texts {
            let Some(region) = r.region else {
                continue;
            };
            let view = match region {
                ScrollRegion::Dialogue => &mut self.dialogue,
                ScrollRegion::History => &mut self.history,
                ScrollRegion::Choices
                | ScrollRegion::Settings
                | ScrollRegion::Menu
                | ScrollRegion::Saves => continue,
            };
            let shape = TextEngine::key(r);
            let buffer = &text.buffers[&shape];
            let offsets = TextEngine::line_offsets(r);
            let visible = r.visible.unwrap_or(r.text.len());
            let mut bottom = 0f32;
            let mut lines = vec![];
            for line in buffer.layout_runs() {
                let off = offsets[line.line_i];
                let start = off + line.glyphs.iter().map(|g| g.start).min().unwrap_or(0);
                if line.glyphs.iter().any(|g| off + g.end <= visible)
                    || (line.glyphs.is_empty() && off <= visible)
                {
                    bottom = bottom.max(line.line_top + line.line_height);
                    lines.push((start, line.line_top));
                }
            }
            if region == ScrollRegion::Dialogue && bottom > r.height {
                // A fullscreen text window has no room above or below; keep
                // its overflow controls out of the readable clipped content.
                r.height = dialogue_content_height(r.y, r.height, p.height);
            }
            let max = (bottom - r.height).max(0.);
            if view.follow {
                view.offset = max;
            } else if !view.shape.is_empty() && view.shape != shape {
                // Preserve the first visible logical character when width/font changes.
                view.offset = lines
                    .iter()
                    .rev()
                    .find(|(start, _)| *start <= view.anchor)
                    .map(|(_, top)| *top)
                    .unwrap_or(0.);
            }
            view.offset = view.offset.clamp(0., max);
            view.anchor = lines
                .iter()
                .rev()
                .find(|(_, top)| *top <= view.offset + 0.5)
                .map(|(start, _)| *start)
                .unwrap_or(0);
            view.shape = shape;
            r.scroll = view.offset;
            if max > 0. {
                views.push(ScrollView {
                    menu: None,
                    region,
                    rect: [r.x, r.y, r.width, r.height],
                    offset: view.offset,
                    max,
                    step: (r.height - r.line_height).max(r.line_height),
                });
            }
        }
        if views.iter().any(|v| v.region == ScrollRegion::Dialogue) {
            if let Some(index) = p.dialogue_hint.take() {
                p.texts.remove(index);
            }
            if let Some(index) = p.dialogue_hint_quad.take() {
                p.quads.remove(index);
            }
        }
        self.choice_offset = p
            .scrolls
            .iter()
            .find(|v| v.region == ScrollRegion::Choices)
            .map(|v| v.offset)
            .unwrap_or(0.);
        self.panels.settings = p
            .scrolls
            .iter()
            .find(|v| v.region == ScrollRegion::Settings)
            .map_or(0., |v| v.offset);
        self.panels.menu = p
            .scrolls
            .iter()
            .find(|v| v.region == ScrollRegion::Menu)
            .map_or(0., |v| v.offset);
        self.panels.saves = p
            .scrolls
            .iter()
            .find(|v| v.region == ScrollRegion::Saves)
            .map_or(0., |v| v.offset);
        for v in views {
            controls(&mut p, v, m, messages);
        }
        apply_dialogue_offset(&mut p, m);
        project_ruby(&mut p, m, text);
        project_inline_images(&mut p, text);
        p
    }
}

fn project_ruby(packet: &mut DrawPacket, model: &UiModel, engine: &mut TextEngine) {
    use unicode_segmentation::UnicodeSegmentation;
    let Some(dialogue) = &model.dialogue else {
        return;
    };
    if dialogue.ruby.is_empty() {
        return;
    }
    let Some((base_index, base)) = packet
        .texts
        .iter()
        .enumerate()
        .find(|(_, r)| r.region == Some(ScrollRegion::Dialogue))
        .map(|(i, r)| (i, r.clone()))
    else {
        return;
    };
    let Some(buffer) = engine.buffers.get(&TextEngine::key(&base)) else {
        return;
    };
    let offsets = TextEngine::line_offsets(&base);
    let visible = base.visible.unwrap_or(base.text.len());
    let mut annotations = vec![];
    for (start, end, reading) in &dialogue.ruby {
        if *end > visible
            || *start >= *end
            || *end > base.text.len()
            || !base.text.is_char_boundary(*start)
            || !base.text.is_char_boundary(*end)
        {
            continue;
        }
        let base_clusters: Vec<_> = base.text[*start..*end]
            .grapheme_indices(true)
            .map(|(i, _)| start + i)
            .collect();
        let ruby_clusters: Vec<_> = reading.graphemes(true).collect();
        if base_clusters.is_empty() {
            continue;
        }
        for line in buffer.layout_runs() {
            let offset = offsets[line.line_i];
            let glyphs: Vec<_> = line
                .glyphs
                .iter()
                .filter(|g| offset + g.start >= *start && offset + g.end <= *end)
                .collect();
            if glyphs.is_empty() {
                continue;
            }
            let first = glyphs.iter().map(|g| offset + g.start).min().unwrap();
            let last = glyphs.iter().map(|g| offset + g.end).max().unwrap();
            let lo = base_clusters.partition_point(|i| *i < first) * ruby_clusters.len()
                / base_clusters.len();
            let hi = base_clusters.partition_point(|i| *i < last) * ruby_clusters.len()
                / base_clusters.len();
            if lo == hi {
                continue;
            }
            let left = glyphs.iter().map(|g| g.x).fold(f32::INFINITY, f32::min);
            let right = glyphs.iter().map(|g| g.x + g.w).fold(0., f32::max);
            let width = (right - left).max(1.);
            let mut run = base.clone();
            run.text = ruby_clusters[lo..hi].concat();
            run.x = base.x + left;
            run.y = base.y + line.line_top - base.scroll - base.size * 0.6;
            run.width = 8192.;
            run.size = base.size * 0.45;
            run.line_height = base.size * 0.6;
            run.height = run.line_height;
            run.visible = None;
            run.scroll = 0.;
            run.emphasis.clear();
            run.images.clear();
            run.region = None;
            // Include the reserved annotation space in the same clipping
            // surface and inherit the decorated window's opacity/shadow.
            let clip = base
                .clip
                .unwrap_or([base.x, base.y, base.width, base.height]);
            run.clip = Some([
                clip[0],
                clip[1] - base.size * 0.6,
                clip[2],
                clip[3] + base.size * 0.6,
            ]);
            annotations.push((run, width));
        }
    }
    for (mut run, width) in annotations {
        engine.layout_texts(std::slice::from_ref(&run));
        let measured = engine
            .buffers
            .get(&TextEngine::key(&run))
            .map(|b| {
                b.layout_runs()
                    .flat_map(|l| l.glyphs.iter().map(|g| g.x + g.w))
                    .fold(0., f32::max)
            })
            .unwrap_or(width);
        if measured > width {
            run.size *= width / measured;
            run.width = width + 1.;
        } else {
            run.x += (width - measured) / 2.;
            run.width = width + 1.;
        }
        let index = packet.texts.len();
        packet.texts.push(run);
        if let Some(layer) = &mut packet.window_layers {
            if layer.texts.contains(&base_index) {
                layer.texts.push(index);
            }
        }
        if let Some(layer) = &mut packet.menu_layers {
            if layer.texts.contains(&base_index) {
                layer.texts.push(index);
            }
        }
    }
}

fn project_inline_images(packet: &mut DrawPacket, engine: &TextEngine) {
    let mut images = vec![];
    for (index, run) in packet
        .texts
        .iter()
        .enumerate()
        .filter(|(_, r)| !r.monochrome && !r.preflight_only && !r.images.is_empty())
    {
        let Some(buffer) = engine.buffers.get(&TextEngine::key(run)) else {
            continue;
        };
        let offsets = TextEngine::line_offsets(run);
        let visible = run.visible.unwrap_or(run.text.len());
        for placement in &run.images {
            let at = placement.offset as usize;
            if at + 3 > visible {
                continue;
            }
            for line in buffer.layout_runs() {
                let offset = offsets[line.line_i];
                let Some(glyph) = line.glyphs.iter().find(|g| offset + g.start == at) else {
                    continue;
                };
                let image = &placement.image;
                let scale = run.image_scale;
                let box_height =
                    (image.height + image.margins[2] + image.margins[3]) as f32 * scale;
                let align = match image.align {
                    InlineImageAlign::Top => 0.,
                    InlineImageAlign::Center => (line.line_height - box_height) / 2.,
                    InlineImageAlign::Bottom => line.line_height - box_height,
                };
                let rect = [
                    run.x + glyph.x + image.margins[0] as f32 * scale,
                    run.y + line.line_top - run.scroll + align + image.margins[2] as f32 * scale,
                    image.width as f32 * scale,
                    image.height as f32 * scale,
                ];
                let clip = run.clip.unwrap_or([run.x, run.y, run.width, run.height]);
                images.push((
                    index,
                    Quad {
                        corners: None,
                        rect,
                        color: [1., 1., 1., run.color[3]],
                        asset: Some(image.asset.clone()),
                        clip: Some(clip),
                    },
                ));
                break;
            }
        }
    }
    for (text, quad) in images {
        if let Some(layer) = &mut packet.window_layers {
            if layer.texts.contains(&text) {
                layer.quads.push(quad);
                continue;
            }
        }
        if let Some(layer) = &mut packet.menu_layers {
            if layer.texts.contains(&text) {
                layer.quads.push(quad);
                continue;
            }
        }
        if let Some(position) = packet
            .menu_paint
            .iter()
            .position(|p| matches!(p, MenuPaint::Text(i) if *i == text))
        {
            let index = packet
                .menu_quad_range
                .map_or(packet.quads.len(), |(_, end)| end);
            packet.quads.insert(index, quad);
            for paint in &mut packet.menu_paint {
                if let MenuPaint::Quad(i) = paint {
                    if *i >= index {
                        *i += 1;
                    }
                }
            }
            if let Some((_, end)) = &mut packet.menu_quad_range {
                *end += 1;
            }
            if let Some(i) = &mut packet.dialogue_hint_quad {
                if *i >= index {
                    *i += 1;
                }
            }
            packet
                .menu_paint
                .insert(position + 1, MenuPaint::Quad(index));
        } else {
            packet.quads.push(quad);
        }
    }
}

pub(super) fn controls(p: &mut DrawPacket, view: ScrollView, m: &UiModel, messages: &Messages) {
    let first_quad = p.quads.len();
    let first_text = p.texts.len();
    let bounds = control_rects(&view, p.width, p.height);
    // Always reachable outside the clipped content, including while a Gate is latched.
    for (i, delta, label, enabled) in [
        (0, -1, "scroll-back", view.offset > 0.5),
        (1, 1, "scroll-forward", view.offset < view.max - 0.5),
    ] {
        let rect = bounds[i];
        let full_label = messages.text(&m.ui_locale, label);
        let painted_label = if matches!(
            view.region,
            ScrollRegion::Menu | ScrollRegion::Saves | ScrollRegion::Settings
        ) && rect[2] < 120.
        {
            messages.text(&m.ui_locale, &format!("{label}-compact"))
        } else {
            full_label.clone()
        };
        p.button(
            painted_label,
            UiAction::Scroll {
                region: view.region,
                delta,
            },
            rect,
            false,
            &m.theme,
        );
        p.semantics.last_mut().unwrap().label = full_label;
        p.semantics.last_mut().unwrap().enabled = enabled;
        if !enabled {
            p.texts.last_mut().unwrap().color = m.theme.muted;
        }
    }
    if view.region == ScrollRegion::Dialogue {
        let appearance = m.dialogue_appearance;
        for quad in &mut p.quads[first_quad..] {
            quad.color[3] *= appearance.opacity * appearance.background_opacity;
        }
        for text in &mut p.texts[first_text..] {
            text.color[3] *= appearance.opacity * appearance.text_opacity;
        }
    }
    p.scrolls.push(view);
}

// Reserve a control row only for overflowing text when neither side of the
// authored window has room. Its remaining content still uses the scroll view.
fn dialogue_content_height(top: f32, height: f32, viewport_height: f32) -> f32 {
    if top < 46. && top + height + 46. > viewport_height {
        (viewport_height - top - 46.).max(0.)
    } else {
        height
    }
}

fn control_rects(view: &ScrollView, width: f32, height: f32) -> [[f32; 4]; 2] {
    let [x, y, w, h] = view.rect;
    if view.region != ScrollRegion::Dialogue {
        let button_width = ((w - 6.) / 2.).min(144.);
        let button_height = if matches!(
            view.region,
            ScrollRegion::Menu | ScrollRegion::Saves | ScrollRegion::Settings
        ) {
            44.
        } else {
            36.
        };
        return [0., 1.].map(|i| {
            [
                x + i * (button_width + 6.),
                y + h + 2.,
                button_width,
                button_height,
            ]
        });
    }
    let button_width = ((w - 6.) / 2.)
        .clamp(44., 144.)
        .min(((width - 6.) / 2.).max(0.));
    let x = x.clamp(0., (width - 2. * button_width - 6.).max(0.));
    let button_height = 44.;
    let y = if y + h + 2. + button_height <= height {
        y + h + 2.
    } else {
        (y - 2. - button_height).max(0.)
    };
    [0., 1.].map(|i| [x + i * (button_width + 6.), y, button_width, button_height])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dialogue(rect: [f32; 4]) -> ScrollView {
        ScrollView {
            menu: None,
            region: ScrollRegion::Dialogue,
            rect,
            offset: 0.,
            max: 105.,
            step: 135.,
        }
    }

    #[test]
    fn bottom_dialogue_has_visible_touch_controls_without_covering_text() {
        // Imported 1024x768 textbox and its scaled 320x240 landscape viewport.
        for (width, height, rect) in [
            (1024., 768., [17., 578., 990., 175.]),
            (320., 240., [5.3, 180.6, 309.4, 54.6]),
        ] {
            let view = dialogue(rect);
            for [x, y, w, h] in control_rects(&view, width, height) {
                assert!(w >= 44. && h >= 44., "touch control too small");
                assert!(
                    x >= 0. && y >= 0. && x + w <= width && y + h <= height,
                    "scroll control clipped by viewport"
                );
                assert!(
                    y + h <= rect[1] || y >= rect[1] + rect[3],
                    "scroll control covers readable text"
                );
            }
        }
    }

    #[test]
    fn full_height_dialogue_reserves_room_and_keeps_content_scrollable() {
        let height = dialogue_content_height(0., 480., 480.);
        assert!(height > 0. && height < 480.);
        let view = dialogue([0., 0., 640., height]);
        for [_, y, _, h] in control_rects(&view, 640., 480.) {
            assert!(y >= height && y + h <= 480.);
        }
        // A textbox with room above doesn't sacrifice any text height.
        assert_eq!(dialogue_content_height(100., 380., 480.), 380.);
    }

    #[test]
    fn normal_dialogue_uses_room_below_while_other_scroll_regions_keep_layout() {
        let view = dialogue([12., 300., 616., 100.]);
        for [_, y, _, h] in control_rects(&view, 640., 480.) {
            assert_eq!(y, 402.);
            assert!(h >= 44.);
        }
        let history = ScrollView {
            region: ScrollRegion::History,
            ..view
        };
        assert_eq!(
            control_rects(&history, 640., 480.)[0],
            [12., 402., 144., 36.]
        );
    }
}
