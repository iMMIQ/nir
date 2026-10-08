//! Shared geometry and scoped controls for opt-in authored history audition.
use super::*;

pub(crate) fn window_capacities(
    packet: &DrawPacket,
    menu: &ImageMenu,
    m: &UiModel,
) -> std::collections::BTreeMap<String, (u32, usize)> {
    let mut result = std::collections::BTreeMap::new();
    if !menu.elements.iter().any(|e| {
        matches!(
            e.content,
            MenuContent::HistoryWindow {
                voice_controls: true,
                ..
            }
        )
    }) {
        return result;
    }
    let positions = menu.element_positions(
        &m.menu_locals,
        &m.profile,
        &m.menu_reading_modes,
        &m.menu_story,
        m.history_total > 0,
    );
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
        .map(|e| menu_node(e, positions[&e.id], m, None, [1.; 4]))
        .collect();
    for (id, quad) in scene_layout(packet, &nodes, m.stage, 1.) {
        let Some(e) = menu.elements.iter().find(|e| e.id == id) else {
            continue;
        };
        let MenuContent::HistoryWindow {
            voice_controls: true,
            row_height,
            limit,
            ..
        } = e.content
        else {
            continue;
        };
        let [_, y, w, h] = quad.rect;
        if w <= 0. || h <= 0. {
            continue;
        }
        let parent = quad.clip.unwrap_or(quad.rect);
        let top = y.max(parent[1]).max(0.);
        let bottom = (y + h).min(parent[1] + parent[3]).min(packet.height);
        let row_height = (row_height * w / e.rect[2]).max(if w >= 280. { 52. } else { 76. });
        let capacity =
            (((bottom - top).max(0.) / row_height).floor() as usize).clamp(1, limit as usize);
        result.insert(id, (m.menu_instance, capacity));
    }
    result
}

pub(crate) fn voice_layout(
    width: f32,
    enabled: bool,
    voices: usize,
) -> (f32, f32, Option<[f32; 4]>) {
    if !enabled || voices == 0 || width < 72. {
        return (width, 0., None);
    }
    if width >= 280. {
        (width - 126., 0., Some([width - 118., 0., 118., 44.]))
    } else {
        (width, 52., Some([0., 0., width.min(118.), 44.]))
    }
}
pub(crate) struct VoiceButton<'a> {
    pub window: &'a str,
    pub layout: Option<u32>,
    pub key: usize,
    pub id: u32,
    pub rect: [f32; 4],
    pub clip: [f32; 4],
    pub enabled: bool,
    pub alpha: f32,
}
pub(crate) fn button(
    packet: &mut DrawPacket,
    m: &UiModel,
    messages: &Messages,
    control: VoiceButton<'_>,
) {
    let VoiceButton {
        window,
        layout,
        key,
        id,
        rect,
        clip,
        enabled,
        alpha,
    } = control;
    // Only complete, touch-sized targets are offered; clipped rows remain text.
    if rect[0] < clip[0]
        || rect[1] < clip[1]
        || rect[0] + rect[2] > clip[0] + clip[2] + 0.01
        || rect[1] + rect[3] > clip[1] + clip[3] + 0.01
    {
        return;
    }
    let playback = m
        .history_voice
        .as_ref()
        .filter(|voice| voice.entry == key && voice.window.as_deref() == Some(window));
    let active = playback.is_some_and(|voice| !voice.failed);
    let label = if playback.is_some_and(|voice| voice.failed) {
        "history-voice-failed"
    } else if active {
        "history-voice-stop"
    } else {
        "history-voice"
    };
    let full = messages.text(&m.ui_locale, label);
    let compact = if rect[2] < 118. {
        messages.text(
            &m.ui_locale,
            if label == "history-voice-failed" {
                "retry"
            } else if active {
                "history-voice-stop-compact"
            } else {
                "voice"
            },
        )
    } else {
        full.clone()
    };
    let q = packet.quads.len();
    let t = packet.texts.len();
    packet.button(
        compact,
        UiAction::MenuHistoryVoice {
            instance: m.menu_instance,
            revision: m.menu_revision,
            window: window.into(),
            layout,
            entry: key,
            stop: active,
        },
        rect,
        active,
        &m.theme,
    );
    let node = packet.semantics.last_mut().unwrap();
    node.id = id;
    node.label = full;
    node.enabled = enabled
        && !m.loading
        && !m.locale_pending
        && m.fault.is_none()
        && m.menu_transition.is_none()
        && alpha > 0.;
    for quad in &mut packet.quads[q..] {
        quad.clip = Some(clip);
        quad.color[3] *= alpha;
    }
    for run in &mut packet.texts[t..] {
        run.clip = Some(clip);
        run.color[3] *= alpha;
        run.size = 14.;
        run.line_height = 21.;
        run.x = rect[0] + 12.;
        run.width = rect[2] - 24.;
        run.y = rect[1] + 10.;
    }
    packet
        .menu_paint
        .extend((q..packet.quads.len()).map(MenuPaint::Quad));
    packet
        .menu_paint
        .extend((t..packet.texts.len()).map(MenuPaint::Text));
}
