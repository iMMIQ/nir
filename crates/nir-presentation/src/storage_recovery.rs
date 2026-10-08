//! A native host service control, independent of story actions and clocks.
use super::{DrawPacket, Messages, Screen, UiModel};
use nir_format::UiAction;
use serde::Serialize;

// Ordinary projection generates ascending control IDs. Reserving the maximum
// prevents a disappearing host button from aliasing a later authored control.
pub const CONTROL_ID: u32 = u32::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Status {
    pub pending: bool,
    pub retry_enabled: bool,
    pub save_pending: bool,
}
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Layout {
    pub status: Status,
    pub card: [f32; 4],
    pub retry: [f32; 4],
}
impl Layout {
    pub fn new(status: Status, screen: Screen, width: f32, height: f32) -> Option<Self> {
        // Reading remains unobstructed. The existing status is available in
        // menus/title where recovery can be explicitly requested.
        if matches!(screen, Screen::Story | Screen::Ended)
            || !width.is_finite()
            || !height.is_finite()
            || width < 128.
            || height < 160.
        {
            return None;
        }
        let w = (width - 16.).min(440.);
        let card = [width - w - 8., 8., w, 128.];
        Some(Self {
            status,
            card,
            retry: [card[0] + 12., card[1] + 76., w - 24., 44.],
        })
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        contains(self.card, x, y)
    }
    pub fn hit_retry(&self, x: f32, y: f32, button: u8) -> bool {
        button == 0 && self.status.retry_enabled && contains(self.retry, x, y)
    }
    pub fn paint(&self, packet: &mut DrawPacket, model: &UiModel, messages: &Messages) {
        // A covered authored control must not activate through keyboard
        // navigation or accessibility while this small host card is visible.
        for node in &mut packet.semantics {
            let r = node.rect;
            let c = self.card;
            if r[0] < c[0] + c[2] && r[0] + r[2] > c[0] && r[1] < c[1] + c[3] && r[1] + r[3] > c[1]
            {
                node.enabled = false;
            }
        }
        let msg = |key| messages.text(&model.ui_locale, key);
        let [x, y, w, _] = self.card;
        let small = w < 220.;
        packet.rect(self.card, model.theme.panel);
        packet.text(
            msg(if self.status.save_pending {
                "storage-save-unconfirmed"
            } else if self.status.pending {
                "storage-retrying"
            } else {
                "storage-unavailable"
            }),
            x + 12.,
            y + 4.,
            w - 24.,
            if small { 12. } else { 13. },
            model.theme.accent,
        );
        packet.texts.last_mut().unwrap().height = if small { 64. } else { 36. };
        if !small {
            packet.text(
                msg("storage-recovery-hint"),
                x + 12.,
                y + 40.,
                w - 24.,
                12.,
                model.theme.text,
            );
            packet.texts.last_mut().unwrap().height = 32.;
        }
        packet.button(
            msg("retry"),
            UiAction::Retry,
            self.retry,
            self.status.retry_enabled,
            &model.theme,
        );
        let node = packet.semantics.last_mut().unwrap();
        node.id = CONTROL_ID;
        node.enabled = self.status.retry_enabled;
    }
}
fn contains(r: [f32; 4], x: f32, y: f32) -> bool {
    x.is_finite() && y.is_finite() && x >= r[0] && y >= r[1] && x < r[0] + r[2] && y < r[1] + r[3]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_is_nonmodal_and_has_a_touch_sized_control_on_portrait_and_landscape() {
        let status = Status {
            pending: false,
            retry_enabled: true,
            save_pending: false,
        };
        for (w, h) in [
            (128., 160.),
            (240., 320.),
            (320., 240.),
            (720., 1280.),
            (1280., 720.),
        ] {
            for screen in [
                Screen::Title,
                Screen::Menu,
                Screen::Settings,
                Screen::Saves,
                Screen::History,
            ] {
                let l = Layout::new(status, screen, w, h).unwrap();
                for [x, y, a, b] in [l.card, l.retry] {
                    assert!(x >= 0. && y >= 0. && x + a <= w && y + b <= h);
                }
                assert!(l.retry[2] >= 44. && l.retry[3] >= 44.);
                assert!(l.hit_retry(l.retry[0] + 20., l.retry[1] + 20., 0));
                assert!(!l.contains(0., 0.));
                assert!(!l.hit_retry(f32::NAN, 0., 0));
            }
            for screen in [Screen::Story, Screen::Ended] {
                assert!(Layout::new(status, screen, w, h).is_none());
            }
        }
    }
    #[test]
    fn pending_disables_only_when_every_failed_kind_has_native_work_in_flight() {
        for enabled in [false, true] {
            let l = Layout::new(
                Status {
                    pending: true,
                    retry_enabled: enabled,
                    save_pending: false,
                },
                Screen::Settings,
                320.,
                240.,
            )
            .unwrap();
            assert_eq!(l.hit_retry(l.retry[0] + 20., l.retry[1] + 20., 0), enabled);
            assert!(!l.hit_retry(l.retry[0] + 20., l.retry[1] + 20., 2));
        }
    }
}
