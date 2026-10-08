//! Player-level certification of a real converted package (P6.2): the full
//! main route and every replay entry driven through the shared Player with
//! Auto reading, unlock-gated replay entries, held skip over already-read
//! text, interface hide/restore, menu switching and mid-scene save/load with
//! rollback. Unlike the VM walk in `tests.rs`, these routes go through player
//! input routing, reading policies, menu control resolution and storage
//! transactions.
use super::tests::{options, sdk};
use super::*;
use nir_format::UiAction;
use nir_player::{AppCommand, AppEvent, Player};
use nir_presentation::Screen;

fn settle(p: &mut Player, commands: Vec<AppCommand>) -> Vec<AppCommand> {
    let mut q = commands;
    let mut other = vec![];
    for _ in 0..60 {
        if q.is_empty() {
            return other;
        }
        let mut next = vec![];
        for c in q {
            match c {
                AppCommand::GetAssets {
                    request, assets, ..
                } => {
                    for asset in assets {
                        next.extend(p.pump(vec![AppEvent::AssetReady { request, asset }], 10_000));
                    }
                }
                AppCommand::PreparePresentation { request } => {
                    next.extend(p.pump(vec![AppEvent::PresentationReady { request }], 10_000))
                }
                AppCommand::PrepareLocale { request, .. } => {
                    next.extend(p.pump(vec![AppEvent::LocaleReady { request }], 10_000))
                }
                _ => other.push(c),
            }
        }
        q = next;
    }
    panic!("preparation did not converge")
}

fn action(p: &mut Player, a: UiAction) -> Vec<AppCommand> {
    p.pump(
        vec![AppEvent::Action {
            action: a,
            interaction: p.current_interaction(),
            sequence: p.core().state().last_input + 1,
            session: p.generation.session,
        }],
        10_000,
    )
}

fn tick(p: &mut Player, delta_us: u64) {
    // A host ends non-looping audio when playback finishes; the certification
    // harness plays that role too, or voice clips would stay running forever
    // and pin their decoded assets against the memory ledger. Loops (BGM) and
    // timed fades end through their own story-clock policies.
    let mut events: Vec<AppEvent> = p
        .core()
        .state()
        .tasks
        .values()
        .filter(|t| {
            t.state == nir_core::TaskState::Running
                && matches!(t.effect, nir_format::Effect::Audio { looped: false, .. })
        })
        .map(|t| AppEvent::AudioEnded {
            domain: nir_format::TimeDomain::Story,
            task: t.id,
            session: p.generation.session,
        })
        .collect();
    events.push(AppEvent::Tick { delta_us });
    let commands = p.pump(events, 10_000);
    settle(p, commands);
}

fn booted(program: &nir_format::Program) -> Player {
    let mut p = Player::new(program.clone(), "release".into(), "Certify".into()).unwrap();
    let commands = p.pump(vec![], 10_000);
    settle(&mut p, commands);
    assert_eq!(p.screen, Screen::Title);
    p
}

fn started(program: &nir_format::Program) -> Player {
    let mut p = booted(program);
    let commands = action(&mut p, UiAction::NewGame);
    settle(&mut p, commands);
    assert_eq!(p.screen, Screen::Story);
    p
}

/// One round of story driving. `advance` decides who commits a fully
/// revealed page: manual input or the automatic reader. Returns false when
/// the program has finished (outcome set) or left the story screen.
fn round(p: &mut Player, automatic: bool) -> bool {
    assert!(
        p.diagnostic.is_none() && p.error.is_none(),
        "{:?}/{:?}",
        p.diagnostic,
        p.error
    );
    assert!(
        p.core().state().fault.is_none(),
        "{:?}",
        p.core().state().fault
    );
    if p.core().state().outcome.is_some() || p.screen != Screen::Story {
        return false;
    }
    if p.paused() {
        // A restored session (load, rollback, replay return) waits for the
        // resume input before accepting story input; Continue releases it
        // without advancing the dialogue.
        let commands = action(p, UiAction::Continue);
        settle(p, commands);
        return true;
    }
    if let Some(choice) = p.core().state().choice.as_ref() {
        // The corpus is linear; a pending choice still commits through the
        // same input path rather than stalling the certification.
        let option = choice
            .options
            .iter()
            .find(|option| option.enabled)
            .unwrap()
            .id
            .clone();
        let commands = action(p, UiAction::Choose { option });
        settle(p, commands);
        return true;
    }
    if let Some((_, d)) = p.core().dialogue() {
        if !d.at_gate && d.awaiting_advance && !automatic && !p.auto {
            let commands = action(p, UiAction::Advance);
            settle(p, commands);
            return true;
        }
    }
    // Auto reading waits out reveal plus the fixed LPB wait; one coarse tick
    // per round keeps the certified route fast without changing semantics.
    tick(p, if automatic { 5_000_000 } else { 1_000_000 });
    true
}

