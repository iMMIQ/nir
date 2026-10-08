//! Host-owned output recovery controls. These never become story actions.
use super::{DrawPacket, Messages, Screen, UiModel};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Opening,
    Failed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Retry,
    Menu,
}
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Layout {
    pub status: Status,
    pub card: [f32; 4],
    pub retry: [f32; 4],
    pub menu: [f32; 4],
}
impl Layout {
    pub fn new(status: Status, screen: Screen, width: f32, height: f32) -> Option<Self> {
        // Menus stay fully operable, including saving while output is missing.
        // Returning to Story shows this host notice again if still needed.
        if !matches!(screen, Screen::Story | Screen::Ended)
            || !width.is_finite()
            || !height.is_finite()
            || width < 128.
            || height < 160.
        {
            return None;
        }
        let w = (width - 8.).min(440.);
        let card = [(width - w) / 2., (height - 144.) / 2., w, 144.];
        let button_w = (w - 32.) / 2.;
        Some(Self {
            status,
            card,
            retry: [card[0] + 12., card[1] + 92., button_w, 44.],
            menu: [card[0] + 20. + button_w, card[1] + 92., button_w, 44.],
        })
    }
    pub fn hit(&self, x: f32, y: f32, button: u8) -> Option<Action> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let contains = |r: [f32; 4]| x >= r[0] && y >= r[1] && x < r[0] + r[2] && y < r[1] + r[3];
        match button {
            0 if self.status == Status::Failed && contains(self.retry) => Some(Action::Retry),
            0 if contains(self.menu) => Some(Action::Menu),
            // Preserve the native right-click menu shortcut while blocked.
            2 => Some(Action::Menu),
            _ => None,
        }
    }
    pub fn paint(
        &self,
        packet: &mut DrawPacket,
        model: &UiModel,
        messages: &Messages,
        focus: Option<Action>,
    ) {
        let msg = |key| messages.text(&model.ui_locale, key);
        let [x, y, w, _] = self.card;
        let size = if w < 220. { 12. } else { 16. };
        packet.rect(
            [0., 0., packet.width, packet.height],
            [0.02, 0.045, 0.05, 0.72],
        );
        packet.rect(self.card, model.theme.panel);
        packet.text(
            msg(if self.status == Status::Opening {
                "sound-reconnecting"
            } else {
                "sound-unavailable"
            }),
            x + 12.,
            y + 8.,
            w - 24.,
            size,
            model.theme.accent,
        );
        packet.texts.last_mut().unwrap().height = 36.;
        packet.text(
            msg("sound-recovery-hint"),
            x + 12.,
            y + 44.,
            w - 24.,
            12.,
            model.theme.text,
        );
        packet.texts.last_mut().unwrap().height = 40.;
        for (action, rect, label, enabled) in [
            (
                Action::Retry,
                self.retry,
                msg(if self.status == Status::Opening {
                    "retrying"
                } else {
                    "retry"
                }),
                self.status == Status::Failed,
            ),
            (Action::Menu, self.menu, msg("menu"), true),
        ] {
            packet.rect(
                rect,
                if enabled {
                    model.theme.accent
                } else {
                    model.theme.background
                },
            );
            packet.text(
                label,
                rect[0] + 8.,
                rect[1] + 4.,
                rect[2] - 16.,
                13.,
                if enabled {
                    model.theme.background
                } else {
                    model.theme.muted
                },
            );
            packet.texts.last_mut().unwrap().height = 40.;
            if enabled && focus == Some(action) {
                let [x, y, w, h] = rect;
                for edge in [
                    [x, y, w, 2.],
                    [x, y + h - 2., w, 2.],
                    [x, y, 2., h],
                    [x + w - 2., y, 2., h],
                ] {
                    packet.rect(edge, [1., 0.85, 0.35, 1.]);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn controls_fit_portrait_landscape_and_small_viewports() {
        for (width, height) in [
            (320., 240.),
            (240., 320.),
            (1280., 720.),
            (160., 160.),
            (128., 160.),
        ] {
            let layout = Layout::new(Status::Failed, Screen::Story, width, height).unwrap();
            for [x, y, w, h] in [layout.card, layout.retry, layout.menu] {
                assert!(x >= 0. && y >= 0. && x + w <= width && y + h <= height);
            }
            assert!(layout.retry[2] >= 44. && layout.retry[3] >= 44.);
            assert!(layout.menu[2] >= 44. && layout.menu[3] >= 44.);
            assert!(layout.retry[0] + layout.retry[2] <= layout.menu[0]);
        }
    }
    #[test]
    fn opening_disables_retry_but_keeps_menu_and_faults_enable_retry() {
        for status in [Status::Opening, Status::Failed] {
            let l = Layout::new(status, Screen::Story, 320., 240.).unwrap();
            assert_eq!(
                l.hit(l.retry[0] + 20., l.retry[1] + 20., 0),
                (status == Status::Failed).then_some(Action::Retry)
            );
            assert_eq!(
                l.hit(l.menu[0] + 20., l.menu[1] + 20., 0),
                Some(Action::Menu)
            );
            assert_eq!(l.hit(0., 0., 0), None);
            assert_eq!(l.hit(f32::NAN, 0., 0), None);
        }
    }
    #[test]
    fn menus_and_title_never_get_covered_by_recovery_controls() {
        for screen in [
            Screen::Menu,
            Screen::Saves,
            Screen::Settings,
            Screen::History,
            Screen::Title,
        ] {
            assert!(Layout::new(Status::Failed, screen, 320., 240.).is_none());
        }
    }
}
