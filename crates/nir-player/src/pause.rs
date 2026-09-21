use std::{cell::Cell, collections::BTreeMap, rc::Rc};

/// A unique pause owner. Dropping it releases only this owner's pause.
#[derive(Debug)]
pub struct PauseToken {
    count: Rc<Cell<usize>>,
    pub reason: String,
}
impl Drop for PauseToken {
    fn drop(&mut self) {
        self.count.set(self.count.get() - 1);
    }
}
#[derive(Default)]
pub(crate) struct Pauses {
    count: Rc<Cell<usize>>,
    named: BTreeMap<String, PauseToken>,
}
impl Pauses {
    pub fn acquire(&self, reason: String) -> PauseToken {
        self.count.set(self.count.get() + 1);
        PauseToken {
            count: self.count.clone(),
            reason,
        }
    }
    pub fn insert(&mut self, reason: String) {
        if !self.named.contains_key(&reason) {
            let token = self.acquire(reason.clone());
            self.named.insert(reason, token);
        }
    }
    pub fn remove(&mut self, reason: &str) {
        self.named.remove(reason);
    }
    pub fn retain(&mut self, mut keep: impl FnMut(&String) -> bool) {
        self.named.retain(|reason, _| keep(reason));
    }
    pub fn is_empty(&self) -> bool {
        self.count.get() == 0
    }
}
