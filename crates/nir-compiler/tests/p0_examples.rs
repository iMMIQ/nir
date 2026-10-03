//! P0 acceptance fixtures (docs/NIR-NEXT-P0-BASELINE.md): original neutral
//! examples that must check, run their authored scenarios, derive exactly the
//! capabilities their content uses, and drive their menu entry functions
//! through the runtime to the replay outcome.
use nir_compiler::*;
use nir_format::{MenuValueInput, UiAction};
use nir_player::{AppCommand, AppEvent, Player};
use nir_presentation::Screen;
use std::path::{Path, PathBuf};
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples").join(name)
}
fn requires(root: &Path) -> Vec<String> {
    load_project(root).unwrap().program.requires.clone()
}
fn assert_requires(root: &Path, caps: &[&str]) {
    let got = requires(root);
    for cap in caps {
        assert!(got.iter().any(|c| c == cap), "{cap} missing from {got:?}");
    }
}

#[test]
fn reading_lamp_checks_runs_scenarios_and_derives_dialogue_capabilities() {
    let root = fixture("reading-lamp");
    let p = load_project(&root).unwrap();
    let e = compile(&p.program).unwrap();
    validate_executable(&e).unwrap();
    assert_eq!(test_project(&root).unwrap(), ["sunrise", "rest"]);
    assert_requires(
        &root,
        &[
            "text.voice-binding.v1",
            "text.voice-timer.v1",
            "text.window-transition.v1",
            "audio.gain.v1",
            "audio.stop.v1",
            "story.typed-result.v1",
            "player.auto-delay-policy.v1",
        ],
    );
    // The authored fixed Auto policy survives into the program defaults.
    assert_eq!(p.program.player.auto_delay_policy, nir_format::AutoDelayPolicy::Fixed);
    assert_eq!(p.program.player.auto_delay_us.0, 2_500_000);
    // One explicit voice binding on the spoken page, sampled-remaining.
    let bindings: Vec<_> = p
        .program
        .functions
        .values()
        .flat_map(|f| f.blocks.values())
        .flat_map(|b| &b.ops)
        .filter_map(|op| {
            if let nir_format::Operation::DialogueVoice { wait, .. } = &op.operation {
                Some(*wait)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(bindings, [nir_format::VoiceWaitPolicy::SampledRemaining]);
}

#[test]
fn replay_atlas_checks_runs_tour_and_derives_menu_capabilities() {
    let root = fixture("replay-atlas");
    let p = load_project(&root).unwrap();
    let e = compile(&p.program).unwrap();
    validate_executable(&e).unwrap();
    assert_eq!(test_project(&root).unwrap(), ["tour"]);
    assert_requires(
        &root,
        &[
            "ui.replay.v1",
            "ui.menu-effects.v1",
            "ui.menu-transition.v1",
            "ui.menu-element-tween.v1",
        ],
    );
    // The story grants exactly atlas.north; atlas.south stays a locked entry.
    let unlocks: std::collections::BTreeSet<_> = p
        .program
        .functions
        .values()
        .flat_map(|f| f.blocks.values())
        .flat_map(|b| &b.ops)
        .filter_map(|op| {
            if let nir_format::Operation::ProfileMerge { key } = &op.operation {
                Some(key.clone())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(unlocks, ["atlas.north".to_string()].into_iter().collect::<std::collections::BTreeSet<_>>());
    let gallery = &p.program.theme.image_menus["gallery"];
    let guard = |id: &str| {
        gallery
            .elements
            .iter()
            .find(|el| el.id == id)
            .and_then(|el| match &el.content {
                nir_format::MenuContent::Button { requires, .. } => requires.clone(),
                _ => None,
            })
            .unwrap()
    };
    assert_eq!(guard("north"), "atlas.north");
    assert_eq!(guard("south"), "atlas.south");
    assert!(unlocks.contains(&guard("north")));
    assert!(!unlocks.contains(&guard("south")));
    assert!(gallery.effects.as_ref().unwrap().music.is_some());
}

#[test]
fn replay_atlas_entry_functions_finish_replay_completed() {
    let p = load_project(&fixture("replay-atlas")).unwrap();
    let validated = nir_core::ValidatedProgram::new(p.program.clone()).unwrap();
    for entry in ["replay_north", "replay_south"] {
        assert_eq!(&drive(&validated, entry), "replay_completed");
    }
    assert_eq!(&drive(&validated, &p.program.entry), "completed");
}

#[test]
fn voyage_log_checks_runs_tour_and_derives_system_page_capabilities() {
    let root = fixture("voyage-log");
    let p = load_project(&root).unwrap();
    let e = compile(&p.program).unwrap();
    validate_executable(&e).unwrap();
    assert_eq!(test_project(&root).unwrap(), ["tour"]);
    // One authored page covers state, values, services, storage, history,
    // navigation and the stack container: each capability derives from use.
    assert_requires(
        &root,
        &[
            "ui.menu-state.v1",
            "ui.menu-values.v1",
            "ui.menu-services.v1",
            "ui.menu-storage.v1",
            "ui.menu-history.v1",
            "ui.menu-navigation.v1",
            "ui.menu-stack.v1",
        ],
    );
    let system = &p.program.theme.image_menus["system"];
    assert_eq!(system.locals.len(), 5);
    assert!(system.uses_stack());
    assert!(p.program.theme.menu_overlay.as_deref() == Some("system"));
}

/// P0 fixture C at player level: the tabbed system page is driven through the
/// same input routing, control resolution and services a host uses. Page
/// visibility follows the tab local, value controls commit preferences and
/// locals, the saves tab resolves its slot local into the save transaction,
/// and the history window pages within its authored bounds.
#[test]
fn voyage_log_player_drives_tabs_values_saves_and_history() {
    let p = load_project(&fixture("voyage-log")).unwrap();
    let mut player = Player::new(
        p.program.clone(),
        "release".into(),
        "夜航日志 · Voyage Log".into(),
    )
    .unwrap();
    let commands = player.pump(vec![], 10_000);
    settle(&mut player, commands);
    assert_eq!(player.screen, Screen::Title);

    // The title panel button pushes the system page one level deep; the
    // title screen itself stays, only the active page changes.
    tap(&mut player, 640., 408.).expect("title panel button");
    assert_eq!(player.screen, Screen::Title);
    assert_eq!(player.menu_depth(), 1);
    // Default tab is 设置: the speed slider is live, the save page is not.
    assert!(
        matches!(hit(&player, 530., 196.), Some(UiAction::MenuValue { control, .. }) if control == "speed"),
        "the settings rows are interactive on the default tab"
    );
    assert!(hit(&player, 210., 392.).is_none(), "save is hidden on the settings tab");
    // Back returns to the title with the page state discarded.
    tap(&mut player, 1105., 658.).expect("back button");
    assert_eq!(player.screen, Screen::Title);
    assert_eq!(player.menu_depth(), 0);

    // Start reading; four advances leave four committed pages plus the
    // revealing fifth already recorded, so history holds five lines.
    let commands = action(&mut player, UiAction::NewGame);
    settle(&mut player, commands);
    assert_eq!(player.screen, Screen::Story);
    advance_pages(&mut player, 4);

    // The in-story menu key opens the same authored system page (overlay),
    // which is the root page of the menu screen — no parent frames.
    let commands = action(&mut player, UiAction::Menu);
    settle(&mut player, commands);
    assert_eq!(player.screen, Screen::Menu);
    assert_eq!(player.menu_depth(), 0);
    assert!(player.paused(), "the overlay pauses the story clock");

    // Settings tab: the volume slider commits the preference, the reduced
    // motion toggle commits, and the local glow slider snaps to its step.
    commit_value(&mut player, 530., 284., MenuValueInput::Number(0.8));
    assert_eq!(player.preferences.bgm_volume, 0.8);
    commit_value(&mut player, 530., 372., MenuValueInput::Bool(true));
    assert!(player.preferences.reduced_motion);
    commit_value(&mut player, 530., 548., MenuValueInput::Number(44.0));
    assert_eq!(
        player.model().menu_locals.get("glow"),
        Some(&nir_format::MenuValue::Int(40)),
        "the glow slider snaps to its authored step of 10"
    );

    // Saves tab: the slot buttons move the local slot and the save button
    // resolves it into the storage transaction for that slot.
    tap(&mut player, 340., 88.).expect("saves tab");
    assert!(
        matches!(hit(&player, 550., 278.), Some(UiAction::MenuControl { control, .. }) if control == "slot-2"),
        "the slot selectors are live on the saves tab"
    );
    tap(&mut player, 550., 278.).expect("slot-2 selector");
    assert_eq!(
        player.model().menu_locals.get("slot"),
        Some(&nir_format::MenuValue::Int(2))
    );
    let save = hit(&player, 210., 392.).expect("save button");
    let UiAction::MenuControl { control, .. } = &save else {
        panic!("save is a control, got {save:?}");
    };
    assert_eq!(control, "save");
    let commands = action(&mut player, save.clone());
    let (job, slot, revision) = settle(&mut player, commands)
        .iter()
        .find_map(|c| match c {
            AppCommand::Save { slot, expected_revision, job, .. } => Some((*job, *slot, *expected_revision)),
            _ => None,
        })
        .expect("save transaction for the selected slot");
    assert_eq!(slot, 2);
    // The host completes the save; the menu keeps its page afterwards.
    let commands = player.pump(vec![AppEvent::Saved { job, slot, revision }], 10_000);
    settle(&mut player, commands);

    // History tab: two visible rows over five recorded lines page away from
    // the newest entry, clamp at the oldest (5 - 2 = 3), then page back.
    tap(&mut player, 530., 88.).expect("history tab");
    assert_eq!(offset(&player), Some(nir_format::MenuValue::Int(0)));
    tap(&mut player, 210., 428.).expect("older button");
    assert_eq!(offset(&player), Some(nir_format::MenuValue::Int(1)));
    let mut oldest = 1;
    for _ in 0..4 {
        // At the clamp the older button disables instead of paging again.
        if hit(&player, 210., 428.).is_none() {
            break;
        }
        tap(&mut player, 210., 428.).expect("older button");
        let next = offset(&player);
        if next == Some(nir_format::MenuValue::Int(oldest)) {
            break;
        }
        oldest += 1;
    }
    assert!(
        offset(&player) == Some(nir_format::MenuValue::Int(oldest)),
        "the offset clamps at the oldest page"
    );
    for _ in 0..4 {
        if offset(&player) == Some(nir_format::MenuValue::Int(0)) {
            break;
        }
        tap(&mut player, 550., 428.).expect("newer button");
    }
    assert_eq!(offset(&player), Some(nir_format::MenuValue::Int(0)));

    // Closing the overlay resumes the story on the same page.
    tap(&mut player, 1105., 658.).expect("back button");
    assert_eq!(player.screen, Screen::Story);
    assert_eq!(player.menu_depth(), 0);
    assert!(!player.paused());
    assert!(player.core().state().outcome.is_none());
}


// --- player-level drive harness (same contracts as nir-player's suites) -------

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
                AppCommand::GetAssets { request, assets, .. } => {
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

fn hit(p: &Player, x: f32, y: f32) -> Option<UiAction> {
    let mut model = p.model();
    model.stage = [1280., 720.];
    nir_presentation::project(&model, 1280., 720., &nir_presentation::Messages::default()).hit(x, y)
}

fn offset(p: &Player) -> Option<nir_format::MenuValue> {
    p.model().menu_locals.get("offset").cloned()
}

/// Resolve and commit the control under (x, y), replacing its value the way a
/// host does when a drag or key press picks a concrete number.
fn commit_value(p: &mut Player, x: f32, y: f32, value: MenuValueInput) {
    let Some(UiAction::MenuValue { instance, revision, control, .. }) = hit(p, x, y) else {
        panic!("no value control at ({x}, {y})");
    };
    let commands = action(p, UiAction::MenuValue { instance, revision, control, value });
    settle(p, commands);
}

fn tap(p: &mut Player, x: f32, y: f32) -> Option<UiAction> {
    let found = hit(p, x, y);
    if let Some(a) = found.clone() {
        let commands = action(p, a);
        settle(p, commands);
    }
    found
}

fn advance_pages(p: &mut Player, pages: usize) {
    for _ in 0..pages {
        for _ in 0..10_000 {
            if let Some((_, d)) = p.core().dialogue() {
                if !d.at_gate && d.awaiting_advance {
                    break;
                }
            }
            if p.core().state().outcome.is_some() || p.screen != Screen::Story {
                break;
            }
            let commands = p.pump(vec![AppEvent::Tick { delta_us: 1_000_000 }], 10_000);
            settle(p, commands);
        }
        let (_, d) = p.core().dialogue().expect("dialogue page");
        assert!(!d.at_gate && d.awaiting_advance);
        let commands = action(p, UiAction::Advance);
        settle(p, commands);
    }
}

/// Advance/feed loop shared with the corpus harness: prepared activations,
/// gate ticks, dialogue advances, and time for everything else.
fn drive(validated: &nir_core::ValidatedProgram, entry: &str) -> String {
    let mut core = nir_core::Core::new_at(
        validated.clone(),
        "p0".into(),
        "zh-Hans".into(),
        entry,
    )
    .unwrap();
    for sequence in 1..100_000 {
        let input = if let Some(pending) = &core.state().pending {
            nir_core::CoreInput::Prepared {
                activation: pending.id,
            }
        } else if let Some((_, d)) = core.dialogue() {
            if d.at_gate {
                nir_core::CoreInput::Time {
                    delta_us: 1_000_000,
                }
            } else {
                nir_core::CoreInput::Advance {
                    interaction: d.interaction,
                    sequence,
                }
            }
        } else {
            nir_core::CoreInput::Time {
                delta_us: 1_000_000,
            }
        };
        core.step(input, 10_000);
        assert!(core.state().fault.is_none(), "{entry}: {:?}", core.state().fault);
        if let Some(outcome) = core.state().outcome.clone() {
            return outcome;
        }
    }
    panic!("{entry} did not finish");
}
