use std::time::{Duration, Instant};

pub(crate) enum Signal {
    Focused(bool),
    Occluded(bool),
    Suspended(bool),
    Closing(bool),
}

/// Native window and application visibility have independent owners. A
/// focus/occlusion event cannot release an application suspension.
pub(crate) struct Lifecycle {
    focused: bool,
    occluded: bool,
    suspended: bool,
    closing: bool,
    previous: Instant,
}

impl Lifecycle {
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            focused: true,
            occluded: false,
            suspended: false,
            closing: false,
            previous: now,
        }
    }

    pub(crate) fn signal(&mut self, signal: Signal, now: Instant) {
        match signal {
            Signal::Focused(value) => self.focused = value,
            Signal::Occluded(value) => self.occluded = value,
            Signal::Suspended(value) => self.suspended = value,
            Signal::Closing(value) => self.closing = value,
        }
        // Reset even when another owner still holds the pause. Neither
        // background time nor surface-rebinding time belongs to the story.
        self.previous = now;
    }

    pub(crate) fn hidden(&self) -> bool {
        self.suspended || self.closing || !self.focused || self.occluded
    }

    pub(crate) fn audio_paused(&self, domain_paused: bool, bus_paused: bool) -> bool {
        // A queued unpause/start from before the lifecycle event cannot
        // restart a sink while the native host itself is still hidden.
        self.hidden() || domain_paused || bus_paused
    }

    pub(crate) fn elapsed(&mut self, now: Instant, requested: bool) -> Option<u32> {
        if !self.hidden() && requested {
            let elapsed = now
                .duration_since(self.previous)
                .as_micros()
                .min(u32::MAX as u128) as u32;
            self.previous += Duration::from_micros(elapsed as u64);
            Some(elapsed)
        } else {
            self.previous = now;
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nir_format::{TimeDomain, UiAction};
    use nir_player::{AppCommand, AppEvent, Player};

    #[test]
    fn suspend_without_focus_events_freezes_clock_and_both_audio_domains() {
        let start = Instant::now();
        let mut host = Lifecycle::new(start);
        let mut player = playing();
        let session = player.generation.session;
        let location = player.core().location();
        let tick = player.core().state().tick_us;
        let history = player.core().state().history.len();
        let music = player.core().state().handles["music"];

        host.signal(Signal::Suspended(true), start);
        let commands = player.pump(vec![AppEvent::Hidden(host.hidden())], 1000);
        for domain in [TimeDomain::Story, TimeDomain::ForegroundUi] {
            assert!(
                player.domain_paused(domain),
                "suspend must not depend on focus loss"
            );
            assert!(commands.iter().any(|command| matches!(command,
                AppCommand::AudioPause { domain: d, paused: true } if *d == domain)));
        }
        assert_eq!(host.elapsed(start + Duration::from_secs(300), true), None);
        // Even a late host tick cannot make Player consume the background wait.
        player.pump(
            vec![AppEvent::Tick {
                delta_us: 300_000_000,
            }],
            1000,
        );
        assert_eq!(player.core().state().tick_us, tick);
        assert_eq!(player.core().location(), location);
        assert_eq!(player.core().state().history.len(), history);

        host.signal(Signal::Suspended(false), start + Duration::from_secs(301));
        let resumed = player.pump(vec![AppEvent::Hidden(host.hidden())], 1000);
        assert!(!player.paused());
        assert_eq!(
            host.elapsed(
                start + Duration::from_secs(301) + Duration::from_millis(8),
                true
            ),
            Some(8000)
        );
        assert_eq!(player.generation.session, session);
        assert_eq!(player.core().state().handles["music"], music);
        assert!(commands
            .iter()
            .chain(resumed.iter())
            .all(|command| !matches!(
                command,
                AppCommand::AudioStart { .. }
                    | AppCommand::AudioStop { .. }
                    | AppCommand::AudioReset { .. }
            )));
    }

    #[test]
    fn window_events_cannot_release_suspension_and_resume_preserves_other_owners() {
        let now = Instant::now();
        for (focus, occluded) in [(false, false), (true, true), (false, true), (true, false)] {
            let mut host = Lifecycle::new(now);
            host.signal(Signal::Suspended(true), now);
            host.signal(Signal::Focused(focus), now);
            host.signal(Signal::Occluded(occluded), now);
            assert!(host.hidden());
            host.signal(Signal::Suspended(false), now);
            assert_eq!(host.hidden(), !focus || occluded);
            host.signal(Signal::Focused(true), now);
            assert_eq!(host.hidden(), occluded);
            host.signal(Signal::Occluded(false), now);
            assert!(!host.hidden());
        }
    }

    #[test]
    fn menu_owner_survives_background_resume_without_restarting_music() {
        let now = Instant::now();
        let mut host = Lifecycle::new(now);
        let mut player = playing();
        let music = player.core().state().handles["music"];
        action(&mut player, UiAction::Menu);
        assert!(player.paused());
        assert!(!player.domain_paused(TimeDomain::Story));
        for suspended in [true, true, false, false] {
            host.signal(Signal::Suspended(suspended), now);
            let commands = player.pump(vec![AppEvent::Hidden(host.hidden())], 1000);
            assert!(player.paused(), "menu must retain its own pause");
            assert_eq!(player.domain_paused(TimeDomain::Story), suspended);
            assert_eq!(player.core().state().handles["music"], music);
            assert!(commands.iter().all(|command| !matches!(
                command,
                AppCommand::AudioStart { .. }
                    | AppCommand::AudioStop { .. }
                    | AppCommand::AudioReset { .. }
            )));
        }
    }

    #[test]
    fn clock_keeps_fractional_time_and_discards_inactive_time() {
        let now = Instant::now();
        let mut host = Lifecycle::new(now);
        assert_eq!(
            host.elapsed(now + Duration::from_nanos(1500), true),
            Some(1)
        );
        assert_eq!(
            host.elapsed(now + Duration::from_nanos(2100), true),
            Some(1)
        );
        assert_eq!(host.elapsed(now + Duration::from_secs(120), false), None);
        assert_eq!(
            host.elapsed(
                now + Duration::from_secs(120) + Duration::from_millis(8),
                true
            ),
            Some(8000)
        );
    }

    #[test]
    fn queued_audio_policy_cannot_override_host_visibility_or_bus_pause() {
        let now = Instant::now();
        for signal in [
            Signal::Suspended(true),
            Signal::Closing(true),
            Signal::Focused(false),
            Signal::Occluded(true),
        ] {
            let mut host = Lifecycle::new(now);
            host.signal(signal, now);
            assert!(host.audio_paused(false, false));
        }
        let host = Lifecycle::new(now);
        for (domain, bus) in [(false, false), (true, false), (false, true), (true, true)] {
            assert_eq!(host.audio_paused(domain, bus), domain || bus);
        }
    }

    #[test]
    fn cancelling_close_releases_only_its_pause_and_discards_the_storage_wait() {
        let now = Instant::now();
        let mut host = Lifecycle::new(now);
        let mut player = playing();
        let session = player.generation.session;
        let location = player.core().location();
        let music = player.core().state().handles["music"];
        let tick = player.core().state().tick_us;
        host.signal(Signal::Closing(true), now);
        let stopped = player.pump(vec![AppEvent::Hidden(host.hidden())], 1000);
        host.signal(Signal::Focused(true), now + Duration::from_secs(30));
        assert!(host.hidden(), "focus cannot bypass pending storage");
        assert_eq!(host.elapsed(now + Duration::from_secs(60), true), None);
        host.signal(Signal::Closing(false), now + Duration::from_secs(61));
        let resumed = player.pump(vec![AppEvent::Hidden(host.hidden())], 1000);
        assert!(!player.paused());
        assert_eq!(player.core().location(), location);
        assert_eq!(player.core().state().tick_us, tick);
        assert_eq!(player.generation.session, session);
        assert_eq!(player.core().state().handles["music"], music);
        assert_eq!(
            host.elapsed(
                now + Duration::from_secs(61) + Duration::from_millis(8),
                true
            ),
            Some(8000)
        );
        assert!(stopped
            .iter()
            .chain(resumed.iter())
            .all(|command| !matches!(
                command,
                AppCommand::AudioStart { .. }
                    | AppCommand::AudioStop { .. }
                    | AppCommand::AudioReset { .. }
            )));

        for other in [
            Signal::Suspended(true),
            Signal::Focused(false),
            Signal::Occluded(true),
        ] {
            let mut host = Lifecycle::new(now);
            host.signal(other, now);
            host.signal(Signal::Closing(true), now);
            host.signal(Signal::Closing(false), now);
            assert!(host.hidden(), "cancelling close must preserve other owners");
        }
    }

    #[test]
    fn failed_save_during_close_keeps_the_menu_failure_and_allows_another_save() {
        use crate::close::{Close, CloseAction};

        let now = Instant::now();
        let mut host = Lifecycle::new(now);
        let mut close = Close::default();
        let mut player = playing();
        let commands = action(&mut player, UiAction::Saves);
        settle(&mut player, commands);
        let session = player.generation.session;
        let location = player.core().location();
        let tick = player.core().state().tick_us;
        let music = player.core().state().handles["music"];
        let job = action(&mut player, UiAction::Save { slot: 0 })
            .into_iter()
            .find_map(|command| match command {
                AppCommand::Save { job, .. } => Some(job),
                _ => None,
            })
            .unwrap();
        close.request();
        host.signal(Signal::Closing(true), now);
        player.pump(vec![AppEvent::Hidden(host.hidden())], 1000);
        player.pump(
            vec![AppEvent::SaveFailed {
                job,
                message: "controlled close-time save failure".into(),
            }],
            1000,
        );
        let failure = player.status.clone();
        assert!(!failure.is_empty());
        assert_eq!(player.diagnostic.as_ref().unwrap().code, "E_STORAGE");
        close.save_failed();
        assert_eq!(close.poll(0, false), CloseAction::Cancel);
        host.signal(Signal::Closing(false), now + Duration::from_secs(30));
        let resumed = player.pump(vec![AppEvent::Hidden(host.hidden())], 1000);
        assert_eq!(player.status, failure);
        assert_eq!(player.screen, nir_presentation::Screen::Saves);
        assert!(player.paused(), "the save menu retains its own pause");
        assert!(!player.domain_paused(TimeDomain::Story));
        assert_eq!(player.generation.session, session);
        assert_eq!(player.core().location(), location);
        assert_eq!(player.core().state().tick_us, tick);
        assert_eq!(player.core().state().handles["music"], music);
        assert!(resumed.iter().all(|command| !matches!(
            command,
            AppCommand::AudioStart { .. }
                | AppCommand::AudioStop { .. }
                | AppCommand::AudioReset { .. }
        )));
        assert!(action(&mut player, UiAction::Save { slot: 0 })
            .into_iter()
            .any(|command| matches!(command, AppCommand::Save { job: next, .. } if next != job)));
    }

    #[test]
    fn focus_gain_between_surface_destroy_and_resume_is_retained() {
        let now = Instant::now();
        let mut host = Lifecycle::new(now);
        let mut player = playing();
        let session = player.generation.session;
        let tick = player.core().state().tick_us;
        let music = player.core().state().handles["music"];
        host.signal(Signal::Focused(false), now);
        player.pump(vec![AppEvent::Hidden(host.hidden())], 1000);
        host.signal(Signal::Suspended(true), now);
        player.pump(vec![AppEvent::Hidden(host.hidden())], 1000);
        // A window-less host only records the signal; it does not admit
        // callbacks, draw or release the Player pause while suspended.
        host.signal(Signal::Focused(true), now + Duration::from_secs(30));
        assert!(host.hidden());
        assert!(player.domain_paused(TimeDomain::Story));
        host.signal(Signal::Suspended(false), now + Duration::from_secs(31));
        let commands = player.pump(vec![AppEvent::Hidden(host.hidden())], 1000);
        assert!(!player.paused());
        assert_eq!(player.generation.session, session);
        assert_eq!(player.core().state().tick_us, tick);
        assert_eq!(player.core().state().handles["music"], music);
        assert!(commands.iter().all(|command| !matches!(
            command,
            AppCommand::AudioStart { .. }
                | AppCommand::AudioStop { .. }
                | AppCommand::AudioReset { .. }
        )));
    }

    #[test]
    fn restored_session_still_requires_continue_after_suspend_and_resume() {
        let now = Instant::now();
        let mut host = Lifecycle::new(now);
        let mut player = playing();
        let envelope = action(&mut player, UiAction::Save { slot: 0 })
            .into_iter()
            .find_map(|command| match command {
                AppCommand::Save { envelope, .. } => Some(envelope),
                _ => None,
            })
            .unwrap();
        let commands = player.pump(vec![AppEvent::Loaded { envelope }], 1000);
        settle(&mut player, commands);
        assert!(player.paused());
        let session = player.generation.session;
        let tick = player.core().state().tick_us;
        for suspended in [true, false] {
            host.signal(Signal::Suspended(suspended), now);
            let commands = player.pump(vec![AppEvent::Hidden(host.hidden())], 1000);
            assert!(
                player.paused(),
                "only Continue can release restored-session ownership"
            );
            assert_eq!(player.generation.session, session);
            assert_eq!(player.core().state().tick_us, tick);
            assert!(commands.iter().all(|command| !matches!(
                command,
                AppCommand::AudioStart { .. }
                    | AppCommand::AudioStop { .. }
                    | AppCommand::AudioReset { .. }
            )));
        }
        action(&mut player, UiAction::Continue);
        assert!(!player.paused());
    }

    fn action(player: &mut Player, action: UiAction) -> Vec<AppCommand> {
        player.pump(
            vec![AppEvent::Action {
                action,
                interaction: player.current_interaction(),
                sequence: player.core().state().last_input + 1,
                session: player.generation.session,
            }],
            1000,
        )
    }

    fn settle(player: &mut Player, mut commands: Vec<AppCommand>) {
        for _ in 0..30 {
            if commands.is_empty() {
                return;
            }
            let mut next = vec![];
            for command in commands {
                match command {
                    AppCommand::GetAssets {
                        request, assets, ..
                    } => {
                        for asset in assets {
                            next.extend(
                                player.pump(vec![AppEvent::AssetReady { request, asset }], 1000),
                            );
                        }
                    }
                    AppCommand::PreparePresentation { request } => next
                        .extend(player.pump(vec![AppEvent::PresentationReady { request }], 1000)),
                    AppCommand::PrepareLocale { request, .. } => {
                        next.extend(player.pump(vec![AppEvent::LocaleReady { request }], 1000))
                    }
                    _ => {}
                }
            }
            commands = next;
        }
        panic!("preparation did not converge");
    }

    #[test]
    fn output_recovery_retains_menu_pause_music_and_never_catches_up_wait_time() {
        use crate::audio_output_state::OutputState;
        use nir_format::AudioBus;
        let mut player = playing();
        let session = player.generation.session;
        let music = player.core().state().handles["music"];
        let tick = player.core().state().tick_us;
        action(&mut player, UiAction::Menu);
        let output_wait = player.acquire_audio_output_wait();
        let mut output = OutputState::default();
        let generation = output.request().unwrap();
        let commands = player.pump(
            vec![AppEvent::Tick {
                delta_us: 30_000_000,
            }],
            1000,
        );
        assert!(player.paused());
        assert_eq!(player.core().state().tick_us, tick);
        assert!(commands.iter().all(|c| !matches!(
            c,
            AppCommand::AudioStart { .. }
                | AppCommand::AudioStop { .. }
                | AppCommand::AudioReset { .. }
        )));
        assert!(output.ready(generation));
        drop(output_wait);
        let commands = player.pump(vec![], 1000);
        assert!(player.paused(), "output readiness must not close the menu");
        assert_eq!(player.generation.session, session);
        assert_eq!(player.core().state().handles["music"], music);
        assert!(!player.bus_paused(TimeDomain::Story, AudioBus::Bgm));
        assert!(player.bus_paused(TimeDomain::Story, AudioBus::Voice));
        assert!(commands.iter().all(|c| !matches!(
            c,
            AppCommand::AudioStart { .. }
                | AppCommand::AudioStop { .. }
                | AppCommand::AudioReset { .. }
        )));
        action(&mut player, UiAction::Close);
        assert!(!player.paused());
        assert_eq!(player.core().state().tick_us, tick);
    }

    #[test]
    fn output_loss_discards_held_skip_even_without_a_keyup_event() {
        let mut player = playing();
        action(&mut player, UiAction::HoldSkip { pressed: true });
        let wait = player.acquire_audio_output_wait();
        action(&mut player, UiAction::HoldSkip { pressed: false });
        let location = player.core().location();
        let tick = player.core().state().tick_us;
        player.pump(
            vec![AppEvent::Tick {
                delta_us: 30_000_000,
            }],
            1000,
        );
        assert_eq!(player.core().location(), location);
        assert_eq!(player.core().state().tick_us, tick);
        drop(wait);
        player.pump(vec![], 1000);
        assert!(
            !player.model().skip,
            "a missing keyup must not reactivate skip on recovery"
        );
        assert_eq!(player.core().location(), location);
    }

    fn playing() -> Player {
        let program = serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        let mut player = Player::new(program, "release".into(), "Test".into()).unwrap();
        let commands = player.pump(vec![], 1000);
        settle(&mut player, commands);
        let commands = action(&mut player, UiAction::NewGame);
        settle(&mut player, commands);
        player
    }
}
