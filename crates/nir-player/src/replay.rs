use super::*;
use menu::FrozenMenu;

/// Budget for stepping a replay candidate's entry block between barriers.
/// The entry block only walks to its first activation or content barrier;
/// story work itself never runs in the candidate.
const REPLAY_ENTRY_BUDGET: u32 = 1_000;

/// One isolated replay transaction. The launching session is frozen on
/// entry; the replay core becomes live only after its candidate prepares,
/// and the frozen session returns only as a revalidated restore candidate.
/// A failed candidate at either boundary keeps the currently live session.
pub(super) struct ReplayWork {
    pub phase: ReplayPhase,
    snapshot: Snapshot,
    checkpoints: Vec<Snapshot>,
    screen: Screen,
    return_screen: Screen,
    image_menu: String,
    overlay_menu: Option<String>,
    menu: FrozenMenu,
    auto: bool,
    skip: bool,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReplayPhase {
    /// The replay candidate is still preparing; the frozen session stays
    /// live and its page stays on screen.
    Entering,
    /// The replay core is the live session.
    Active,
    /// The frozen session is coming back as a restore candidate.
    Returning,
}
impl Player {
    /// The live core belongs to a replay: profile writes, saves and loads
    /// are isolated for the whole time it runs, including its return.
    pub(super) fn replay_live(&self) -> bool {
        self.replay_work
            .as_ref()
            .is_some_and(|w| w.phase != ReplayPhase::Entering)
    }
    pub(super) fn replay_entering(&self) -> bool {
        self.replay_work
            .as_ref()
            .is_some_and(|w| w.phase == ReplayPhase::Entering)
    }
    pub fn replay_phase(&self) -> &'static str {
        match self.replay_work.as_ref().map(|w| w.phase) {
            Some(ReplayPhase::Entering) => "entering",
            Some(ReplayPhase::Active) => "active",
            Some(ReplayPhase::Returning) => "returning",
            None => "inactive",
        }
    }
    /// Freeze the launching session and prepare the replay candidate. The
    /// candidate's entry block runs only inside the candidate: its sounds,
    /// traces and profile intents do not exist until it goes live.
    pub(super) fn begin_replay(&mut self, function: &str, budget: &mut u32) -> Result<()> {
        if self.replay_work.is_some()
            || self.prepare.is_some()
            || self.candidate.is_some()
            || self.restore_work.is_some()
            || self.slot_load.is_some()
            || !matches!(self.screen, Screen::Title | Screen::Menu)
        {
            return Ok(());
        }
        let work = ReplayWork {
            phase: ReplayPhase::Entering,
            snapshot: self.core.snapshot(),
            checkpoints: self.checkpoints.clone(),
            screen: self.screen,
            return_screen: self.return_screen,
            image_menu: self.image_menu.clone(),
            overlay_menu: self.overlay_menu.clone(),
            menu: self.menu_session.freeze(),
            auto: self.auto,
            skip: self.skip,
        };
        let mut candidate = Core::new_at(
            self.validated.clone(),
            self.release.clone(),
            self.effective_text_locale.clone(),
            function,
        )?;
        let output = candidate.step(CoreInput::None, (*budget).min(REPLAY_ENTRY_BUDGET));
        *budget -= output.work_used;
        let needs = self.replay_entry_needs(&output.intents)?;
        self.replay_work = Some(work);
        self.candidate = Some(candidate);
        let started = if needs.is_empty() {
            self.begin_replay_media()
        } else {
            self.begin_content(ContentPurpose::ReplayEntry, needs)
        };
        if let Err(error) = started {
            return Err(self.abort_replay_entry(error));
        }
        Ok(())
    }
    /// Content the replay entry block crossed before its first media
    /// barrier; the candidate cannot go live without it resident.
    fn replay_entry_needs(&self, intents: &[CoreIntent]) -> Result<Vec<ContentRequest>> {
        let mut needs = Vec::new();
        for intent in intents {
            if let CoreIntent::PrepareContent { module, locale } = intent {
                needs.extend(self.content_requirements(module, Some(locale.as_str()), true)?);
            }
        }
        Ok(needs)
    }
    pub(super) fn begin_replay_media(&mut self) -> Result<()> {
        let Some(candidate) = self.candidate.clone() else {
            return Ok(());
        };
        // The activation is the candidate's own pending cue: its commit steps
        // the replay core past exactly the barrier it stopped at.
        let activation = candidate
            .state()
            .pending
            .as_ref()
            .map(|pending| pending.id)
            .unwrap_or(0);
        let assets = self.state_assets(&candidate);
        self.begin_prepare(Purpose::Replay, activation, assets)
    }
    /// A replay entry that cannot even start (admission, catalog) dies whole:
    /// the live session and its page never noticed it, and its admission
    /// retry has no candidate left to commit.
    fn abort_replay_entry(&mut self, mut error: Diagnostic) -> Diagnostic {
        self.failed_admission = None;
        self.pauses.remove("prepare");
        self.discard_replay_entry();
        if let Some(details) = error.details.as_mut() {
            details.recovery.retain(|r| !matches!(r, Recovery::Retry));
        }
        error
    }
    /// The replay entry content arrived; run the candidate to its next
    /// barrier and start its media preparation once nothing is missing.
    pub(super) fn continue_replay_entry(&mut self) -> Result<()> {
        if !self.replay_entering() {
            return Ok(());
        }
        let Some(mut candidate) = self.candidate.take() else {
            self.replay_work = None;
            return Ok(());
        };
        let output = candidate.step(CoreInput::None, REPLAY_ENTRY_BUDGET);
        self.candidate = Some(candidate);
        let needs = match self.replay_entry_needs(&output.intents) {
            Ok(needs) => needs,
            Err(error) => return Err(self.abort_replay_entry(error)),
        };
        let started = if needs.is_empty() {
            self.begin_replay_media()
        } else {
            self.begin_content(ContentPurpose::ReplayEntry, needs)
        };
        if let Err(error) = started {
            return Err(self.abort_replay_entry(error));
        }
        Ok(())
    }
    /// A cancelled or superseded entering transaction discards the frozen
    /// state and the candidate together: the live session never noticed it.
    pub(super) fn discard_replay_entry(&mut self) {
        if self.replay_entering() {
            self.replay_work = None;
            self.candidate = None;
        }
    }
    /// Live-session swap after the replay candidate prepared: one session
    /// bump resets all audio, checkpoints start over, and the frozen menu
    /// surfaces are closed until the return transaction restores them.
    pub(super) fn commit_replay_enter(&mut self, activation: u32, budget: &mut u32) -> Result<()> {
        let candidate = self
            .candidate
            .take()
            .ok_or_else(|| Diagnostic::new("E_REPLAY", "commit", "no candidate"))?;
        if let Some(work) = &mut self.replay_work {
            work.phase = ReplayPhase::Active;
        }
        self.generation.session += 1;
        self.set_interface_hidden(false);
        self.held_skip = false;
        self.failed_admission = None;
        self.reset_audio();
        self.auto = false;
        self.skip = false;
        self.checkpoints.clear();
        self.menu_peek = false;
        self.slot_restore = false;
        self.core = candidate;
        self.screen = Screen::Story;
        self.return_screen = Screen::Story;
        self.pauses.retain(|r| r == "hidden");
        self.ui_pauses.retain(|r| r == "hidden");
        self.cancel_menu_preparation();
        self.error = None;
        self.diagnostic = None;
        self.status.clear();
        let input = if self.core.state().pending.is_some() {
            CoreInput::Prepared { activation }
        } else {
            CoreInput::None
        };
        self.step(input, budget)
    }
    /// Begin the return: the frozen session revalidates and prepares as a
    /// restore candidate while the replay stays live and recoverable.
    pub(super) fn begin_replay_return(&mut self) -> Result<()> {
        let returning = self
            .replay_work
            .as_mut()
            .is_some_and(|work| work.phase == ReplayPhase::Active);
        if !returning {
            return Ok(());
        }
        let snapshot = self.replay_work.as_ref().unwrap().snapshot.clone();
        if let Some(work) = &mut self.replay_work {
            work.phase = ReplayPhase::Returning;
        }
        self.restore(snapshot)
    }
    /// Return commit: the frozen session, its checkpoints, menu page and
    /// navigation locals come back under a fresh session and menu instance.
    pub(super) fn commit_replay_return(&mut self, candidate: Core) -> Result<()> {
        let Some(work) = self.replay_work.take() else {
            return Err(Diagnostic::new("E_REPLAY", "commit", "no frozen session"));
        };
        let mut candidate = candidate;
        candidate.set_locale(&self.effective_text_locale)?;
        self.generation.session += 1;
        self.set_interface_hidden(false);
        self.held_skip = false;
        self.failed_admission = None;
        self.reset_audio();
        self.slot_restore = false;
        self.core = candidate;
        self.restore_work = None;
        self.touch_snapshot_content(self.core.state())?;
        self.screen = work.screen;
        self.return_screen = work.return_screen;
        self.pauses.retain(|r| r == "hidden");
        if work.screen == Screen::Menu {
            self.pauses.insert("menu".into());
        }
        self.checkpoints = work.checkpoints;
        self.auto = work.auto;
        self.skip = work.skip;
        self.held_skip = false;
        self.image_menu = work.image_menu;
        self.overlay_menu = work.overlay_menu;
        self.menu_session
            .unfreeze(work.menu, self.generation.session)?;
        self.prepared_menu = None;
        self.auto_elapsed = 0;
        self.auto_wait_delay = None;
        self.restart_audio();
        Ok(())
    }
}
#[cfg(test)]
mod replay_tests {
    use super::*;

