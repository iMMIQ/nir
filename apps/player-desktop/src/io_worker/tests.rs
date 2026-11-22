use super::*;
use std::time::{Duration, Instant};

#[test]
fn metadata_reloads_preserve_bad_originals_and_recover_each_kind_independently() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(temp.path(), "game", "release", &"a".repeat(64)).unwrap());
    let prefs = Preferences {
        font_scale: 1.4,
        ..Default::default()
    };
    storage.write_preferences(&prefs).unwrap();
    storage
        .merge_profile(BTreeSet::from(["previous.line".into()]))
        .unwrap();
    let path = storage.root.join("preferences.json");
    let healthy = std::fs::read(&path).unwrap();
    std::fs::write(&path, b"{broken preferences").unwrap();
    let mut io = IoWorker::new(storage.clone());
    for kind in [PersistenceKind::Preferences, PersistenceKind::Profile] {
        io.submit(IoRequest::ReadMetadata(kind)).unwrap();
        assert!(io.metadata_pending(kind));
    }
    let result = drain_all(&mut io);
    assert!(
        matches!(&result[0],IoReply::Event(AppEvent::PersistenceReadFailed { kind:PersistenceKind::Preferences,message }) if !message.is_empty())
    );
    assert!(
        matches!(&result[1],IoReply::Event(AppEvent::ProfileRecovered(keys)) if keys.contains("previous.line"))
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"{broken preferences");
    for kind in [PersistenceKind::Preferences, PersistenceKind::Profile] {
        assert!(!io.metadata_pending(kind));
    }
    std::fs::write(&path, healthy).unwrap();
    io.submit(IoRequest::ReadMetadata(PersistenceKind::Preferences))
        .unwrap();
    let result = drain_all(&mut io);
    assert!(
        matches!(&result[..],[IoReply::Event(AppEvent::PreferencesRecovered(value))] if *value==prefs)
    );
    assert_eq!(
        storage.profile().unwrap(),
        BTreeSet::from(["previous.line".into()])
    );
}

#[test]
fn metadata_receipt_reserves_its_kind_until_original_reply_and_reports_worker_death_as_read_failure(
) {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(temp.path(), "game", "release", &"a".repeat(64)).unwrap());
    let (started, ready) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let (mut io, worker) = IoWorker::with_driver(storage, move |request| {
        assert!(matches!(
            request,
            IoRequest::ReadMetadata(PersistenceKind::Preferences)
        ));
        started.send(()).unwrap();
        gate.recv().unwrap();
        panic!("controlled read worker death");
    });
    io.submit(IoRequest::ReadMetadata(PersistenceKind::Preferences))
        .unwrap();
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(io.metadata_pending(PersistenceKind::Preferences));
    assert!(!io.metadata_pending(PersistenceKind::Profile));
    assert!(io.drain().is_none());
    release.send(()).unwrap();
    assert!(worker.join().is_err());
    let result = drain_all(&mut io);
    assert!(
        matches!(&result[..],[IoReply::Event(AppEvent::PersistenceReadFailed { kind:PersistenceKind::Preferences,message })] if message.contains("E_IO_CLOSED") && !message.contains("write may have completed"))
    );
    assert!(!io.metadata_pending(PersistenceKind::Preferences));
    io.submit(IoRequest::ReadMetadata(PersistenceKind::Preferences))
        .unwrap();
    let result = drain_all(&mut io);
    assert!(matches!(
        &result[..],
        [IoReply::Event(AppEvent::PreferencesRecovered(_))]
    ));
}

