//! A window close waits for storage without blocking the owner thread.
//! Only completed replies release the storage worker's outstanding count.

#[derive(Default)]
pub(crate) enum Close {
    #[default]
    Idle,
    Waiting,
    SaveFailed,
    SavePending,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CloseAction {
    Continue,
    Wait,
    Exit,
    Cancel,
}

impl Close {
    /// Repeated close requests never bypass an outstanding write or erase a
    /// failure that has yet to be presented to the player.
    pub(crate) fn request(&mut self) -> bool {
        if self.pending() {
            return false;
        }
        *self = Self::Waiting;
        true
    }

    pub(crate) fn pending(&self) -> bool {
        !matches!(self, Self::Idle)
    }

    pub(crate) fn save_failed(&mut self) {
        if self.pending() {
            *self = Self::SaveFailed;
        }
    }
    pub(crate) fn save_pending(&mut self) {
        if self.pending() {
            *self = Self::SavePending;
        }
    }

    pub(crate) fn cancel(&mut self) {
        *self = Self::Idle;
    }

    /// A dialog or queued owner event can still produce a storage request;
    /// wait until those have been admitted as well as the current writes.
    pub(crate) fn poll(&mut self, outstanding: usize, owner_pending: bool) -> CloseAction {
        match self {
            Self::Idle => CloseAction::Continue,
            Self::SaveFailed | Self::SavePending => {
                self.cancel();
                CloseAction::Cancel
            }
            Self::Waiting if outstanding == 0 && !owner_pending => CloseAction::Exit,
            Self::Waiting => CloseAction::Wait,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_waits_for_all_completed_writes_and_an_open_dialog() {
        let mut close = Close::default();
        assert_eq!(close.poll(3, false), CloseAction::Continue);
        assert!(close.request());
        for count in [3, 2, 1] {
            assert_eq!(close.poll(count, false), CloseAction::Wait);
            assert!(!close.request());
        }
        assert_eq!(close.poll(0, true), CloseAction::Wait);
        assert_eq!(close.poll(0, false), CloseAction::Exit);
    }

    #[test]
    fn failed_save_cancels_even_the_last_pending_write() {
        let mut close = Close::default();
        close.request();
        close.save_failed();
        assert!(
            !close.request(),
            "a repeated close cannot hide save failure"
        );
        assert_eq!(close.poll(0, false), CloseAction::Cancel);
        assert!(!close.pending());
        assert_eq!(close.poll(0, false), CloseAction::Continue);
        // A later explicit close is a new user decision, after the failure
        // has been shown; it must not be turned into a permanent exit lock.
        assert!(close.request());
        assert_eq!(close.poll(0, false), CloseAction::Exit);
    }

    #[test]
    fn cancelling_close_keeps_pending_writes_in_the_background() {
        let mut close = Close::default();
        close.request();
        assert_eq!(close.poll(2, false), CloseAction::Wait);
        close.cancel();
        assert_eq!(close.poll(2, false), CloseAction::Continue);
        close.request();
        assert_eq!(close.poll(2, false), CloseAction::Wait);
    }

    #[test]
    fn failure_outside_a_close_does_not_request_one() {
        let mut close = Close::default();
        close.save_failed();
        assert_eq!(close.poll(0, false), CloseAction::Continue);
    }

    #[test]
    fn pending_save_cancels_close_without_claiming_completion() {
        let mut close = Close::default();
        assert!(close.request());
        close.save_pending();
        assert!(!close.request(), "repeated close cannot erase the notice");
        assert_eq!(close.poll(1, true), CloseAction::Cancel);
        assert_eq!(close.poll(1, true), CloseAction::Continue);
        assert!(!close.pending());
        assert!(close.request());
        assert_eq!(close.poll(1, false), CloseAction::Wait);
        assert_eq!(close.poll(0, true), CloseAction::Wait);
        assert_eq!(close.poll(0, false), CloseAction::Exit);
    }

    #[test]
    fn pending_save_outside_close_does_not_request_one() {
        let mut close = Close::default();
        close.save_pending();
        assert_eq!(close.poll(1, false), CloseAction::Continue);
        assert!(!close.pending());
    }
}
