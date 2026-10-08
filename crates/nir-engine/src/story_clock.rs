//! Bounded observations of the actual VM clock, at its owner. Host snapshots
//! may arrive late; driver time between two snapshots is not a recovery step.
#[derive(Clone, Copy)]
pub(crate) struct Sample {
    pub session: u32,
    pub runnable: bool,
    pub tick_us: u64,
}

#[derive(Default, serde::Serialize)]
pub(crate) struct StoryClock {
    pub pause_revision: u32,
    pub resume_revision: u32,
    pub paused_advance_us: u64,
    pub resume_tick_us: Option<u64>,
    pub first_advance_us: Option<u64>,
    pub first_advance_delay_us: Option<u64>,
    #[serde(skip)]
    previous: Option<Sample>,
    #[serde(skip)]
    resumed_at_us: Option<u64>,
}

impl StoryClock {
    pub fn observe(&mut self, sample: Sample, now: impl FnOnce() -> u64) {
        let previous = self.previous.replace(sample);
        let Some(previous) = previous.filter(|previous| {
            previous.session == sample.session && previous.tick_us <= sample.tick_us
        }) else {
            // A new/restored session is a different clock, never a recovery
            // delta from the old session. Revisions remain monotonic.
            self.paused_advance_us = 0;
            self.resume_tick_us = None;
            self.first_advance_us = None;
            self.first_advance_delay_us = None;
            self.resumed_at_us = None;
            return;
        };
        let advance = sample.tick_us - previous.tick_us;
        if previous.runnable && !sample.runnable {
            self.pause_revision = self.pause_revision.saturating_add(1);
            self.paused_advance_us = 0;
            self.resumed_at_us = None;
        } else if !previous.runnable && !sample.runnable {
            self.paused_advance_us = self.paused_advance_us.saturating_add(advance);
        } else if !previous.runnable && sample.runnable {
            self.resume_revision = self.resume_revision.saturating_add(1);
            self.resume_tick_us = Some(previous.tick_us);
            self.first_advance_us = None;
            self.first_advance_delay_us = None;
            self.resumed_at_us = Some(now());
            if advance > 0 {
                self.first_advance_us = Some(advance);
                self.first_advance_delay_us = Some(0);
                self.resumed_at_us = None;
            }
        } else if let Some(resumed_at) = self.resumed_at_us.filter(|_| advance > 0) {
            self.first_advance_us = Some(advance);
            self.first_advance_delay_us = Some(now().saturating_sub(resumed_at));
            self.resumed_at_us = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(runnable: bool, tick_us: u64) -> Sample {
        Sample {
            session: 1,
            runnable,
            tick_us,
        }
    }

    #[test]
    fn captures_first_positive_step_even_if_host_reads_much_later() {
        let mut clock = StoryClock::default();
        clock.observe(sample(true, 16_000), || 0);
        clock.observe(sample(false, 16_000), || 10_000);
        clock.observe(sample(false, 16_000), || 410_000);
        clock.observe(sample(true, 16_000), || 410_000);
        clock.observe(sample(true, 16_000), || 411_000);
        clock.observe(sample(true, 33_000), || 427_000);
        clock.observe(sample(true, 440_000), || 834_000);
        assert_eq!(clock.pause_revision, 1);
        assert_eq!(clock.resume_revision, 1);
        assert_eq!(clock.paused_advance_us, 0);
        assert_eq!(clock.resume_tick_us, Some(16_000));
        assert_eq!(clock.first_advance_us, Some(17_000));
        assert_eq!(clock.first_advance_delay_us, Some(17_000));
    }

    #[test]
    fn records_catch_up_on_release_and_on_the_following_tick() {
        for advance_on_release in [false, true] {
            let mut clock = StoryClock::default();
            clock.observe(sample(true, 16_000), || 0);
            clock.observe(sample(false, 16_000), || 10_000);
            clock.observe(sample(false, 16_000), || 410_000);
            clock.observe(
                sample(true, if advance_on_release { 432_000 } else { 16_000 }),
                || 410_000,
            );
            clock.observe(sample(true, 432_000), || 426_000);
            assert_eq!(clock.first_advance_us, Some(416_000));
            assert!(clock.first_advance_us.unwrap() >= 250_000);
        }
    }

    #[test]
    fn detects_paused_advances_and_rearms_on_a_second_recovery() {
        let mut clock = StoryClock::default();
        clock.observe(sample(true, 0), || 0);
        clock.observe(sample(false, 0), || 1);
        clock.observe(sample(false, 5), || 2);
        assert_eq!(clock.paused_advance_us, 5);
        clock.observe(sample(true, 5), || 3);
        clock.observe(sample(false, 5), || 4);
        clock.observe(sample(true, 5), || 5);
        clock.observe(sample(true, 21), || 21);
        assert_eq!(clock.paused_advance_us, 0);
        assert_eq!(clock.resume_revision, 2);
        assert_eq!(clock.first_advance_us, Some(16));
    }

    #[test]
    fn session_replacement_does_not_compare_unrelated_clocks() {
        let mut clock = StoryClock::default();
        clock.observe(sample(true, 1_000_000), || 0);
        clock.observe(sample(false, 1_000_000), || 1);
        clock.observe(
            Sample {
                session: 2,
                runnable: true,
                tick_us: 5_000_000,
            },
            || 2,
        );
        clock.observe(
            Sample {
                session: 2,
                runnable: true,
                tick_us: 5_016_000,
            },
            || 3,
        );
        assert_eq!(clock.resume_revision, 0);
        assert_eq!(clock.first_advance_us, None);
        assert_eq!(clock.resume_tick_us, None);
    }
}
