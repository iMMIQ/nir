//! Storage I/O off the owner thread.
//!
//! Save, load, slot listing and preference/profile writes are blocking file
//! work that the owner thread previously performed mid-command-cycle. One
//! dedicated worker consumes them in FIFO order (a save followed by a load
//! of the same slot must not see the pre-write file), and the owner drains
//! at most one reply per turn, mirroring the loader's admission pacing.
//!
//! Save and load errors were always engine events (`Saved`/`SaveFailed`,
//! `SlotLoaded`/`SlotLoadFailed`); list and persist errors were fatal via
//! `?` - the `Fatal` reply keeps that fail-fast contract. Event replies now
//! reach the engine one turn later than the command that caused them, which
//! the engine already tolerates on asynchronous hosts.
use crate::Storage;
use anyhow::{anyhow, Result};
use nir_format::Preferences;
use nir_player::{AppEvent, SaveEnvelope};
use nir_presentation::SlotView;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::mpsc,
    thread,
};

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
    WritePreferences(Preferences),
    MergeProfile(BTreeSet<String>),
}
#[derive(Debug)]
pub(crate) enum IoReply {
    /// An engine event produced by the request, ready to feed.
    Event(AppEvent),
    /// A persist finished without an engine-visible result.
    Done,
    /// An error the owner must treat as fatal, as `?` did inline.
    Fatal(String),
}
pub(crate) struct IoWorker {
    send: mpsc::Sender<IoRequest>,
    replies: mpsc::Receiver<IoReply>,
    outstanding: usize,
}
impl IoWorker {
    pub(crate) fn new(storage: std::sync::Arc<Storage>) -> Self {
        let (send, jobs) = mpsc::channel::<IoRequest>();
        let (done, replies) = mpsc::channel::<IoReply>();
        thread::Builder::new()
            .name("nir-storage".into())
            .spawn(move || run_storage_worker(storage, jobs, done))
            .expect("E_IO_THREAD: storage worker failed to start");
        Self {
            send,
            replies,
            outstanding: 0,
        }
    }
    pub(crate) fn outstanding(&self) -> usize {
        self.outstanding
    }
    pub(crate) fn submit(&mut self, request: IoRequest) -> Result<()> {
        self.send
            .send(request)
            .map_err(|_| anyhow!("E_IO_CLOSED"))?;
        self.outstanding += 1;
        Ok(())
    }
    /// Takes at most one finished reply. One delivery per owner turn keeps
    /// ADR-007 admission pacing.
    pub(crate) fn drain(&mut self) -> Option<IoReply> {
        let reply = self.replies.try_recv().ok()?;
        self.outstanding -= 1;
        Some(reply)
    }
}
fn run_storage_worker(
    storage: std::sync::Arc<Storage>,
    jobs: mpsc::Receiver<IoRequest>,
    replies: mpsc::Sender<IoReply>,
) {
    for request in jobs {
        let reply = match request {
            IoRequest::Save {
                slot,
                expected_revision,
                job,
                envelope,
            } => {
                let event = match storage.save(slot, expected_revision, &envelope) {
                    Ok(()) => AppEvent::Saved {
                        job,
                        slot,
                        revision: envelope.revision,
                    },
                    Err(e) => AppEvent::SaveFailed {
                        job,
                        message: e.to_string(),
                    },
                };
                IoReply::Event(event)
            }
            IoRequest::Load { slot, job } => {
                let event = match storage.load(slot) {
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
                };
                IoReply::Event(event)
            }
            IoRequest::List => list_slots(&storage),
            IoRequest::WritePreferences(preferences) => {
                persist(storage.write_preferences(&preferences))
            }
            IoRequest::MergeProfile(keys) => persist(storage.merge_profile(keys)),
        };
        if replies.send(reply).is_err() {
            return;
        }
    }
}
fn persist(result: Result<()>) -> IoReply {
    match result {
        Ok(()) => IoReply::Done,
        Err(e) => IoReply::Fatal(e.to_string()),
    }
}
fn list_slots(storage: &Storage) -> IoReply {
    let mut rows = Vec::new();
    let mut revisions = BTreeMap::new();
    for slot in 0..3 {
        let value = match storage.load(slot) {
            Ok(value) => value,
            Err(e) => return IoReply::Fatal(e.to_string()),
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
        });
    }
    IoReply::Event(AppEvent::Slots(rows, revisions))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn envelope(slot: u32, revision: u32) -> Box<SaveEnvelope> {
        let program = serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        let player = nir_player::Player::new(program, "a".repeat(64), "Test".into()).unwrap();
        let snapshot = player.core().snapshot();
        Box::new(SaveEnvelope {
            format: 1,
            slot,
            revision,
            digest: nir_content::digest(&serde_json::to_vec(&snapshot).unwrap()),
            snapshot,
        })
    }

    fn drain_all(io: &mut IoWorker) -> Vec<IoReply> {
        let mut drained = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        while io.outstanding() > 0 {
            assert!(Instant::now() < deadline, "io replies never arrived");
            if let Some(reply) = io.drain() {
                drained.push(reply);
            } else {
                thread::sleep(Duration::from_millis(1));
            }
        }
        drained
    }

    #[test]
    fn saves_then_loads_round_trip_through_engine_events() {
        let temp = tempfile::tempdir().unwrap();
        let release = "a".repeat(64);
        let storage =
            std::sync::Arc::new(Storage::open(temp.path(), "game", "release", &release).unwrap());
        let mut io = IoWorker::new(storage.clone());
        io.submit(IoRequest::Save {
            slot: 0,
            expected_revision: 0,
            job: 4,
            envelope: envelope(0, 1),
        })
        .unwrap();
        io.submit(IoRequest::Load { slot: 0, job: 5 }).unwrap();
        let drained = drain_all(&mut io);
        assert_eq!(drained.len(), 2);
        match &drained[0] {
            IoReply::Event(AppEvent::Saved {
                job,
                slot,
                revision,
                ..
            }) => assert_eq!((*job, *slot, *revision), (4, 0, 1)),
            other => panic!("unexpected save reply: {other:?}"),
        }
        match &drained[1] {
            IoReply::Event(AppEvent::SlotLoaded { job, envelope }) => {
                assert_eq!(*job, 5);
                assert_eq!(envelope.revision, 1);
            }
            other => panic!("unexpected load reply: {other:?}"),
        }
    }

    #[test]
    fn replies_preserve_submission_order_for_same_slot() {
        let temp = tempfile::tempdir().unwrap();
        let release = "a".repeat(64);
        let storage =
            std::sync::Arc::new(Storage::open(temp.path(), "game", "release", &release).unwrap());
        let mut io = IoWorker::new(storage);
        io.submit(IoRequest::Load { slot: 1, job: 1 }).unwrap();
        io.submit(IoRequest::Save {
            slot: 1,
            expected_revision: 0,
            job: 2,
            envelope: envelope(1, 1),
        })
        .unwrap();
        io.submit(IoRequest::List).unwrap();
        let drained = drain_all(&mut io);
        // FIFO: the empty load precedes the save, and the listing after it
        // observes the written slot.
        match &drained[0] {
            IoReply::Event(AppEvent::SlotLoadFailed { message, .. }) => {
                assert_eq!(message, "E_SAVE_MISSING");
            }
            other => panic!("unexpected first reply: {other:?}"),
        }
        assert!(matches!(
            &drained[1],
            IoReply::Event(AppEvent::Saved { revision: 1, .. })
        ));
        match &drained[2] {
            IoReply::Event(AppEvent::Slots(rows, revisions)) => {
                assert_eq!(rows.len(), 3);
                assert!(rows[1].exists);
                assert_eq!(revisions.get(&1), Some(&1));
            }
            other => panic!("unexpected list reply: {other:?}"),
        }
    }

    #[test]
    fn preference_and_profile_writes_report_done() {
        let temp = tempfile::tempdir().unwrap();
        let release = "a".repeat(64);
        let storage =
            std::sync::Arc::new(Storage::open(temp.path(), "game", "release", &release).unwrap());
        let mut io = IoWorker::new(storage.clone());
        io.submit(IoRequest::WritePreferences(Preferences {
            font_scale: 1.5,
            ..Default::default()
        }))
        .unwrap();
        io.submit(IoRequest::MergeProfile(BTreeSet::from(["seen".into()])))
            .unwrap();
        let drained = drain_all(&mut io);
        assert!(matches!(drained[0], IoReply::Done));
        assert!(matches!(drained[1], IoReply::Done));
        assert_eq!(storage.preferences().unwrap().unwrap().font_scale, 1.5);
        assert!(storage.profile().unwrap().contains("seen"));
    }
}
