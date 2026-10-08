use std::{cell::Cell, collections::BTreeMap, rc::Rc};

/// A unique pause owner. Dropping it releases only this owner's pause.
#[derive(Debug)]
pub struct PauseToken {
    count: Rc<Cell<usize>>,
    audio_count: Option<Rc<Cell<usize>>>,
    pub reason: String,
}
impl Drop for PauseToken {
    fn drop(&mut self) {
        self.count.set(self.count.get() - 1);
        if let Some(count) = &self.audio_count {
            count.set(count.get() - 1);
        }
    }
}
#[derive(Default)]
pub(crate) struct Pauses {
    count: Rc<Cell<usize>>,
    audio_count: Rc<Cell<usize>>,
    named: BTreeMap<String, PauseToken>,
}
impl Pauses {
    pub fn acquire(&self, reason: String) -> PauseToken {
        self.acquire_with_audio(reason, true)
    }
    pub fn acquire_barrier(&self, reason: String) -> PauseToken {
        self.acquire_with_audio(reason, false)
    }
    fn acquire_with_audio(&self, reason: String, audio: bool) -> PauseToken {
        self.count.set(self.count.get() + 1);
        let audio_count = audio.then(|| {
            self.audio_count.set(self.audio_count.get() + 1);
            self.audio_count.clone()
        });
        PauseToken {
            count: self.count.clone(),
            audio_count,
            reason,
        }
    }
    pub fn insert(&mut self, reason: String) {
        self.insert_with_audio(reason, true);
    }
    /// A resource barrier freezes logical progress without stopping playback.
    pub fn insert_barrier(&mut self, reason: String) {
        self.insert_with_audio(reason, false);
    }
    fn insert_with_audio(&mut self, reason: String, audio: bool) {
        if !self.named.contains_key(&reason) {
            let token = self.acquire_with_audio(reason.clone(), audio);
            self.named.insert(reason, token);
        }
    }
    pub fn remove(&mut self, reason: &str) {
        self.named.remove(reason);
    }
    pub fn retain(&mut self, mut keep: impl FnMut(&String) -> bool) {
        self.named.retain(|reason, _| keep(reason));
    }
    pub fn contains(&self, reason: &str) -> bool {
        self.named.contains_key(reason)
    }
    pub fn is_empty(&self) -> bool {
        self.count.get() == 0
    }
    pub fn audio_paused(&self) -> bool {
        self.audio_count.get() != 0
    }
    pub fn is_only_named(&self, reason: &str) -> bool {
        self.count.get() == 1 && self.named.contains_key(reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn barriers_and_explicit_tokens_have_separate_playback_ownership() {
        let mut pauses = Pauses::default();
        pauses.insert_barrier("prepare".into());
        pauses.insert_barrier("prepare".into());
        pauses.insert_barrier("content".into());
        assert!(!pauses.is_empty());
        assert!(!pauses.audio_paused());
        let one = pauses.acquire("prepare".into());
        let two = pauses.acquire("prepare".into());
        pauses.insert("hidden".into());
        pauses.remove("prepare");
        pauses.retain(|reason| reason == "content");
        drop(one);
        assert!(pauses.audio_paused());
        drop(two);
        assert!(!pauses.audio_paused());
        assert!(!pauses.is_empty());
        pauses.remove("content");
        assert!(pauses.is_empty());
    }
}