#[test]
fn failed_exports_preserve_old_files_then_retry_without_disabling_other_storage() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(temp.path(), "game", "release", &"a".repeat(64)).unwrap());
    let path = storage.root.join("export.json");
    std::fs::write(&path, b"original export").unwrap();
    let blocked = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::create_dir(&blocked).unwrap();
    let mut io = IoWorker::new(storage.clone());
    io.submit(IoRequest::Export {
        job: 7,
        path: path.clone(),
        json: "replacement".into(),
    })
    .unwrap();
    io.submit(IoRequest::WritePreferences(Preferences::default()))
        .unwrap();
    let result = drain_all(&mut io);
    assert!(
        matches!(&result[0],IoReply::Event(AppEvent::ExportFailed {job:7,message}) if !message.is_empty())
    );
    assert!(matches!(
        &result[1],
        IoReply::Event(AppEvent::PersistenceStored(PersistenceKind::Preferences))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"original export");
    std::fs::remove_dir(blocked).unwrap();
    io.submit(IoRequest::Export {
        job: 8,
        path: path.clone(),
        json: "replacement".into(),
    })
    .unwrap();
    let result = drain_all(&mut io);
    assert!(matches!(
        &result[0],
        IoReply::Event(AppEvent::Exported { job: 8 })
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
    assert!(storage.preferences().unwrap().is_some());
}

#[test]
fn export_is_an_unmerged_fifo_barrier_while_owner_admission_remains_nonblocking() {
    let send = Arc::new(Mailbox::default());
    let jobs = send.clone();
    let (done, replies) = mpsc::sync_channel(1);
    let (started, ready) = mpsc::channel();
    let (resume, blocked) = mpsc::channel();
    let (record, records) = mpsc::channel();
    thread::spawn(move || {
        run_requests(jobs, done, |request| {
            match request {
                IoRequest::List => {
                    started.send(()).unwrap();
                    blocked.recv().unwrap();
                }
                IoRequest::WritePreferences(p) => {
                    record.send(format!("pref:{}", p.font_scale)).unwrap()
                }
                IoRequest::Export { job, json, .. } => {
                    record.send(format!("export:{job}:{json}")).unwrap()
                }
                _ => panic!("unexpected request"),
            }
            IoReply::Done
        })
    });
    let mut io = IoWorker {
        send,
        replies,
        outstanding: 0,
        rejected_profile: BTreeSet::new(),
        storage: None,
        unconfirmed_saves: BTreeMap::new(),
    };
    io.submit(IoRequest::List).unwrap();
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    for value in [1., 2.] {
        io.submit(IoRequest::WritePreferences(Preferences {
            font_scale: value,
            ..Default::default()
        }))
        .unwrap();
    }
    io.submit(IoRequest::Export {
        job: 1,
        path: "first.json".into(),
        json: "first".into(),
    })
    .unwrap();
    io.submit(IoRequest::Export {
        job: 2,
        path: "second.json".into(),
        json: "second".into(),
    })
    .unwrap();
    for value in [3., 4.] {
        io.submit(IoRequest::WritePreferences(Preferences {
            font_scale: value,
            ..Default::default()
        }))
        .unwrap();
    }
    assert_eq!(io.outstanding(), 7);
    assert!(io.drain().is_none());
    resume.send(()).unwrap();
    assert_eq!(drain_all(&mut io).len(), 5);
    assert_eq!(
        records.try_iter().collect::<Vec<_>>(),
        ["pref:2", "export:1:first", "export:2:second", "pref:4"]
    );
    assert!(matches!(
        IoFailure::Export(9).event("capacity".into()),
        AppEvent::ExportFailed { job: 9, .. }
    ));
}

fn envelope(slot: u32, revision: u32) -> Box<SaveEnvelope> {
    let program = serde_json::from_str(include_str!("../../../../fixtures/rain.json")).unwrap();
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
        assert!(
            Instant::now() < deadline,
            "io replies never arrived: {}",
            io.deadline_snapshot()
        );
        if let Some(reply) = io.drain() {
            // A pending notice leaves the physical request outstanding;
            // this helper counts terminal results, as its callers require.
            if !matches!(reply, IoReply::Event(AppEvent::SavePending { .. })) {
                drained.push(reply);
            }
        } else {
            thread::sleep(Duration::from_millis(1));
        }
    }
    drained
}

#[test]
fn close_waits_for_blocked_storage_and_cancels_instead_of_hiding_save_failure() {
    use crate::close::{Close, CloseAction};

    let send = Arc::new(Mailbox::default());
    let jobs = send.clone();
    let (done, replies) = mpsc::sync_channel(1);
    let (started, blocked) = mpsc::channel();
    let (resume, release) = mpsc::channel();
    let worker = thread::spawn(move || {
        run_requests(jobs, done, |request| match request {
            IoRequest::WritePreferences(_) => {
                started.send(()).unwrap();
                release.recv().unwrap();
                IoReply::Done
            }
            IoRequest::MergeProfile(_) => IoReply::Done,
            IoRequest::Save { job, .. } => IoReply::Event(AppEvent::SaveFailed {
                job,
                message: "controlled write failure".into(),
            }),
            _ => panic!("unexpected request"),
        });
    });
    let mut io = IoWorker {
        send,
        replies,
        outstanding: 0,
        rejected_profile: BTreeSet::new(),
        storage: None,
        unconfirmed_saves: BTreeMap::new(),
    };
    io.submit(IoRequest::WritePreferences(Preferences::default()))
        .unwrap();
    blocked.recv_timeout(Duration::from_secs(5)).unwrap();
    for i in 0..64 {
        io.submit(IoRequest::MergeProfile(BTreeSet::from([format!(
            "read.{i}"
        )])))
        .unwrap();
    }
    io.submit(IoRequest::Save {
        slot: 0,
        expected_revision: 0,
        job: 4,
        envelope: envelope(0, 1),
    })
    .unwrap();

    let mut close = Close::default();
    close.request();
    // Polling is nonblocking while the backend deliberately remains
    // blocked. A second close must not discard any of the 66 requests.
    for _ in 0..32 {
        assert!(io.drain().is_none());
        assert_eq!(io.outstanding(), 66);
        assert_eq!(close.poll(io.outstanding(), false), CloseAction::Wait);
        assert!(!close.request());
    }
    resume.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut completions = 0;
    while io.outstanding() > 0 {
        assert!(
            Instant::now() < deadline,
            "controlled backend did not finish"
        );
        if let Some(reply) = io.drain() {
            completions += 1;
            match reply {
                IoReply::Done => {
                    assert_eq!(close.poll(io.outstanding(), false), CloseAction::Wait);
                }
                IoReply::Event(AppEvent::SaveFailed { job, message }) => {
                    assert_eq!(job, 4);
                    assert_eq!(message, "controlled write failure");
                    close.save_failed();
                    assert!(!close.request());
                    assert_eq!(close.poll(io.outstanding(), false), CloseAction::Cancel);
                }
                other => panic!("unexpected close reply: {other:?}"),
            }
        } else {
            thread::sleep(Duration::from_millis(1));
        }
    }
    assert_eq!(completions, 3, "64 queued progress writes share one batch");
    assert_eq!(close.poll(0, false), CloseAction::Continue);
    drop(io);
    worker.join().unwrap();
}

#[test]
fn adjacent_preferences_keep_latest_without_crossing_save_or_load() {
    let send = Arc::new(Mailbox::default());
    let jobs = send.clone();
    let (done, replies) = mpsc::sync_channel(1);
    let (entered, blocked) = mpsc::channel();
    let (resume, continued) = mpsc::channel();
    let worker = thread::spawn(move || {
        let mut first = true;
        let mut scale = 1.0;
        let mut recorded = Vec::new();
        run_requests(jobs, done, |request| {
            if first {
                first = false;
                entered.send(()).unwrap();
                continued.recv().unwrap();
            }
            match request {
                IoRequest::WritePreferences(p) => {
                    scale = p.font_scale;
                    recorded.push(("preferences", scale));
                }
                IoRequest::Save { .. } => recorded.push(("save", scale)),
                IoRequest::Load { .. } => recorded.push(("load", scale)),
                _ => panic!("unexpected request"),
            }
            IoReply::Done
        });
        recorded
    });
    let mut io = IoWorker {
        send,
        replies,
        outstanding: 0,
        rejected_profile: BTreeSet::new(),
        storage: None,
        unconfirmed_saves: BTreeMap::new(),
    };
    io.submit(IoRequest::WritePreferences(Preferences::default()))
        .unwrap();
    blocked.recv_timeout(Duration::from_secs(5)).unwrap();
    for i in 1..=130 {
        io.submit(IoRequest::WritePreferences(Preferences {
            font_scale: 1.0 + i as f32 / 1000.0,
            ..Default::default()
        }))
        .unwrap();
    }
    io.submit(IoRequest::Save {
        slot: 0,
        expected_revision: 0,
        job: 1,
        envelope: envelope(0, 1),
    })
    .unwrap();
    for i in 1..=70 {
        io.submit(IoRequest::WritePreferences(Preferences {
            font_scale: 1.0 + i as f32 / 1000.0,
            ..Default::default()
        }))
        .unwrap();
    }
    io.submit(IoRequest::Load { slot: 0, job: 2 }).unwrap();
    assert_eq!(io.outstanding(), 203);
    assert_eq!(io.send.state.lock().unwrap().batches, 5);
    assert!(io
        .send
        .state
        .lock()
        .unwrap()
        .pending
        .iter()
        .filter(|b| matches!(b.request, IoRequest::MergeProfile(_)))
        .all(|b| b.requests <= 64));
    assert!(io.drain().is_none());
    resume.send(()).unwrap();
    assert_eq!(drain_all(&mut io).len(), 5);
    assert_eq!(io.outstanding(), 0);
    drop(io);
    assert_eq!(
        worker.join().unwrap(),
        vec![
            ("preferences", 1.0),
            ("preferences", 1.13),
            ("save", 1.13),
            ("preferences", 1.07),
            ("load", 1.07)
        ]
    );
}

#[test]
fn admission_and_completed_results_are_bounded_and_rejected_progress_is_retained() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let send = Arc::new(Mailbox::default());
    let jobs = send.clone();
    let (done, replies) = mpsc::sync_channel(1);
    let (entered, blocked) = mpsc::channel();
    let (resume, continued) = mpsc::channel();
    let calls = Arc::new(AtomicUsize::new(0));
    let on_worker = calls.clone();
    let worker = thread::spawn(move || {
        let mut first = true;
        let mut recorded = Vec::new();
        run_requests(jobs, done, |request| {
            on_worker.fetch_add(1, Ordering::SeqCst);
            if first {
                first = false;
                entered.send(()).unwrap();
                continued.recv().unwrap();
            }
            match request {
                IoRequest::WritePreferences(_) => {}
                IoRequest::Load { job, .. } => recorded.push((job, BTreeSet::new())),
                IoRequest::MergeProfile(keys) => recorded.push((999, keys)),
                _ => panic!("rejected save must never execute"),
            }
            IoReply::Done
        });
        recorded
    });
    let mut io = IoWorker {
        send,
        replies,
        outstanding: 0,
        rejected_profile: BTreeSet::new(),
        storage: None,
        unconfirmed_saves: BTreeMap::new(),
    };
    io.submit(IoRequest::WritePreferences(Preferences::default()))
        .unwrap();
    blocked.recv_timeout(Duration::from_secs(5)).unwrap();
    for job in 0..63 {
        io.submit(IoRequest::Load { slot: 1, job }).unwrap();
    }
    let rejected = IoRequest::Save {
        slot: 0,
        expected_revision: 0,
        job: 99,
        envelope: envelope(0, 1),
    };
    let kind = rejected.failure();
    let error = io.submit(rejected).unwrap_err().to_string();
    assert_eq!(error, "E_IO_CAPACITY");
    assert!(matches!(
        kind.event(error),
        AppEvent::SaveFailed { job: 99, .. }
    ));
    for key in ["rejected", "more"] {
        assert!(io
            .submit(IoRequest::MergeProfile(BTreeSet::from([key.into()])))
            .is_err());
    }
    assert_eq!(io.outstanding(), 64);
    assert_eq!(io.send.state.lock().unwrap().batches, 64);
    resume.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while calls.load(Ordering::SeqCst) < 2 {
        assert!(Instant::now() < deadline);
        thread::yield_now();
    }
    // First result in the channel, second blocks the producer. No owner
    // reply has been admitted yet, so the remaining loads cannot run.
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(io.send.state.lock().unwrap().batches, 64);
    assert!(io.drain().is_some());
    io.submit(IoRequest::MergeProfile(BTreeSet::from(["later".into()])))
        .unwrap();
    assert_eq!(io.send.state.lock().unwrap().batches, 64);
    assert_eq!(drain_all(&mut io).len(), 64);
    assert_eq!(io.outstanding(), 0);
    drop(io);
    let recorded = worker.join().unwrap();
    assert_eq!(recorded.len(), 64);
    assert_eq!(
        recorded.last().unwrap().1,
        BTreeSet::from(["rejected".into(), "more".into(), "later".into()])
    );
    assert_eq!(
        recorded[..63].iter().map(|r| r.0).collect::<Vec<_>>(),
        (0..63).collect::<Vec<_>>()
    );
}