fn drive(p: &mut Player, automatic: bool, limit: usize) {
    let mut progress = std::time::Instant::now();
    for round_index in 0..limit {
        if round_index % 32 == 0 && progress.elapsed().as_secs() >= 15 {
            eprintln!(
                "Player route: round={round_index} automatic={automatic} screen={:?} paused={} tick={} history={} position={:?}",
                p.screen, p.paused(), p.core().state().tick_us.0,
                p.core().state().history.len(), p.core().location()
            );
            progress = std::time::Instant::now();
        }
        if !round(p, automatic) {
            return;
        }
    }
    panic!("route did not finish within {limit} rounds");
}

/// Wait out the return-to-title preparation a finished route schedules; the
/// title session still prepares its media after the core's outcome.
fn await_title(p: &mut Player, what: &str) {
    for _ in 0..300 {
        if p.screen == Screen::Title {
            return;
        }
        let commands = p.pump(
            vec![AppEvent::Tick {
                delta_us: 1_000_000,
            }],
            10_000,
        );
        settle(p, commands);
        assert!(
            p.diagnostic.is_none() && p.error.is_none(),
            "{what}: {:?}/{:?}",
            p.diagnostic,
            p.error
        );
    }
    panic!("{what} did not return to the title");
}

/// Manually advance exactly `pages` fully revealed pages.
fn advance_pages(p: &mut Player, pages: usize) {
    for _ in 0..pages {
        for _ in 0..10_000 {
            if let Some((_, d)) = p.core().dialogue() {
                if !d.at_gate && d.awaiting_advance {
                    break;
                }
            }
            round(p, false);
        }
        let (_, d) = p.core().dialogue().expect("dialogue page");
        assert!(!d.at_gate && d.awaiting_advance);
        let commands = action(p, UiAction::Advance);
        settle(p, commands);
    }
}

