use super::*;

pub fn project(m: &UiModel, width: f32, height: f32, messages: &Messages) -> DrawPacket {
    let mut p = project_measured(
        m,
        width,
        height,
        messages,
        &LayoutMeasurements::default(),
        0.,
        &PanelOffsets::default(),
    );
    divert_menu_page(&mut p, m);
    apply_dialogue_offset(&mut p, m);
    p
}

pub(super) fn apply_dialogue_offset(packet: &mut DrawPacket, model: &UiModel) {
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

#[derive(Debug, Default)]
pub(super) struct PanelOffsets {
    pub(super) menu: f32,
    pub(super) saves: f32,
    pub(super) settings: f32,
}

#[derive(Default)]
pub(super) struct LayoutMeasurements<'a> {
    pub(super) choices: &'a [f32],
    pub(super) dialogue_height: Option<f32>,
    pub(super) touch_input: bool,
}

pub(super) fn dialogue_height(m: &UiModel, width: f32, height: f32, measured: Option<f32>) -> f32 {
    measured
        .unwrap_or_else(|| {
            if width < 650. {
                (height * 0.40).max(m.theme.dialogue.height).min(330.)
            } else {
                m.theme.dialogue.height
            }
        })
        .min((height - 88.).max(80.))
}

// Paint and hit testing share the viewport. Partially visible controls may be
// painted as context, but cannot be activated until their whole target fits.
// A narrow settings row puts its value above the controls so neither the
// value nor the hit target is squeezed to make room for the other.
pub(super) fn setting_range(
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

pub(super) fn scroll_panel(
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

pub(super) fn project_measured(
    m: &UiModel,
    width: f32,
    height: f32,
    messages: &Messages,
    measured: &LayoutMeasurements<'_>,
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
    // Responsive reader chrome may reserve space for the illustration. Fixed
    // author windows, image choices and stage decorations keep their geometry.
    if narrow
        && height > width
        && m.screen == Screen::Story
        && m.dialogue.is_some()
        && !m.hidden_dialogue
        && !m.interface_hidden
        && t.dialogue.rect.is_none()
        && t.slots.dialogue == DialogueComponent::Bottom
        && m.choices.is_empty()
        && m.dialogue_decorations.is_empty()
    {
        let minimum_top = 128.;
        let h = dialogue_height(m, width, height, measured.dialogue_height);
        let available = height - margin * 0.6 - minimum_top;
        let stage_height = (width * m.stage[1] / m.stage[0]).min(available - h - 16.);
        if stage_height > 0. {
            let top = minimum_top + (available - stage_height - h - 16.) / 2.;
            p.stage_viewport = Some([0., top, width, stage_height]);
        }
    }
    p.rect([0., 0., width, height], t.background);
    if let Some((src, progress)) = &m.transition {
        let mut a = DrawPacket {
            width,
            height,
            stage_viewport: p.stage_viewport,
            ..Default::default()
        };
        let mut b = DrawPacket {
            width,
            height,
            stage_viewport: p.stage_viewport,
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
                    "01   /   STORY",
                    margin,
                    height - 34.,
                    w.min(250.),
                    11.,
                    t.muted,
                );
            }
            if !narrow {
                p.text(
                    msg(if measured.touch_input {
                        "title-hint-touch"
                    } else {
                        "title-hint"
                    }),
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
                let mut h = dialogue_height(m, width, height, measured.dialogue_height);
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
                if let Some([_, y, _, height]) = p.stage_viewport {
                    top = y + height + 16.;
                }
                let mut left = margin;
                let mut box_width = width - margin * 2.;
                let mut scale = 1.;
                if let Some(rect) = t.dialogue.rect {
                    scale = (width / m.stage[0]).min(height / m.stage[1]);
                    left = (width - m.stage[0] * scale) / 2. + rect[0] * scale;
                    top = (height - m.stage[1] * scale) / 2. + rect[1] * scale;
                    box_width = rect[2] * scale;
                    h = rect[3] * scale;
                } else {
                    p.builtin_dialogue = Some([left, top, box_width, h]);
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
                    width: if t.dialogue.rect.is_some() {
                        box_width - padding * 2.
                    } else {
                        (box_width - padding * 2.).min(960.)
                    },
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
                            if measured.touch_input {
                                "advance-hint-touch"
                            } else {
                                "advance-hint"
                            }
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
                    let id = p.alloc_semantic_id();
                    p.semantics.push(SemanticNode {
                        value: None,
                        id,
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
                        measured
                            .choices
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
            // An enclosed reading surface keeps busy scene art out of menu
            // labels without requiring a blur pass or additional assets.
            p.rect(
                [x - 12., y - 8., w + 24., (height - y - 4.).max(0.)],
                t.background,
            );
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
                    let row_step = if stacked { 112. } else { 85. };
                    for (i, slot) in m.slots.iter().enumerate() {
                        let sy = yy + i as f32 * row_step;
                        p.text(
                            format!(
                                "{:02}   {}",
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
                    let transfer_y =
                        yy + m.slots.len() as f32 * row_step + if stacked { 0. } else { 18. };
                    let export_width = if stacked || !m.can_import {
                        w
                    } else {
                        (w - 12.) / 2.
                    };
                    p.button(
                        msg("export"),
                        UiAction::Export,
                        [x, transfer_y, export_width, 44.],
                        false,
                        t,
                    );
                    if m.can_import {
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
                    }
                    let view_height = (height - 128. - yy).max(1.);
                    let transfer_height = if stacked && m.can_import { 96. } else { 44. };
                    let content_height = transfer_y - yy + transfer_height;
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
    paint_control_feedback(&mut p, t, None, None);
    p
}
