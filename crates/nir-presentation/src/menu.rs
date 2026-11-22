use super::*;

pub(super) fn menu_service_enabled(
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
                && slot
                    .resolve_in(&m.menu_locals, m.slots.len().max(1) as u32)
                    .is_some_and(|slot| {
                        !m.busy_slots.contains(&slot)
                            && !m
                                .slots
                                .iter()
                                .any(|row| row.slot == slot && row.error.is_some())
                    })
        }
        ImageMenuAction::LoadSlot { slot } => {
            !m.replay_active
                && slot
                    .resolve_in(&m.menu_locals, m.slots.len().max(1) as u32)
                    .is_some_and(|slot| {
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
pub(super) fn menu_value_visual(
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
pub(super) fn menu_node(
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
pub(super) fn menu_elements(
    packet: &mut DrawPacket,
    menu: &ImageMenu,
    m: &UiModel,
    messages: &Messages,
) {
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
                            .and_then(|slot| {
                                slot.resolve_in(&m.menu_locals, m.slots.len().max(1) as u32)
                            })
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
                        .and_then(|slot| {
                            slot.resolve_in(&m.menu_locals, m.slots.len().max(1) as u32)
                        })
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
