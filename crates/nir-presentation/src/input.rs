use super::*;

/// Shared semantic routing. Hosts handle physical focus/IME and gesture identity;
/// only an empty Story background can fall back to dialogue advancement.
pub fn pointer_action(
    packet: &DrawPacket,
    model: &UiModel,
    x: f32,
    y: f32,
    button: u8,
) -> Option<UiAction> {
    if !x.is_finite() || !y.is_finite() || x < 0. || y < 0. || x > packet.width || y > packet.height
    {
        return None;
    }
    if model.interface_hidden && matches!(button, 0 | 2) {
        return Some(UiAction::RestoreInterface);
    }
    match button {
        0 => {
            if let Some(node) = packet.hit_node(x, y) {
                if node.action == UiAction::Advance
                    && (model.screen != Screen::Story
                        || model.loading
                        || model.paused
                        || !model.choices.is_empty())
                {
                    return None;
                }
                if !node.enabled {
                    return None;
                }
                if let Some(SemanticValue::Scrollbar {
                    value,
                    max,
                    thumb_top,
                    thumb_bottom,
                    ..
                }) = &node.value
                {
                    let mut action = node.action.clone();
                    if let UiAction::MenuHistoryScroll { input, .. } = &mut action {
                        *input = if y < *thumb_top {
                            HistoryScrollInput::Page { delta: -1 }
                        } else if y > *thumb_bottom {
                            HistoryScrollInput::Page { delta: 1 }
                        } else {
                            HistoryScrollInput::Position {
                                ratio: if *max > 0. { value / max } else { 0. },
                            }
                        };
                        return Some(action);
                    }
                    return None;
                }
                if let Some(SemanticValue::Range {
                    min,
                    max,
                    step,
                    from,
                    to,
                    ..
                }) = &node.value
                {
                    let value = min + ((x - from) / (to - from)).clamp(0., 1.) * (max - min);
                    return range_action(node, value, *min, *max, *step);
                }
                return Some(node.action.clone());
            }
            (model.screen == Screen::Story
                && !model.loading
                && !model.paused
                && model.choices.is_empty()
                && model.dialogue.is_some())
            .then_some(UiAction::Advance)
        }
        2 => match model.screen {
            Screen::Title if model.menu_depth > 0 => Some(UiAction::Close),
            Screen::Story => Some(UiAction::Menu),
            Screen::Menu | Screen::Settings | Screen::Saves | Screen::History => {
                Some(UiAction::Close)
            }
            _ => None,
        },
        _ => None,
    }
}

fn range_action(
    node: &SemanticNode,
    value: f32,
    min: f32,
    max: f32,
    step: f32,
) -> Option<UiAction> {
    if !node.enabled || !value.is_finite() {
        return None;
    }
    let UiAction::MenuValue {
        instance,
        revision,
        control,
        ..
    } = &node.action
    else {
        return None;
    };
    let value = if value >= max {
        max
    } else {
        (min + ((value - min) / step).round() * step).clamp(min, max)
    };
    Some(UiAction::MenuValue {
        instance: *instance,
        revision: *revision,
        control: control.clone(),
        value: nir_format::MenuValueInput::Number(value),
    })
}

/// 0/1 left/right, 2/3 home/end, 4/5 up/down. Vertical scrollbars
/// decrease on up; horizontal ranges increase on up, preserving old controls.
pub fn value_action(node: &SemanticNode, direction: u8) -> Option<UiAction> {
    if let Some(SemanticValue::Scrollbar {
        value, max, step, ..
    }) = &node.value
    {
        if !node.enabled || *max <= 0. {
            return None;
        }
        let value = match direction {
            0 | 4 => value - step,
            1 | 5 => value + step,
            2 => 0.,
            3 => *max,
            _ => return None,
        }
        .clamp(0., *max);
        let mut action = node.action.clone();
        if let UiAction::MenuHistoryScroll { input, .. } = &mut action {
            *input = HistoryScrollInput::Position { ratio: value / max };
            return Some(action);
        }
        return None;
    }

    let Some(SemanticValue::Range {
        min,
        max,
        step,
        value,
        ..
    }) = &node.value
    else {
        return None;
    };
    let target = match direction {
        0 | 5 => value - step,
        1 | 4 => value + step,
        2 => *min,
        3 => *max,
        _ => return None,
    };
    range_action(node, target, *min, *max, *step)
}

/// DOM/native focus can precede the owner's queued focus notification. Resolve
/// the presented control directly, retaining its page identity and reading the
/// current value/layout. Reused numeric IDs cannot authorize a different page.
pub fn control_value_action(
    packet: &DrawPacket,
    id: u32,
    expected: &UiAction,
    direction: u8,
) -> Option<UiAction> {
    let node = packet
        .semantics
        .iter()
        .find(|n| n.id == id && n.action.same_focus_target(expected) && focusable(n, packet))?;
    value_action(node, direction)
}

