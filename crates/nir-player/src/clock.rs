use std::{cell::Cell, rc::Rc};

/// Keeps the foreground timer scheduled while a finite UI effect is active.
/// The owner drops its lease on completion, cancellation or view disposal.
pub struct ForegroundClockToken {
    users: Rc<Cell<usize>>,
}
impl Drop for ForegroundClockToken {
    fn drop(&mut self) {
        self.users.set(self.users.get() - 1);
    }
}
#[derive(Default)]
pub(crate) struct ForegroundClockDemand {
    users: Rc<Cell<usize>>,
}
impl ForegroundClockDemand {
    pub fn acquire(&self) -> Option<ForegroundClockToken> {
        if self.users.get() >= nir_format::MAX_TASKS {
            return None;
        }
        self.users.set(self.users.get() + 1);
        Some(ForegroundClockToken {
            users: self.users.clone(),
        })
    }
    pub fn active(&self) -> bool {
        self.users.get() > 0
    }
}