/// Opt-in local corpus certification; no proprietary content is stored here.
#[test]
#[ignore = "requires NIR_IMPORT_SOURCE and NIR_IMPORT_OUT"]
fn real_livenovel_player_certifies_full_routes() {
    let source = PathBuf::from(std::env::var_os("NIR_IMPORT_SOURCE").expect("NIR_IMPORT_SOURCE"));
    let out = PathBuf::from(std::env::var_os("NIR_IMPORT_OUT").expect("NIR_IMPORT_OUT"));
    if !out.exists() {
        let mut opts = options(&source, &out);
        opts.entry = None;
        let report = convert(&opts, &sdk()).unwrap();
        assert_eq!(report.errors, 0);
        assert!(report.written);
    }
    let program = crate::load_project(&out).unwrap().program;
    // The importer exposes corpus replays as guarded story entries: an
    // `Entry` control bound to the replay wrapper function, locked behind a
    // `requires` profile key. Finishing returns through the theme's
    // return-to-title outcome, which reopens the menu owning the entry.
    let replay_entries: Vec<String> = program
        .theme
        .image_menus
        .values()
        .flat_map(|menu| menu.controls())
        .filter_map(|(_, action, _)| match action {
            nir_format::ImageMenuAction::Entry { function } => Some(function.clone()),
            _ => None,
        })
        .collect();
    let expected_unlocks: std::collections::BTreeSet<String> = program
        .theme
        .image_menus
        .values()
        .flat_map(|menu| menu.controls())
        .filter_map(|(_, _, requires)| requires.map(str::to_owned))
        .filter(|key| key.starts_with("lm.replay."))
        .collect();
    assert!(
        !replay_entries.is_empty(),
        "corpus must expose replay entries"
    );

    // A. Auto reading completes the main route and unlocks the replay set.
    let mut p = started(&program);
    let commands = action(&mut p, UiAction::ToggleAuto);
    settle(&mut p, commands);
    assert!(p.auto);
    drive(&mut p, true, 200_000);
    assert!(p.core().state().outcome.is_some());
    let unlocks: std::collections::BTreeSet<String> = p
        .profile
        .iter()
        .filter(|key| key.starts_with("lm.replay."))
        .cloned()
        .collect();
    assert_eq!(unlocks, expected_unlocks);
    assert!(p.profile.iter().any(|key| key.starts_with("read:")));

    // B0. Before any unlock, the replay menu's guarded entries refuse to
    // execute: dispatch dies in control resolution without switching the
    // session or leaving the menu.
    let mut fresh = booted(&program);
    {
        let model = fresh.model();
        let opener = program.theme.image_menus[&model.image_menu]
            .controls()
            .find(|(_, action, _)| matches!(action, nir_format::ImageMenuAction::Menu { .. }))
            .expect("title menu links the replay menu")
            .0
            .to_string();
        let commands = action(
            &mut fresh,
            UiAction::MenuControl {
                instance: model.menu_instance,
                revision: model.menu_revision,
                control: opener,
            },
        );
        settle(&mut fresh, commands);
        let replay_menu = fresh.model().image_menu.clone();
        let (control, requires) = program.theme.image_menus[&replay_menu]
            .controls()
            .find_map(|(id, _, requires)| requires.map(|key| (id.to_string(), key.to_string())))
            .expect("replay menu has a guarded entry");
        assert!(
            !fresh.profile.contains(&requires),
            "fresh profile is locked"
        );
        let before = fresh.generation.session;
        let model = fresh.model();
        let commands = action(
            &mut fresh,
            UiAction::MenuControl {
                instance: model.menu_instance,
                revision: model.menu_revision,
                control,
            },
        );
        settle(&mut fresh, commands);
        assert_eq!(fresh.generation.session, before, "locked entry refused");
        assert_eq!(
            fresh.screen,
            Screen::Title,
            "locked entry stays on the title"
        );
        assert_eq!(
            fresh.model().image_menu,
            replay_menu,
            "locked entry stays on the menu"
        );
    }

    // B. Every replay entry runs to its outcome and returns to the replay
    // menu through the theme's return-to-title outcome. Entries are
    // activated through authored menu controls — the same path hosts take —
    // so their availability and unlock guards are part of the certificate.
    assert!(!p.auto, "the route outcome ended automatic reading");
    let commands = action(&mut p, UiAction::Title);
    settle(&mut p, commands);
    assert_eq!(p.screen, Screen::Title);
    let model = p.model();
    let opener = program.theme.image_menus[&model.image_menu]
        .controls()
        .find(|(_, action, _)| matches!(action, nir_format::ImageMenuAction::Menu { .. }))
        .expect("title menu links the replay menu")
        .0
        .to_string();
    let commands = action(
        &mut p,
        UiAction::MenuControl {
            instance: model.menu_instance,
            revision: model.menu_revision,
            control: opener,
        },
    );
    settle(&mut p, commands);
    let replay_menu = p.model().image_menu.clone();
    let entries: Vec<(String, String)> = program.theme.image_menus[&replay_menu]
        .controls()
        .filter_map(|(id, action, _)| match action {
            nir_format::ImageMenuAction::Entry { function } => {
                Some((id.to_string(), function.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        entries
            .iter()
            .map(|(_, function)| function.to_string())
            .collect::<Vec<_>>(),
        replay_entries,
        "replay menu exposes every entry"
    );
    for (control, function) in &entries {
        let model = p.model();
        let before = p.generation.session;
        let commands = action(
            &mut p,
            UiAction::MenuControl {
                instance: model.menu_instance,
                revision: model.menu_revision,
                control: control.clone(),
            },
        );
        settle(&mut p, commands);
        assert!(p.generation.session > before, "{function} switched session");
        assert_eq!(p.screen, Screen::Story, "{function} entered story");
        drive(&mut p, false, 200_000);
        await_title(&mut p, function);
        assert_eq!(
            p.model().image_menu,
            replay_menu,
            "return reopens the replay menu"
        );
        assert!(p.core().state().fault.is_none());
    }

    // C. Held skip fast-forwards the already-read main route.
    let commands = action(&mut p, UiAction::NewGame);
    settle(&mut p, commands);
    assert_eq!(p.screen, Screen::Story);
    advance_pages(&mut p, 1);
    let held = p.current_interaction();
    tick(&mut p, 5_000_000);
    assert_eq!(
        p.current_interaction(),
        held,
        "no automatic advance without auto or skip"
    );
    let commands = action(&mut p, UiAction::HoldSkip { pressed: true });
    settle(&mut p, commands);
    tick(&mut p, 5_000_000);
    assert!(
        p.current_interaction() > held,
        "held skip advanced read page"
    );
    drive(&mut p, false, 200_000);
    assert!(
        p.core().state().outcome.is_some(),
        "held skip completed route"
    );
    let commands = action(&mut p, UiAction::HoldSkip { pressed: false });
    settle(&mut p, commands);

    // D. Interface hide, menu switching, mid-scene save/load and rollback on
    // a fresh session.
    let mut p = started(&program);
    advance_pages(&mut p, 3);
    let (_, d) = p.core().dialogue().unwrap();
    let saved = (d.text_id.clone(), d.interaction);
    // Hide keeps the story running (default continue policy) and restoring
    // consumes the input without advancing.
    let commands = action(&mut p, UiAction::ToggleInterface);
    settle(&mut p, commands);
    assert!(p.interface_hidden());
    let story_us = p.core().state().tick_us;
    tick(&mut p, 1_000_000);
    assert!(
        p.core().state().tick_us > story_us,
        "hidden story keeps running"
    );
    let commands = action(&mut p, UiAction::RestoreInterface);
    settle(&mut p, commands);
    assert!(!p.interface_hidden());
    assert_eq!(p.core().dialogue().unwrap().1.interaction, saved.1);
    // Menu pauses the story; closing resumes the same page.
    let commands = action(&mut p, UiAction::Menu);
    settle(&mut p, commands);
    assert_eq!(p.screen, Screen::Menu);
    assert!(p.paused());
    let commands = action(&mut p, UiAction::Close);
    settle(&mut p, commands);
    assert_eq!(p.screen, Screen::Story);
    assert!(!p.paused());
    assert_eq!(p.core().dialogue().unwrap().1.interaction, saved.1);
    // Save mid-scene, advance past it, roll back, then load the slot back.
    let commands = action(&mut p, UiAction::Menu);
    settle(&mut p, commands);
    // The menu pause freezes the page, so the slot envelope is taken from
    // exactly this reading position — reveal progress and gate/awaiting
    // flags included.
    let saved_position = {
        let (_, d) = p.core().dialogue().unwrap();
        (
            d.text_id.clone(),
            d.span,
            d.cluster,
            d.at_gate,
            d.awaiting_advance,
        )
    };
    let (job, envelope) = action(&mut p, UiAction::Save { slot: 0 })
        .into_iter()
        .find_map(|c| match c {
            AppCommand::Save { job, envelope, .. } => Some((job, envelope)),
            _ => None,
        })
        .unwrap();
    let commands = p.pump(
        vec![AppEvent::Saved {
            job,
            slot: 0,
            revision: 1,
        }],
        10_000,
    );
    settle(&mut p, commands);
    let commands = action(&mut p, UiAction::Close);
    settle(&mut p, commands);
    advance_pages(&mut p, 3);
    let ahead = p.current_interaction();
    let commands = action(&mut p, UiAction::Rollback);
    settle(&mut p, commands);
    assert!(
        p.current_interaction() < ahead,
        "rollback moved behind the last input"
    );
    // Rollback restores under a "restored" pause; release it before storage.
    let commands = action(&mut p, UiAction::Continue);
    settle(&mut p, commands);
    let job = action(&mut p, UiAction::Load { slot: 0 })
        .into_iter()
        .find_map(|c| match c {
            AppCommand::Load { job, .. } => Some(job),
            _ => None,
        })
        .unwrap();
    let commands = p.pump(
        vec![AppEvent::SlotLoaded {
            job,
            envelope: envelope.clone(),
        }],
        10_000,
    );
    settle(&mut p, commands);
    let (_, d) = p.core().dialogue().unwrap();
    // Restore re-mints the interaction identity by design (fresh tokens on
    // top of the session epoch carry stale-input rejection), so the
    // certificate pins the page and its exact reading position instead.
    assert_eq!(
        (
            d.text_id.clone(),
            d.span,
            d.cluster,
            d.at_gate,
            d.awaiting_advance
        ),
        saved_position,
        "slot restored the saved page at its reading position"
    );
    drive(&mut p, false, 200_000);
    assert!(p.core().state().outcome.is_some(), "loaded route completes");
}