pub fn primary_action(packet: &DrawPacket, model: &UiModel) -> Option<UiAction> {
    if model.interface_hidden {
        return Some(UiAction::RestoreInterface);
    }
    if model.loading {
        return None;
    }
    let visible = |action: UiAction| {
        packet
            .semantics
            .iter()
            .any(|n| n.enabled && n.action == action)
            .then_some(action)
    };
    match model.screen {
        Screen::Title => packet
            .semantics
            .iter()
            .find(|n| {
                n.enabled
                    && (n.action == UiAction::NewGame
                        || match &n.action {
                            UiAction::MenuControl { control, .. } => model
                                .theme
                                .image_menus
                                .get(&model.image_menu)
                                .is_some_and(|menu| {
                                    menu.controls().any(|(id, action, _)| {
                                        id == control
                                            && matches!(
                                                action,
                                                nir_format::ImageMenuAction::NewGame
                                            )
                                    })
                                }),
                            _ => false,
                        })
            })
            .map(|n| n.action.clone()),
        Screen::Story if model.paused => visible(UiAction::Continue),
        Screen::Story
            if model.choices.is_empty() && (model.dialogue.is_some() || model.hidden_dialogue) =>
        {
            Some(UiAction::Advance)
        }
        _ => None,
    }
}

/// Keyboard focus is transient, and bound to both the rendered control and its
/// session/interaction/screen. Reused numeric ids cannot activate another action.
#[derive(Default)]
pub struct KeyboardFocus {
    target: Option<(u32, UiAction, (u32, u32), Screen)>,
}
impl KeyboardFocus {
    pub fn clear(&mut self) {
        self.target = None;
    }
    pub fn node<'a>(
        &self,
        packet: &'a DrawPacket,
        identity: (u32, u32),
        screen: Screen,
    ) -> Option<&'a SemanticNode> {
        let (id, action, owner, origin) = self.target.as_ref()?;
        if *owner != identity || *origin != screen {
            return None;
        }
        packet
            .semantics
            .iter()
            .find(|n| n.id == *id && n.action.same_focus_target(action) && focusable(n, packet))
    }
    pub fn select(
        &mut self,
        packet: &DrawPacket,
        identity: (u32, u32),
        screen: Screen,
        id: Option<u32>,
    ) {
        self.target = id
            .and_then(|id| {
                packet
                    .semantics
                    .iter()
                    .find(|n| n.id == id && focusable(n, packet))
            })
            .map(|n| (n.id, n.action.clone(), identity, screen));
    }
    /// 0/1: previous/next in declaration order; 2/3/4/5: left/right/up/down.
    pub fn navigate(
        &mut self,
        packet: &DrawPacket,
        identity: (u32, u32),
        screen: Screen,
        direction: u8,
    ) -> Option<u32> {
        if direction > 5 {
            return None;
        }
        let nodes: Vec<_> = packet
            .semantics
            .iter()
            .filter(|n| focusable(n, packet))
            .collect();
        if nodes.is_empty() {
            self.clear();
            return None;
        }
        let current = self.node(packet, identity, screen);
        let selected = if direction < 2 {
            let index = current.and_then(|c| nodes.iter().position(|n| n.id == c.id));
            let i = match index {
                Some(i) if direction == 0 => (i + nodes.len() - 1) % nodes.len(),
                Some(i) => (i + 1) % nodes.len(),
                None if direction == 0 => nodes.len() - 1,
                None => 0,
            };
            nodes[i]
        } else if let Some(current) = current {
            let center =
                |n: &SemanticNode| (n.rect[0] + n.rect[2] / 2., n.rect[1] + n.rect[3] / 2.);
            let (x, y) = center(current);
            nodes
                .iter()
                .copied()
                .filter_map(|n| {
                    let (nx, ny) = center(n);
                    let dx = nx - x;
                    let dy = ny - y;
                    let (forward, cross) = match direction {
                        2 => (-dx, dy.abs()),
                        3 => (dx, dy.abs()),
                        4 => (-dy, dx.abs()),
                        _ => (dy, dx.abs()),
                    };
                    (forward > 0.5).then_some((n, forward * forward + cross * cross * 4.))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(n, _)| n)
                .unwrap_or(current)
        } else {
            nodes[0]
        };
        let id = selected.id;
        self.select(packet, identity, screen, Some(id));
        Some(id)
    }
}
fn focusable(node: &SemanticNode, packet: &DrawPacket) -> bool {
    let [x, y, w, h] = node.rect;
    node.enabled
        && node.rect.iter().all(|v| v.is_finite())
        && w > 0.
        && h > 0.
        && x < packet.width
        && y < packet.height
        && x + w > 0.
        && y + h > 0.
}

