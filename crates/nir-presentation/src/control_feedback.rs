use super::*;

/// A transient visual target; it grants no authority to activate a control.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlTarget {
    action: UiAction,
    rect: [f32; 4],
}

impl ControlTarget {
    pub fn at(packet: &DrawPacket, x: f32, y: f32) -> Option<Self> {
        packet.hit_node(x, y).filter(|n| n.enabled).map(|n| Self {
            action: n.action.clone(),
            rect: n.rect,
        })
    }
    fn matches(&self, node: &SemanticNode) -> bool {
        self.action == node.action && self.rect == node.rect
    }
}

/// Paint only builtin solid buttons. Image menus and author surfaces retain
/// their own state assets, opacity and ordering.
pub fn paint_control_feedback(
    packet: &mut DrawPacket,
    theme: &Theme,
    hover: Option<&ControlTarget>,
    pressed: Option<&ControlTarget>,
) {
    for node in &packet.semantics {
        if matches!(node.action, UiAction::Advance | UiAction::RestoreInterface)
            || packet.menu_controls.contains_key(&node.id)
            || node.rect[2] <= 0.
            || node.rect[3] <= 0.
            || (node.enabled && !hover.is_some_and(|h| h.matches(node)))
        {
            continue;
        }
        let Some(quad) = packet.quads.iter_mut().rev().find(|q| {
            let [x, y, w, h] = q.rect;
            let [cx, cy, cw, ch] = q.clip.unwrap_or(q.rect);
            let visible = [
                x.max(cx),
                y.max(cy),
                ((x + w).min(cx + cw) - x.max(cx)).max(0.),
                ((y + h).min(cy + ch) - y.max(cy)).max(0.),
            ];
            visible == node.rect
        }) else {
            continue;
        };
        if quad.asset.is_some() || quad.corners.is_some() {
            continue;
        }
        if !node.enabled {
            for (i, color) in quad.color[..3].iter_mut().enumerate() {
                *color = theme.panel[i] * 0.6 + theme.background[i] * 0.4;
            }
            for run in &mut packet.texts {
                if run.region.is_none()
                    && run.x >= quad.rect[0]
                    && run.y >= quad.rect[1]
                    && run.x < quad.rect[0] + quad.rect[2]
                    && run.y < quad.rect[1] + quad.rect[3]
                {
                    run.color[..3].copy_from_slice(&theme.muted[..3]);
                }
            }
        } else if hover.is_some_and(|h| h.matches(node)) {
            let amount = if pressed.is_some_and(|p| p.matches(node)) {
                0.32
            } else {
                0.16
            };
            // Selected buttons already use accent; brighten them instead of
            // erasing their persistent selection with an identical tint.
            let target = if quad.color[..3] == theme.accent[..3] {
                theme.text
            } else {
                theme.accent
            };
            for (color, target) in quad.color[..3].iter_mut().zip(target) {
                *color += (target - *color) * amount;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn button(enabled: bool) -> DrawPacket {
        let mut p = DrawPacket::default();
        p.button(
            "Menu".into(),
            UiAction::Menu,
            [10., 10., 100., 44.],
            false,
            &Theme::default(),
        );
        p.semantics[0].enabled = enabled;
        p
    }
    #[test]
    fn hover_and_press_change_paint_but_preserve_hit_targets_and_text_geometry() {
        let theme = Theme::default();
        let mut normal = button(true);
        let target = ControlTarget::at(&normal, 20., 20.).unwrap();
        let geometry = normal.texts[0].clone();
        let original = normal.quads[0].color;
        paint_control_feedback(&mut normal, &theme, Some(&target), None);
        let hover = normal.quads[0].color;
        assert_ne!(original, hover);
        let mut press = button(true);
        paint_control_feedback(&mut press, &theme, Some(&target), Some(&target));
        assert_ne!(press.quads[0].color, hover);
        assert_eq!(press.texts[0], geometry);
        assert_eq!(press.hit(20., 20.), Some(UiAction::Menu));
        let mut stale = button(true);
        stale.semantics[0].action = UiAction::History;
        paint_control_feedback(&mut stale, &theme, Some(&target), Some(&target));
        assert_eq!(stale.quads[0].color, original);
    }
    #[test]
    fn disabled_buttons_cannot_hover_and_authored_images_are_not_repainted() {
        let theme = Theme::default();
        let mut p = button(false);
        assert!(ControlTarget::at(&p, 20., 20.).is_none());
        paint_control_feedback(&mut p, &theme, None, None);
        assert_eq!(p.texts[0].color, theme.muted);
        assert!(p.hit(20., 20.).is_none());
        let mut authored = button(true);
        authored.quads[0].asset = Some("author.button".into());
        let mut backdrop = authored.quads[0].clone();
        backdrop.asset = None;
        authored.quads.insert(0, backdrop);
        let target = ControlTarget::at(&authored, 20., 20.).unwrap();
        let original = authored.quads.clone();
        paint_control_feedback(&mut authored, &theme, Some(&target), Some(&target));
        assert_eq!(authored.quads, original);
    }
}