#[test]
fn disconnected_worker_keeps_save_uncertain_without_a_readable_backend() {
    let send = Arc::new(Mailbox::default());
    let jobs = send.clone();
    let (done, replies) = mpsc::sync_channel(1);
    let mut io = IoWorker {
        send,
        replies,
        outstanding: 0,
        rejected_profile: BTreeSet::new(),
        storage: None,
        unconfirmed_saves: BTreeMap::new(),
    };
    io.submit(IoRequest::Save {
        slot: 0,
        expected_revision: 0,
        job: 1,
        envelope: envelope(0, 1),
    })
    .unwrap();
    drop(jobs);
    drop(done);
    assert!(
        matches!(io.drain(), Some(IoReply::Event(AppEvent::SavePending {job: 1, message}))
        if message.starts_with("E_IO_CLOSED:"))
    );
    assert_eq!(io.outstanding(), 0);
    assert!(io.has_unconfirmed_saves());
    assert!(io.delayed_save_pending());
    assert!(!io.confirmation_retry_enabled());
    assert!(io.drain().is_none());
    assert!(io.submit(IoRequest::List).is_err());
}

#[test]
fn worker_death_settles_each_identity_and_retains_queued_progress_for_a_new_request() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(temp.path(), "game", "release", &"a".repeat(64)).unwrap());
    let (started, ready) = mpsc::channel();
    let (resume, blocked) = mpsc::channel();
    let (mut io, worker) = IoWorker::with_driver(storage.clone(), move |_| {
        started.send(()).unwrap();
        blocked.recv().unwrap();
        panic!("controlled storage thread death outside mailbox lock");
    });
    io.submit(IoRequest::WritePreferences(Preferences::default()))
        .unwrap();
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    for key in ["read.first", "read.second"] {
        io.submit(IoRequest::MergeProfile(BTreeSet::from([key.into()])))
            .unwrap();
    }
    io.submit(IoRequest::Save {
        slot: 0,
        expected_revision: 0,
        job: 7,
        envelope: envelope(0, 1),
    })
    .unwrap();
    io.submit(IoRequest::Load { slot: 0, job: 8 }).unwrap();
    let path = storage.root.join("export.json");
    io.submit(IoRequest::Export {
        job: 9,
        path: path.clone(),
        json: "unconfirmed export".into(),
    })
    .unwrap();
    io.submit(IoRequest::List).unwrap();
    assert_eq!(io.outstanding(), 7);
    assert_eq!(io.send.state.lock().unwrap().receipts.len(), 6);
    resume.send(()).unwrap();
    assert!(worker.join().is_err());
    let replies = drain_all(&mut io);
    assert_eq!(replies.len(), 6);
    assert!(matches!(
        &replies[0],
        IoReply::Event(AppEvent::PersistenceFailed {
            kind: PersistenceKind::Preferences,
            ..
        })
    ));
    assert!(matches!(
        &replies[1],
        IoReply::Event(AppEvent::PersistenceFailed {
            kind: PersistenceKind::Profile,
            ..
        })
    ));
    assert!(matches!(
        &replies[2],
        IoReply::Event(AppEvent::SlotLoadFailed { job: 8, .. })
    ));
    assert!(matches!(
        &replies[3],
        IoReply::Event(AppEvent::ExportFailed { job: 9, .. })
    ));
    assert!(
        matches!(&replies[4],IoReply::Event(AppEvent::Slots(rows,revisions)) if rows.len()==3 && rows.iter().all(|row|row.error.is_some()) && revisions.is_empty())
    );
    // The replacement worker reads only after every old receipt has been
    // delivered. Only that read can turn uncertainty into a failed save.
    assert!(
        matches!(&replies[5],IoReply::Event(AppEvent::SaveFailed {job:7,message}) if message.starts_with("E_IO_NOT_COMMITTED:"))
    );
    assert!(!io.has_unconfirmed_saves());
    assert_eq!(io.outstanding(), 0);
    assert!(io.send.state.lock().unwrap().receipts.is_empty());
    assert!(storage.load(0).unwrap().is_none());
    assert!(!path.exists());
    // Nothing is replayed merely because the owner continues draining.
    for _ in 0..100 {
        assert!(io.drain().is_none());
    }
    assert!(!path.exists());
    io.submit(IoRequest::MergeProfile(BTreeSet::from(
        ["read.next".into()],
    )))
    .unwrap();
    io.submit(IoRequest::Save {
        slot: 0,
        expected_revision: 0,
        job: 10,
        envelope: envelope(0, 1),
    })
    .unwrap();
    let replies = drain_all(&mut io);
    assert!(matches!(
        &replies[0],
        IoReply::Event(AppEvent::PersistenceStored(PersistenceKind::Profile))
    ));
    assert!(matches!(
        &replies[1],
        IoReply::Event(AppEvent::Saved {
            job: 10,
            revision: 1,
            ..
        })
    ));
    assert_eq!(
        storage.profile().unwrap(),
        BTreeSet::from([
            "read.first".into(),
            "read.second".into(),
            "read.next".into()
        ])
    );
    assert!(
        !path.exists(),
        "old export must never be automatically replayed"
    );
}