    const LIMIT: u64 = crate::MEMORY_LEDGER_LIMIT;

    /// rain.json plus a locked system-overlay Replay control and a one-line
    /// replay function that merges a profile key and ends with an outcome.
    fn program() -> Program {
        let mut program: Program =
            serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        program.requires.push("ui.replay.v1".into());
        program.requires.push("ui.menu-services.v1".into());
        program.functions.insert(
            "replay".into(),
            serde_json::from_value(serde_json::json!({
                "entry": "start",
                "blocks": {
                    "start": {"ops": [], "terminator": {"type": "activate", "cue": "arrival", "next": "wait"}},
                    "wait": {"ops": [], "terminator": {
                        "type": "await",
                        "conditions": [{"task": "line", "milestone": {"type": "finished"}}],
                        "next": "mark", "on_cancelled": "mark", "on_failed": "mark"
                    }},
                    "mark": {"ops": [{"id": "seen.once", "operation": {"type": "profile_merge", "key": "replay-seen"}}],
                             "terminator": {"type": "end", "outcome": "replay-done"}}
                }
            }))
            .unwrap(),
        );
        let button = |id: &str, action: nir_format::ImageMenuAction, requires: Option<String>| {
            nir_format::ImageButton {
                id: id.into(),
                label: id.into(),
                asset: "bg.station".into(),
                hover_asset: Some("bg.river".into()),
                locked_asset: None,
                rect: [100., 100., 200., 60.],
                action,
                requires,
            }
        };
        program.theme.image_menus.insert(
            "system".into(),
            nir_format::ImageMenu {
                builtin_navigation: true,
                story_exports: BTreeMap::new(),
                locals: BTreeMap::new(),
                elements: vec![],
                background: "bg.river".into(),
                buttons: vec![
                    button(
                        "replay",
                        nir_format::ImageMenuAction::Replay {
                            function: "replay".into(),
                        },
                        Some("seen".into()),
                    ),
                    button("exit", nir_format::ImageMenuAction::ExitReplay, None),
                ],
                effects: None,
            },
        );
        program.theme.menu_overlay = Some("system".into());
        program
    }

