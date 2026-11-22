//! Bounded, nonblocking owner admission for FIFO storage work.
//! A dedicated thread writes files without holding the mailbox lock. At most
//! 64 batches (queued, executing or awaiting owner delivery) are admitted.
//! Adjacent profile deltas merge up to 64 logical requests per batch;
//! preference updates replace the pending snapshot in constant space.
//! Save/load/list and unlike writes remain barriers.
//! Completed results have a one-entry channel: large loaded snapshots cannot
//! accumulate while the owner admits only one completion per turn.
use crate::{atomic_write, Storage};
use anyhow::{anyhow, Result};
use nir_format::{Diagnostic, Preferences};
use nir_player::{AppEvent, PersistenceKind, SaveEnvelope};
use nir_presentation::SlotView;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::PathBuf,
    sync::{mpsc, Arc, Condvar, Mutex},
    thread,
    time::{Duration, Instant},
};
const MAX_BATCHES: usize = 64;
const MAX_PROFILE_BATCH_REQUESTS: usize = 64;
const SAVE_CONFIRMATION_NOTICE: Duration = Duration::from_secs(3);

#[derive(Debug)]
pub(crate) enum IoRequest {
    Save {
        slot: u32,
        expected_revision: u32,
        job: u32,
        envelope: Box<SaveEnvelope>,
    },
    Load {
        slot: u32,
        job: u32,
    },
    List,
    ConfirmSave(SaveCheck),
    ReadMetadata(PersistenceKind),
    Export {
        job: u32,
        path: PathBuf,
        json: String,
    },
    WritePreferences(Preferences),
    MergeProfile(BTreeSet<String>),
    MergeProfileValues(BTreeMap<String, nir_format::Value>),
}
#[derive(Debug, Clone, Copy)]
pub(crate) enum IoFailure {
    Save(u32),
    Load(u32),
    List(u32),
    ConfirmSave(u32),
    Export(u32),
    Persist(PersistenceKind),
    ReadMetadata(PersistenceKind),
}
impl IoFailure {
    pub(crate) fn event(self, message: String) -> AppEvent {
        match self {
            Self::Save(job) => AppEvent::SaveFailed { job, message },
            Self::ConfirmSave(job) => AppEvent::SavePending { job, message },
            Self::Load(job) => AppEvent::SlotLoadFailed { job, message },
            Self::List(slots) => AppEvent::Slots(
                (0..slots)
                    .map(|slot| SlotView {
                        slot,
                        error: Some(message.clone()),
                        ..Default::default()
                    })
                    .collect(),
                BTreeMap::new(),
            ),
            Self::Persist(kind) => AppEvent::PersistenceFailed { kind, message },
            Self::ReadMetadata(kind) => AppEvent::PersistenceReadFailed { kind, message },
            Self::Export(job) => AppEvent::ExportFailed { job, message },
        }
    }
}
impl IoRequest {
    pub(crate) fn failure_for(&self, save_slots: u32) -> IoFailure {
        match self {
            Self::Save { job, .. } => IoFailure::Save(*job),
            Self::Load { job, .. } => IoFailure::Load(*job),
            Self::List => IoFailure::List(save_slots),
            Self::ConfirmSave(check) => IoFailure::ConfirmSave(check.job),
            Self::ReadMetadata(kind) => IoFailure::ReadMetadata(*kind),
            Self::Export { job, .. } => IoFailure::Export(*job),
            Self::WritePreferences(_) => IoFailure::Persist(PersistenceKind::Preferences),
            Self::MergeProfile(_) => IoFailure::Persist(PersistenceKind::Profile),
            Self::MergeProfileValues(_) => IoFailure::Persist(PersistenceKind::ProfileValues),
        }
    }
    #[cfg(test)]
    pub(crate) fn failure(&self) -> IoFailure {
        self.failure_for(nir_format::DEFAULT_SAVE_SLOTS)
    }
}
#[derive(Debug)]
pub(crate) enum IoReply {
    Event(AppEvent),
    #[cfg(test)]
    Done,
}
struct Completion {
    reply: IoReply,
    requests: usize,
}
struct Batch {
    request: IoRequest,
    requests: usize,
}
// Only a compact confirmation identity survives worker death, never another
// copy of the snapshot. The replacement worker can read but cannot replay it.
#[derive(Debug, Clone)]
pub(crate) struct SaveCheck {
    job: u32,
    slot: u32,
    expected_revision: u32,
    revision: u32,
    digest: String,
}
impl SaveCheck {
    fn read(&self, storage: &Storage) -> AppEvent {
        match storage.load(self.slot) {
            Ok(Some(record))
                if record.revision == self.revision && record.digest == self.digest =>
            {
                AppEvent::Saved {
                    job: self.job,
                    slot: self.slot,
                    revision: self.revision,
                }
            }
            Ok(record) if record.as_ref().map_or(0, |r| r.revision) == self.expected_revision => {
                AppEvent::SaveFailed {
                    job: self.job,
                    message:
                        "E_IO_NOT_COMMITTED: the stopped worker did not leave this save on disk"
                            .into(),
                }
            }
            Ok(_) => AppEvent::SaveFault {
                job: self.job,
                diagnostic: Box::new(Diagnostic::new(
                    "E_SAVE_CONFLICT",
                    "save",
                    "the stored record does not match the unconfirmed write",
                )),
            },
            Err(error) => AppEvent::SavePending {
                job: self.job,
                message: format!(
                    "E_IO_CONFIRM_READ: unable to confirm the stopped worker's write: {error}"
                ),
            },
        }
    }
}
struct UnconfirmedSave {
    check: SaveCheck,
    attempted: bool,
}
// Keep only settlement identity and progress keys, never a second save/export
// payload. Receipts remain until owner delivery, including while the worker
// executes or waits for reply capacity. A disconnected sender cannot lose the
// identity of its last request or turn an unconfirmed write into success.
struct Receipt {
    failure: IoFailure,
    requests: usize,
    profile: BTreeSet<String>,
    admitted: Instant,
    pending_notified: bool,
    save_check: Option<SaveCheck>,
}
#[derive(Default)]
struct QueueState {
    pending: VecDeque<Batch>,
    receipts: VecDeque<Receipt>,
    batches: usize,
    closed: bool,
    fault: Option<String>,
}
#[derive(Default)]
struct Mailbox {
    state: Mutex<QueueState>,
    ready: Condvar,
    #[cfg(test)]
    phase: std::sync::atomic::AtomicU8,
    #[cfg(test)]
    thread_id: std::sync::atomic::AtomicU32,
}
pub(crate) struct IoWorker {
    send: Arc<Mailbox>,
    replies: mpsc::Receiver<Completion>,
    outstanding: usize,
    // Admission failure is distinct from a completed write. Carry rejected
    // deltas into a later admitted profile merge, after all FIFO barriers.
    rejected_profile: BTreeSet<String>,
    storage: Option<Arc<Storage>>,
    unconfirmed_saves: BTreeMap<u32, UnconfirmedSave>,
}
impl IoWorker {
    #[cfg(test)]
    pub(crate) fn with_driver(
        storage: Arc<Storage>,
        handle: impl FnMut(IoRequest) -> IoReply + Send + 'static,
    ) -> (Self, thread::JoinHandle<()>) {
        let send = Arc::new(Mailbox::default());
        let jobs = send.clone();
        let (done, replies) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || run_requests(jobs, done, handle));
        (
            Self {
                send,
                replies,
                outstanding: 0,
                rejected_profile: BTreeSet::new(),
                storage: Some(storage),
                unconfirmed_saves: BTreeMap::new(),
            },
            worker,
        )
    }
    pub(crate) fn new(storage: Arc<Storage>) -> Self {
        let send = Arc::new(Mailbox::default());
        let jobs = send.clone();
        let (done, replies) = mpsc::sync_channel(1);
        let backend = storage.clone();
        if let Err(error) = thread::Builder::new()
            .name("nir-storage".into())
            .spawn(move || run_storage_worker(backend, jobs, done))
        {
            let mut state = send.state.lock().unwrap();
            state.closed = true;
            state.fault = Some(format!("E_IO_THREAD: {error}"));
        }
        Self {
            send,
            replies,
            outstanding: 0,
            rejected_profile: BTreeSet::new(),
            storage: Some(storage),
            unconfirmed_saves: BTreeMap::new(),
        }
    }
    pub(crate) fn outstanding(&self) -> usize {
        self.outstanding
    }
    pub(crate) fn metadata_pending(&self, kind: PersistenceKind) -> bool {
        self.send.state.lock().unwrap().receipts.iter().any(|receipt| {
            matches!(receipt.failure, IoFailure::Persist(k) | IoFailure::ReadMetadata(k) if k == kind)
        })
    }
    pub(crate) fn has_unconfirmed_saves(&self) -> bool {
        !self.unconfirmed_saves.is_empty()
    }
    fn confirmation_active(&self, job: u32) -> bool {
        self.send
            .state
            .lock()
            .unwrap()
            .receipts
            .iter()
            .any(|r| matches!(r.failure, IoFailure::ConfirmSave(id) if id == job))
    }
    pub(crate) fn confirmation_retry_enabled(&self) -> bool {
        self.storage.is_some()
            && !(self.outstanding > 0 && self.send.state.lock().unwrap().closed)
            && self
                .unconfirmed_saves
                .keys()
                .any(|job| !self.confirmation_active(*job))
    }
    pub(crate) fn confirmation_in_flight(&self) -> bool {
        self.unconfirmed_saves
            .keys()
            .any(|job| self.confirmation_active(*job))
    }
    fn start_confirmations(&mut self, retry: bool) {
        if self.storage.is_none()
            || (self.outstanding > 0 && self.send.state.lock().unwrap().closed)
        {
            return;
        }
        let checks = self
            .unconfirmed_saves
            .values()
            .filter(|s| (retry || !s.attempted) && !self.confirmation_active(s.check.job))
            .map(|s| s.check.clone())
            .collect::<Vec<_>>();
        for check in checks {
            self.unconfirmed_saves
                .get_mut(&check.job)
                .unwrap()
                .attempted = true;
            // An admission/spawn failure leaves the original save uncertain
            // and retryable. It must not spin on every owner turn.
            let _ = self.submit(IoRequest::ConfirmSave(check));
        }
    }
    pub(crate) fn retry_unconfirmed_saves(&mut self) {
        self.start_confirmations(true);
    }
    pub(crate) fn submit(&mut self, mut request: IoRequest) -> Result<()> {
        // Reopen only for a new request, after all old identities have been
        // settled. Never replay saves/exports whose write outcome is unknown.
        if self.outstanding == 0 && self.send.state.lock().unwrap().closed {
            if let Some(storage) = &self.storage {
                let rejected = std::mem::take(&mut self.rejected_profile);
                let unconfirmed = std::mem::take(&mut self.unconfirmed_saves);
                *self = Self::new(storage.clone());
                self.rejected_profile = rejected;
                self.unconfirmed_saves = unconfirmed;
            }
        }
        if let IoRequest::MergeProfile(keys) = &mut request {
            keys.append(&mut self.rejected_profile);
        }
        let save_slots = self
            .storage
            .as_ref()
            .map_or(nir_format::DEFAULT_SAVE_SLOTS, |s| s.save_slots());
        let failure = request.failure_for(save_slots);
        let save_check = match &request {
            IoRequest::Save {
                slot,
                expected_revision,
                job,
                envelope,
            } => Some(SaveCheck {
                job: *job,
                slot: *slot,
                expected_revision: *expected_revision,
                revision: envelope.revision,
                digest: envelope.digest.clone(),
            }),
            _ => None,
        };
        let profile = match &request {
            IoRequest::MergeProfile(keys) => keys.clone(),
            _ => BTreeSet::new(),
        };
        let mut state = self.send.state.lock().unwrap();
        let error = if state.closed {
            Some(state.fault.clone().unwrap_or_else(|| "E_IO_CLOSED".into()))
        } else {
            let merged = state.pending.back_mut().is_some_and(|batch| {
                match (&mut batch.request, &mut request) {
                    (IoRequest::MergeProfile(keys), IoRequest::MergeProfile(next)) => {
                        if batch.requests == MAX_PROFILE_BATCH_REQUESTS {
                            return false;
                        }
                        keys.append(next)
                    }
                    (
                        IoRequest::MergeProfileValues(values),
                        IoRequest::MergeProfileValues(next),
                    ) => {
                        if batch.requests == MAX_PROFILE_BATCH_REQUESTS {
                            return false;
                        }
                        values.extend(std::mem::take(next));
                    }
                    (IoRequest::WritePreferences(current), IoRequest::WritePreferences(next)) => {
                        *current = std::mem::take(next)
                    }
                    _ => return false,
                }
                batch.requests += 1;
                true
            });
            if merged {
                let receipt = state.receipts.back_mut().unwrap();
                receipt.requests += 1;
                receipt.profile.extend(profile);
                None
            } else if state.batches == MAX_BATCHES
                || (!matches!(failure, IoFailure::ConfirmSave(_))
                    && state.batches + self.unconfirmed_saves.len() >= MAX_BATCHES)
            {
                Some("E_IO_CAPACITY".into())
            } else {
                state.receipts.push_back(Receipt {
                    failure,
                    requests: 1,
                    profile,
                    admitted: Instant::now(),
                    pending_notified: false,
                    save_check,
                });
                state.pending.push_back(Batch {
                    request,
                    requests: 1,
                });
                state.batches += 1;
                self.outstanding += 1;
                drop(state);
                self.send.ready.notify_one();
                return Ok(());
            }
        };
        if let Some(error) = error {
            if let IoRequest::MergeProfile(keys) = request {
                self.rejected_profile.extend(keys);
            }
            return Err(anyhow!(error));
        }
        self.outstanding += 1;
        drop(state);
        self.send.ready.notify_one();
        Ok(())
    }
    pub(crate) fn drain(&mut self) -> Option<IoReply> {
        self.start_confirmations(false);
        let reply = self.drain_at(Instant::now());
        // Admit the first read before reporting physical idle, including on
        // a title/menu screen whose story clock no longer schedules turns.
        self.start_confirmations(false);
        reply
    }
    fn drain_at(&mut self, now: Instant) -> Option<IoReply> {
        let mut completion = match self.replies.try_recv() {
            Ok(c) => c,
            Err(mpsc::TryRecvError::Disconnected) => {
                let mut state = self.send.state.lock().unwrap();
                state.closed = true;
                state.pending.clear();
                let receipt = state.receipts.pop_front()?;
                self.outstanding -= receipt.requests;
                state.batches -= 1;
                self.rejected_profile.extend(receipt.profile);
                if let Some(check) = receipt.save_check {
                    let job = check.job;
                    self.unconfirmed_saves.insert(
                        job,
                        UnconfirmedSave {
                            check,
                            attempted: false,
                        },
                    );
                    return Some(IoReply::Event(AppEvent::SavePending { job, message: "E_IO_CLOSED: write confirmation was lost; the stopped worker's record needs checking".into() }));
                }
                return Some(IoReply::Event(receipt.failure.event(
                    match receipt.failure {
                        IoFailure::Save(_) | IoFailure::Export(_) | IoFailure::Persist(_) => "E_IO_CLOSED: storage worker stopped before confirmation; the write may have completed",
                        _ => "E_IO_CLOSED: storage worker stopped before confirming this request",
                    }.into(),
                )));
            }
            Err(mpsc::TryRecvError::Empty) => {
                // A filesystem operation cannot safely be cancelled after a
                // deadline. Notify without freeing its receipt or replaying
                // the write; the original native reply still decides success.
                let mut state = self.send.state.lock().unwrap();
                let receipt = state.receipts.iter_mut().find(|receipt| {
                    matches!(receipt.failure, IoFailure::Save(_))
                        && !receipt.pending_notified
                        && now.saturating_duration_since(receipt.admitted)
                            >= SAVE_CONFIRMATION_NOTICE
                })?;
                receipt.pending_notified = true;
                let IoFailure::Save(job) = receipt.failure else {
                    unreachable!()
                };
                return Some(IoReply::Event(AppEvent::SavePending {
                    job,
                    message: "E_IO_CONFIRMING: native save reply is still pending".into(),
                }));
            }
        };
        let mut state = self.send.state.lock().unwrap();
        let receipt = state.receipts.pop_front().unwrap();
        debug_assert_eq!(receipt.requests, completion.requests);
        if let IoFailure::ConfirmSave(job) = receipt.failure {
            let terminal = self
                .unconfirmed_saves
                .get(&job)
                .is_some_and(|entry| match &completion.reply {
                    IoReply::Event(AppEvent::Saved {
                        job: id,
                        slot,
                        revision,
                    }) => {
                        *id == job && *slot == entry.check.slot && *revision == entry.check.revision
                    }
                    IoReply::Event(
                        AppEvent::SaveFailed { job: id, .. } | AppEvent::SaveFault { job: id, .. },
                    ) => *id == job,
                    _ => false,
                });
            if terminal {
                self.unconfirmed_saves.remove(&job);
            } else if !matches!(&completion.reply,IoReply::Event(AppEvent::SavePending {job:id,..}) if *id==job)
            {
                completion.reply = IoReply::Event(AppEvent::SavePending {
                    job,
                    message:
                        "E_IO_CONFIRM_READ: confirmation reply did not match the original request"
                            .into(),
                });
            }
        }
        if matches!(
            &completion.reply,
            IoReply::Event(AppEvent::PersistenceFailed {
                kind: PersistenceKind::Profile,
                ..
            })
        ) {
            self.rejected_profile.extend(receipt.profile);
        }
        self.outstanding -= completion.requests;
        state.batches -= 1;
        Some(completion.reply)
    }
    pub(crate) fn delayed_save_pending(&self) -> bool {
        self.has_unconfirmed_saves()
            || self
                .send
                .state
                .lock()
                .unwrap()
                .receipts
                .iter()
                .any(|receipt| {
                    matches!(receipt.failure, IoFailure::Save(_)) && receipt.pending_notified
                })
    }
    #[cfg(test)]
    fn deadline_snapshot(&self) -> String {
        use std::sync::atomic::Ordering;
        let phase = self.send.phase.load(Ordering::Relaxed);
        let tid = self.send.thread_id.load(Ordering::Relaxed);
        let state = match self.send.state.try_lock() {
            Ok(state) => format!(
                "batches={} queued={} closed={} receipts={:?}",
                state.batches,
                state.pending.len(),
                state.closed,
                state
                    .receipts
                    .iter()
                    .map(|r| (
                        r.failure,
                        r.requests,
                        r.admitted.elapsed(),
                        r.pending_notified
                    ))
                    .collect::<Vec<_>>()
            ),
            Err(error) => format!("mailbox unavailable: {error}"),
        };
        #[cfg(target_os = "linux")]
        let kernel = ["wchan", "syscall", "status"].map(|file| {
            std::fs::read_to_string(format!("/proc/self/task/{tid}/{file}"))
                .unwrap_or_else(|error| error.to_string())
        });
        #[cfg(not(target_os = "linux"))]
        let kernel = ["unavailable"];
        format!(
            "outstanding={} phase={phase} tid={tid} {state} kernel={kernel:?}",
            self.outstanding
        )
    }
}
impl Drop for IoWorker {
    fn drop(&mut self) {
        self.send
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .closed = true;
        self.send.ready.notify_all();
    }
}
fn run_storage_worker(
    storage: Arc<Storage>,
    jobs: Arc<Mailbox>,
    replies: mpsc::SyncSender<Completion>,
) {
    let mut failed_profile = BTreeSet::new();
    run_requests(jobs, replies, |request| match request {
        IoRequest::Save {
            slot,
            expected_revision,
            job,
            envelope,
        } => IoReply::Event(match storage.save(slot, expected_revision, &envelope) {
            Ok(()) => AppEvent::Saved {
                job,
                slot,
                revision: envelope.revision,
            },
            Err(e) => AppEvent::SaveFailed {
                job,
                message: e.to_string(),
            },
        }),
        IoRequest::Load { slot, job } => IoReply::Event(match storage.load(slot) {
            Ok(Some(envelope)) => AppEvent::SlotLoaded {
                job,
                envelope: Box::new(envelope),
            },
            Ok(None) => AppEvent::SlotLoadFailed {
                job,
                message: "E_SAVE_MISSING".into(),
            },
            Err(e) => AppEvent::SlotLoadFailed {
                job,
                message: e.to_string(),
            },
        }),
        IoRequest::List => list_slots(&storage),
        IoRequest::ConfirmSave(check) => IoReply::Event(check.read(&storage)),
        IoRequest::ReadMetadata(kind) => IoReply::Event(match kind {
            PersistenceKind::Preferences => match storage.preferences() {
                Ok(value) => AppEvent::PreferencesRecovered(value.unwrap_or_default()),
                Err(error) => AppEvent::PersistenceReadFailed {
                    kind,
                    message: error.to_string(),
                },
            },
            PersistenceKind::ProfileValues => match storage.profile_values() {
                Ok(value) => AppEvent::ProfileValuesRecovered(value),
                Err(error) => AppEvent::PersistenceReadFailed {
                    kind,
                    message: error.to_string(),
                },
            },
            PersistenceKind::Profile => match storage.profile() {
                Ok(value) => AppEvent::ProfileRecovered(value),
                Err(error) => AppEvent::PersistenceReadFailed {
                    kind,
                    message: error.to_string(),
                },
            },
        }),
        IoRequest::Export { job, path, json } => {
            IoReply::Event(match atomic_write(&path, json.as_bytes()) {
                Ok(()) => AppEvent::Exported { job },
                Err(e) => AppEvent::ExportFailed {
                    job,
                    message: e.to_string(),
                },
            })
        }
        IoRequest::WritePreferences(p) => {
            persist(storage.write_preferences(&p), PersistenceKind::Preferences)
        }
        IoRequest::MergeProfileValues(values) => persist(
            storage.merge_profile_values(values),
            PersistenceKind::ProfileValues,
        ),
        IoRequest::MergeProfile(mut keys) => {
            keys.append(&mut failed_profile);
            let result = storage.merge_profile(keys.clone());
            if result.is_err() {
                failed_profile = keys;
            } else {
                failed_profile.clear();
            }
            persist(result, PersistenceKind::Profile)
        }
    });
}
fn run_requests(
    jobs: Arc<Mailbox>,
    replies: mpsc::SyncSender<Completion>,
    mut handle: impl FnMut(IoRequest) -> IoReply,
) {
    #[cfg(all(test, target_os = "linux"))]
    if let Ok(path) = std::fs::read_link("/proc/thread-self") {
        if let Some(tid) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.parse().ok())
        {
            jobs.thread_id
                .store(tid, std::sync::atomic::Ordering::Relaxed);
        }
    }
    loop {
        #[cfg(test)]
        jobs.phase.store(1, std::sync::atomic::Ordering::Relaxed); // mailbox/condition wait
        let batch = {
            let mut state = jobs.state.lock().unwrap();
            while state.pending.is_empty() && !state.closed {
                state = jobs.ready.wait(state).unwrap();
            }
            let Some(batch) = state.pending.pop_front() else {
                return;
            };
            batch
        };
        // No mailbox mutex is held during filesystem work or reply backpressure.
        #[cfg(test)]
        jobs.phase.store(2, std::sync::atomic::Ordering::Relaxed); // backend handler
        let reply = handle(batch.request);
        #[cfg(test)]
        jobs.phase.store(3, std::sync::atomic::Ordering::Relaxed); // reply backpressure
        if replies
            .send(Completion {
                reply,
                requests: batch.requests,
            })
            .is_err()
        {
            return;
        }
    }
}
fn persist(result: Result<()>, kind: PersistenceKind) -> IoReply {
    IoReply::Event(match result {
        Ok(()) => AppEvent::PersistenceStored(kind),
        Err(e) => AppEvent::PersistenceFailed {
            kind,
            message: e.to_string(),
        },
    })
}
fn list_slots(storage: &Storage) -> IoReply {
    let mut rows = Vec::new();
    let mut revisions = BTreeMap::new();
    for slot in 0..storage.save_slots() {
        let (value, error) = match storage.load(slot) {
            Ok(value) => (value, None),
            Err(e) => (None, Some(e.to_string())),
        };
        if let Some(ref envelope) = value {
            revisions.insert(slot, envelope.revision);
        }
        rows.push(SlotView {
            slot,
            label: value
                .as_ref()
                .map(|s| format!("#{}", s.revision))
                .unwrap_or_default(),
            exists: value.is_some(),
            error,
        });
    }
    IoReply::Event(AppEvent::Slots(rows, revisions))
}

#[cfg(test)]
mod tests;
