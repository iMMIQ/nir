//! A recovery gesture stays consumed if the device becomes ready before up.
use nir_presentation::audio_recovery::{Action, Status};

#[derive(Debug, Clone, Copy)]
pub(crate) enum Key {
    Next,
    Previous,
    Activate,
    Menu,
    Retry,
    Other,
}
/// Navigation belongs to the host notice, never to the covered story choice.
#[derive(Default)]
pub(crate) struct Keyboard {
    focus: Option<Action>,
}
impl Keyboard {
    pub fn focused(&self) -> Option<Action> {
        self.focus
    }
    pub fn sync(&mut self, status: Option<Status>) {
        match (status, self.focus) {
            (None, _) => self.focus = None,
            (Some(Status::Opening), Some(Action::Retry)) => self.focus = Some(Action::Menu),
            _ => {}
        }
    }
    pub fn event(&mut self, status: Status, key: Key) -> Option<Action> {
        self.sync(Some(status));
        let retry_enabled = status == Status::Failed;
        match key {
            Key::Next | Key::Previous => {
                self.focus = Some(match self.focus {
                    Some(Action::Retry) => Action::Menu,
                    Some(Action::Menu) if retry_enabled => Action::Retry,
                    None if retry_enabled && matches!(key, Key::Next) => Action::Retry,
                    _ => Action::Menu,
                });
                None
            }
            Key::Activate => match self.focus {
                Some(Action::Menu) => Some(Action::Menu),
                _ if retry_enabled => Some(Action::Retry),
                _ => None,
            },
            Key::Menu => Some(Action::Menu),
            Key::Retry => retry_enabled.then_some(Action::Retry),
            Key::Other => None,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pointer {
    Mouse(u8),
    Touch(u64),
}
#[derive(Debug, Clone, Copy)]
pub(crate) enum Phase {
    Down,
    Move,
    Up,
    Cancel,
}
struct Press {
    pointer: Pointer,
    position: (f32, f32),
    target: Option<Action>,
}
#[derive(Default)]
pub(crate) struct Gesture {
    pressed: Option<Press>,
}
impl Gesture {
    pub fn clear(&mut self) {
        self.pressed = None;
    }
    pub fn event(
        &mut self,
        pointer: Pointer,
        phase: Phase,
        position: (f32, f32),
        visible: bool,
        target: Option<Action>,
    ) -> (bool, Option<Action>) {
        if let Some(press) = &self.pressed {
            let same = pointer == press.pointer;
            let action = if same
                && matches!(phase, Phase::Up)
                && visible
                && press.target == target
                && (position.0 - press.position.0).hypot(position.1 - press.position.1) < 20.
            {
                target
            } else {
                None
            };
            if same && matches!(phase, Phase::Up | Phase::Cancel) {
                self.pressed = None;
            }
            return (true, action);
        }
        if visible && matches!(phase, Phase::Down) {
            self.pressed = Some(Press {
                pointer,
                position,
                target,
            });
        }
        (visible, None)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keyboard_visits_enabled_controls_and_retains_menu_through_retry() {
        let mut keyboard = Keyboard::default();
        assert_eq!(keyboard.event(Status::Failed, Key::Next), None);
        assert_eq!(keyboard.focused(), Some(Action::Retry));
        keyboard.event(Status::Failed, Key::Next);
        assert_eq!(keyboard.focused(), Some(Action::Menu));
        assert_eq!(
            keyboard.event(Status::Failed, Key::Activate),
            Some(Action::Menu)
        );
        keyboard.event(Status::Failed, Key::Previous);
        assert_eq!(
            keyboard.event(Status::Failed, Key::Activate),
            Some(Action::Retry)
        );
        keyboard.sync(Some(Status::Opening));
        assert_eq!(keyboard.focused(), Some(Action::Menu));
        assert_eq!(keyboard.event(Status::Opening, Key::Retry), None);
        keyboard.event(Status::Opening, Key::Next);
        keyboard.event(Status::Opening, Key::Previous);
        assert_eq!(
            keyboard.event(Status::Opening, Key::Activate),
            Some(Action::Menu)
        );
        keyboard.sync(None);
        assert_eq!(keyboard.focused(), None);
    }
    #[test]
    fn opening_does_not_retry_on_enter_and_other_keys_do_not_change_focus() {
        let mut keyboard = Keyboard::default();
        assert_eq!(keyboard.event(Status::Opening, Key::Activate), None);
        assert_eq!(keyboard.event(Status::Opening, Key::Other), None);
        assert_eq!(keyboard.focused(), None);
        keyboard.event(Status::Failed, Key::Previous);
        assert_eq!(keyboard.focused(), Some(Action::Menu));
        assert_eq!(keyboard.event(Status::Failed, Key::Other), None);
        assert_eq!(keyboard.focused(), Some(Action::Menu));
        assert_eq!(
            keyboard.event(Status::Opening, Key::Menu),
            Some(Action::Menu)
        );
    }
    #[test]
    fn ready_between_down_and_up_consumes_the_entire_mouse_or_touch_gesture() {
        for pointer in [Pointer::Mouse(0), Pointer::Touch(7)] {
            let mut g = Gesture::default();
            assert_eq!(
                g.event(pointer, Phase::Down, (10., 10.), true, Some(Action::Retry)),
                (true, None)
            );
            assert_eq!(
                g.event(pointer, Phase::Up, (10., 10.), false, None),
                (true, None)
            );
            assert_eq!(
                g.event(pointer, Phase::Down, (10., 10.), false, None),
                (false, None)
            );
        }
    }
    #[test]
    fn menu_requires_a_complete_same_target_gesture_without_dragging() {
        let mut g = Gesture::default();
        let p = Pointer::Touch(7);
        g.event(p, Phase::Down, (10., 10.), true, Some(Action::Menu));
        assert_eq!(
            g.event(p, Phase::Up, (10., 10.), true, Some(Action::Menu)),
            (true, Some(Action::Menu))
        );
        g.event(p, Phase::Down, (10., 10.), true, Some(Action::Menu));
        assert_eq!(
            g.event(p, Phase::Up, (50., 10.), true, Some(Action::Menu)),
            (true, None)
        );
        g.event(p, Phase::Down, (10., 10.), true, None);
        assert_eq!(
            g.event(p, Phase::Up, (10., 10.), true, Some(Action::Retry)),
            (true, None)
        );
    }
    #[test]
    fn secondary_contacts_and_cancel_cannot_activate_or_replace_the_first_press() {
        let mut g = Gesture::default();
        let p = Pointer::Touch(7);
        let other = Pointer::Touch(8);
        g.event(p, Phase::Down, (10., 10.), true, Some(Action::Retry));
        assert_eq!(
            g.event(other, Phase::Down, (10., 10.), true, Some(Action::Menu)),
            (true, None)
        );
        assert_eq!(
            g.event(other, Phase::Up, (10., 10.), true, Some(Action::Menu)),
            (true, None)
        );
        assert_eq!(
            g.event(p, Phase::Cancel, (10., 10.), true, None),
            (true, None)
        );
        assert_eq!(
            g.event(p, Phase::Up, (10., 10.), true, Some(Action::Retry)),
            (true, None)
        );
    }
}
