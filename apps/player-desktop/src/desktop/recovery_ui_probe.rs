use super::*;
use winit::platform::x11::EventLoopBuilderExtX11;

fn state(runtime: &Runtime) -> serde_json::Value {
    serde_json::from_str(&runtime.engine.state()).unwrap()
}
fn export_request(runtime: &mut Runtime) -> (u32, String) {
    runtime.engine.begin_turn();
    runtime.sequence += 1;
    runtime
        .engine
        .input(UiAction::Export, runtime.sequence)
        .unwrap();
    runtime
        .engine
        .take_commands()
        .into_iter()
        .find_map(|command| {
            if let AppCommand::Export { job, json } = command {
                Some((job, json))
            } else {
                None
            }
        })
        .expect("export snapshot command")
}
fn until(runtime: &mut Runtime, condition: impl Fn(&Runtime) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !condition(runtime) {
        runtime.turn().unwrap();
        assert!(
            Instant::now() < deadline,
            "native recovery probe: {}",
            runtime.engine.state()
        );
        std::thread::yield_now();
    }
}
fn title_pending_metadata_keyboard(runtime: &mut Runtime, activation: &str) -> serde_json::Value {
    until(runtime, |r| {
        r.engine.focus_actions().contains(&UiAction::Retry)
    });
    let focus =
        (activation == "keyboard").then_some(nir_presentation::storage_recovery::CONTROL_ID);
    engine_result(runtime.engine.focus_control(focus)).unwrap();
    assert_eq!(
        runtime.engine.native_storage_recovery_focused(),
        activation == "keyboard"
    );
    let before = state(runtime);
    let backend = runtime.storage.clone();
    let (started, ready) = std::sync::mpsc::channel();
    let (release, gate) = std::sync::mpsc::channel();
    let (driver, worker) = IoWorker::with_driver(runtime.storage.clone(), move |request| {
        assert!(matches!(
            request,
            IoRequest::ReadMetadata(nir_player::PersistenceKind::Preferences)
        ));
        started.send(()).unwrap();
        gate.recv().unwrap();
        IoReply::Event(match backend.preferences() {
            Ok(value) => AppEvent::PreferencesRecovered(value.unwrap_or_default()),
            Err(error) => AppEvent::PersistenceReadFailed {
                kind: nir_player::PersistenceKind::Preferences,
                message: error.to_string(),
            },
        })
    });
    runtime.io = driver;
    match activation {
        "keyboard" => runtime.input(UiAction::Retry).unwrap(),
        "touch" => {
            let layout = runtime.engine.native_storage_recovery_layout().unwrap();
            runtime.cursor = (layout.retry[0] + 20., layout.retry[1] + 20.);
            for phase in [RecoveryPhase::Down, RecoveryPhase::Up] {
                assert!(runtime
                    .metadata_pointer(RecoveryPointer::Touch(63), phase)
                    .unwrap());
            }
        }
        "shortcut" => runtime.retry_metadata().unwrap(),
        _ => panic!("unknown metadata activation"),
    }
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    runtime.turn().unwrap(); // Observe the actual disabled-control projection.
    let pending = state(runtime);
    let next = runtime.engine.primary_action();
    runtime.retry_metadata().unwrap();
    assert_eq!(
        runtime.io.outstanding(),
        1,
        "a repeated retry must not queue another read"
    );
    release.send(()).unwrap();
    until(runtime, |r| r.io.outstanding() == 0);
    runtime.io = IoWorker::new(runtime.storage.clone());
    worker.join().unwrap();
    let report = serde_json::json!({"activation":activation,"before":before,"pending":pending,"primaryWhileBusy":next,"after":state(runtime)});
    eprintln!("NIR_METADATA_TITLE_KEYBOARD {}", report);
    assert!(
        next.is_none() || next == Some(UiAction::Retry),
        "a disabled metadata retry must not fall through to Begin reading"
    );
    assert_eq!(pending["screen"], "Title");
    assert_eq!(pending["sequence"], before["sequence"]);
    report
}
fn pending_saves(
    runtime: &mut Runtime,
    source: &mut rodio::mixer::MixerSource,
) -> Vec<serde_json::Value> {
    let mut pending_save_checks = Vec::new();
    for outcome in ["success", "failure"] {
        until(runtime, |r| r.io.outstanding() == 0);
        let slot_file = runtime.storage.slot(0).unwrap();
        let old_record = fs::read(&slot_file).unwrap();
        let old_revision = runtime.storage.load(0).unwrap().unwrap().revision;
        let blocked = slot_file.with_extension(format!("{}.tmp", std::process::id()));
        if outcome == "failure" {
            fs::create_dir(&blocked).unwrap();
        }
        let backend = runtime.storage.clone();
        let (started, ready) = std::sync::mpsc::channel();
        let (release, gate) = std::sync::mpsc::channel();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed_calls = calls.clone();
        let (driver, worker) = IoWorker::with_driver(runtime.storage.clone(), move |request| {
            match request {
                IoRequest::Save {
                    slot,
                    expected_revision,
                    job,
                    envelope,
                } => {
                    observed_calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    // The success case has already committed on disk;
                    // the failure case has not attempted its blocked write.
                    // Neither case publishes a terminal result before release.
                    let committed = if outcome == "success" {
                        Some(backend.save(slot, expected_revision, &envelope))
                    } else {
                        None
                    };
                    started.send(()).unwrap();
                    gate.recv().unwrap();
                    IoReply::Event(
                        match committed
                            .unwrap_or_else(|| backend.save(slot, expected_revision, &envelope))
                        {
                            Ok(()) => AppEvent::Saved {
                                job,
                                slot,
                                revision: envelope.revision,
                            },
                            Err(error) => AppEvent::SaveFailed {
                                job,
                                message: error.to_string(),
                            },
                        },
                    )
                }
                IoRequest::List => {
                    let mut rows = Vec::new();
                    let mut revisions = BTreeMap::new();
                    for slot in 0..3 {
                        let record = backend.load(slot).unwrap();
                        if let Some(record) = &record {
                            revisions.insert(slot, record.revision);
                        }
                        rows.push(nir_presentation::SlotView {
                            slot,
                            exists: record.is_some(),
                            label: record
                                .map(|r| format!("#{}", r.revision))
                                .unwrap_or_default(),
                            ..Default::default()
                        });
                    }
                    IoReply::Event(AppEvent::Slots(rows, revisions))
                }
                IoRequest::MergeProfile(keys) => {
                    IoReply::Event(match backend.merge_profile(keys) {
                        Ok(()) => AppEvent::PersistenceStored(nir_player::PersistenceKind::Profile),
                        Err(error) => AppEvent::PersistenceFailed {
                            kind: nir_player::PersistenceKind::Profile,
                            message: error.to_string(),
                        },
                    })
                }
                IoRequest::WritePreferences(p) => {
                    IoReply::Event(match backend.write_preferences(&p) {
                        Ok(()) => {
                            AppEvent::PersistenceStored(nir_player::PersistenceKind::Preferences)
                        }
                        Err(error) => AppEvent::PersistenceFailed {
                            kind: nir_player::PersistenceKind::Preferences,
                            message: error.to_string(),
                        },
                    })
                }
                _ => panic!("unexpected pending-save probe request"),
            }
        });
        runtime.io = driver;
        let before = state(runtime);
        let bgm_keys = runtime
            .voices
            .iter()
            .filter(|(_, voice)| voice.bus == AudioBus::Bgm)
            .map(|(key, _)| *key)
            .collect::<Vec<_>>();
        assert!(!bgm_keys.is_empty());
        runtime.input(UiAction::Save { slot: 0 }).unwrap();
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        runtime.request_close("NIR pending save probe").unwrap();
        until(runtime, |r| r.io.delayed_save_pending());
        let pending = state(runtime);
        assert_eq!(runtime.io.outstanding(), 1);
        assert_eq!(pending["diagnostic"]["code"], "E_STORAGE_UNCERTAIN");
        assert_eq!(pending["error"], serde_json::Value::Null);
        assert!(pending["status"].as_str().unwrap().contains("确认"));
        assert_eq!(runtime.close_action(), CloseAction::Cancel);
        runtime.cancel_close("NIR pending save probe").unwrap();
        // A later close after the once-only notice also stays open.
        runtime.request_close("NIR pending save probe").unwrap();
        assert_eq!(runtime.close_action(), CloseAction::Cancel);
        runtime.cancel_close("NIR pending save probe").unwrap();
        // Duplicate save input is handled by the actual Player busy-slot
        // policy, rather than a stub refusing a second filesystem write.
        for _ in 0..8 {
            runtime.input(UiAction::Save { slot: 0 }).unwrap();
            runtime.turn().unwrap();
            assert_eq!(runtime.io.outstanding(), 1);
            assert_eq!(state(runtime)["diagnostic"], pending["diagnostic"]);
        }
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        let visible = state(runtime);
        for key in [
            "session",
            "interaction",
            "position",
            "tick_us",
            "history_count",
            "sequence",
        ] {
            assert_eq!(visible[key], before[key]);
        }
        let pending_samples = source.by_ref().take(9600).collect::<Vec<_>>();
        assert!(pending_samples.iter().any(|s| s.abs() > 0.00001));
        assert!(runtime
            .voices
            .values()
            .filter(|v| v.bus == AudioBus::Bgm)
            .all(|v| !v.sink.is_paused()));
        assert_eq!(
            runtime
                .voices
                .iter()
                .filter(|(_, voice)| voice.bus == AudioBus::Bgm)
                .map(|(key, _)| *key)
                .collect::<Vec<_>>(),
            bgm_keys
        );
        runtime.input(UiAction::Close).unwrap();
        until(runtime, |r| {
            let s = state(r);
            s["screen"] == "Story"
                && s["paused"] == false
                && s["loading"] == false
                && !s["dialogue"].is_null()
        });
        let reading_before = state(runtime);
        runtime.input(UiAction::Advance).unwrap();
        until(runtime, |r| {
            let s = state(r);
            if s["screen"] != "Story" || s["paused"] != false || s["loading"] != false {
                return false;
            }
            if reading_before["dialogue"]["ready"] == false {
                s["dialogue"]["ready"] == true
                    && s["dialogue"]["visible"] != reading_before["dialogue"]["visible"]
            } else {
                s["interaction"] != reading_before["interaction"]
                    || s["position"] != reading_before["position"]
            }
        });
        let reading_after = state(runtime);
        assert!(
            runtime.io.delayed_save_pending(),
            "reading did not settle or replace the original save"
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        runtime.input(UiAction::Saves).unwrap();
        until(runtime, |r| state(r)["screen"] == "Saves");
        release.send(()).unwrap();
        until(runtime, |r| r.io.outstanding() == 0);
        let settled = state(runtime);
        for key in [
            "session",
            "interaction",
            "position",
            "tick_us",
            "history_count",
        ] {
            assert_eq!(
                settled[key], reading_after[key],
                "confirmation must not restore the earlier save point"
            );
        }
        assert!(!runtime.io.delayed_save_pending());
        assert_eq!(settled["error"], serde_json::Value::Null);
        if outcome == "success" {
            assert_eq!(settled["diagnostic"], serde_json::Value::Null);
            assert_eq!(
                runtime.storage.load(0).unwrap().unwrap().revision,
                old_revision + 1
            );
        } else {
            assert_eq!(settled["diagnostic"]["code"], "E_STORAGE");
            assert_eq!(fs::read(&slot_file).unwrap(), old_record);
            fs::remove_dir(blocked).unwrap();
        }
        assert_eq!(runtime.close_action(), CloseAction::Continue);
        runtime.request_close("NIR pending save probe").unwrap();
        until(runtime, |r| r.engine.pending_events() == 0);
        assert_eq!(runtime.close_action(), CloseAction::Exit);
        runtime.cancel_close("NIR pending save probe").unwrap();
        pending_save_checks.push(serde_json::json!({
            "outcome":outcome,"before":before,"pending":pending,"visibleAfterCancel":visible,"settled":settled,
            "saveCalls":calls.load(std::sync::atomic::Ordering::Relaxed),"outstandingDuringNotice":1,
            "pendingCloseCancelled":true,"laterCloseCancelled":true,"freshCloseAfterSettlement":true,
            "readingBefore":reading_before,"readingAfter":reading_after,"bgmKeysDuringMenu":bgm_keys,
            "softwareNonzeroSamples":pending_samples.iter().filter(|s| s.abs()>0.00001).count()
        }));
        // All physical requests and owner replies have settled before
        // replacing this controlled worker with the ordinary backend.
        runtime.io = IoWorker::new(runtime.storage.clone());
        worker.join().unwrap();
    }
    pending_save_checks
}

#[test]
#[ignore = "requires NIR_RECOVERY_UI_PACKAGE, local X11/Vulkan and isolated ALSA config"]
#[allow(deprecated)]
fn lost_native_save_confirmation_recovers_without_replay_or_rewinding_reading() {
    let package = PathBuf::from(std::env::var_os("NIR_RECOVERY_UI_PACKAGE").unwrap());
    let bundle = Arc::new(Bundle::open(&package.join("data")).unwrap());
    let mut builder = EventLoop::builder();
    builder.with_x11().with_any_thread(true);
    let event_loop = builder.build().unwrap();
    let window = Arc::new(
        event_loop
            .create_window(window_attributes("NIR lost save probe", true))
            .unwrap(),
    );
    let data = tempfile::tempdir().unwrap();
    let mut runtime = Runtime::new(window, bundle, data.path().to_owned(), None).unwrap();
    let (source_tx, sources) = std::sync::mpsc::channel();
    runtime.output_worker = OutputWorker::with_opener(move |_, _| {
        let (mixer, source) = rodio::mixer::mixer(2, 48_000);
        source_tx.send(source).unwrap();
        Ok(OutputHandle {
            mixer,
            rate: 48_000,
        })
    });
    runtime.output = OutputState::default();
    runtime.audio = None;
    runtime.engine.begin_turn();
    runtime.retry_output(true).unwrap();
    until(&mut runtime, |r| r.engine.is_ready() && !r.output.blocked());
    runtime.input(UiAction::NewGame).unwrap();
    until(&mut runtime, |r| {
        state(r)["screen"] == "Story"
            && state(r)["loading"] == false
            && r.voices.values().any(|v| v.bus == AudioBus::Bgm)
    });
    let mut source = sources.recv_timeout(Duration::from_secs(5)).unwrap();
    runtime.input(UiAction::Saves).unwrap();
    until(&mut runtime, |r| {
        r.io.outstanding() == 0 && state(r)["screen"] == "Saves"
    });
    runtime.input(UiAction::Save { slot: 0 }).unwrap();
    until(&mut runtime, |r| {
        r.io.outstanding() == 0 && state(r)["status"] == "已保存"
    });
    let mut checks = Vec::new();
    for outcome in ["matching", "not-written", "conflict", "unreadable"] {
        until(&mut runtime, |r| r.io.outstanding() == 0);
        let before = state(&runtime);
        let path = runtime.storage.slot(0).unwrap();
        let old_record = fs::read(&path).unwrap();
        let old_revision = runtime.storage.load(0).unwrap().unwrap().revision;
        let bgm_keys = runtime
            .voices
            .iter()
            .filter(|(_, v)| v.bus == AudioBus::Bgm)
            .map(|(key, _)| *key)
            .collect::<Vec<_>>();
        assert!(!bgm_keys.is_empty());
        let backend = runtime.storage.clone();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = calls.clone();
        let (sent, records) = std::sync::mpsc::channel();
        let (driver, worker) = IoWorker::with_driver(runtime.storage.clone(), move |request| {
            let IoRequest::Save {
                slot,
                expected_revision,
                job,
                mut envelope,
            } = request
            else {
                panic!("only the original save may execute on the dying worker")
            };
            count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if outcome != "not-written" {
                backend.save(slot, expected_revision, &envelope).unwrap();
            }
            let written = fs::read(backend.slot(slot).unwrap()).unwrap();
            if outcome == "unreadable" {
                fs::write(backend.slot(slot).unwrap(), b"{unreadable save").unwrap();
            } else if outcome == "conflict" {
                envelope.snapshot.tick_us.0 += 1;
                envelope.digest =
                    nir_content::digest(&serde_json::to_vec(&envelope.snapshot).unwrap());
                crate::atomic_write(
                    &backend.slot(slot).unwrap(),
                    &serde_json::to_vec(&envelope).unwrap(),
                )
                .unwrap();
            }
            sent.send((job, written)).unwrap();
            panic!("controlled native save thread death before owner confirmation");
        });
        runtime.io = driver;
        runtime.input(UiAction::Save { slot: 0 }).unwrap();
        assert!(worker.join().is_err());
        let (job, committed) = records.recv().unwrap();
        let record_after_loss = fs::read(&path).unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        runtime.turn().unwrap(); // Admit the original uncertain event, not the subsequent read.
        let pending = state(&runtime);
        assert!(runtime.io.has_unconfirmed_saves());
        assert!(
            runtime
                .engine
                .native_storage_recovery_layout()
                .unwrap()
                .status
                .save_pending
        );
        assert_eq!(pending["diagnostic"]["code"], "E_STORAGE_UNCERTAIN");
        assert_eq!(pending["error"], serde_json::Value::Null);
        for _ in 0..8 {
            runtime.input(UiAction::Save { slot: 0 }).unwrap();
        }
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        runtime.request_close("NIR lost save probe").unwrap();
        assert_eq!(runtime.close_action(), CloseAction::Cancel);
        runtime.cancel_close("NIR lost save probe").unwrap();
        let mut continued_reading = None;
        let mut touch_release = None;
        if outcome == "unreadable" {
            until(&mut runtime, |r| r.io.outstanding() == 0);
            let layout = runtime.engine.native_storage_recovery_layout().unwrap();
            assert!(layout.status.retry_enabled && !layout.status.pending);
            for _ in 0..100 {
                runtime.turn().unwrap();
                assert_eq!(runtime.io.outstanding(), 0);
            }
            assert_eq!(fs::read(&path).unwrap(), record_after_loss);
            // Reading remains usable even while a stopped worker's result
            // cannot be determined. A later confirmation cannot restore it.
            runtime.input(UiAction::Close).unwrap();
            until(&mut runtime, |r| {
                state(r)["screen"] == "Story"
                    && state(r)["loading"] == false
                    && state(r)["paused"] == false
            });
            let reading_before = state(&runtime);
            runtime.input(UiAction::Advance).unwrap();
            until(&mut runtime, |r| {
                let s = state(r);
                s["screen"] == "Story"
                    && s["loading"] == false
                    && s["paused"] == false
                    && if reading_before["dialogue"]["ready"] == false {
                        s["dialogue"]["ready"] == true
                    } else {
                        s["interaction"] != reading_before["interaction"]
                            || s["position"] != reading_before["position"]
                    }
            });
            continued_reading = Some(state(&runtime));
            assert!(runtime.io.has_unconfirmed_saves());
            runtime.input(UiAction::Saves).unwrap();
            until(&mut runtime, |r| {
                r.io.outstanding() == 0 && state(r)["screen"] == "Saves"
            });
            let layout = runtime.engine.native_storage_recovery_layout().unwrap();
            runtime.cursor = (layout.retry[0] + 20., layout.retry[1] + 20.);
            for phase in [RecoveryPhase::Down, RecoveryPhase::Up] {
                assert!(runtime
                    .metadata_pointer(RecoveryPointer::Touch(81), phase)
                    .unwrap());
            }
            assert_eq!(runtime.io.outstanding(), 1);
            runtime.retry_metadata().unwrap();
            assert_eq!(
                runtime.io.outstanding(),
                1,
                "duplicate retries must not replay a write or queue reads"
            );
            until(&mut runtime, |r| r.io.outstanding() == 0);
            assert!(runtime.io.has_unconfirmed_saves());
            assert_eq!(fs::read(&path).unwrap(), record_after_loss);
            // External repair belongs only to this fixture.
            crate::atomic_write(&path, &committed).unwrap();
            let repaired_modified = fs::metadata(&path).unwrap().modified().unwrap();
            let before_retry = state(&runtime);
            for phase in [RecoveryPhase::Down, RecoveryPhase::Up] {
                assert!(runtime
                    .metadata_pointer(RecoveryPointer::Touch(82), phase)
                    .unwrap());
            }
            assert!(runtime
                .metadata_pointer(RecoveryPointer::Touch(83), RecoveryPhase::Down)
                .unwrap());
            until(&mut runtime, |r| {
                r.io.outstanding() == 0 && !r.io.has_unconfirmed_saves()
            });
            assert!(runtime.engine.native_storage_recovery_layout().is_none());
            let settled = state(&runtime);
            assert!(runtime
                .metadata_pointer(RecoveryPointer::Touch(83), RecoveryPhase::Up)
                .unwrap());
            let released = state(&runtime);
            for key in [
                "session",
                "interaction",
                "position",
                "tick_us",
                "history_count",
                "variables",
            ] {
                assert_eq!(
                    settled[key], before_retry[key],
                    "late read confirmation must not rewind reading"
                );
                assert_eq!(
                    released[key], settled[key],
                    "touch release must not hit the newly uncovered menu"
                );
            }
            assert_eq!(fs::read(&path).unwrap(), committed);
            assert_eq!(
                fs::metadata(&path).unwrap().modified().unwrap(),
                repaired_modified
            );
            touch_release = Some(
                serde_json::json!({"before":before_retry,"settled":settled,"released":released}),
            );
        } else {
            until(&mut runtime, |r| {
                r.io.outstanding() == 0 && !r.io.has_unconfirmed_saves()
            });
            let after_read = state(&runtime);
            for key in [
                "session",
                "interaction",
                "position",
                "tick_us",
                "history_count",
                "variables",
            ] {
                assert_eq!(
                    after_read[key], before[key],
                    "read-only confirmation must preserve the current session"
                );
            }
            assert_eq!(fs::read(&path).unwrap(), record_after_loss);
            assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
        }
        let settled = state(&runtime);
        assert_eq!(settled["error"], serde_json::Value::Null);
        match outcome {
            "matching" | "unreadable" => {
                assert_eq!(settled["diagnostic"], serde_json::Value::Null);
                assert_eq!(
                    runtime.storage.load(0).unwrap().unwrap().revision,
                    old_revision + 1
                );
            }
            "not-written" => {
                assert_eq!(settled["diagnostic"]["code"], "E_STORAGE");
                assert_eq!(fs::read(&path).unwrap(), old_record);
            }
            "conflict" => assert_eq!(settled["diagnostic"]["code"], "E_SAVE_CONFLICT"),
            _ => unreachable!(),
        }
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(
            runtime
                .voices
                .iter()
                .filter(|(_, v)| v.bus == AudioBus::Bgm)
                .map(|(key, _)| *key)
                .collect::<Vec<_>>(),
            bgm_keys
        );
        let samples = source.by_ref().take(9600).collect::<Vec<_>>();
        assert!(samples.iter().any(|v| v.abs() > 0.00001));
        checks.push(serde_json::json!({"outcome":outcome,"originalJob":job,"saveCalls":1,"before":before,"pending":pending,"settled":settled,"continuedReading":continued_reading,"touchRelease":touch_release,"bgmKeys":bgm_keys,"softwareNonzeroSamples":samples.iter().filter(|v| v.abs()>0.00001).count(),"oldRevision":old_revision,"recordAfterLossSha256":nir_content::digest(&record_after_loss),"originalWrittenRecordSha256":nir_content::digest(&committed)}));
        eprintln!("NIR_LOST_SAVE_CASE {}", checks.last().unwrap());
        // Only a fresh explicit user save is allowed after settlement.
        runtime.input(UiAction::Save { slot: 0 }).unwrap();
        until(&mut runtime, |r| {
            r.io.outstanding() == 0 && state(r)["status"] == "已保存"
        });
    }
    if let Some(path) = std::env::var_os("NIR_RECOVERY_UI_REPORT") {
        fs::write(path, serde_json::to_vec_pretty(&serde_json::json!({"lostSaveChecks":checks,"after":state(&runtime),"limits":["Controlled thread death with actual durable Storage writes, not physical power loss","Hidden X11/Vulkan window and native handlers, not Android touch events","Actual rodio software mixer, not device or acoustic output"]})).unwrap()).unwrap();
    }
}

#[test]
#[ignore = "requires NIR_RECOVERY_UI_PACKAGE, local X11/Vulkan and isolated ALSA config"]
#[allow(deprecated)]
fn pending_native_save_keeps_reading_and_busy_slot_until_original_reply() {
    let package = PathBuf::from(std::env::var_os("NIR_RECOVERY_UI_PACKAGE").unwrap());
    let bundle = Arc::new(Bundle::open(&package.join("data")).unwrap());
    let mut builder = EventLoop::builder();
    builder.with_x11().with_any_thread(true);
    let event_loop = builder.build().unwrap();
    let window = Arc::new(
        event_loop
            .create_window(window_attributes("NIR pending save probe", true))
            .unwrap(),
    );
    let data = tempfile::tempdir().unwrap();
    let mut runtime = Runtime::new(window, bundle, data.path().to_owned(), None).unwrap();
    let (source_tx, sources) = std::sync::mpsc::channel();
    runtime.output_worker = OutputWorker::with_opener(move |_, _| {
        let (mixer, source) = rodio::mixer::mixer(2, 48_000);
        source_tx.send(source).unwrap();
        Ok(OutputHandle {
            mixer,
            rate: 48_000,
        })
    });
    runtime.output = OutputState::default();
    runtime.audio = None;
    runtime.engine.begin_turn();
    runtime.retry_output(true).unwrap();
    until(&mut runtime, |r| r.engine.is_ready() && !r.output.blocked());
    runtime.input(UiAction::NewGame).unwrap();
    until(&mut runtime, |r| {
        state(r)["screen"] == "Story"
            && state(r)["loading"] == false
            && r.voices.values().any(|v| v.bus == AudioBus::Bgm)
    });
    let mut source = sources.recv_timeout(Duration::from_secs(5)).unwrap();
    runtime.input(UiAction::Saves).unwrap();
    until(&mut runtime, |r| {
        r.io.outstanding() == 0 && state(r)["screen"] == "Saves"
    });
    runtime.input(UiAction::Save { slot: 0 }).unwrap();
    until(&mut runtime, |r| {
        r.io.outstanding() == 0 && state(r)["status"] == "已保存"
    });
    let checks = pending_saves(&mut runtime, &mut source);
    if let Some(path) = std::env::var_os("NIR_RECOVERY_UI_REPORT") {
        fs::write(path, serde_json::to_vec_pretty(&serde_json::json!({
            "pendingSaveChecks":checks,"after":state(&runtime),
            "limits":["Hidden actual X11/Vulkan window and renderer submissions; no independent pixel comparison",
                "Actual rodio software mixer and MP3 queues; no CPAL/device/acoustic listening",
                "Native handler inputs, not physical Android touch/lifecycle events"]
        })).unwrap()).unwrap();
    }
}

#[test]
#[ignore = "requires NIR_RECOVERY_UI_PACKAGE, local X11/Vulkan and isolated ALSA config"]
#[allow(deprecated)]
fn window_menu_save_load_and_touch_retry_keep_restore_pause_without_click_through() {
    use nir_presentation::audio_recovery::Status;
    let package = PathBuf::from(std::env::var_os("NIR_RECOVERY_UI_PACKAGE").unwrap());
    let bundle = Arc::new(Bundle::open(&package.join("data")).unwrap());
    let mut builder = EventLoop::builder();
    builder.with_x11().with_any_thread(true);
    let event_loop = builder.build().unwrap();
    let window = Arc::new(
        event_loop
            .create_window(window_attributes("NIR recovery probe", true))
            .unwrap(),
    );
    let data = tempfile::tempdir().unwrap();
    let mut runtime = Runtime::new(window, bundle, data.path().to_owned(), None).unwrap();
    // Only the error/open result is controlled. The actual host, loader,
    // Winit window, renderer, Player, MP3 queues and durable storage run.
    let (source_tx, sources) = std::sync::mpsc::channel();
    let mut attempts = 0;
    runtime.output_worker = OutputWorker::with_opener(move |_, _| {
        attempts += 1;
        if attempts == 1 {
            return Err("controlled output unavailable".into());
        }
        let (mixer, source) = rodio::mixer::mixer(2, 48_000);
        source_tx.send(source).unwrap();
        Ok(OutputHandle {
            mixer,
            rate: 48_000,
        })
    });
    runtime.output = OutputState::default();
    runtime.audio = None;
    runtime.engine.begin_turn();
    runtime.retry_output(true).unwrap();
    until(&mut runtime, |r| r.engine.is_ready());
    runtime.input(UiAction::NewGame).unwrap();
    until(&mut runtime, |r| {
        r.engine
            .native_audio_recovery_layout()
            .is_some_and(|l| l.status == Status::Failed)
    });
    let initial = state(&runtime);
    let layout = runtime.engine.native_audio_recovery_layout().unwrap();
    assert!(initial["paused"].as_bool().unwrap());
    runtime.cursor = (layout.menu[0] + 20., layout.menu[1] + 20.);
    assert!(runtime
        .recovery_pointer(RecoveryPointer::Touch(7), RecoveryPhase::Down)
        .unwrap());
    assert!(runtime
        .recovery_pointer(RecoveryPointer::Touch(7), RecoveryPhase::Up)
        .unwrap());
    assert_eq!(state(&runtime)["screen"], "Menu");
    assert!(runtime.engine.native_audio_recovery_layout().is_none());
    assert!(runtime.output.blocked());
    assert!(!runtime.recovery_key(&Key::Named(NamedKey::Tab)).unwrap());
    runtime.input(UiAction::Close).unwrap();
    assert!(runtime.recovery_key(&Key::Named(NamedKey::Tab)).unwrap());
    runtime.turn().unwrap();
    assert_eq!(state(&runtime)["native_audio_recovery_focus"], "retry");
    assert!(runtime.recovery_key(&Key::Named(NamedKey::Tab)).unwrap());
    runtime.turn().unwrap();
    let keyboard_menu = state(&runtime);
    assert_eq!(keyboard_menu["native_audio_recovery_focus"], "menu");
    assert_eq!(keyboard_menu["position"], initial["position"]);
    assert_eq!(keyboard_menu["tick_us"], initial["tick_us"]);
    assert!(runtime.recovery_key(&Key::Named(NamedKey::Enter)).unwrap());
    assert_eq!(state(&runtime)["screen"], "Menu");
    runtime.input(UiAction::Saves).unwrap();
    runtime.input(UiAction::Save { slot: 0 }).unwrap();
    until(&mut runtime, |r| r.storage.load(0).unwrap().is_some());
    assert_eq!(runtime.storage.load(0).unwrap().unwrap().revision, 1);
    until(&mut runtime, |r| state(r)["status"] == "已保存");
    let saved_status = state(&runtime)["status"].clone();
    runtime.input(UiAction::Load { slot: 0 }).unwrap();
    until(&mut runtime, |r| {
        state(r)["screen"] == "Story"
            && state(r)["loading"] == false
            && r.engine.native_audio_recovery_layout().is_some()
    });
    let restored = state(&runtime);
    runtime.input(UiAction::Continue).unwrap();
    assert_eq!(state(&runtime)["paused"], true);
    assert_eq!(state(&runtime)["position"], restored["position"]);
    assert_eq!(state(&runtime)["sequence"], restored["sequence"]);
    let keys: Vec<_> = runtime.voices.keys().copied().collect();
    assert!(!keys.is_empty());
    let layout = runtime.engine.native_audio_recovery_layout().unwrap();
    runtime.cursor = (layout.retry[0] + 20., layout.retry[1] + 20.);
    runtime
        .recovery_pointer(RecoveryPointer::Touch(7), RecoveryPhase::Down)
        .unwrap();
    runtime
        .recovery_pointer(RecoveryPointer::Touch(7), RecoveryPhase::Up)
        .unwrap();
    assert!(runtime.output.pending());
    assert!(runtime.recovery_key(&Key::Named(NamedKey::Enter)).unwrap());
    // A second touch starts on the disabled retry while opening. Finish
    // it only after the notice is gone and Continue is now visible.
    runtime
        .recovery_pointer(RecoveryPointer::Touch(8), RecoveryPhase::Down)
        .unwrap();
    until(&mut runtime, |r| !r.output.blocked());
    let ready = state(&runtime);
    assert!(runtime
        .recovery_pointer(RecoveryPointer::Touch(8), RecoveryPhase::Up)
        .unwrap());
    let released = state(&runtime);
    assert!(runtime.engine.native_audio_recovery_layout().is_none());
    assert_eq!(
        released["native_audio_recovery_focus"],
        serde_json::Value::Null
    );
    assert_eq!(runtime.voices.keys().copied().collect::<Vec<_>>(), keys);
    assert_eq!(released["session"], restored["session"]);
    assert_eq!(released["position"], restored["position"]);
    assert_eq!(released["sequence"], restored["sequence"]);
    assert_eq!(released["tick_us"], restored["tick_us"]);
    assert_eq!(
        released["paused"], true,
        "output readiness must retain load's Continue pause"
    );
    let mut source = sources.recv_timeout(Duration::from_secs(5)).unwrap();
    // Continue is deliberately separate from the recovery gesture.
    runtime.input(UiAction::Continue).unwrap();
    // This snapshot was taken at a cue preparation boundary. Continue
    // admits that cue's resources; its separate loading pause can remain
    // until owner admission completes, even though restore is released.
    until(&mut runtime, |r| {
        state(r)["loading"] == false && state(r)["paused"] == false
    });
    assert_eq!(state(&runtime)["paused"], false);
    let samples: Vec<_> = source.by_ref().take(9600).collect();
    assert!(samples.iter().any(|s| s.abs() > 0.00001));
    until(&mut runtime, |r| r.io.outstanding() == 0);
    runtime
        .storage
        .write_preferences(runtime.engine.preferences())
        .unwrap();
    runtime.storage.merge_profile(BTreeSet::new()).unwrap();
    let prefs_file = runtime.storage.root.join("preferences.json");
    let profile_file = runtime.storage.root.join("profile.json");
    let old_prefs = fs::read(&prefs_file).unwrap();
    let old_profile = fs::read(&profile_file).unwrap();
    let prefs_block = prefs_file.with_extension(format!("{}.tmp", std::process::id()));
    let profile_block = profile_file.with_extension(format!("{}.tmp", std::process::id()));
    fs::create_dir(&prefs_block).unwrap();
    fs::create_dir(&profile_block).unwrap();
    runtime.input(UiAction::Settings).unwrap();
    let persistence_before = state(&runtime);
    let bgm_before = runtime
        .voices
        .iter()
        .filter(|(_, v)| v.bus == AudioBus::Bgm)
        .map(|(key, _)| *key)
        .collect::<Vec<_>>();
    assert!(!bgm_before.is_empty());
    runtime
        .input(UiAction::Volume {
            bus: AudioBus::Bgm,
            delta: -0.1,
        })
        .unwrap();
    runtime
        .submit_storage(IoRequest::MergeProfile(BTreeSet::from([
            "probe.failed".into()
        ])))
        .unwrap();
    until(&mut runtime, |r| r.io.outstanding() == 0);
    let persistence_failed = state(&runtime);
    assert_eq!(persistence_failed["error"], serde_json::Value::Null);
    for key in ["session", "tick_us", "position", "interaction"] {
        assert_eq!(persistence_failed[key], persistence_before[key]);
    }
    assert_eq!(fs::read(&prefs_file).unwrap(), old_prefs);
    assert_eq!(fs::read(&profile_file).unwrap(), old_profile);
    assert_eq!(
        runtime
            .voices
            .iter()
            .filter(|(_, v)| v.bus == AudioBus::Bgm)
            .map(|(key, _)| *key)
            .collect::<Vec<_>>(),
        bgm_before
    );
    assert!(runtime
        .voices
        .values()
        .filter(|v| v.bus == AudioBus::Bgm)
        .all(|v| !v.sink.is_paused()));
    let during_failure = source.by_ref().take(9600).collect::<Vec<_>>();
    assert!(during_failure.iter().any(|v| v.abs() > 0.00001));
    // A failed preference write during close cancels exit after the owner
    // admits the failure, just as a failed slot save already did.
    runtime.request_close("NIR persistence probe").unwrap();
    runtime
        .submit_storage(IoRequest::WritePreferences(
            runtime.engine.preferences().clone(),
        ))
        .unwrap();
    until(&mut runtime, |r| r.io.outstanding() == 0);
    assert_eq!(runtime.close.poll(0, false), CloseAction::Cancel);
    runtime.cancel_close("NIR persistence probe").unwrap();
    fs::remove_dir(&prefs_block).unwrap();
    fs::remove_dir(&profile_block).unwrap();
    runtime
        .input(UiAction::Volume {
            bus: AudioBus::Bgm,
            delta: 0.1,
        })
        .unwrap();
    runtime
        .submit_storage(IoRequest::MergeProfile(BTreeSet::from([
            "probe.later".into()
        ])))
        .unwrap();
    until(&mut runtime, |r| r.io.outstanding() == 0);
    assert!(runtime
        .storage
        .profile()
        .unwrap()
        .is_superset(&BTreeSet::from([
            "probe.failed".into(),
            "probe.later".into()
        ])));
    assert_eq!(
        runtime.storage.preferences().unwrap().unwrap(),
        *runtime.engine.preferences()
    );
    let persistence_recovered = state(&runtime);
    assert_eq!(persistence_recovered["error"], serde_json::Value::Null);
    for key in ["session", "tick_us", "position", "interaction"] {
        assert_eq!(persistence_recovered[key], persistence_before[key]);
    }
    let export_file = runtime.storage.root.join("probe-export.json");
    fs::write(&export_file, b"old export stays intact").unwrap();
    let export_block = export_file.with_extension(format!("{}.tmp", std::process::id()));
    fs::create_dir(&export_block).unwrap();
    let export_before = state(&runtime);
    let (job, json) = export_request(&mut runtime);
    let (send, replies) = std::sync::mpsc::channel();
    runtime.dialog = Some(PendingDialog {
        replies,
        failure: dialog::DialogFailure::Export(job),
    });
    send.send(DialogOutcome::ExportSelected {
        job,
        path: Some(export_file.clone()),
        json,
    })
    .unwrap();
    runtime.request_close("NIR export probe").unwrap();
    runtime.storage_turn().unwrap();
    assert_eq!(runtime.io.outstanding(), 1);
    until(&mut runtime, |r| r.io.outstanding() == 0);
    let export_failed = state(&runtime);
    assert_eq!(export_failed["diagnostic"]["location"], "export");
    assert_eq!(export_failed["error"], serde_json::Value::Null);
    assert_eq!(runtime.close.poll(0, false), CloseAction::Cancel);
    runtime.cancel_close("NIR export probe").unwrap();
    assert_eq!(fs::read(&export_file).unwrap(), b"old export stays intact");
    let export_samples = source.by_ref().take(9600).collect::<Vec<_>>();
    assert!(export_samples.iter().any(|s| s.abs() > 0.00001));
    let (job, json) = export_request(&mut runtime);
    let (send, replies) = std::sync::mpsc::channel();
    runtime.dialog = Some(PendingDialog {
        replies,
        failure: dialog::DialogFailure::Export(job),
    });
    send.send(DialogOutcome::ExportSelected {
        job,
        path: None,
        json,
    })
    .unwrap();
    runtime.storage_turn().unwrap();
    assert_eq!(runtime.io.outstanding(), 0);
    assert_eq!(fs::read(&export_file).unwrap(), b"old export stays intact");
    fs::remove_dir(&export_block).unwrap();
    let (job, json) = export_request(&mut runtime);
    let (send, replies) = std::sync::mpsc::channel();
    runtime.dialog = Some(PendingDialog {
        replies,
        failure: dialog::DialogFailure::Export(job),
    });
    send.send(DialogOutcome::ExportSelected {
        job,
        path: Some(export_file.clone()),
        json,
    })
    .unwrap();
    runtime.storage_turn().unwrap();
    until(&mut runtime, |r| r.io.outstanding() == 0);
    let exported: nir_player::SaveEnvelope =
        nir_content::parse(&fs::read(&export_file).unwrap(), "export probe").unwrap();
    assert_eq!(
        exported.digest,
        nir_content::digest(&serde_json::to_vec(&exported.snapshot).unwrap())
    );
    let export_recovered = state(&runtime);
    assert_ne!(export_recovered["diagnostic"]["location"], "export");
    // A picker thread exiting without an outcome also keeps the window.
    let (job, _) = export_request(&mut runtime);
    let (send, replies) = std::sync::mpsc::channel();
    runtime.dialog = Some(PendingDialog {
        replies,
        failure: dialog::DialogFailure::Export(job),
    });
    drop(send);
    runtime.storage_turn().unwrap();
    let export_disconnected = state(&runtime);
    assert_eq!(export_disconnected["diagnostic"]["location"], "export");
    assert!(export_disconnected["diagnostic"]["message"]
        .as_str()
        .unwrap()
        .contains("E_DIALOG_THREAD"));
    for current in [&export_failed, &export_recovered, &export_disconnected] {
        for key in ["session", "tick_us", "position", "interaction"] {
            assert_eq!(current[key], export_before[key]);
        }
        assert_eq!(current["error"], serde_json::Value::Null);
    }
    assert_eq!(
        runtime
            .voices
            .iter()
            .filter(|(_, v)| v.bus == AudioBus::Bgm)
            .map(|(key, _)| *key)
            .collect::<Vec<_>>(),
        bgm_before
    );
    assert!(runtime
        .voices
        .values()
        .filter(|v| v.bus == AudioBus::Bgm)
        .all(|v| !v.sink.is_paused()));
    let export_checks = serde_json::json!({
        "before":export_before,"failed":export_failed,"recovered":export_recovered,"disconnected":export_disconnected,
        "oldFilePreserved":true,"cancelSubmittedNoWrite":true,"exportDigestValid":true,
        "closeCancelledOnFailure":true,"softwareNonzeroSamples":export_samples.iter().filter(|s|s.abs()>0.00001).count(),
    });
    let healthy_slot = runtime.storage.load(0).unwrap().unwrap();
    let mut startup_checks = vec![];
    for kind in ["preferences", "profile", "both"] {
        let data = tempfile::tempdir().unwrap();
        let bundle = Arc::new(Bundle::open(&package.join("data")).unwrap());
        let storage = Storage::open(
            data.path(),
            &bundle.manifest.game_id,
            &bundle.manifest.profile,
            &bundle.release,
        )
        .unwrap();
        storage.save(0, 0, &healthy_slot).unwrap();
        let preferences = Preferences {
            font_scale: 1.4,
            bgm_volume: 0.2,
            ..Default::default()
        };
        storage.write_preferences(&preferences).unwrap();
        storage
            .merge_profile(BTreeSet::from(["fixture.prior".into()]))
            .unwrap();
        let prefs_file = storage.root.join("preferences.json");
        let profile_file = storage.root.join("profile.json");
        let healthy_prefs = fs::read(&prefs_file).unwrap();
        let healthy_profile = fs::read(&profile_file).unwrap();
        if kind != "profile" {
            fs::write(&prefs_file, b"{broken preferences").unwrap();
        }
        if kind != "preferences" {
            fs::write(&profile_file, b"[1]").unwrap();
        }
        let old_prefs = fs::read(&prefs_file).unwrap();
        let old_profile = fs::read(&profile_file).unwrap();
        drop(storage);
        let window = Arc::new(
            event_loop
                .create_window(window_attributes("NIR startup metadata probe", true))
                .unwrap(),
        );
        let mut probe = Runtime::new(window, bundle, data.path().to_owned(), None).unwrap();
        until(&mut probe, |r| {
            r.engine.is_ready() && state(r)["loading"] == false && r.io.outstanding() == 0
        });
        let startup = state(&probe);
        assert_eq!(startup["screen"], "Title");
        assert_eq!(startup["error"], serde_json::Value::Null);
        assert_eq!(startup["diagnostic"]["code"], "E_STORAGE");
        assert_eq!(
            startup["diagnostic"]["details"]["operation"],
            "load_metadata"
        );
        assert!(!startup["status"].as_str().unwrap().is_empty());
        if kind == "profile" {
            assert_eq!(*probe.engine.preferences(), preferences);
        }
        assert_eq!(fs::read(&prefs_file).unwrap(), old_prefs);
        assert_eq!(fs::read(&profile_file).unwrap(), old_profile);
        let title_keyboard_check = if kind == "preferences" {
            ["keyboard", "touch", "shortcut"]
                .into_iter()
                .map(|activation| title_pending_metadata_keyboard(&mut probe, activation))
                .collect::<Vec<_>>()
        } else {
            vec![]
        };
        probe.input(UiAction::Saves).unwrap();
        until(&mut probe, |r| {
            state(r)["screen"] == "Saves" && state(r)["loading"] == false
        });
        probe.input(UiAction::Load { slot: 0 }).unwrap();
        until(&mut probe, |r| {
            state(r)["screen"] == "Story" && state(r)["loading"] == false && r.io.outstanding() == 0
        });
        let loaded = state(&probe);
        assert_eq!(loaded["error"], serde_json::Value::Null);
        assert_eq!(loaded["paused"], true);
        assert_eq!(loaded["position"], restored["position"]);
        assert_eq!(loaded["diagnostic"]["code"], "E_STORAGE");
        let before = state(&probe);
        if kind != "profile" {
            probe
                .input(UiAction::Volume {
                    bus: AudioBus::Bgm,
                    delta: -0.15,
                })
                .unwrap();
        }
        if kind != "preferences" {
            probe
                .submit_storage(IoRequest::MergeProfile(BTreeSet::from([
                    "probe.startup.failed".into(),
                ])))
                .unwrap();
        }
        until(&mut probe, |r| r.io.outstanding() == 0);
        let failed = state(&probe);
        for key in ["session", "tick_us", "position", "interaction", "paused"] {
            assert_eq!(failed[key], before[key]);
        }
        assert_eq!(failed["error"], serde_json::Value::Null);
        assert_eq!(fs::read(&prefs_file).unwrap(), old_prefs);
        assert_eq!(fs::read(&profile_file).unwrap(), old_profile);
        assert!(
            probe.engine.native_storage_recovery_layout().is_none(),
            "reading is not covered by metadata recovery"
        );
        probe.input(UiAction::Settings).unwrap();
        until(&mut probe, |r| {
            r.engine.native_storage_recovery_layout().is_some()
                && r.engine.focus_actions().contains(&UiAction::Retry)
        });
        // Visit the actual semantic host control through the existing
        // keyboard focus cycle. Its Retry is consumed by Runtime, not VM.
        for _ in 0..512 {
            engine_result(probe.engine.navigate_focus(1)).unwrap();
            if probe.engine.native_storage_recovery_focused() {
                break;
            }
        }
        assert!(
            probe.engine.native_storage_recovery_focused(),
            "metadata recovery lost from keyboard cycle: actions={:?}, center={:?}, state={}",
            probe.engine.focus_actions(),
            probe.engine.focused_center(),
            probe.engine.state()
        );
        let action = probe.engine.primary_action().unwrap();
        assert_eq!(action, UiAction::Retry);
        probe.input(action).unwrap();
        until(&mut probe, |r| r.io.outstanding() == 0);
        let bad_retry = state(&probe);
        assert_eq!(bad_retry["diagnostic"]["code"], "E_STORAGE");
        assert_eq!(fs::read(&prefs_file).unwrap(), old_prefs);
        assert_eq!(fs::read(&profile_file).unwrap(), old_profile);
        // Only the fixture repairs originals, then ordinary worker writes recover.
        if kind != "profile" {
            fs::write(&prefs_file, &healthy_prefs).unwrap();
        }
        if kind != "preferences" {
            fs::write(&profile_file, &healthy_profile).unwrap();
        }
        let layout = probe.engine.native_storage_recovery_layout().unwrap();
        probe.cursor = (layout.retry[0] + 20., layout.retry[1] + 20.);
        assert!(probe
            .metadata_pointer(RecoveryPointer::Touch(41), RecoveryPhase::Down)
            .unwrap());
        assert!(probe
            .metadata_pointer(RecoveryPointer::Touch(41), RecoveryPhase::Up)
            .unwrap());
        let working = probe.engine.native_storage_recovery_layout().unwrap();
        assert!(working.status.pending && !working.status.retry_enabled);
        assert!(probe
            .metadata_pointer(RecoveryPointer::Touch(42), RecoveryPhase::Down)
            .unwrap());
        let partial_recovery = if kind == "both" {
            until(&mut probe, |r| {
                r.engine.native_metadata_failure_kinds()
                    == BTreeSet::from([nir_player::PersistenceKind::Profile])
            });
            let partial = state(&probe);
            assert_eq!(partial["diagnostic"]["location"], "profile");
            Some(partial)
        } else {
            None
        };
        until(&mut probe, |r| {
            r.io.outstanding() == 0 && r.engine.native_metadata_failure_kinds().is_empty()
        });
        let read_recovered = state(&probe);
        assert!(probe.engine.native_storage_recovery_layout().is_none());
        assert!(probe
            .metadata_pointer(RecoveryPointer::Touch(42), RecoveryPhase::Up)
            .unwrap());
        assert_eq!(state(&probe)["position"], read_recovered["position"]);
        assert_eq!(
            probe.engine.preferences().font_scale,
            1.4,
            "untouched preference is recovered from disk"
        );
        if kind != "profile" {
            assert!(
                (probe.engine.preferences().bgm_volume - 0.15).abs() < 0.00001,
                "current-session edit must win over recovered disk value 0.2"
            );
        }
        probe.input(UiAction::Close).unwrap();
        until(&mut probe, |r| state(r)["screen"] == "Story");
        if kind != "profile" {
            probe
                .input(UiAction::Volume {
                    bus: AudioBus::Bgm,
                    delta: 0.1,
                })
                .unwrap();
            until(&mut probe, |r| r.io.outstanding() == 0);
            assert_eq!(
                probe.storage.preferences().unwrap().unwrap(),
                *probe.engine.preferences()
            );
            if kind == "both" {
                assert_eq!(
                    state(&probe)["diagnostic"],
                    serde_json::Value::Null,
                    "both metadata reads and failed writes were explicitly recovered"
                );
            }
        }
        if kind != "preferences" {
            probe
                .submit_storage(IoRequest::MergeProfile(BTreeSet::from([
                    "probe.startup.next".into(),
                ])))
                .unwrap();
            until(&mut probe, |r| r.io.outstanding() == 0);
            let profile = probe.storage.profile().unwrap();
            for key in [
                "fixture.prior",
                "probe.startup.failed",
                "probe.startup.next",
            ] {
                assert!(profile.contains(key));
            }
        }
        let recovered = state(&probe);
        assert_eq!(recovered["diagnostic"], serde_json::Value::Null);
        assert_eq!(recovered["error"], serde_json::Value::Null);
        assert_eq!(recovered["paused"], true);
        startup_checks.push(serde_json::json!({"kind":kind,"startup":startup,"loaded":loaded,"failedWrite":failed,"titleKeyboard":title_keyboard_check,"badRetry":bad_retry,"partialRecovery":partial_recovery,"readRecovered":read_recovered,"recovered":recovered,"originalBadRecordsPreserved":true,"healthySlotLoaded":true,"failedProgressRetriedAfterFixtureRepair":kind!="preferences","touchUpConsumedAfterRecovery":true,"keyboardRetryUsedActualSemanticControl":true}));
    }
    // A real thread unwinds outside the mailbox lock with an accepted
    // Player save and other requests queued behind it. Keep the actual
    // window/Player/renderer/audio while identities are settled one turn
    // at a time, then explicitly submit fresh operations.
    runtime.input(UiAction::Saves).unwrap();
    until(&mut runtime, |r| r.io.outstanding() == 0);
    let death_before = state(&runtime);
    let old_prefs = fs::read(&prefs_file).unwrap();
    let old_profile = fs::read(&profile_file).unwrap();
    assert!(runtime.storage.load(2).unwrap().is_none());
    let (death_started, death_ready) = std::sync::mpsc::channel();
    let (resume, blocked) = std::sync::mpsc::channel();
    let (driver, worker) = IoWorker::with_driver(runtime.storage.clone(), move |request| {
        assert!(matches!(request, IoRequest::Save { slot: 2, .. }));
        death_started.send(()).unwrap();
        blocked.recv().unwrap();
        panic!("controlled native storage thread death before save write");
    });
    runtime.io = driver;
    runtime.input(UiAction::Save { slot: 2 }).unwrap();
    death_ready.recv_timeout(Duration::from_secs(5)).unwrap();
    runtime
        .submit_storage(IoRequest::MergeProfile(BTreeSet::from([
            "probe.thread.failed".into(),
        ])))
        .unwrap();
    runtime
        .submit_storage(IoRequest::WritePreferences(
            runtime.engine.preferences().clone(),
        ))
        .unwrap();
    let (job, json) = export_request(&mut runtime);
    runtime
        .submit_storage(IoRequest::Export {
            job,
            path: export_file.clone(),
            json,
        })
        .unwrap();
    let old_export = fs::read(&export_file).unwrap();
    runtime.request_close("NIR storage death probe").unwrap();
    resume.send(()).unwrap();
    assert!(worker.join().is_err());
    until(&mut runtime, |r| r.io.outstanding() == 0);
    let death_failed = state(&runtime);
    for key in [
        "session",
        "interaction",
        "position",
        "tick_us",
        "history_count",
        "sequence",
    ] {
        assert_eq!(death_failed[key], death_before[key]);
    }
    assert_eq!(death_failed["error"], serde_json::Value::Null);
    assert_eq!(death_failed["diagnostic"]["code"], "E_STORAGE");
    assert!(runtime.window.is_some());
    assert_eq!(runtime.close.poll(0, false), CloseAction::Cancel);
    runtime.cancel_close("NIR storage death probe").unwrap();
    assert!(runtime.storage.load(2).unwrap().is_none());
    assert_eq!(fs::read(&prefs_file).unwrap(), old_prefs);
    assert_eq!(fs::read(&profile_file).unwrap(), old_profile);
    assert_eq!(fs::read(&export_file).unwrap(), old_export);
    let death_samples = source.by_ref().take(9600).collect::<Vec<_>>();
    assert!(death_samples.iter().any(|s| s.abs() > 0.00001));
    runtime.input(UiAction::Save { slot: 2 }).unwrap();
    until(&mut runtime, |r| r.io.outstanding() == 0);
    assert_eq!(runtime.storage.load(2).unwrap().unwrap().revision, 1);
    runtime
        .submit_storage(IoRequest::MergeProfile(BTreeSet::from([
            "probe.thread.next".into(),
        ])))
        .unwrap();
    runtime
        .submit_storage(IoRequest::WritePreferences(
            runtime.engine.preferences().clone(),
        ))
        .unwrap();
    let (job, json) = export_request(&mut runtime);
    runtime
        .submit_storage(IoRequest::Export {
            job,
            path: export_file.clone(),
            json,
        })
        .unwrap();
    until(&mut runtime, |r| r.io.outstanding() == 0);
    let death_recovered = state(&runtime);
    for key in [
        "session",
        "interaction",
        "position",
        "tick_us",
        "history_count",
        "sequence",
    ] {
        assert_eq!(death_recovered[key], death_before[key]);
    }
    assert_eq!(death_recovered["error"], serde_json::Value::Null);
    let profile = runtime.storage.profile().unwrap();
    assert!(profile.contains("probe.thread.failed") && profile.contains("probe.thread.next"));
    assert_eq!(
        runtime
            .voices
            .iter()
            .filter(|(_, v)| v.bus == AudioBus::Bgm)
            .map(|(key, _)| *key)
            .collect::<Vec<_>>(),
        bgm_before
    );
    assert!(runtime
        .voices
        .values()
        .filter(|v| v.bus == AudioBus::Bgm)
        .all(|v| !v.sink.is_paused()));
    let storage_death_checks = serde_json::json!({"before":death_before,"failed":death_failed,"recovered":death_recovered,
        "acceptedSaveSettled":true,"closeCancelled":true,"oldFilesPreserved":true,"explicitNewSaveRevision":1,
        "failedProgressKeysRecovered":true,"bgmKeys":bgm_before,"softwareNonzeroSamples":death_samples.iter().filter(|s|s.abs()>0.00001).count()});
    if let Some(path) = std::env::var_os("NIR_RECOVERY_UI_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&serde_json::json!({
            "initial":initial,"keyboardMenu":keyboard_menu,"savedStatus":saved_status,
            "restored":restored,"ready":ready,"released":released,
            "persistenceBefore":persistence_before,"persistenceFailed":persistence_failed,"persistenceRecovered":persistence_recovered,
            "persistenceBgmKeys":bgm_before,"persistenceSoftwareNonzeroSamples":during_failure.iter().filter(|s|s.abs()>0.00001).count(),
            "oldPersistenceFilesKeptOnFailure":true,"failedProgressRetried":true,"closeCancelledOnPersistenceFailure":true,
            "exportChecks":export_checks,
            "startupChecks":startup_checks,
            "storageDeathChecks":storage_death_checks,
            "afterContinue":state(&runtime),"renderedFrames":state(&runtime)["frames"],
            "sourceNonzeroSamples":samples.iter().filter(|s|s.abs()>0.00001).count(),
            "limits":["Controlled output worker and software mixer; no CPAL/device/hardware listening.",
                "Hidden actual X11/Vulkan window; states and frame submissions, not independent pixel comparison.",
                "Native pointer handler invoked with Touch identity; no physical Android events."]
        })).unwrap()).unwrap();
    }
}
