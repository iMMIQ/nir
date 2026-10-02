//! Player-owned foreground UI effect state for menu pages. Story audio tasks
//! live in `Core`; these voices never enter snapshots and are identified by a
//! private task counter, so they cannot collide with story task ids even
//! without the domain separation the host already enforces.
use nir_format::{MenuReadingMode, ScalarTween};
use std::collections::BTreeMap;

/// A page fade driven by the foreground clock. Enter fades run 0 -> 1, close
/// fades 1 -> 0; both are finite by validation (fade_us <= 2_000_000).
pub(super) struct PageFade {
    pub closing: bool,
    pub start_us: u64,
    pub duration_us: u64,
    /// Authored transition style. A spatial style (wipe/mask) diverts the page
    /// into the offscreen page root for the same duration; dissolve or no
    /// style keeps the legacy whole-layer alpha fade.
    pub style: Option<nir_format::StageTransition>,
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
    /// Raw reveal ramp 0 -> 1; the direction is the composite flip, so unlike
    /// `sample` it never inverts for a closing page.
    fn progress(&self, now_us: u64) -> f32 {
        let elapsed = now_us.saturating_sub(self.start_us) as f32;
        if self.duration_us == 0 {
            1.
        } else {
            (elapsed / self.duration_us as f32).clamp(0., 1.)
        }
    }
    fn finished(&self, now_us: u64) -> bool {
        now_us.saturating_sub(self.start_us) >= self.duration_us
    }
    /// Whether this fade diverts the page into the page root. Dissolve is the
    /// legacy fade and needs no reveal machinery.
    fn spatial(&self) -> bool {
        self.style.as_ref().is_some_and(|s| !s.is_default())
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

/// One in-flight element enter animation. The tween interpolates `from` to
/// the element's authored value (zero for offsets), so a finished track is
/// indistinguishable from no track and is dropped rather than settled.
pub(super) struct ElementTrack {
    pub element: String,
    pub property: nir_format::MenuElementProperty,
    pub track: ScalarTween,
    /// The page's enter-boundary instant. The property holds `from` until
    /// `delay_us` has elapsed on the foreground clock.
    pub start_us: u64,
    pub delay_us: u64,
}
impl ElementTrack {
    fn value(&self, now_us: u64) -> f32 {
        let elapsed = now_us.saturating_sub(self.start_us.saturating_add(self.delay_us));
        self.track.sample(nir_format::Micros(elapsed))
    }
    fn finished(&self, now_us: u64) -> bool {
        now_us.saturating_sub(self.start_us.saturating_add(self.delay_us))
            >= self.track.duration_us.0
    }
}

pub(super) struct MenuEffectsState {
    /// Session the voices and fades belong to; hosts reset audio per session.
    pub session: u32,
    /// Page (menu id, instance) whose enter effects already fired. `None`
    /// means no page owns the current voices.
    pub entered: Option<(String, u32)>,
    pub fade: Option<PageFade>,
    /// In-flight element enter animations for the current page.
    pub elements: Vec<ElementTrack>,
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
            elements: Vec::new(),
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
                } else if fade.spatial() {
                    // The page-root reveal owns visibility; folding an alpha
                    // ramp on top would double-fade the revealed page.
                    1.
                } else {
                    fade.sample(now_us)
                }
            }
        }
    }
    /// The in-flight page-root reveal: (style, to_visible, progress). Only
    /// spatial styles divert; dissolve and unstyled fades return `None`.
    pub fn transition(
        &self,
        now_us: u64,
        reduced_motion: bool,
    ) -> Option<(nir_format::StageTransition, bool, f32)> {
        let fade = self.fade.as_ref()?;
        if reduced_motion || !fade.spatial() {
            return None;
        }
        Some((
            fade.style.clone().unwrap(),
            !fade.closing,
            fade.progress(now_us),
        ))
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
    /// Starts the page's element enter animations against its authored
    /// element values. Tracks whose element vanished (validation keeps this
    /// impossible) are skipped, not fatal.
    pub fn start_elements(
        &mut self,
        page: Option<&nir_format::ImageMenu>,
        authored: &[nir_format::MenuElementTween],
        now_us: u64,
    ) {
        self.elements.clear();
        self.elements.extend(authored.iter().filter_map(|t| {
            let base = match t.property {
                nir_format::MenuElementProperty::Opacity => page?
                    .elements
                    .iter()
                    .find(|e| e.id == t.element)?
                    .opacity,
                nir_format::MenuElementProperty::Scale => {
                    page?.elements.iter().find(|e| e.id == t.element)?.scale
                }
                // Offsets are transient displacement; their rest value is zero.
                nir_format::MenuElementProperty::OffsetX
                | nir_format::MenuElementProperty::OffsetY => 0.,
            };
            Some(ElementTrack {
                element: t.element.clone(),
                property: t.property,
                track: ScalarTween {
                    from: t.from,
                    base,
                    to: base,
                    duration_us: t.duration_us,
                    easing: t.easing,
                    finish: nir_format::FinishPolicy::CommitEnd,
                    cancel: nir_format::CancelPolicy::CommitCurrent,
                },
                start_us: now_us,
                delay_us: t.delay_us.0,
            })
        }));
    }
    /// The in-flight element overrides for projection. Absent properties keep
    /// their authored values; offsets are added displacement.
    pub fn element_animations(
        &self,
        now_us: u64,
    ) -> BTreeMap<String, nir_presentation::ElementAnimation> {
        let mut out: BTreeMap<String, nir_presentation::ElementAnimation> = BTreeMap::new();
        for track in &self.elements {
            let entry = out.entry(track.element.clone()).or_default();
            let value = track.value(now_us);
            match track.property {
                nir_format::MenuElementProperty::Opacity => entry.opacity = Some(value),
                nir_format::MenuElementProperty::Scale => entry.scale = Some(value),
                nir_format::MenuElementProperty::OffsetX => entry.offset[0] = value,
                nir_format::MenuElementProperty::OffsetY => entry.offset[1] = value,
            }
        }
        out
    }
    /// Drops finished tracks; returns how many drained this tick.
    pub fn drain_finished_elements(&mut self, now_us: u64) -> usize {
        let before = self.elements.len();
        self.elements.retain(|track| !track.finished(now_us));
        before - self.elements.len()
    }
    /// The furthest-along in-flight track's normalized progress across its
    /// delay and ramp, or `None` when nothing animates. Hosts use it to
    /// report presentation progress the way they do page reveals.
    pub fn element_progress(&self, now_us: u64) -> Option<f32> {
        self.elements
            .iter()
            .map(|track| {
                let total = track.delay_us.saturating_add(track.track.duration_us.0);
                if total == 0 {
                    1.
                } else {
                    now_us.saturating_sub(track.start_us).min(total) as f32 / total as f32
                }
            })
            .max_by(|a, b| a.total_cmp(b))
    }
    /// Stops owning the current page without touching pending exits.
    pub fn retire_page(&mut self) {
        self.entered = None;
        self.fade = None;
        self.elements.clear();
    }
}
