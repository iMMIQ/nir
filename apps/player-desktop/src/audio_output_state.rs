//! Output-device recovery owns a pause independently of menus and visibility.
//! Old stream callbacks and opening replies cannot resume a newer attempt.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Phase {
    #[default]
    Missing,
    Opening,
    Ready,
    Failed,
}
#[derive(Debug, Default)]
pub(crate) struct OutputState {
    generation: u64,
    phase: Phase,
}
impl OutputState {
    pub fn request(&mut self) -> Option<u64> {
        if self.phase == Phase::Opening {
            return None;
        }
        let next = self.generation.checked_add(1)?;
        self.generation = next;
        self.phase = Phase::Opening;
        Some(next)
    }
    pub fn ready(&mut self, generation: u64) -> bool {
        if generation != self.generation || self.phase != Phase::Opening {
            return false;
        }
        self.phase = Phase::Ready;
        true
    }
    pub fn failed(&mut self, generation: u64) -> bool {
        if generation != self.generation || !matches!(self.phase, Phase::Opening | Phase::Ready) {
            return false;
        }
        self.phase = Phase::Failed;
        true
    }
    pub fn pending(&self) -> bool {
        self.phase == Phase::Opening
    }
    pub fn blocked(&self) -> bool {
        self.phase != Phase::Ready
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_retries_do_not_queue_device_opens() {
        let mut s = OutputState::default();
        assert!(s.blocked());
        let first = s.request().unwrap();
        assert!(s.pending());
        for _ in 0..32 {
            assert_eq!(s.request(), None);
        }
        assert!(s.ready(first));
        assert!(!s.blocked());
        assert!(!s.ready(first));
    }
    #[test]
    fn old_callbacks_and_replies_cannot_release_a_new_device_wait() {
        let mut s = OutputState::default();
        let a = s.request().unwrap();
        assert!(s.ready(a));
        assert!(s.failed(a));
        let b = s.request().unwrap();
        assert!(!s.ready(a));
        assert!(!s.failed(a));
        assert!(s.blocked());
        assert!(s.ready(b));
        assert!(!s.failed(a));
        assert!(!s.blocked());
    }
    #[test]
    fn error_while_opening_cannot_be_undone_by_a_late_ready_reply() {
        let mut s = OutputState::default();
        let a = s.request().unwrap();
        assert!(s.failed(a));
        assert!(!s.ready(a));
        assert!(!s.pending());
        assert!(s.blocked());
        let b = s.request().unwrap();
        assert!(s.ready(b));
    }
    #[test]
    fn generation_exhaustion_does_not_reuse_an_old_callback_identity() {
        let mut s = OutputState {
            generation: u64::MAX,
            phase: Phase::Failed,
        };
        assert_eq!(s.request(), None);
        assert!(s.blocked());
        assert!(!s.pending());
    }
}
