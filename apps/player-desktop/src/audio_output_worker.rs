//! Bounded device-opening protocol; platform streams stay on their worker.
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc::{self, Receiver, SyncSender, TryRecvError},
    Arc,
};
pub(crate) enum WorkerReply<T> {
    Ready { generation: u64, handle: T },
    Failed { generation: u64, message: String },
}
/// One executing open, one queued request, and one buffered reply. The owner
/// policy coalesces repeated retry inputs; terminal replies are never dropped.
pub(crate) struct Worker<T> {
    requests: SyncSender<u64>,
    replies: Receiver<WorkerReply<T>>,
    fault: Arc<AtomicU64>,
    last_generation: u64,
    closed: bool,
}
impl<T: Send + 'static> Worker<T> {
    #[cfg(test)]
    pub fn with_opener(
        open: impl FnMut(u64, Arc<AtomicU64>) -> Result<T, String> + Send + 'static,
    ) -> Self {
        Self::with_factory(move || open)
    }
    pub(crate) fn with_factory<F>(factory: impl FnOnce() -> F + Send + 'static) -> Self
    where
        F: FnMut(u64, Arc<AtomicU64>) -> Result<T, String> + 'static,
    {
        let (requests, input) = mpsc::sync_channel(1);
        let (output, replies) = mpsc::sync_channel(1);
        let fault = Arc::new(AtomicU64::new(0));
        let signal = fault.clone();
        std::thread::spawn(move || {
            let mut open = factory();
            while let Ok(generation) = input.recv() {
                let reply = match open(generation, signal.clone()) {
                    Ok(handle) => WorkerReply::Ready { generation, handle },
                    Err(message) => WorkerReply::Failed {
                        generation,
                        message: message.chars().take(1024).collect(),
                    },
                };
                if output.send(reply).is_err() {
                    break;
                }
            }
        });
        Self {
            requests,
            replies,
            fault,
            last_generation: 0,
            closed: false,
        }
    }
    pub fn request(&mut self, generation: u64) -> bool {
        if self.requests.try_send(generation).is_err() {
            return false;
        }
        self.last_generation = generation;
        true
    }
    pub fn take_fault(&self) -> u64 {
        self.fault.swap(0, Ordering::AcqRel)
    }
    pub fn closed(&self) -> bool {
        self.closed
    }
    pub fn drain(&mut self) -> Option<WorkerReply<T>> {
        match self.replies.try_recv() {
            Ok(reply) => Some(reply),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) if !self.closed => {
                self.closed = true;
                Some(WorkerReply::Failed {
                    generation: self.last_generation,
                    message: "E_AUDIO_OUTPUT_WORKER: output worker stopped".into(),
                })
            }
            Err(TryRecvError::Disconnected) => None,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_output_state::OutputState;
    use std::time::{Duration, Instant};
    fn reply<T: Send + 'static>(worker: &mut Worker<T>) -> WorkerReply<T> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(reply) = worker.drain() {
                return reply;
            }
            assert!(Instant::now() < deadline, "output worker reply timeout");
            std::thread::yield_now();
        }
    }
    #[test]
    fn platform_state_is_created_and_kept_on_worker_even_when_not_send() {
        let mut worker = Worker::with_factory(|| {
            let value = std::rc::Rc::new(std::cell::Cell::new(0u8));
            move |_, _| {
                value.set(value.get() + 1);
                Ok(value.get())
            }
        });
        for generation in [1, 2] {
            assert!(worker.request(generation));
            match reply(&mut worker) {
                WorkerReply::Ready { handle, .. } => assert_eq!(handle, generation as u8),
                _ => panic!("expected worker-local state"),
            }
        }
    }
    #[test]
    fn blocked_open_does_not_block_owner_and_duplicate_inputs_are_coalesced() {
        let (entered, started) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let mut worker = Worker::with_opener(move |generation, _| {
            entered.send(generation).unwrap();
            gate.recv().unwrap();
            Ok(generation)
        });
        let mut policy = OutputState::default();
        let generation = policy.request().unwrap();
        assert!(worker.request(generation));
        assert_eq!(
            started.recv_timeout(Duration::from_secs(5)).unwrap(),
            generation
        );
        for _ in 0..32 {
            assert!(worker.drain().is_none());
            assert_eq!(policy.request(), None);
        }
        release.send(()).unwrap();
        match reply(&mut worker) {
            WorkerReply::Ready { generation, handle } => {
                assert_eq!(generation, handle);
                assert!(policy.ready(generation));
            }
            _ => panic!("expected output handle"),
        }
    }
    #[test]
    fn callbacks_during_open_and_old_callbacks_do_not_resurrect_failed_output() {
        let mut worker = Worker::with_opener(|generation, fault| {
            fault.fetch_max(generation, Ordering::Release);
            Ok(generation)
        });
        let mut policy = OutputState::default();
        let generation = policy.request().unwrap();
        assert!(worker.request(generation));
        let row = reply(&mut worker);
        assert_eq!(worker.take_fault(), generation);
        assert!(policy.failed(generation));
        if let WorkerReply::Ready { generation, .. } = row {
            assert!(!policy.ready(generation));
        } else {
            panic!("expected handle");
        }
        let next = policy.request().unwrap();
        assert!(!policy.failed(generation));
        assert!(policy.ready(next));
    }
    #[test]
    fn panicked_output_worker_is_reported_once_and_never_as_success() {
        let mut worker = Worker::<u8>::with_opener(|_, _| panic!("controlled output crash"));
        assert!(worker.request(1));
        match reply(&mut worker) {
            WorkerReply::Failed { generation, .. } => assert_eq!(generation, 1),
            _ => panic!("dead output worker returned ready"),
        }
        assert!(worker.closed());
        assert!(worker.drain().is_none());
        assert!(!worker.request(2));
    }
}