#[cfg(test)]
mod tests {
    use super::*;
    fn packet() -> DrawPacket {
        let mut p = DrawPacket {
            width: 300.,
            height: 200.,
            ..Default::default()
        };
        for (id, rect, enabled) in [
            (1, [10., 10., 40., 30.], true),
            (2, [100., 10., 40., 30.], false),
            (3, [100., 60., 40., 30.], true),
            (4, [500., 0., 40., 30.], true),
        ] {
            p.semantics.push(SemanticNode {
                id,
                rect,
                enabled,
                label: id.to_string(),
                action: UiAction::Save { slot: id },
                locale: "en".into(),
                value: None,
            });
        }
        p
    }
    #[test]
    fn range_input_snaps_clamps_and_preserves_gesture_identity() {
        let mut node = packet().semantics.remove(0);
        node.action = UiAction::MenuValue {
            instance: 7,
            revision: 9,
            control: "volume".into(),
            value: nir_format::MenuValueInput::Number(0.5),
        };
        node.value = Some(SemanticValue::Range {
            min: 0.,
            max: 1.,
            step: 0.1,
            value: 0.5,
            from: 10.,
            to: 110.,
        });
        let increased = value_action(&node, 1).unwrap();
        assert!(node.action.same_pointer_target(&increased));
        assert!(
            matches!(increased, UiAction::MenuValue { value: nir_format::MenuValueInput::Number(v), .. } if (v-0.6).abs()<0.0001)
        );
        assert!(matches!(
            value_action(&node, 3),
            Some(UiAction::MenuValue {
                value: nir_format::MenuValueInput::Number(1.),
                ..
            })
        ));
        node.enabled = false;
        assert!(value_action(&node, 1).is_none());
    }
    #[test]
    fn value_key_can_precede_focus_notification_but_cannot_follow_a_reused_control() {
        let mut packet = packet();
        packet.semantics[0].action = UiAction::MenuHistoryScroll {
            instance: 7,
            revision: 9,
            window: "records".into(),
            layout: 11,
            control: Some("scroll".into()),
            input: HistoryScrollInput::Position { ratio: 0.5 },
        };
        packet.semantics[0].value = Some(SemanticValue::Scrollbar {
            value: 50.,
            max: 100.,
            step: 10.,
            from: 10.,
            to: 110.,
            thumb_top: 40.,
            thumb_bottom: 60.,
        });
        let expected = packet.semantics[0].action.clone();
        // No KeyboardFocus has been selected, as when DOM focus has queued
        // an owner notification and the key arrives before that owner turn.
        assert!(matches!(
            control_value_action(&packet, 1, &expected, 2),
            Some(UiAction::MenuHistoryScroll {
                input: HistoryScrollInput::Position { ratio: 0. },
                ..
            })
        ));
        if let UiAction::MenuHistoryScroll {
            revision, layout, ..
        } = &mut packet.semantics[0].action
        {
            *revision = 10;
            *layout = 12;
        }
        assert!(matches!(
            control_value_action(&packet, 1, &expected, 3),
            Some(UiAction::MenuHistoryScroll {
                revision: 10,
                layout: 12,
                input: HistoryScrollInput::Position { ratio: 1. },
                ..
            })
        ));
        packet.semantics[0].enabled = false;
        assert!(control_value_action(&packet, 1, &expected, 2).is_none());
        packet.semantics[0].enabled = true;
        if let UiAction::MenuHistoryScroll { instance, .. } = &mut packet.semantics[0].action {
            *instance = 8;
        }
        assert!(control_value_action(&packet, 1, &expected, 2).is_none());
        assert!(control_value_action(&packet, 2, &expected, 2).is_none());
    }
    #[test]
    fn focus_cycles_enabled_visible_controls_and_moves_spatially() {
        let p = packet();
        let mut f = KeyboardFocus::default();
        assert_eq!(f.navigate(&p, (1, 2), Screen::Saves, 1), Some(1));
        assert_eq!(f.navigate(&p, (1, 2), Screen::Saves, 3), Some(3));
        assert_eq!(f.navigate(&p, (1, 2), Screen::Saves, 5), Some(3));
        assert_eq!(f.navigate(&p, (1, 2), Screen::Saves, 1), Some(1));
        assert_eq!(f.navigate(&p, (1, 2), Screen::Saves, 0), Some(3));
        assert_eq!(f.navigate(&p, (1, 2), Screen::Saves, 4), Some(1));
    }
    #[test]
    fn stale_focus_does_not_follow_reused_control_ids() {
        let mut p = packet();
        let mut f = KeyboardFocus::default();
        f.select(&p, (1, 2), Screen::Saves, Some(1));
        assert!(f.node(&p, (2, 2), Screen::Saves).is_none());
        assert!(f.node(&p, (1, 3), Screen::Saves).is_none());
        assert!(f.node(&p, (1, 2), Screen::Menu).is_none());
        p.semantics[0].action = UiAction::NewGame;
        assert!(f.node(&p, (1, 2), Screen::Saves).is_none());
        f.select(&p, (1, 2), Screen::Saves, Some(2));
        assert!(f.node(&p, (1, 2), Screen::Saves).is_none());
    }
}