    fn boot() -> Player {
        let mut p = Player::new(program(), "release".into(), "Test".into()).unwrap();
        let c = p.pump(vec![], 1000);
        settle(&mut p, c);
        p
    }

    fn action(p: &mut Player, a: UiAction) -> Vec<AppCommand> {
        p.pump(
            vec![AppEvent::Action {
                action: a,
                interaction: p.current_interaction(),
                sequence: p.core.state().last_input + 1,
                session: p.generation.session,
            }],
            1000,
        )
    }

    /// Menu control click with the page's current authority stamp. The host
    /// stamps the last rendered authority, so settle one turn first: profile
    /// or page-entry deltas mint their revision before the click lands.
    fn menu_click(p: &mut Player, control: &str) -> Vec<AppCommand> {
        p.pump(vec![], 1000);
        let event = AppEvent::Action {
            action: UiAction::MenuControl {
                instance: p.menu_session.instance,
                revision: p.menu_session.revision,
                control: control.into(),
            },
            interaction: 0,
            sequence: p.core.state().last_input + 1,
            session: p.generation.session,
        };
        p.pump(vec![event], 1000)
    }

    /// Drives host asset/presentation acknowledgements to convergence.
    fn settle(p: &mut Player, commands: Vec<AppCommand>) -> Vec<AppCommand> {
        let mut q = commands;
        let mut other = vec![];
        for _ in 0..30 {
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
                            next.extend(
                                p.pump(vec![AppEvent::AssetReady { request, asset }], 1000),
                            );
                        }
                    }
                    AppCommand::PreparePresentation { request } => {
                        next.extend(p.pump(vec![AppEvent::PresentationReady { request }], 1000))
                    }
                    _ => other.push(c),
                }
            }
            q = next;
        }
        panic!("prepare did not converge");
    }

    /// A story waiting at the intro dialogue with the system overlay open
    /// and settled.
    fn story_overlay() -> Player {
        let mut p = boot();
        let c = action(&mut p, UiAction::NewGame);
        settle(&mut p, c);
        p.pump(vec![AppEvent::Tick { delta_us: 200_000 }], 1000);
        let c = action(&mut p, UiAction::Menu);
        settle(&mut p, c);
        drain(&mut p);
        // One empty turn lets the page-entry sync mint its fresh authority.
        p.pump(vec![], 1000);
        p
    }

    /// Unlocks the replay control, clicks it and drives the replay live.
    fn enter_replay(p: &mut Player) {
        p.profile.insert("seen".into());
        let c = menu_click(p, "replay");
        assert_eq!(p.replay_phase(), "entering");
        settle(p, c);
        assert_eq!(p.replay_phase(), "active", "replay did not go live");
        assert_eq!(p.screen, Screen::Story);
        assert_eq!(p.core.state().frames[0].function, "replay");
    }

    /// Drives pending preparation commands to quiescence.
    fn drain(p: &mut Player) {
        for _ in 0..4 {
            let cmds = std::mem::take(&mut p.commands);
            if cmds.is_empty() {
                break;
            }
            settle(p, cmds);
        }
    }

    #[test]
    fn locked_replay_control_rejects_until_profile_unlocks() {
        let mut p = story_overlay();
        let before = p.generation.session;
        menu_click(&mut p, "replay");
        assert_eq!(p.replay_phase(), "inactive");
        assert_eq!(p.screen, Screen::Menu);
        assert_eq!(p.generation.session, before);
        assert!(p.replay_work.is_none());
    }

    #[test]
    fn replay_freezes_session_and_returns_after_outcome() {
        let mut p = story_overlay();
        let frozen_location = p.core.location().to_string();
        let frozen_checkpoints = p.checkpoints.len();
        let frozen_instance = p.menu_session.instance;
        let frozen_session = p.generation.session;

        // Double delivery of the same click: only the first resolves. The
        // profile unlock mints a fresh revision before the page is stamped.
        p.profile.insert("seen".into());
        p.pump(vec![], 1000);
        let frozen_revision = p.menu_session.revision;
        let click = AppEvent::Action {
            action: UiAction::MenuControl {
                instance: p.menu_session.instance,
                revision: p.menu_session.revision,
                control: "replay".into(),
            },
            interaction: 0,
            sequence: 90,
            session: p.generation.session,
        };
        let commands = p.pump(vec![click.clone(), click], 1000);
        assert_eq!(p.replay_phase(), "entering");
        // The frozen page owns the screen while the candidate prepares.
        assert_eq!(p.screen, Screen::Menu);
        assert_eq!(p.generation.session, frozen_session);
        settle(&mut p, commands);
        assert_eq!(p.replay_phase(), "active");
        assert_eq!(p.screen, Screen::Story);
        assert!(p.generation.session > frozen_session);
        assert_eq!(p.core.state().frames[0].function, "replay");
        // The replay's checkpoint ledger starts over; the frozen one waits.
        assert!(p.checkpoints.len() <= 1);

        p.pump(
            vec![AppEvent::Tick {
                delta_us: 2_000_000,
            }],
            1000,
        );
        // The outcome lands inside this pump and starts the return prepare;
        // the phase is observable as "returning" only before it commits.
        let c = action(&mut p, UiAction::Advance);
        assert_eq!(p.replay_phase(), "returning");
        // Manual exit during the return changes nothing: it is idempotent.
        action(&mut p, UiAction::ExitReplay);
        assert_eq!(p.replay_phase(), "returning");
        settle(&mut p, c);

        // The replayed line's profile merge never escapes the replay.
        assert!(!p.profile.contains("replay-seen"));
        assert!(p
            .commands
            .iter()
            .all(|c| !matches!(c, AppCommand::PersistProfile { .. })));
        drain(&mut p);
        assert_eq!(p.replay_phase(), "inactive");
        assert!(p.replay_work.is_none());
        assert!(p.generation.session > frozen_session + 1);
        // The frozen session resumed exactly where it froze.
        assert_eq!(p.core.location(), frozen_location);
        assert_eq!(p.checkpoints.len(), frozen_checkpoints);
        assert_eq!(p.screen, Screen::Menu);
        assert!(p.menu_session.instance > frozen_instance);
        // Pre-freeze menu authority is stale after the return: the exact
        // click that launched the replay cannot relaunch it.
        p.pump(
            vec![AppEvent::Action {
                action: UiAction::MenuControl {
                    instance: frozen_instance,
                    revision: frozen_revision,
                    control: "replay".into(),
                },
                interaction: 0,
                sequence: 91,
                session: p.generation.session,
            }],
            1000,
        );
        assert_eq!(p.replay_phase(), "inactive");
        assert!(p.prepare.is_none());
    }

    #[test]
    fn manual_exit_replay_returns_the_frozen_session() {
        let mut p = story_overlay();
        let frozen_location = p.core.location().to_string();
        enter_replay(&mut p);
        let replay_session = p.generation.session;
        let c = action(&mut p, UiAction::ExitReplay);
        assert_eq!(p.replay_phase(), "returning");
        settle(&mut p, c);
        drain(&mut p);
        assert_eq!(p.replay_phase(), "inactive");
        assert!(p.generation.session > replay_session);
        assert_eq!(p.core.location(), frozen_location);
        assert_eq!(p.screen, Screen::Menu);
    }

    #[test]
    fn exit_replay_without_a_live_replay_is_rejected() {
        let mut p = story_overlay();
        action(&mut p, UiAction::ExitReplay);
        assert_eq!(p.replay_phase(), "inactive");
        assert!(p.prepare.is_none());
        assert!(p.replay_work.is_none());
        // The overlay control form is rejected at dispatch as well.
        menu_click(&mut p, "exit");
        assert_eq!(p.replay_phase(), "inactive");
        assert!(p.prepare.is_none());
    }

    #[test]
    fn nested_replay_and_storage_die_at_the_runtime_recheck() {
        let mut p = story_overlay();
        enter_replay(&mut p);
        let session = p.generation.session;

        // Reopen the overlay inside the replay: its page prepares as a
        // normal menu page, never as another replay candidate.
        let c = action(&mut p, UiAction::Menu);
        settle(&mut p, c);
        drain(&mut p);
        assert_eq!(p.screen, Screen::Menu);
        menu_click(&mut p, "replay");
        assert_eq!(p.replay_phase(), "active");
        assert!(p
            .prepare
            .as_ref()
            .is_none_or(|p| !matches!(p.purpose, Purpose::Replay)));

        // Nested saves and loads die at dispatch even from the live replay's
        // own overlay, exactly as a stale projection would have offered them.
        let commands = action(&mut p, UiAction::Save { slot: 0 });
        assert!(commands
            .iter()
            .all(|c| !matches!(c, AppCommand::Save { .. })));
        let commands = action(&mut p, UiAction::Load { slot: 0 });
        assert!(commands
            .iter()
            .all(|c| !matches!(c, AppCommand::Load { .. })));
        assert!(p.slot_load.is_none());
        let commands = action(&mut p, UiAction::Export);
        assert!(commands
            .iter()
            .all(|c| !matches!(c, AppCommand::Export { .. })));
        let commands = action(&mut p, UiAction::Import);
        assert!(commands.iter().all(|c| !matches!(c, AppCommand::Import)));
        assert_eq!(p.generation.session, session);

        // The exit works through the overlay while live, and the return
        // still lands on the frozen session.
        let c = menu_click(&mut p, "exit");
        assert_eq!(p.replay_phase(), "returning");
        settle(&mut p, c);
        drain(&mut p);
        assert_eq!(p.replay_phase(), "inactive");
        assert_eq!(p.screen, Screen::Menu);
    }

    #[test]
    fn failed_replay_resource_keeps_the_frozen_page_and_recovers() {
        let mut p = story_overlay();
        let frozen_session = p.generation.session;
        p.profile.insert("seen".into());
        let commands = menu_click(&mut p, "replay");
        assert_eq!(p.replay_phase(), "entering");
        let request = commands
            .iter()
            .find_map(|c| match c {
                AppCommand::GetAssets { request, .. } => Some(*request),
                _ => None,
            })
            .expect("replay prepare fetches assets");
        // The candidate's fetch fails: the frozen page and session survive.
        p.pump(
            vec![AppEvent::AssetFailed {
                request,
                message: "decode failed".into(),
            }],
            1000,
        );
        assert_eq!(p.replay_phase(), "entering");
        assert_eq!(p.screen, Screen::Menu);
        assert_eq!(p.generation.session, frozen_session);
        assert!(p.error.is_some());
        assert!(p.prepare.as_ref().is_some_and(|p| p.failed));
        // Retry restarts the candidate's own preparation.
        let c = action(&mut p, UiAction::Retry);
        settle(&mut p, c);
        assert_eq!(p.replay_phase(), "active");
        assert_eq!(p.screen, Screen::Story);
        assert_eq!(p.core.state().frames[0].function, "replay");
    }

    #[test]
    fn replay_admission_failure_drops_the_whole_transaction() {
        let mut p = story_overlay();
        let frozen_session = p.generation.session;
        // The arrival cue's voice audio is absent from the frozen context, so
        // joint admission needs real headroom: leaving zero bytes must fail.
        let fill = LIMIT - p.memory_used();
        let pressure = p
            .ledger
            .reserve(&BTreeMap::from([("@test-pressure".into(), fill)]))
            .unwrap();
        p.profile.insert("seen".into());
        menu_click(&mut p, "replay");
        assert_eq!(p.replay_phase(), "inactive");
        assert!(p.replay_work.is_none());
        assert!(p.candidate.is_none());
        assert!(p.failed_admission.is_none());
        assert!(!p.is_loading());
        assert_eq!(p.screen, Screen::Menu);
        assert_eq!(p.generation.session, frozen_session);
        assert!(p.error.is_some());
        // The fault offers no retry: the candidate no longer exists.
        let diagnostic = p.diagnostic.as_ref().unwrap();
        let details = diagnostic.details.as_ref().unwrap();
        assert!(!details.recovery.contains(&Recovery::Retry));
        // A later attempt, with budget again, works from scratch.
        drop(pressure);
        let c = action(&mut p, UiAction::Retry);
        settle(&mut p, c);
        assert_eq!(p.replay_phase(), "inactive");
        // Retry cannot resurrect the dropped transaction; a fresh click can.
        let c = menu_click(&mut p, "replay");
        settle(&mut p, c);
        assert_eq!(p.replay_phase(), "active");
    }

    #[test]
    fn title_and_newgame_abandon_a_live_replay() {
        let mut p = story_overlay();
        enter_replay(&mut p);
        let c = action(&mut p, UiAction::Title);
        settle(&mut p, c);
        assert_eq!(p.replay_phase(), "inactive");
        assert!(p.replay_work.is_none());
        assert_eq!(p.screen, Screen::Title);

        let mut p = story_overlay();
        enter_replay(&mut p);
        let c = action(&mut p, UiAction::NewGame);
        settle(&mut p, c);
        assert_eq!(p.replay_phase(), "inactive");
        assert!(p.replay_work.is_none());
        assert_eq!(p.screen, Screen::Story);
        assert_eq!(p.core.state().frames[0].function, "main");
    }

    #[test]
    fn device_loss_during_entry_resumes_the_replay_candidate() {
        let mut p = story_overlay();
        p.profile.insert("seen".into());
        menu_click(&mut p, "replay");
        assert_eq!(p.replay_phase(), "entering");
        p.pump(vec![AppEvent::DeviceLost], 1000);
        assert_eq!(p.replay_phase(), "entering");
        let commands = p.pump(vec![AppEvent::DeviceReady], 1000);
        settle(&mut p, commands);
        assert_eq!(p.replay_phase(), "active");
        assert_eq!(p.core.state().frames[0].function, "replay");
    }

    #[test]
    fn old_entry_from_title_still_replaces_the_session() {
        let mut p = boot();
        p.profile.insert("seen".into());
        let c = action(&mut p, UiAction::NewGame);
        settle(&mut p, c);
        assert_eq!(p.screen, Screen::Story);
        assert_eq!(p.core.state().frames[0].function, "main");
        assert_eq!(p.replay_phase(), "inactive");
    }
}