#[test]
fn confirmed_reply_survives_worker_death_and_written_save_is_confirmed_by_reading() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(temp.path(), "game", "release", &"a".repeat(64)).unwrap());
    let backend = storage.clone();
    let (mut io, worker) = IoWorker::with_driver(storage.clone(), move |request| match request {
        IoRequest::WritePreferences(p) => {
            backend.write_preferences(&p).unwrap();
            IoReply::Event(AppEvent::PersistenceStored(PersistenceKind::Preferences))
        }
        IoRequest::Save {
            slot,
            expected_revision,
            envelope,
            ..
        } => {
            backend.save(slot, expected_revision, &envelope).unwrap();
            panic!("controlled death after committed write before confirmation");
        }
        _ => panic!("unexpected controlled request"),
    });
    io.submit(IoRequest::WritePreferences(Preferences::default()))
        .unwrap();
    io.submit(IoRequest::Save {
        slot: 0,
        expected_revision: 0,
        job: 1,
        envelope: envelope(0, 1),
    })
    .unwrap();
    assert!(worker.join().is_err());
    let original = std::fs::read(storage.slot(0).unwrap()).unwrap();
    let modified = std::fs::metadata(storage.slot(0).unwrap())
        .unwrap()
        .modified()
        .unwrap();
    let replies = drain_all(&mut io);
    assert_eq!(replies.len(), 2);
    assert!(matches!(
        &replies[0],
        IoReply::Event(AppEvent::PersistenceStored(PersistenceKind::Preferences))
    ));
    assert!(matches!(
        &replies[1],
        IoReply::Event(AppEvent::Saved {
            job: 1,
            slot: 0,
            revision: 1
        })
    ));
    assert!(!io.has_unconfirmed_saves());
    assert_eq!(std::fs::read(storage.slot(0).unwrap()).unwrap(), original);
    assert_eq!(
        std::fs::metadata(storage.slot(0).unwrap())
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );
    // Old expected revision cannot silently overwrite the already written slot.
    io.submit(IoRequest::Save {
        slot: 0,
        expected_revision: 0,
        job: 2,
        envelope: envelope(0, 1),
    })
    .unwrap();
    let replies = drain_all(&mut io);
    assert!(matches!(
        &replies[0],
        IoReply::Event(AppEvent::SaveFailed { job: 2, .. })
    ));
    assert_eq!(std::fs::read(storage.slot(0).unwrap()).unwrap(), original);
    io.submit(IoRequest::List).unwrap();
    let replies = drain_all(&mut io);
    assert!(
        matches!(&replies[0],IoReply::Event(AppEvent::Slots(_,revisions)) if revisions.get(&0)==Some(&1))
    );
    io.submit(IoRequest::Save {
        slot: 0,
        expected_revision: 1,
        job: 3,
        envelope: envelope(0, 2),
    })
    .unwrap();
    let replies = drain_all(&mut io);
    assert!(matches!(
        &replies[0],
        IoReply::Event(AppEvent::Saved {
            job: 3,
            revision: 2,
            ..
        })
    ));
    assert_eq!(storage.load(0).unwrap().unwrap().revision, 2);
}

#[test]
fn idle_disconnection_closes_admission_and_next_new_operation_reopens_storage() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(temp.path(), "game", "release", &"a".repeat(64)).unwrap());
    let (done, replies) = mpsc::sync_channel(1);
    drop(done);
    let mut io = IoWorker {
        send: Arc::new(Mailbox::default()),
        replies,
        outstanding: 0,
        rejected_profile: BTreeSet::new(),
        storage: Some(storage.clone()),
        unconfirmed_saves: BTreeMap::new(),
    };
    assert!(io.drain().is_none());
    assert!(io.send.state.lock().unwrap().closed);
    io.submit(IoRequest::WritePreferences(Preferences {
        font_scale: 1.4,
        ..Default::default()
    }))
    .unwrap();
    assert!(matches!(
        &drain_all(&mut io)[0],
        IoReply::Event(AppEvent::PersistenceStored(PersistenceKind::Preferences))
    ));
    assert_eq!(storage.preferences().unwrap().unwrap().font_scale, 1.4);
}

#[test]
fn lost_confirmation_keeps_corrupt_record_and_requires_explicit_read_retry() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(temp.path(), "game", "release", &"a".repeat(64)).unwrap());
    let backend = storage.clone();
    let (recorded, bytes) = mpsc::channel();
    let (mut io, worker) = IoWorker::with_driver(storage.clone(), move |request| {
        let IoRequest::Save {
            slot,
            expected_revision,
            envelope,
            ..
        } = request
        else {
            panic!("no replay")
        };
        backend.save(slot, expected_revision, &envelope).unwrap();
        recorded
            .send(std::fs::read(backend.slot(slot).unwrap()).unwrap())
            .unwrap();
        // Simulate a record made unreadable before owner confirmation.
        std::fs::write(backend.slot(slot).unwrap(), b"{broken record").unwrap();
        panic!("controlled loss of save confirmation");
    });
    io.submit(IoRequest::Save {
        slot: 1,
        expected_revision: 0,
        job: 71,
        envelope: envelope(1, 1),
    })
    .unwrap();
    assert!(worker.join().is_err());
    let committed = bytes.recv().unwrap();
    assert!(matches!(
        io.drain(),
        Some(IoReply::Event(AppEvent::SavePending { job: 71, .. }))
    ));
    assert!(io.confirmation_in_flight());
    assert!(drain_all(&mut io).is_empty());
    assert!(io.has_unconfirmed_saves());
    assert!(io.confirmation_retry_enabled());
    for _ in 0..100 {
        assert!(
            io.drain().is_none(),
            "read failure must not retry every frame"
        );
        assert_eq!(io.outstanding(), 0);
    }
    assert_eq!(
        std::fs::read(storage.slot(1).unwrap()).unwrap(),
        b"{broken record"
    );
    io.retry_unconfirmed_saves();
    for _ in 0..8 {
        io.retry_unconfirmed_saves();
    }
    assert_eq!(
        io.outstanding(),
        1,
        "repeated gestures share the original read"
    );
    assert!(drain_all(&mut io).is_empty());
    assert!(io.delayed_save_pending());
    // Only the fixture repairs the record; recovery never writes it.
    atomic_write(&storage.slot(1).unwrap(), &committed).unwrap();
    let modified = std::fs::metadata(storage.slot(1).unwrap())
        .unwrap()
        .modified()
        .unwrap();
    io.retry_unconfirmed_saves();
    let replies = drain_all(&mut io);
    assert_eq!(replies.len(), 1);
    assert!(matches!(
        &replies[0],
        IoReply::Event(AppEvent::Saved {
            job: 71,
            slot: 1,
            revision: 1
        })
    ));
    assert!(!io.has_unconfirmed_saves());
    assert_eq!(std::fs::read(storage.slot(1).unwrap()).unwrap(), committed);
    assert_eq!(
        std::fs::metadata(storage.slot(1).unwrap())
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );
}

#[test]
fn lost_confirmation_reports_conflict_for_changed_snapshot_or_revision_without_overwrite() {
    for changed_revision in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let storage =
            Arc::new(Storage::open(temp.path(), "game", "release", &"a".repeat(64)).unwrap());
        let backend = storage.clone();
        let (mut io, worker) = IoWorker::with_driver(storage.clone(), move |request| {
            let IoRequest::Save {
                slot,
                expected_revision,
                mut envelope,
                ..
            } = request
            else {
                panic!("no replay")
            };
            backend.save(slot, expected_revision, &envelope).unwrap();
            envelope.snapshot.tick_us.0 += 1;
            envelope.digest = nir_content::digest(&serde_json::to_vec(&envelope.snapshot).unwrap());
            if changed_revision {
                envelope.revision += 1;
            }
            atomic_write(
                &backend.slot(slot).unwrap(),
                &serde_json::to_vec(&envelope).unwrap(),
            )
            .unwrap();
            panic!("controlled loss after another record replaced the save");
        });
        io.submit(IoRequest::Save {
            slot: 2,
            expected_revision: 0,
            job: 72,
            envelope: envelope(2, 1),
        })
        .unwrap();
        assert!(worker.join().is_err());
        let changed = std::fs::read(storage.slot(2).unwrap()).unwrap();
        let replies = drain_all(&mut io);
        assert_eq!(replies.len(), 1);
        assert!(
            matches!(&replies[0],IoReply::Event(AppEvent::SaveFault {job:72,diagnostic}) if diagnostic.code=="E_SAVE_CONFLICT")
        );
        assert!(!io.has_unconfirmed_saves());
        assert_eq!(std::fs::read(storage.slot(2).unwrap()).unwrap(), changed);
        assert_eq!(
            storage.load(2).unwrap().unwrap().revision,
            if changed_revision { 2 } else { 1 }
        );
    }
}

