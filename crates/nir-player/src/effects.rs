//! Player-owned foreground UI effect state for menu pages. Story audio tasks
//! live in `Core`; these voices never enter snapshots and are identified by a
//! private task counter, so they cannot collide with story task ids even
//! without the domain separation the host already enforces.
use nir_format::MenuReadingMode;
use std::collections::BTreeMap;

/// A page fade driven by the foreground clock. Enter fades run 0 -> 1, close
/// fades 1 -> 0; both are finite by validation (fade_us <= 2_000_000).
pub(super) struct PageFade {
    pub closing: bool,
    pub start_us: u64,
    pub duration_us: u64,
}
impl PageFade {
    fn sample(&self, now_us: u64) -> f32 {
        let elapsed = now_us.saturating_sub(self.start_us) as f32;
        let progress = if self.duration_us == 0 {
            1.
        } else {
            (elapsed / self.duration_us as f32).clamp(0., 1.)
        };
        if self.closing {
            1. - progress
        } else {
            progress
        }
    }
    fn finished(&self, now_us: u64) -> bool {
        now_us.saturating_sub(self.start_us) >= self.duration_us
    }
}

/// A foreground-domain voice started by page effects, awaiting its natural
/// end (one-shots) or an explicit stop (looping music).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct UiVoice {
    pub task: u32,
    pub session: u32,
}

/// A menu exit accepted but held until the finite close fade finishes.
pub(super) struct DeferredExit {
    pub kind: DeferredExitKind,
    pub interaction: u32,
    pub sequence: u32,
}
pub(super) enum DeferredExitKind {
    /// The shared Close path: pop exhausted, the screen itself is leaving.
    CloseScreen { cancelled_slot_restore: bool },
    /// A verified reading control that closes the overlay, then toggles.
    ReadingClose { mode: MenuReadingMode },
}

pub(super) struct MenuEffectsState {
    /// Session the voices and fades belong to; hosts reset audio per session.
    pub session: u32,
    /// Page (menu id, instance) whose enter effects already fired. `None`
    /// means no page owns the current voices.
    pub entered: Option<(String, u32)>,
    pub fade: Option<PageFade>,
    pub music: Option<UiVoice>,
    pub sounds: BTreeMap<u32, UiVoice>,
    pub next_task: u32,
    pub closing: Option<DeferredExit>,
}
impl MenuEffectsState {
    pub fn new(session: u32) -> Self {
        Self {
            session,
            entered: None,
            fade: None,
            music: None,
            sounds: BTreeMap::new(),
            next_task: 0,
            closing: None,
        }
    }
    pub fn alloc_task(&mut self) -> u32 {
        self.next_task = self.next_task.saturating_add(1);
        self.next_task
    }
    pub fn opacity(&self, now_us: u64, reduced_motion: bool) -> f32 {
        match &self.fade {
            None => 1.,
            Some(fade) => {
                if reduced_motion {
                    if fade.closing {
                        0.
                    } else {
                        1.
                    }
                } else {
                    fade.sample(now_us)
                }
            }
        }
    }
    /// Takes the deferred exit when its close fade has fully elapsed.
    pub fn take_finished_close(&mut self, now_us: u64) -> Option<DeferredExit> {
        let finished = self
            .fade
            .as_ref()
            .is_some_and(|fade| fade.closing && fade.finished(now_us));
        if finished {
            self.fade = None;
            self.closing.take()
        } else {
            None
        }
    }
    /// Clears a completed enter fade so the page rests at full opacity.
    pub fn settle_enter(&mut self, now_us: u64) -> bool {
        if self
            .fade
            .as_ref()
            .is_some_and(|fade| !fade.closing && fade.finished(now_us))
        {
            self.fade = None;
        }
        self.fade.is_none()
    }
    /// Stops owning the current page without touching pending exits.
    pub fn retire_page(&mut self) {
        self.entered = None;
        self.fade = None;
    }
}