#[test]
fn failed_confirmation_worker_or_mismatched_reply_cannot_settle_original_save() {
    for reply_kind in ["death", "job", "slot", "revision"] {
        let temp = tempfile::tempdir().unwrap();
        let storage =
            Arc::new(Storage::open(temp.path(), "game", "release", &"a".repeat(64)).unwrap());
        let record = envelope(0, 1);
        storage.save(0, 0, &record).unwrap();
        let original = std::fs::read(storage.slot(0).unwrap()).unwrap();
        let (mut io, worker) = IoWorker::with_driver(storage.clone(), move |request| {
            let IoRequest::ConfirmSave(_) = request else {
                panic!("confirmation must be read-only")
            };
            if reply_kind == "death" {
                panic!("controlled confirmation reader death");
            }
            IoReply::Event(AppEvent::Saved {
                job: if reply_kind == "job" { 99 } else { 73 },
                slot: if reply_kind == "slot" { 1 } else { 0 },
                revision: if reply_kind == "revision" { 2 } else { 1 },
            })
        });
        // Compact identity is exactly what survives the original write.
        io.unconfirmed_saves.insert(
            73,
            UnconfirmedSave {
                check: SaveCheck {
                    job: 73,
                    slot: 0,
                    expected_revision: 0,
                    revision: 1,
                    digest: record.digest.clone(),
                },
                attempted: false,
            },
        );
        io.start_confirmations(false);
        assert!(drain_all(&mut io).is_empty());
        assert!(io.has_unconfirmed_saves());
        assert_eq!(io.outstanding(), 0);
        for _ in 0..100 {
            assert!(io.drain().is_none());
        }
        let checks = std::mem::take(&mut io.unconfirmed_saves);
        drop(io);
        if reply_kind == "death" {
            assert!(worker.join().is_err());
        } else {
            worker.join().unwrap();
        }
        // A fresh read worker is explicitly admitted; the save never is.
        let mut io = IoWorker::new(storage.clone());
        io.unconfirmed_saves = checks;
        io.retry_unconfirmed_saves();
        let replies = drain_all(&mut io);
        assert!(matches!(
            &replies[0],
            IoReply::Event(AppEvent::Saved {
                job: 73,
                slot: 0,
                revision: 1
            })
        ));
        assert!(!io.has_unconfirmed_saves());
        assert_eq!(std::fs::read(storage.slot(0).unwrap()).unwrap(), original);
    }
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
fn persistence_errors_keep_old_files_and_other_requests_usable() {
    let temp = tempfile::tempdir().unwrap();
    let release = "a".repeat(64);
    let storage =
        std::sync::Arc::new(Storage::open(temp.path(), "game", "release", &release).unwrap());
    storage.write_preferences(&Preferences::default()).unwrap();
    storage
        .merge_profile(BTreeSet::from(["old".into()]))
        .unwrap();
    storage.save(1, 0, &envelope(1, 1)).unwrap();
    let prefs = storage.root.join("preferences.json");
    let profile = storage.root.join("profile.json");
    let before = [
        std::fs::read(&prefs).unwrap(),
        std::fs::read(&profile).unwrap(),
    ];
    // Real atomic_write fails to open its temporary file. The good final
    // files remain present and readable; no permission/root-user assumption.
    let prefs_block = prefs.with_extension(format!("{}.tmp", std::process::id()));
    let profile_block = profile.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::create_dir(&prefs_block).unwrap();
    std::fs::create_dir(&profile_block).unwrap();
    let mut io = IoWorker::new(storage.clone());
    io.submit(IoRequest::WritePreferences(Preferences {
        font_scale: 1.5,
        ..Default::default()
    }))
    .unwrap();
    io.submit(IoRequest::MergeProfile(BTreeSet::from(["failed".into()])))
        .unwrap();
    io.submit(IoRequest::Load { slot: 1, job: 9 }).unwrap();
    let replies = drain_all(&mut io);
    assert!(matches!(
        &replies[0],
        IoReply::Event(AppEvent::PersistenceFailed {
            kind: PersistenceKind::Preferences,
            ..
        })
    ));
    assert!(matches!(
        &replies[1],
        IoReply::Event(AppEvent::PersistenceFailed {
            kind: PersistenceKind::Profile,
            ..
        })
    ));
    assert!(matches!(
        &replies[2],
        IoReply::Event(AppEvent::SlotLoaded { job: 9, .. })
    ));
    assert_eq!(std::fs::read(&prefs).unwrap(), before[0]);
    assert_eq!(std::fs::read(&profile).unwrap(), before[1]);
    std::fs::remove_dir(&prefs_block).unwrap();
    std::fs::remove_dir(&profile_block).unwrap();
    io.submit(IoRequest::WritePreferences(Preferences {
        font_scale: 1.7,
        ..Default::default()
    }))
    .unwrap();
    io.submit(IoRequest::MergeProfile(BTreeSet::from(["later".into()])))
        .unwrap();
    let recovered = drain_all(&mut io);
    assert!(recovered
        .iter()
        .all(|reply| matches!(reply, IoReply::Event(AppEvent::PersistenceStored(_)))));
    assert_eq!(
        storage.profile().unwrap(),
        BTreeSet::from(["old".into(), "failed".into(), "later".into()])
    );
    assert_eq!(storage.preferences().unwrap().unwrap().font_scale, 1.7);

    if let Some(path) = std::env::var_os("NIR_PERSISTENCE_PROBE_REPORT") {
        std::fs::write(path,serde_json::to_vec_pretty(&serde_json::json!({
            "replyDebug":replies.iter().map(|r|format!("{r:?}")).collect::<Vec<_>>(),
            "preferencesFailure":matches!(&replies[0],IoReply::Event(AppEvent::PersistenceFailed {kind:PersistenceKind::Preferences,..})),
            "profileFailure":matches!(&replies[1],IoReply::Event(AppEvent::PersistenceFailed {kind:PersistenceKind::Profile,..})),
            "oldFilesIntact":true,"healthyLoadAfterFailures":true,"outstanding":io.outstanding(),
            "scope":"Actual current Storage/IoWorker in explicit tmpfs with synthetic temp-file obstacles; not actual native GUI, fsync latency or physical audio."
        })).unwrap()).unwrap();
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
    assert!(matches!(
        drained[0],
        IoReply::Event(AppEvent::PersistenceStored(PersistenceKind::Preferences))
    ));
    assert!(matches!(
        drained[1],
        IoReply::Event(AppEvent::PersistenceStored(PersistenceKind::Profile))
    ));
    assert_eq!(storage.preferences().unwrap().unwrap().font_scale, 1.5);
    assert!(storage.profile().unwrap().contains("seen"));
}

#[test]
fn pending_profile_writes_merge_without_crossing_save_or_load() {
    use std::sync::{Arc, Mutex};

    // Block a real worker driver, then accumulate progress as if fsync
    // were slow. The backend records writes and observes Save/Load in
    // FIFO order; this test concerns scheduling, not filesystem durability.
    let send = Arc::new(Mailbox::default());
    let jobs = send.clone();
    let (done, replies) = mpsc::sync_channel(1);
    let (started, blocked) = mpsc::channel();
    let (resume, release) = mpsc::channel();
    let records = Arc::new(Mutex::new(Vec::new()));
    let recorded = records.clone();
    let worker = thread::spawn(move || {
        let mut profile = BTreeSet::from(["loaded".to_owned()]);
        let mut saved = None;
        run_requests(jobs, done, |request| match request {
            IoRequest::WritePreferences(_) => {
                started.send(()).unwrap();
                release.recv().unwrap();
                IoReply::Done
            }
            IoRequest::MergeProfile(keys) => {
                profile.extend(keys);
                recorded.lock().unwrap().push(("profile", profile.clone()));
                IoReply::Done
            }
            IoRequest::Save {
                job,
                slot,
                envelope,
                ..
            } => {
                recorded.lock().unwrap().push(("save", profile.clone()));
                let revision = envelope.revision;
                saved = Some(envelope);
                IoReply::Event(AppEvent::Saved {
                    job,
                    slot,
                    revision,
                })
            }
            IoRequest::Load { job, .. } => {
                recorded.lock().unwrap().push(("load", profile.clone()));
                IoReply::Event(AppEvent::SlotLoaded {
                    job,
                    envelope: saved.take().unwrap(),
                })
            }
            IoRequest::MergeProfileValues(_)
            | IoRequest::List
            | IoRequest::Export { .. }
            | IoRequest::ReadMetadata(_)
            | IoRequest::ConfirmSave(_) => {
                panic!("unexpected storage request")
            }
        });
    });
    let mut io = IoWorker {
        send,
        replies,
        outstanding: 0,
        rejected_profile: BTreeSet::new(),
        storage: None,
        unconfirmed_saves: BTreeMap::new(),
    };
    io.submit(IoRequest::WritePreferences(Preferences::default()))
        .unwrap();
    blocked.recv_timeout(Duration::from_secs(5)).unwrap();
    let before: BTreeSet<_> = (0..64).map(|i| format!("read.{i}")).collect();
    for key in &before {
        io.submit(IoRequest::MergeProfile(BTreeSet::from([key.clone()])))
            .unwrap();
    }
    io.submit(IoRequest::Save {
        slot: 0,
        expected_revision: 0,
        job: 1,
        envelope: envelope(0, 1),
    })
    .unwrap();
    io.submit(IoRequest::MergeProfile(BTreeSet::from(["after".into()])))
        .unwrap();
    io.submit(IoRequest::Load { slot: 0, job: 2 }).unwrap();
    assert_eq!(io.outstanding(), 68);
    assert!(
        io.drain().is_none(),
        "blocked storage cannot acknowledge work"
    );
    resume.send(()).unwrap();
    let drained = drain_all(&mut io);
    assert_eq!(io.outstanding(), 0);
    drop(io);
    worker.join().unwrap();

    let recorded = records.lock().unwrap();
    assert_eq!(
        recorded
            .iter()
            .filter(|(kind, _)| *kind == "profile")
            .count(),
        2,
        "pending progress should write once on each side of the save boundary"
    );
    assert_eq!(
        recorded.iter().map(|(kind, _)| *kind).collect::<Vec<_>>(),
        ["profile", "save", "profile", "load"]
    );
    let mut expected = before;
    expected.insert("loaded".into());
    assert_eq!(recorded[0].1, expected);
    assert_eq!(recorded[1].1, expected);
    expected.insert("after".into());
    assert_eq!(recorded[2].1, expected);
    assert_eq!(recorded[3].1, expected);
    assert_eq!(drained.len(), 5);
    assert!(matches!(drained[0], IoReply::Done));
    assert!(matches!(drained[1], IoReply::Done));
    assert!(matches!(
        drained[2],
        IoReply::Event(AppEvent::Saved {
            job: 1,
            revision: 1,
            ..
        })
    ));
    assert!(matches!(drained[3], IoReply::Done));
    assert!(matches!(
        drained[4],
        IoReply::Event(AppEvent::SlotLoaded { job: 2, .. })
    ));
}

#[test]
fn profile_batches_are_bounded_and_listing_preferences_are_barriers() {
    let send = Arc::new(Mailbox::default());
    let jobs = send.clone();
    let (done, replies) = mpsc::sync_channel(1);
    let mut io = IoWorker {
        send,
        replies,
        outstanding: 0,
        rejected_profile: BTreeSet::new(),
        storage: None,
        unconfirmed_saves: BTreeMap::new(),
    };
    for i in 0..150 {
        io.submit(IoRequest::MergeProfile(BTreeSet::from([format!(
            "before.{i}"
        )])))
        .unwrap();
    }
    io.submit(IoRequest::List).unwrap();
    for i in 0..10 {
        io.submit(IoRequest::MergeProfile(BTreeSet::from([format!(
            "middle.{i}"
        )])))
        .unwrap();
    }
    io.submit(IoRequest::WritePreferences(Preferences::default()))
        .unwrap();
    for i in 0..5 {
        io.submit(IoRequest::MergeProfile(BTreeSet::from([format!(
            "after.{i}"
        )])))
        .unwrap();
    }
    let worker = thread::spawn(move || {
        let mut recorded = vec![];
        run_requests(jobs, done, |request| {
            recorded.push(match request {
                IoRequest::MergeProfile(keys) => ("profile", keys.len()),
                IoRequest::List => ("list", 0),
                IoRequest::WritePreferences(_) => ("preferences", 0),
                _ => panic!("unexpected request"),
            });
            IoReply::Done
        });
        recorded
    });
    assert_eq!(io.outstanding(), 167);
    assert_eq!(drain_all(&mut io).len(), 7);
    drop(io);
    assert_eq!(
        worker.join().unwrap(),
        vec![
            ("profile", 64),
            ("profile", 64),
            ("profile", 22),
            ("list", 0),
            ("profile", 10),
            ("preferences", 0),
            ("profile", 5)
        ]
    );
}

#[test]
fn failed_profile_batch_releases_accounting_and_preserves_the_error() {
    let send = Arc::new(Mailbox::default());
    let jobs = send.clone();
    let (done, replies) = mpsc::sync_channel(1);
    let mut io = IoWorker {
        send,
        replies,
        outstanding: 0,
        rejected_profile: BTreeSet::new(),
        storage: None,
        unconfirmed_saves: BTreeMap::new(),
    };
    for _ in 0..3 {
        io.submit(IoRequest::MergeProfile(BTreeSet::from(["seen".into()])))
            .unwrap();
    }
    io.submit(IoRequest::WritePreferences(Preferences::default()))
        .unwrap();
    for _ in 0..2 {
        io.submit(IoRequest::MergeProfile(BTreeSet::from(["after".into()])))
            .unwrap();
    }
    let worker = thread::spawn(move || {
        run_requests(jobs, done, |request| match request {
            IoRequest::MergeProfile(_) => IoReply::Event(AppEvent::PersistenceFailed {
                kind: PersistenceKind::Profile,
                message: "E_STORAGE_TEST".into(),
            }),
            IoRequest::WritePreferences(_) => IoReply::Done,
            _ => panic!("unexpected request"),
        });
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    for remaining in [3, 2, 0] {
        let reply = loop {
            assert!(Instant::now() < deadline);
            if let Some(reply) = io.drain() {
                break reply;
            }
            thread::yield_now();
        };
        assert_eq!(io.outstanding(), remaining);
        if remaining == 2 {
            assert!(matches!(reply, IoReply::Done));
        } else {
            assert!(
                matches!(reply, IoReply::Event(AppEvent::PersistenceFailed {kind: PersistenceKind::Profile, message}) if message == "E_STORAGE_TEST")
            );
        }
    }
    drop(io);
    worker.join().unwrap();
}

#[test]
fn queued_profile_batches_persist_union_and_preferences_after_reopen() {
    use crate::close::{Close, CloseAction};

    let temp = tempfile::tempdir().unwrap();
    let release = "a".repeat(64);
    let storage =
        std::sync::Arc::new(Storage::open(temp.path(), "game", "release", &release).unwrap());
    storage
        .merge_profile(BTreeSet::from(["loaded".into()]))
        .unwrap();
    let send = Arc::new(Mailbox::default());
    let jobs = send.clone();
    let (done, replies) = mpsc::sync_channel(1);
    let mut io = IoWorker {
        send,
        replies,
        outstanding: 0,
        rejected_profile: BTreeSet::new(),
        storage: None,
        unconfirmed_saves: BTreeMap::new(),
    };
    let mut expected = BTreeSet::from(["loaded".to_owned()]);
    for i in 0..64 {
        let key = if i == 0 {
            "loaded".to_owned()
        } else {
            format!("read.{i}")
        };
        expected.insert(key.clone());
        io.submit(IoRequest::MergeProfile(BTreeSet::from([key])))
            .unwrap();
    }
    io.submit(IoRequest::WritePreferences(Preferences {
        font_scale: 1.25,
        ..Default::default()
    }))
    .unwrap();
    expected.insert("after".into());
    io.submit(IoRequest::MergeProfile(BTreeSet::from(["after".into()])))
        .unwrap();
    let mut close = Close::default();
    close.request();
    assert_eq!(close.poll(io.outstanding(), false), CloseAction::Wait);
    // Prequeue before starting to exercise an actual batch deterministically.
    let on_worker = storage.clone();
    let worker = thread::spawn(move || run_storage_worker(on_worker, jobs, done));
    let drained = drain_all(&mut io);
    assert_eq!(drained.len(), 3);
    assert!(drained
        .iter()
        .all(|reply| matches!(reply, IoReply::Event(AppEvent::PersistenceStored(_)))));
    assert_eq!(close.poll(io.outstanding(), false), CloseAction::Exit);
    drop(io);
    worker.join().unwrap();
    drop(storage);
    let reopened = Storage::open(temp.path(), "game", "release", &release).unwrap();
    assert_eq!(reopened.profile().unwrap(), expected);
    assert_eq!(reopened.preferences().unwrap().unwrap().font_scale, 1.25);
}

#[test]
fn corrupt_or_unreadable_slot_does_not_hide_healthy_slots_or_overwrite_files() {
    for failure in ["malformed", "checksum", "directory"] {
        let temp = tempfile::tempdir().unwrap();
        let release = "a".repeat(64);
        let storage =
            std::sync::Arc::new(Storage::open(temp.path(), "game", "release", &release).unwrap());
        storage.save(1, 0, &envelope(1, 1)).unwrap();
        let directory = storage.root.join("releases").join(&release);
        let bad = directory.join("slot-0.json");
        let good = directory.join("slot-1.json");
        match failure {
            "malformed" => std::fs::write(&bad, b"{broken").unwrap(),
            "checksum" => {
                let mut corrupt = envelope(0, 1);
                corrupt.digest = "0".repeat(64);
                std::fs::write(&bad, serde_json::to_vec(&corrupt).unwrap()).unwrap();
            }
            "directory" => std::fs::create_dir(&bad).unwrap(),
            _ => unreachable!(),
        }
        let bad_before = std::fs::read(&bad).ok();
        let good_before = std::fs::read(&good).unwrap();
        let mut io = IoWorker::new(storage.clone());
        io.submit(IoRequest::List).unwrap();
        io.submit(IoRequest::Load { slot: 1, job: 7 }).unwrap();
        io.submit(IoRequest::Save {
            slot: 0,
            expected_revision: 0,
            job: 8,
            envelope: envelope(0, 1),
        })
        .unwrap();
        let replies = drain_all(&mut io);
        let IoReply::Event(AppEvent::Slots(rows, revisions)) = &replies[0] else {
            panic!("One bad slot killed listing: {:?}", replies[0]);
        };
        assert_eq!(rows.len(), 3);
        assert!(!rows[0].exists && rows[0].error.is_some());
        assert!(rows[1].exists && rows[1].error.is_none());
        assert!(!rows[2].exists && rows[2].error.is_none());
        assert_eq!(revisions, &BTreeMap::from([(1, 1)]));
        assert!(
            matches!(&replies[1], IoReply::Event(AppEvent::SlotLoaded { job: 7, envelope }) if envelope.slot == 1)
        );
        assert!(matches!(
            &replies[2],
            IoReply::Event(AppEvent::SaveFailed { job: 8, .. })
        ));
        assert_eq!(std::fs::read(&bad).ok(), bad_before);
        assert_eq!(bad.is_dir(), failure == "directory");
        assert_eq!(std::fs::read(&good).unwrap(), good_before);
        assert_eq!(io.outstanding(), 0);
    }
}

#[test]
fn queued_saves_notify_once_at_the_deadline_without_releasing_fifo_receipts() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(temp.path(), "game", "release", &"a".repeat(64)).unwrap());
    let backend = storage.clone();
    let (entered, ready) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let (mut io, worker) = IoWorker::with_driver(storage.clone(), move |request| match request {
        IoRequest::WritePreferences(p) => {
            entered.send(()).unwrap();
            gate.recv().unwrap();
            persist(backend.write_preferences(&p), PersistenceKind::Preferences)
        }
        IoRequest::Save {
            slot,
            expected_revision,
            job,
            envelope,
        } => {
            backend.save(slot, expected_revision, &envelope).unwrap();
            IoReply::Event(AppEvent::Saved {
                job,
                slot,
                revision: envelope.revision,
            })
        }
        _ => panic!("unexpected request"),
    });
    io.submit(IoRequest::WritePreferences(Preferences::default()))
        .unwrap();
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    for slot in 0..2 {
        io.submit(IoRequest::Save {
            slot,
            expected_revision: 0,
            job: 90 + slot,
            envelope: envelope(slot, 1),
        })
        .unwrap();
    }
    let admissions = io
        .send
        .state
        .lock()
        .unwrap()
        .receipts
        .iter()
        .skip(1)
        .map(|r| r.admitted)
        .collect::<Vec<_>>();
    assert!(io
        .drain_at(admissions[0] + SAVE_CONFIRMATION_NOTICE - Duration::from_nanos(1))
        .is_none());
    assert!(!io.delayed_save_pending());
    let first = io.drain_at(admissions[0] + SAVE_CONFIRMATION_NOTICE);
    let second = io.drain_at(admissions[1] + SAVE_CONFIRMATION_NOTICE);
    let outstanding = io.outstanding();
    let still_pending = io.delayed_save_pending();
    let repeated = (0..100)
        .filter_map(|_| io.drain_at(admissions[1] + Duration::from_secs(30)))
        .collect::<Vec<_>>();
    let before = [storage.load(0).unwrap(), storage.load(1).unwrap()];
    release.send(()).unwrap();
    let terminal = drain_all(&mut io);
    let cleared = !io.delayed_save_pending();
    drop(io);
    worker.join().unwrap();
    assert!(matches!(
        first,
        Some(IoReply::Event(AppEvent::SavePending { job: 90, .. }))
    ));
    assert!(matches!(
        second,
        Some(IoReply::Event(AppEvent::SavePending { job: 91, .. }))
    ));
    assert_eq!(outstanding, 3);
    assert!(still_pending && cleared);
    assert!(repeated.is_empty());
    assert!(before.iter().all(Option::is_none));
    assert_eq!(terminal.len(), 3);
    assert!(matches!(
        terminal[0],
        IoReply::Event(AppEvent::PersistenceStored(PersistenceKind::Preferences))
    ));
    for slot in 0..2 {
        assert!(matches!(
            &terminal[slot as usize + 1],
            IoReply::Event(AppEvent::Saved { job, slot: saved, revision: 1 })
            if *job == 90 + slot && *saved == slot
        ));
        assert_eq!(storage.load(slot).unwrap().unwrap().revision, 1);
    }
}

#[test]
fn ready_completion_precedes_a_delayed_notice_for_the_same_save() {
    let send = Arc::new(Mailbox::default());
    let (done, replies) = mpsc::sync_channel(1);
    let mut io = IoWorker {
        send,
        replies,
        outstanding: 0,
        rejected_profile: BTreeSet::new(),
        storage: None,
        unconfirmed_saves: BTreeMap::new(),
    };
    io.submit(IoRequest::Save {
        slot: 0,
        expected_revision: 0,
        job: 50,
        envelope: envelope(0, 1),
    })
    .unwrap();
    let admitted = io.send.state.lock().unwrap().receipts[0].admitted;
    // A completion already in the real bounded channel is conclusive;
    // an old admission time must not produce a misleading busy notice.
    done.send(Completion {
        reply: IoReply::Event(AppEvent::Saved {
            job: 50,
            slot: 0,
            revision: 1,
        }),
        requests: 1,
    })
    .unwrap();
    assert!(matches!(
        io.drain_at(admitted + Duration::from_secs(30)),
        Some(IoReply::Event(AppEvent::Saved { job: 50, .. }))
    ));
    assert_eq!(io.outstanding(), 0);
    assert!(!io.delayed_save_pending());
    assert!(io.drain_at(admitted + Duration::from_secs(60)).is_none());
}

#[test]
fn delayed_native_write_failure_preserves_old_save_until_explicit_retry() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(temp.path(), "game", "release", &"a".repeat(64)).unwrap());
    storage.save(0, 0, &envelope(0, 1)).unwrap();
    let path = storage.slot(0).unwrap();
    let original = std::fs::read(&path).unwrap();
    let blocked = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::create_dir(&blocked).unwrap();
    let backend = storage.clone();
    let (entered, ready) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let mut first = true;
    let (mut io, worker) = IoWorker::with_driver(storage.clone(), move |request| {
        let IoRequest::Save {
            slot,
            expected_revision,
            job,
            envelope,
        } = request
        else {
            panic!("unexpected request");
        };
        if first {
            first = false;
            entered.send(()).unwrap();
            gate.recv().unwrap();
        }
        IoReply::Event(match backend.save(slot, expected_revision, &envelope) {
            Ok(()) => AppEvent::Saved {
                job,
                slot,
                revision: envelope.revision,
            },
            Err(error) => AppEvent::SaveFailed {
                job,
                message: error.to_string(),
            },
        })
    });
    io.submit(IoRequest::Save {
        slot: 0,
        expected_revision: 1,
        job: 60,
        envelope: envelope(0, 2),
    })
    .unwrap();
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    let admitted = io.send.state.lock().unwrap().receipts[0].admitted;
    let pending = io.drain_at(admitted + SAVE_CONFIRMATION_NOTICE);
    let outstanding = io.outstanding();
    release.send(()).unwrap();
    let failure = drain_all(&mut io);
    assert!(matches!(
        pending,
        Some(IoReply::Event(AppEvent::SavePending { job: 60, .. }))
    ));
    assert_eq!(outstanding, 1);
    assert!(
        matches!(&failure[..], [IoReply::Event(AppEvent::SaveFailed { job: 60, message })] if !message.is_empty())
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(!io.delayed_save_pending());
    std::fs::remove_dir(blocked).unwrap();
    for _ in 0..100 {
        assert!(io.drain_at(admitted + Duration::from_secs(30)).is_none());
    }
    assert_eq!(
        std::fs::read(&path).unwrap(),
        original,
        "no automatic replay after repair"
    );
    io.submit(IoRequest::Save {
        slot: 0,
        expected_revision: 1,
        job: 61,
        envelope: envelope(0, 2),
    })
    .unwrap();
    let recovered = drain_all(&mut io);
    assert!(matches!(
        &recovered[..],
        [IoReply::Event(AppEvent::Saved {
            job: 61,
            revision: 2,
            ..
        })]
    ));
    assert_eq!(storage.load(0).unwrap().unwrap().revision, 2);
    drop(io);
    worker.join().unwrap();
}

#[test]
fn delayed_native_save_notifies_without_settling_or_replaying_the_write() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(Storage::open(temp.path(), "game", "release", &"a".repeat(64)).unwrap());
    let backend = storage.clone();
    let (committed, commit) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let (mut io, worker) = IoWorker::with_driver(storage.clone(), move |request| {
        let IoRequest::Save {
            slot,
            expected_revision,
            job,
            envelope,
        } = request
        else {
            panic!("unexpected request");
        };
        backend.save(slot, expected_revision, &envelope).unwrap();
        committed.send(()).unwrap();
        gate.recv().unwrap();
        IoReply::Event(AppEvent::Saved {
            job,
            slot,
            revision: envelope.revision,
        })
    });
    io.submit(IoRequest::Save {
        slot: 0,
        expected_revision: 0,
        job: 77,
        envelope: envelope(0, 1),
    })
    .unwrap();
    commit.recv_timeout(Duration::from_secs(2)).unwrap();
    let deadline = Instant::now() + Duration::from_millis(3250);
    let mut pending = None;
    while Instant::now() < deadline {
        if let Some(reply) = io.drain() {
            pending = Some(reply);
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    let outstanding = io.outstanding();
    let premature = (0..100).filter_map(|_| io.drain()).collect::<Vec<_>>();
    let record = storage.load(0).unwrap().unwrap();
    // Always unblock the fixture before an assertion, including baseline
    // failure, so a failed test does not strand a filesystem thread.
    release.send(()).unwrap();
    let terminal = drain_all(&mut io);
    drop(io);
    worker.join().unwrap();
    assert!(
        matches!(
            pending,
            Some(IoReply::Event(AppEvent::SavePending { job: 77, .. }))
        ),
        "a delayed physical reply must show its pending status"
    );
    assert_eq!(outstanding, 1, "notice is not a terminal reply");
    assert!(
        premature.is_empty(),
        "do not repeat a notice or replay a write"
    );
    assert_eq!(record.revision, 1);
    assert_eq!(terminal.len(), 1);
    assert!(matches!(
        &terminal[0],
        IoReply::Event(AppEvent::Saved {
            job: 77,
            revision: 1,
            ..
        })
    ));
}
